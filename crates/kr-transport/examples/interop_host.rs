//! Host de teste para interoperabilidade com o Chromium (`scripts/p2p-interop`).
//!
//! Abre um WebSocket local que faz o papel da sinalização para UM viewer:
//! cada mensagem de texto é um `SignalPayload`. Imprime `LISTENING <porta>`
//! ao subir e `INTEROP_OK rtt_ms=<n> route=<local>/<remote>` quando o canal P2P
//! está aberto, a latência foi medida e a rota foi informada. Sai com código
//! 1 após 30 s sem sucesso.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use kr_transport::{HostPeer, PeerEvent};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

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
        let (mut opened, mut rtt, mut route) = (false, None, None);

        loop {
            tokio::select! {
                frame = stream.next() => match frame {
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
                    PeerEvent::ControlOpen => opened = true,
                    PeerEvent::Rtt(d) => rtt = Some(d),
                    PeerEvent::Route(r) => route = Some(r),
                },
            }
            if let (true, Some(rtt), Some(route)) = (opened, rtt, route.as_ref()) {
                let msg = format!(
                    "INTEROP_OK rtt_ms={} route={:?}/{:?}/{}",
                    rtt.as_millis(),
                    route.local,
                    route.remote,
                    route.protocol
                );
                // Dá tempo de o viewer também medir a própria latência.
                tokio::time::sleep(Duration::from_millis(1500)).await;
                host.close().await;
                break Ok(msg);
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
