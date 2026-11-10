//! Fluxo completo contra o servidor TypeScript de verdade (rodando local ou
//! em produção). Ignorado por padrão; no CI roda com o servidor em memória:
//!
//! ```sh
//! KR_SIGNALING_URL=ws://127.0.0.1:3000/ws cargo test -p kr-signaling --test real_server -- --ignored
//! ```

use std::sync::Arc;
use std::time::Duration;

use kr_identity::DeviceIdentity;
use kr_proto::{ClientMessage, ServerMessage};
use kr_signaling::{ConnectionStatus, SignalingConfig, SignalingEvent, spawn};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;

async fn wait(
    rx: &mut UnboundedReceiver<SignalingEvent>,
    what: &str,
    pred: impl Fn(&SignalingEvent) -> bool,
) -> SignalingEvent {
    timeout(Duration::from_secs(10), async {
        loop {
            let ev = rx.recv().await.expect("cliente encerrou");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timeout esperando {what}"))
}

async fn online(rx: &mut UnboundedReceiver<SignalingEvent>) -> String {
    match wait(rx, "online", |e| {
        matches!(e, SignalingEvent::Status(ConnectionStatus::Online { .. }))
    })
    .await
    {
        SignalingEvent::Status(ConnectionStatus::Online { device_id }) => device_id,
        _ => unreachable!(),
    }
}

#[tokio::test]
#[ignore = "precisa de KR_SIGNALING_URL"]
async fn fluxo_completo_com_servidor_real() {
    let url = std::env::var("KR_SIGNALING_URL").expect("defina KR_SIGNALING_URL");
    let (viewer, mut vrx) = spawn(
        SignalingConfig::new(&url, "Viewer Rust", "test"),
        Arc::new(DeviceIdentity::generate().unwrap()),
    );
    let (host, mut hrx) = spawn(
        SignalingConfig::new(&url, "Host Rust", "test"),
        Arc::new(DeviceIdentity::generate().unwrap()),
    );
    let _viewer_id = online(&mut vrx).await;
    let host_id = online(&mut hrx).await;
    assert_eq!(host_id.len(), 9);

    let rid = "0b7c6a52-8d1e-4f3a-9b2c-7d6e5f4a3b21".to_string();
    viewer
        .send(ClientMessage::ConnectRequest {
            request_id: rid.clone(),
            target_id: host_id.clone(),
        })
        .unwrap();
    wait(&mut vrx, "connect_pending", |e| {
        matches!(
            e,
            SignalingEvent::Message(ServerMessage::ConnectPending { .. })
        )
    })
    .await;
    let incoming = wait(&mut hrx, "incoming_request", |e| {
        matches!(
            e,
            SignalingEvent::Message(ServerMessage::IncomingRequest { .. })
        )
    })
    .await;
    let SignalingEvent::Message(ServerMessage::IncomingRequest { from_name, .. }) = incoming else {
        unreachable!()
    };
    assert_eq!(from_name, "Viewer Rust");

    host.send(ClientMessage::ConnectResponse {
        request_id: rid.clone(),
        accept: true,
    })
    .unwrap();
    wait(&mut vrx, "connect_accepted", |e| {
        matches!(
            e,
            SignalingEvent::Message(ServerMessage::ConnectAccepted { .. })
        )
    })
    .await;
    wait(&mut hrx, "session_ready", |e| {
        matches!(
            e,
            SignalingEvent::Message(ServerMessage::SessionReady { .. })
        )
    })
    .await;

    viewer
        .send(ClientMessage::Signal {
            request_id: rid.clone(),
            payload: serde_json::json!({"kind": "offer"}),
        })
        .unwrap();
    wait(&mut hrx, "signal", |e| {
        matches!(e, SignalingEvent::Message(ServerMessage::Signal { .. }))
    })
    .await;

    host.send(ClientMessage::EndSession { request_id: rid })
        .unwrap();
    wait(&mut vrx, "session_ended", |e| {
        matches!(
            e,
            SignalingEvent::Message(ServerMessage::SessionEnded { .. })
        )
    })
    .await;
    viewer.shutdown();
    host.shutdown();
}
