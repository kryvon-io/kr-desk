//! Transporte WebRTC do lado **host** (o computador acessado), com webrtc-rs.
//!
//! O viewer (WebView2) cria a oferta e o DataChannel `control`; o host responde.
//! A troca de SDP e candidatos ICE passa pela sinalização como `payload`
//! opaco para o servidor, no formato de [`SignalPayload`].
//!
//! Este crate não sabe nada de Tauri: recebe sinais com
//! [`HostPeer::handle_signal`] e publica [`PeerEvent`]s num canal.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kr_proto::IceServer;
use kr_proto::control::{CONTROL_LABEL, CandidateType, ControlMessage};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelState};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionIceEvent,
    RTCPeerConnectionState, RTCSessionDescription, Registry, register_default_interceptors,
};

/// Intervalo entre pings no DataChannel de controle.
pub const PING_INTERVAL: Duration = Duration::from_secs(1);

/// Formato do `payload` de `signal` (igual ao que o viewer TypeScript envia).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalPayload {
    Offer { sdp: String },
    Answer { sdp: String },
    Candidate { candidate: CandidateInit },
}

/// Espelho de `RTCIceCandidateInit` (JSON do navegador, camelCase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateInit {
    pub candidate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdp_mid: Option<String>,
    #[serde(
        default,
        rename = "sdpMLineIndex",
        skip_serializing_if = "Option::is_none"
    )]
    pub sdp_mline_index: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_fragment: Option<String>,
}

impl From<CandidateInit> for RTCIceCandidateInit {
    fn from(c: CandidateInit) -> Self {
        RTCIceCandidateInit {
            candidate: c.candidate,
            sdp_mid: c.sdp_mid,
            sdp_mline_index: c.sdp_mline_index,
            username_fragment: c.username_fragment,
            ..Default::default()
        }
    }
}

impl From<RTCIceCandidateInit> for CandidateInit {
    fn from(c: RTCIceCandidateInit) -> Self {
        CandidateInit {
            candidate: c.candidate,
            sdp_mid: c.sdp_mid,
            sdp_mline_index: c.sdp_mline_index,
            username_fragment: c.username_fragment,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

/// Rota em uso, informada pelo viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Route {
    pub local: CandidateType,
    pub remote: CandidateType,
    pub protocol: String,
    /// `true` se algum lado está usando TURN.
    pub relayed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerEvent {
    /// Mensagem a enviar ao viewer pela sinalização.
    Signal(SignalPayload),
    State(PeerState),
    /// Latência de ida e volta medida no DataChannel.
    Rtt(Duration),
    Route(Route),
    /// DataChannel de controle aberto (canal P2P pronto).
    ControlOpen,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("WebRTC: {0}")]
    WebRtc(#[from] webrtc::error::Error),
    #[error("sinal inválido: {0}")]
    BadSignal(String),
}

pub type Result<T> = std::result::Result<T, TransportError>;

fn to_rtc_ice_servers(servers: &[IceServer]) -> Vec<RTCIceServer> {
    servers
        .iter()
        .map(|s| RTCIceServer {
            urls: s.urls.clone(),
            username: s.username.clone().unwrap_or_default(),
            credential: s.credential.clone().unwrap_or_default(),
        })
        .collect()
}

/// Conexão WebRTC do host para uma sessão.
pub struct HostPeer {
    pc: Arc<dyn PeerConnection>,
    /// Candidatos que chegaram antes da oferta.
    pending: Mutex<Vec<CandidateInit>>,
    has_remote: std::sync::atomic::AtomicBool,
    shared: Arc<Shared>,
}

/// Estado compartilhado com os callbacks do webrtc-rs.
struct Shared {
    events: mpsc::UnboundedSender<PeerEvent>,
    control: Mutex<Option<Arc<dyn DataChannel>>>,
    ping_seq: AtomicU64,
    /// Ping em voo: (número, quando foi enviado).
    in_flight: std::sync::Mutex<Option<(u64, Instant)>>,
}

impl Shared {
    fn emit(&self, ev: PeerEvent) {
        let _ = self.events.send(ev);
    }
}

struct Handler {
    shared: Arc<Shared>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(init) => self
                .shared
                .emit(PeerEvent::Signal(SignalPayload::Candidate {
                    candidate: init.into(),
                })),
            Err(err) => warn!(%err, "candidato local inválido"),
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!(%state, "estado da conexão P2P");
        let mapped = match state {
            RTCPeerConnectionState::Connected => PeerState::Connected,
            RTCPeerConnectionState::Disconnected => PeerState::Disconnected,
            RTCPeerConnectionState::Failed => PeerState::Failed,
            RTCPeerConnectionState::Closed => PeerState::Closed,
            _ => PeerState::Connecting,
        };
        self.shared.emit(PeerEvent::State(mapped));
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let label = dc.label().await.unwrap_or_default();
        if label != CONTROL_LABEL {
            debug!(%label, "DataChannel desconhecido ignorado");
            return;
        }
        *self.shared.control.lock().await = Some(dc.clone());
        tokio::spawn(control_loop(dc, self.shared.clone()));
    }
}

/// Inicia o ping periódico no canal de controle.
fn start_pinger(dc: Arc<dyn DataChannel>, shared: Arc<Shared>) -> tokio::task::JoinHandle<()> {
    shared.emit(PeerEvent::ControlOpen);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PING_INTERVAL);
        loop {
            interval.tick().await;
            let n = shared.ping_seq.fetch_add(1, Ordering::Relaxed);
            *shared.in_flight.lock().expect("mutex") = Some((n, Instant::now()));
            if dc
                .send_text(&ControlMessage::Ping { n }.to_json())
                .await
                .is_err()
            {
                break;
            }
        }
    })
}

