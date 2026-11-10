//! Host de teste para interoperabilidade com o Chromium (`scripts/p2p-interop`).
//!
//! Abre um WebSocket local que faz o papel da sinalização para UM viewer:
//! cada mensagem de texto é um `SignalPayload`, exceto `done`, que o viewer
//! manda quando já mediu a latência, viu a rota e decodificou vídeo.
//!
//! Quando o canal P2P abre, transmite um padrão de teste em H.264 (OpenH264).
//! Imprime `LISTENING <porta>` ao subir e `INTEROP_OK ...` no sucesso. Sai com
//! código 1 após 30 s sem sucesso.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use kr_codec_soft::OpenH264Encoder;
use kr_core::media::pattern::TestPattern;
use kr_core::media::{EncoderFactory, PipelineConfig, VideoEncoder};
use kr_transport::{HostPeer, PeerEvent, VideoStream};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

fn start_video(host: &HostPeer) -> VideoStream {
    let factory: EncoderFactory =
        Box::new(|cfg| OpenH264Encoder::new(cfg).map(|e| Box::new(e) as Box<dyn VideoEncoder>));
    let config = PipelineConfig {
        bitrate_bps: 1_500_000,
        ..PipelineConfig::default()
    };
    VideoStream::start(
        host.video(),
        Box::new(TestPattern::new(640, 360, 30)),
        factory,
        config,
    )
    .expect("thread de vídeo")
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    println!("LISTENING {}", listener.local_addr().unwrap().port());

    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (tcp, _) = listener.accept().await.expect("accept");
        let ws = tokio_tungstenite::accept_async(tcp).await.expect("handshake");
        let (mut sink, mut stream) = ws.split();

        let (tx, mut events) = mpsc::unbounded_channel();
        let host = HostPeer::new(&[], tx).await.expect("HostPeer");
        let mut rtt: Option<Duration> = None;
        let mut route: Option<kr_transport::Route> = None;
        let mut keyframe_requests = 0u32;
        let mut video: Option<VideoStream> = None;

        loop {
            tokio::select! {
                frame = stream.next() => match frame {
                    Some(Ok(Message::Text(t))) if t.as_str() == "done" => {
                        let (Some(rtt), Some(route)) = (rtt, route.as_ref()) else {
                            break Err("viewer terminou antes do host medir RTT/rota".to_string());
                        };
                        let msg = format!(
                            "INTEROP_OK rtt_ms={} route={:?}/{:?}/{} keyframe_requests={}",
                            rtt.as_millis(),
                            route.local,
                            route.remote,
                            route.protocol,
                            keyframe_requests,
                        );
                        drop(video.take());
                        host.close().await;
                        break Ok(msg);
                    }
                    Some(Ok(Message::Text(t))) => {
                        let payload: serde_json::Value = serde_json::from_str(&t).expect("json");
                        if let Err(e) = host.handle_signal(payload).await {
                            eprintln!("erro no sinal: {e}");
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => break Err("viewer desconectou".to_string()),
                },
                Some(ev) = events.recv() => match ev {
                    PeerEvent::Signal(s) => {
                        sink.send(Message::text(serde_json::to_string(&s).unwrap())).await.expect("send");
                    }
                    PeerEvent::State(s) => eprintln!("estado: {s:?}"),
                    PeerEvent::ControlOpen => {
                        if video.is_none() {
                            video = Some(start_video(&host));
                        }
                    }
                    PeerEvent::Rtt(d) => rtt = Some(d),
                    PeerEvent::Route(r) => route = Some(r),
                    PeerEvent::KeyframeRequest => {
                        keyframe_requests += 1;
                        if let Some(v) = &video {
                            v.request_keyframe();
                        }
                    }
                },
            }
        }
    })
    .await;

    match result {
        Ok(Ok(msg)) => println!("{msg}"),
        Ok(Err(e)) => {
            println!("INTEROP_FAIL {e}");
            std::process::exit(1);
        }
        Err(_) => {
            println!("INTEROP_FAIL timeout");
            std::process::exit(1);
        }
    }
}
