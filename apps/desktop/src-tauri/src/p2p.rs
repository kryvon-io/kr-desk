//! Conexões P2P do lado host (este computador sendo acessado).
//!
//! Quando o usuário aceita um pedido, o servidor manda `session_ready`; aqui
//! criamos um [`HostPeer`] e ligamos seus sinais à sinalização e seu estado à
//! interface (evento `p2p-status`).

use std::collections::HashMap;
use std::sync::Arc;

use kr_proto::{ClientMessage, IceServer};
use kr_signaling::SignalingHandle;
use kr_transport::{HostPeer, PeerEvent, PeerState, Route};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, mpsc};
use tracing::{info, warn};

pub const EVENT_P2P: &str = "p2p-status";

/// Estado da conexão P2P enviado à interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct P2pStatus {
    pub request_id: String,
    pub state: PeerState,
    pub rtt_ms: Option<u64>,
    pub route: Option<Route>,
}

#[derive(Default)]
pub struct P2pManager {
    peers: Mutex<HashMap<String, Arc<HostPeer>>>,
}

impl P2pManager {
    /// Cria a conexão do host para uma sessão aceita.
    pub async fn start_host(
        &self,
        app: AppHandle,
        signaling: SignalingHandle,
        request_id: String,
        ice_servers: Vec<IceServer>,
    ) {
        let (tx, mut events) = mpsc::unbounded_channel();
        let peer = match HostPeer::new(&ice_servers, tx).await {
            Ok(p) => Arc::new(p),
            Err(err) => {
                warn!(%err, "falha ao criar a conexão P2P");
                emit(
                    &app,
                    P2pStatus {
                        request_id,
                        state: PeerState::Failed,
                        rtt_ms: None,
                        route: None,
                    },
                );
                return;
            }
        };
        if let Some(old) = self.peers.lock().await.insert(request_id.clone(), peer) {
            old.close().await;
        }
        info!(%request_id, "conexão P2P (host) criada; aguardando oferta");

        tokio::spawn(async move {
            let mut status = P2pStatus {
                request_id: request_id.clone(),
                state: PeerState::Connecting,
                rtt_ms: None,
                route: None,
            };
            emit(&app, status.clone());
            while let Some(ev) = events.recv().await {
                match ev {
                    PeerEvent::Signal(payload) => {
                        let payload = serde_json::to_value(payload).expect("serializável");
                        if let Err(err) = signaling.send(ClientMessage::Signal {
                            request_id: request_id.clone(),
                            payload,
                        }) {
                            warn!(%err, "não foi possível enviar sinal ao viewer");
                        }
                        continue;
                    }
                    PeerEvent::State(s) => status.state = s,
                    PeerEvent::ControlOpen => status.state = PeerState::Connected,
                    PeerEvent::Rtt(d) => status.rtt_ms = Some(d.as_millis() as u64),
                    PeerEvent::Route(r) => status.route = Some(r),
                }
                emit(&app, status.clone());
            }
        });
    }

    /// Sinal do viewer para uma sessão em que somos host. Retorna `false` se
    /// não há conexão host para esse pedido.
    pub async fn handle_signal(&self, request_id: &str, payload: serde_json::Value) -> bool {
        let peer = self.peers.lock().await.get(request_id).cloned();
        match peer {
            Some(peer) => {
                if let Err(err) = peer.handle_signal(payload).await {
                    warn!(%err, "sinal P2P inválido");
                }
                true
            }
            None => false,
        }
    }

    pub async fn close(&self, request_id: &str) {
        if let Some(peer) = self.peers.lock().await.remove(request_id) {
            peer.close().await;
            info!(%request_id, "conexão P2P (host) encerrada");
        }
    }
}

fn emit(app: &AppHandle, status: P2pStatus) {
    let _ = app.emit(EVENT_P2P, status);
}