/// Lê o DataChannel de controle e mantém o ping periódico.
async fn control_loop(dc: Arc<dyn DataChannel>, shared: Arc<Shared>) {
    // Um canal criado pelo outro lado pode chegar já aberto (sem `OnOpen`).
    let mut pinger = match dc.ready_state().await {
        Ok(RTCDataChannelState::Open) => Some(start_pinger(dc.clone(), shared.clone())),
        _ => None,
    };
    while let Some(event) = dc.poll().await {
        match event {
            DataChannelEvent::OnOpen => {
                if pinger.is_none() {
                    pinger = Some(start_pinger(dc.clone(), shared.clone()));
                }
            }
            DataChannelEvent::OnMessage(msg) => {
                // Mensagem recebida também prova que o canal está aberto.
                if pinger.is_none() {
                    pinger = Some(start_pinger(dc.clone(), shared.clone()));
                }
                let Ok(text) = std::str::from_utf8(&msg.data) else {
                    continue;
                };
                match serde_json::from_str::<ControlMessage>(text) {
                    Ok(ControlMessage::Ping { n }) => {
                        let _ = dc.send_text(&ControlMessage::Pong { n }.to_json()).await;
                    }
                    Ok(ControlMessage::Pong { n }) => {
                        let sent = shared
                            .in_flight
                            .lock()
                            .expect("mutex")
                            .take_if(|(m, _)| *m == n);
                        if let Some((_, at)) = sent {
                            shared.emit(PeerEvent::Rtt(at.elapsed()));
                        }
                    }
                    Ok(ControlMessage::Route {
                        local,
                        remote,
                        protocol,
                    }) => {
                        shared.emit(PeerEvent::Route(Route {
                            relayed: local.is_relay() || remote.is_relay(),
                            local,
                            remote,
                            protocol,
                        }));
                    }
                    Err(err) => debug!(%err, "mensagem de controle desconhecida"),
                }
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
    }
    if let Some(t) = pinger {
        t.abort();
    }
}

impl HostPeer {
    /// Cria a conexão; os eventos chegam por `events`.
    pub async fn new(
        ice_servers: &[IceServer],
        events: mpsc::UnboundedSender<PeerEvent>,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            events,
            control: Mutex::new(None),
            ping_seq: AtomicU64::new(1),
            in_flight: std::sync::Mutex::new(None),
        });

        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(to_rtc_ice_servers(ice_servers))
            .build();

        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(Arc::new(Handler {
                shared: shared.clone(),
            }))
            .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
            .build()
            .await?;

        Ok(Self {
            pc: Arc::new(pc),
            pending: Mutex::new(Vec::new()),
            has_remote: std::sync::atomic::AtomicBool::new(false),
            shared,
        })
    }

    /// Processa um `payload` recebido do viewer.
    pub async fn handle_signal(&self, payload: serde_json::Value) -> Result<()> {
        let signal: SignalPayload = serde_json::from_value(payload)
            .map_err(|e| TransportError::BadSignal(e.to_string()))?;
        match signal {
            SignalPayload::Offer { sdp } => {
                let offer = RTCSessionDescription::offer(sdp)?;
                self.pc.set_remote_description(offer).await?;
                self.has_remote.store(true, Ordering::SeqCst);
                let answer = self.pc.create_answer(None).await?;
                self.pc.set_local_description(answer.clone()).await?;
                self.shared
                    .emit(PeerEvent::Signal(SignalPayload::Answer { sdp: answer.sdp }));
                for c in std::mem::take(&mut *self.pending.lock().await) {
                    self.pc.add_ice_candidate(c.into()).await?;
                }
            }
            SignalPayload::Candidate { candidate } => {
                if candidate.candidate.is_empty() {
                    return Ok(()); // fim dos candidatos
                }
                if self.has_remote.load(Ordering::SeqCst) {
                    self.pc.add_ice_candidate(candidate.into()).await?;
                } else {
                    self.pending.lock().await.push(candidate);
                }
            }
            SignalPayload::Answer { .. } => {
                return Err(TransportError::BadSignal("o host não recebe answer".into()));
            }
        }
        Ok(())
    }

    /// Envia uma mensagem de controle ao viewer, se o canal estiver aberto.
    pub async fn send_control(&self, msg: &ControlMessage) -> bool {
        let dc = self.shared.control.lock().await.clone();
        match dc {
            Some(dc) => dc.send_text(&msg.to_json()).await.is_ok(),
            None => false,
        }
    }

    pub async fn close(&self) {
        if let Err(err) = self.pc.close().await {
            warn!(%err, "erro ao fechar a conexão P2P");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_payload_formato_do_navegador() {
        let json = serde_json::json!({
            "kind": "candidate",
            "candidate": {
                "candidate": "candidate:1 1 udp 2122260223 192.168.0.2 54321 typ host",
                "sdpMid": "0",
                "sdpMLineIndex": 0,
                "usernameFragment": "abcd"
            }
        });
        let parsed: SignalPayload = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
        let offer: SignalPayload =
            serde_json::from_value(serde_json::json!({"kind": "offer", "sdp": "v=0"})).unwrap();
        assert_eq!(offer, SignalPayload::Offer { sdp: "v=0".into() });
    }
}
