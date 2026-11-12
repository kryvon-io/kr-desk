//! Host de teste para interoperabilidade com o Chromium (`scripts/p2p-interop`).
//!
//! Abre um WebSocket local que faz o papel da sinalização para UM viewer:
//! cada mensagem de texto é um `SignalPayload`, exceto `done`, que o viewer
//! manda quando já mediu a latência, viu a rota e decodificou vídeo.
//!
//! Antes de qualquer vídeo, os dois lados trocam provas de identidade
//! (fingerprints DTLS assinados). Com `KR_INTEROP_TAMPER=1`, o host assina um
//! fingerprint falso — simula um intermediário — e o viewer deve recusar.
//!
//! Depois de autenticado, transmite um padrão de teste em H.264 (OpenH264)
//! e um cursor. O viewer manda movimentos, um clique e uma tecla, que passam
//! por um `InputController` com um destino que só registra.
//! Imprime `LISTENING <porta>` ao subir e `INTEROP_OK ...` no sucesso. Sai com
//! código 1 após 30 s sem sucesso.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use kr_codec_soft::OpenH264Encoder;
use kr_core::input::{InputController, InputSink};
use kr_core::media::pattern::TestPattern;
use kr_core::media::{EncoderFactory, FrameSource, PipelineConfig, VideoEncoder};
use kr_identity::{DeviceIdentity, verify_peer};
use kr_proto::control::ControlMessage;
use kr_proto::session::Role;
use kr_transport::{HostPeer, PeerEvent, VideoStream};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Identificador da sessão de teste (o mesmo usado pela página).
const RID: &str = "interop-session";

/// Destino de entrada que só registra o que recebeu.
#[derive(Default)]
struct Recorder {
    moves: u32,
    buttons: u32,
    keys: u32,
    wheels: u32,
}

impl InputSink for Recorder {
    fn move_to(&mut self, _x: u16, _y: u16) {
        self.moves += 1;
    }
    fn button(&mut self, _b: kr_proto::control::MouseButton, _down: bool) {
        self.buttons += 1;
    }
    fn wheel(&mut self, _dx: i32, _dy: i32) {
        self.wheels += 1;
    }
    fn key(&mut self, _code: u16, _ext: bool, _down: bool) {
        self.keys += 1;
    }
}

