//! HostPeer contra um "viewer" também em webrtc-rs, na mesma máquina.
//! (A interoperabilidade com o Chromium é testada em `scripts/p2p-interop`.)

use std::sync::Arc;
use std::time::Duration;

use kr_proto::control::{CandidateType, ControlMessage};
use kr_transport::{CandidateInit, HostPeer, PeerEvent, PeerState, SignalPayload};
use tokio::sync::mpsc;
use tokio::time::timeout;
use webrtc::data_channel::DataChannelEvent;
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCPeerConnectionIceEvent,
    RTCSessionDescription,
};

struct ViewerHandler {
    candidates: mpsc::UnboundedSender<CandidateInit>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for ViewerHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(init) = event.candidate.to_json() {
            let _ = self.candidates.send(init.into());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_e_viewer_conectam_e_medem_rtt() {
    // Host
    let (host_tx, mut host_rx) = mpsc::unbounded_channel();
    let host = HostPeer::new(&[], host_tx).await.unwrap();

    // Viewer
    let (cand_tx, mut cand_rx) = mpsc::unbounded_channel();
    let viewer = PeerConnectionBuilder::new()
        .with_handler(Arc::new(ViewerHandler {
            candidates: cand_tx,
        }))
        // Como no app: todas as interfaces (no Windows, um socket preso ao loopback
        // não alcança os candidatos do host, que usam os IPs reais).
        .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
        .build()
        .await
        .unwrap();
    let dc = viewer.create_data_channel("control", None).await.unwrap();
    let offer = viewer.create_offer(None).await.unwrap();
    viewer.set_local_description(offer.clone()).await.unwrap();

    host.handle_signal(serde_json::to_value(SignalPayload::Offer { sdp: offer.sdp }).unwrap())
        .await
        .unwrap();

    // Viewer responde pings e informa a rota quando o canal abre.
    let dc2 = dc.clone();
    tokio::spawn(async move {
        while let Some(ev) = dc2.poll().await {
            match ev {
                DataChannelEvent::OnOpen => {
                    let route = ControlMessage::Route {
                        local: CandidateType::Host,
                        remote: CandidateType::Host,
                        protocol: "udp".into(),
                    };
                    dc2.send_text(&route.to_json()).await.unwrap();
                }
                DataChannelEvent::OnMessage(m) => {
                    let msg: ControlMessage = serde_json::from_slice(&m.data).unwrap();
                    if let ControlMessage::Ping { n } = msg {
                        dc2.send_text(&ControlMessage::Pong { n }.to_json())
                            .await
                            .unwrap();
                    }
                }
                DataChannelEvent::OnClose => break,
                _ => {}
            }
        }
    });

    let mut connected = false;
    let mut opened = false;
    let mut rtt = None;
    let mut route = None;
    timeout(Duration::from_secs(15), async {
        loop {
            tokio::select! {
                Some(c) = cand_rx.recv() => {
                    host.handle_signal(serde_json::to_value(SignalPayload::Candidate { candidate: c }).unwrap())
                        .await
                        .unwrap();
                }
                Some(ev) = host_rx.recv() => match ev {
                    PeerEvent::Signal(SignalPayload::Answer { sdp }) => {
                        viewer.set_remote_description(RTCSessionDescription::answer(sdp).unwrap()).await.unwrap();
                    }
                    PeerEvent::Signal(SignalPayload::Candidate { candidate }) => {
                        viewer.add_ice_candidate(candidate.into()).await.unwrap();
                    }
                    PeerEvent::State(PeerState::Connected) => connected = true,
                    PeerEvent::ControlOpen => opened = true,
                    PeerEvent::Rtt(d) => rtt = Some(d),
                    PeerEvent::Route(r) => route = Some(r),
                    _ => {}
                },
            }
            if connected && opened && rtt.is_some() && route.is_some() {
                break;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timeout: connected={connected} opened={opened} rtt={rtt:?} route={route:?}"));

    assert!(rtt.unwrap() < Duration::from_millis(500));
    let route = route.unwrap();
    assert!(!route.relayed);
    assert_eq!(route.protocol, "udp");

    host.close().await;
    viewer.close().await.unwrap();
}

#[tokio::test]
async fn rejeita_sinal_invalido() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let host = HostPeer::new(&[], tx).await.unwrap();
    assert!(
        host.handle_signal(serde_json::json!({"kind": "nope"}))
            .await
            .is_err()
    );
    assert!(
        host.handle_signal(serde_json::json!({"kind": "answer", "sdp": "v=0"}))
            .await
            .is_err()
    );
    host.close().await;
}