fn start_video(host: &HostPeer) -> VideoStream {
    let factory: EncoderFactory =
        Box::new(|cfg| OpenH264Encoder::new(cfg).map(|e| Box::new(e) as Box<dyn VideoEncoder>));
    let config = PipelineConfig {
        bitrate_bps: 1_500_000,
        ..PipelineConfig::default()
    };
    VideoStream::start(
        host.video(),
        Box::new(|| {
            Ok(
                Box::new(TestPattern::with_monitors(&[(640, 360), (320, 240)], 30))
                    as Box<dyn FrameSource>,
            )
        }),
        factory,
        config,
        Some(host.control_sender()),
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
        let mut monitor_selected: Option<u32> = None;
        let identity = DeviceIdentity::generate().expect("identidade");
        let tamper = std::env::var("KR_INTEROP_TAMPER").is_ok_and(|v| v == "1");
        let mut fingerprints: Option<(String, String)> = None;
        let mut viewer_authenticated = false;
        let mut quality = kr_core::media::quality::QualityController::new(1_500_000, 300_000, 3_000_000);
        let mut rx_reports = 0u32;
        let mut clipboard_from_viewer: Option<String> = None;
        // Senha de acesso não assistido: só pode chegar depois da autenticação.
        let mut unlock: Option<String> = None;
        let mut unlock_before_auth = false;
        let mut offers = 0u32;
        let mut input = InputController::new(Recorder::default());
        let mut video: Option<VideoStream> = None;

        loop {
            tokio::select! {
                frame = stream.next() => match frame {
                    Some(Ok(Message::Text(t))) if t.as_str() == "done" => {
                        let (Some(rtt), Some(route)) = (rtt, route.as_ref()) else {
                            break Err("viewer terminou antes do host medir RTT/rota".to_string());
                        };
                        if !viewer_authenticated {
                            break Err("viewer não se autenticou".to_string());
                        }
                        if offers < 2 {
                            break Err("viewer não reiniciou o ICE".to_string());
                        }
                        if rx_reports == 0 {
                            break Err("viewer não mandou relatórios de recepção".to_string());
                        }
                        if clipboard_from_viewer.as_deref() != Some("olá do viewer ç") {
                            break Err(format!(
                                "área de transferência do viewer não chegou: {clipboard_from_viewer:?}"
                            ));
                        }
                        if unlock_before_auth {
                            break Err("viewer mandou a senha antes de o host se comprovar".to_string());
                        }
                        if unlock.as_deref() != Some("senha-interop") {
                            break Err(format!("senha de acesso não assistido não chegou: {unlock:?}"));
                        }
                        if monitor_selected != Some(1) {
                            break Err("viewer não trocou de monitor".to_string());
                        }
                        let rec = input.sink();
                        if rec.moves == 0 || rec.buttons < 2 || rec.keys < 2 || rec.wheels == 0 {
                            break Err(format!(
                                "entrada incompleta: {} movimentos, {} botões, {} teclas, {} rodas",
                                rec.moves, rec.buttons, rec.keys, rec.wheels
                            ));
                        }
                        let msg = format!(
                            "INTEROP_OK rtt_ms={} route={:?}/{:?}/{} keyframe_requests={} input=moves:{}/buttons:{}/keys:{} rx_reports={} offers={}",
                            rtt.as_millis(),
                            route.local,
                            route.remote,
                            route.protocol,
                            keyframe_requests,
                            rec.moves,
                            rec.buttons,
                            rec.keys,
                            rx_reports,
                            offers,
                        );
                        drop(video.take());
                        host.close().await;
                        break Ok(msg);
                    }
                    Some(Ok(Message::Text(t))) => {
                        let payload: serde_json::Value = serde_json::from_str(&t).expect("json");
                        if payload["kind"] == "offer" {
                            offers += 1;
                        }
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
                    PeerEvent::Fingerprints { local, remote } => {
                        fingerprints = Some((local, remote));
                    }
                    PeerEvent::ControlOpen => {
                        let (local, remote) = fingerprints.clone().expect("fingerprints antes do canal");
                        let signed_local = if tamper { "sha-256 00:11:22".to_string() } else { local };
                        let sig = identity.sign_session(RID, Role::Host, &signed_local, &remote);
                        host.send_control(&ControlMessage::Auth { pk: identity.public_key_b64(), sig }).await;
                        host.send_control(&ControlMessage::Permissions { control: true }).await;
                    }
                    PeerEvent::PeerAuth { pk, sig } => {
                        let (local, remote) = fingerprints.clone().expect("fingerprints");
                        match verify_peer(&pk, &sig, RID, Role::Viewer, &remote, &local) {
                            Ok(()) => {
                                viewer_authenticated = true;
                                if video.is_none() {
                                    video = Some(start_video(&host));
                                }
                                host.send_control(&ControlMessage::Clipboard {
                                    text: "olá do host ✓".into(),
                                })
                                .await;
                            }
                            Err(err) => break Err(format!("viewer não autenticado: {err}")),
                        }
                    }
                    PeerEvent::Rtt(d) => rtt = Some(d),
                    PeerEvent::Route(r) => route = Some(r),
                    PeerEvent::RxReport(report) => {
                        rx_reports += 1;
                        if let (Some(t), Some(v)) = (quality.on_report(report), &video) {
                            v.set_quality(t);
                        }
                    }
                    PeerEvent::Clipboard(text) => clipboard_from_viewer = Some(text),
                    PeerEvent::Unlock(password) => {
                        // O runner confere que isto não acontece no cenário de MITM.
                        println!("UNLOCK_RECEIVED");
                        unlock_before_auth |= !viewer_authenticated;
                        unlock = Some(password);
                    }
                    PeerEvent::SelectMonitor(index) => {
                        monitor_selected = Some(index);
                        if let Some(v) = &video {
                            v.select_monitor(index);
                        }
                    }
                    PeerEvent::Input(msg) => {
                        input.handle(&msg);
                    }
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
