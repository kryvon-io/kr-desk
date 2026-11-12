//! Transporte WebRTC do lado **host** (o computador acessado), com webrtc-rs.
//!
//! O viewer (WebView2) cria a oferta e o DataChannel `control`; o host responde.
//! A troca de SDP e candidatos ICE passa pela sinalização como `payload`
//! opaco para o servidor, no formato de [`SignalPayload`].
//!
//! Este crate não sabe nada de Tauri: recebe sinais com
//! [`HostPeer::handle_signal`] e publica [`PeerEvent`]s num canal. O vídeo da
//! tela sai pela trilha H.264 de [`HostPeer::video`].

mod stream;
mod video;

pub use stream::VideoStream;
pub use video::{H264_FMTP, H264_MIME, VideoSender};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kr_proto::IceServer;
use kr_proto::control::{CONTROL_LABEL, CandidateType, ControlMessage, INPUT_LABEL};
use rtc::interceptor::Slot;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelState};
use webrtc::media_stream::track_local::TrackLocal;
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
    /// O viewer pediu um keyframe (PLI/FIR no RTCP ou `keyframe` no controle).
    KeyframeRequest,
    /// O viewer pediu para transmitir outro monitor.
    SelectMonitor(u32),
    /// Fingerprints DTLS negociados (o nosso e o do viewer, como vemos no SDP).
    Fingerprints {
        local: String,
        remote: String,
    },
    /// Prova de identidade enviada pelo viewer (ver `kr_proto::session`).
    PeerAuth {
        pk: String,
        sig: String,
    },
    /// Relatório de recepção do viewer (qualidade adaptativa).
    RxReport(kr_core::media::quality::RxReport),
    /// Texto copiado no viewer.
    Clipboard(String),
    /// Senha de acesso não assistido enviada pelo viewer.
    Unlock(String),
    /// Entrada remota (mouse/teclado) do viewer. Ao fechar o canal de
    /// controle, chega um `ReleaseAll`.
    Input(ControlMessage),
}

fn is_input(msg: &ControlMessage) -> bool {
    matches!(
        msg,
        ControlMessage::MouseMove { .. }
            | ControlMessage::MouseButton { .. }
            | ControlMessage::Wheel { .. }
            | ControlMessage::Key { .. }
            | ControlMessage::ReleaseAll
            | ControlMessage::CtrlAltDel
    )
}

/// Envia mensagens ao viewer pelo DataChannel de controle. Não segura o
/// canal de eventos do peer (pode ficar com quem consome os eventos).
#[derive(Clone, Default)]
pub struct ControlSender {
    dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
}

impl ControlSender {
    pub async fn send(&self, msg: &ControlMessage) -> bool {
        let dc = self.dc.lock().await.clone();
        match dc {
            Some(dc) => dc.send_text(&msg.to_json()).await.is_ok(),
            None => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("WebRTC: {0}")]
    WebRtc(#[from] webrtc::error::Error),
    #[error("sinal inválido: {0}")]
    BadSignal(String),
    #[error("vídeo: {0}")]
    Video(String),
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
    video: VideoSender,
}

/// Estado compartilhado com os callbacks do webrtc-rs.
struct Shared {
    events: mpsc::UnboundedSender<PeerEvent>,
    control: ControlSender,
    ping_seq: AtomicU64,
    /// Ping em voo: (número, quando foi enviado).
    in_flight: std::sync::Mutex<Option<(u64, Instant)>>,
    /// Candidatos gerados antes de a resposta SDP sair. O viewer precisa da
    /// resposta antes dos candidatos, então eles esperam aqui. `None` = a
    /// resposta já foi enviada.
    early_candidates: std::sync::Mutex<Option<Vec<CandidateInit>>>,
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
            Ok(init) => {
                let mut early = self.shared.early_candidates.lock().expect("mutex");
                match early.as_mut() {
                    Some(queue) => queue.push(init.into()),
                    None => self
                        .shared
                        .emit(PeerEvent::Signal(SignalPayload::Candidate {
                            candidate: init.into(),
                        })),
                }
            }
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
        match label.as_str() {
            CONTROL_LABEL => {
                *self.shared.control.dc.lock().await = Some(dc.clone());
                tokio::spawn(control_loop(dc, self.shared.clone()));
            }
            INPUT_LABEL => {
                tokio::spawn(input_loop(dc, self.shared.clone()));
            }
            _ => debug!(%label, "DataChannel desconhecido ignorado"),
        }
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
                    Ok(ControlMessage::Keyframe) => shared.emit(PeerEvent::KeyframeRequest),
                    Ok(ControlMessage::SelectMonitor { index }) => {
                        shared.emit(PeerEvent::SelectMonitor(index));
                    }
                    Ok(ControlMessage::Auth { pk, sig }) => {
                        shared.emit(PeerEvent::PeerAuth { pk, sig });
                    }
                    Ok(ControlMessage::RxReport {
                        lost,
                        received,
                        rtt_ms,
                    }) => {
                        shared.emit(PeerEvent::RxReport(kr_core::media::quality::RxReport {
                            lost,
                            received,
                            rtt_ms,
                        }));
                    }
                    Ok(ControlMessage::Clipboard { text }) => {
                        if text.chars().count() <= kr_proto::control::MAX_CLIPBOARD_CHARS {
                            shared.emit(PeerEvent::Clipboard(text));
                        }
                    }
                    Ok(ControlMessage::Unlock { password }) => {
                        if password.chars().count() <= kr_proto::control::MAX_PASSWORD_CHARS {
                            shared.emit(PeerEvent::Unlock(password));
                        }
                    }
                    Ok(msg) if is_input(&msg) => shared.emit(PeerEvent::Input(msg)),
                    Ok(other) => debug!(?other, "mensagem de controle ignorada"),
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
    // Sem canal de controle não há como soltar teclas depois: solta agora.
    shared.emit(PeerEvent::Input(ControlMessage::ReleaseAll));
}

/// Lê o DataChannel `input` (movimentos do mouse, não confiável).
async fn input_loop(dc: Arc<dyn DataChannel>, shared: Arc<Shared>) {
    while let Some(event) = dc.poll().await {
        match event {
            DataChannelEvent::OnMessage(msg) => {
                let Ok(text) = std::str::from_utf8(&msg.data) else {
                    continue;
                };
                if let Ok(msg) = serde_json::from_str::<ControlMessage>(text)
                    && is_input(&msg)
                {
                    shared.emit(PeerEvent::Input(msg));
                }
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
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
            control: ControlSender::default(),
            ping_seq: AtomicU64::new(1),
            in_flight: std::sync::Mutex::new(None),
            early_candidates: std::sync::Mutex::new(Some(Vec::new())),
        });

        // Só H.264: o viewer recebe vídeo e o resto vai por DataChannel.
        let mut media_engine = MediaEngine::default();
        media_engine.register_codec(video::h264_codec(), RtpCodecKind::Video)?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)?.with(
            Slot::from(video::KEYFRAME_INTERCEPTOR_SLOT),
            video::KeyframeRequestInterceptor::default(),
        );
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
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);

        // A trilha entra antes da oferta chegar: o m=video `recvonly` do viewer
        // é associado a ela no `set_remote_description`.
        let track = video::new_track()?;
        let sender = pc
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal>)
            .await?;
        let video = VideoSender::new(track, sender);
        let keyframes = video.clone();
        let events = shared.clone();
        tokio::spawn(async move {
            while keyframes.next_keyframe_request().await.is_some() {
                debug!("PLI/FIR recebido");
                events.emit(PeerEvent::KeyframeRequest);
            }
        });

        Ok(Self {
            pc,
            pending: Mutex::new(Vec::new()),
            has_remote: std::sync::atomic::AtomicBool::new(false),
            shared,
            video,
        })
    }

    /// Trilha de vídeo para enviar a tela.
    pub fn video(&self) -> VideoSender {
        self.video.clone()
    }

    /// Processa um `payload` recebido do viewer.
    pub async fn handle_signal(&self, payload: serde_json::Value) -> Result<()> {
        let signal: SignalPayload = serde_json::from_value(payload)
            .map_err(|e| TransportError::BadSignal(e.to_string()))?;
        match signal {
            SignalPayload::Offer { sdp } => {
                // Nova oferta (inclusive reinício de ICE): os candidatos novos
                // esperam a resposta, para o viewer conhecer o novo ufrag antes.
                {
                    let mut early = self.shared.early_candidates.lock().expect("mutex");
                    if early.is_none() {
                        *early = Some(Vec::new());
                    }
                }
                let restart = self.has_remote.load(Ordering::SeqCst);
                if restart {
                    info!("nova oferta do viewer (reinício de ICE)");
                }
                let remote_fp = kr_proto::session::sdp_fingerprint(&sdp);
                let offer = RTCSessionDescription::offer(sdp)?;
                self.pc.set_remote_description(offer).await?;
                self.has_remote.store(true, Ordering::SeqCst);
                let answer = self.pc.create_answer(None).await?;
                self.pc.set_local_description(answer.clone()).await?;
                let local_fp = kr_proto::session::sdp_fingerprint(&answer.sdp);
                self.shared
                    .emit(PeerEvent::Signal(SignalPayload::Answer { sdp: answer.sdp }));
                // Agora os candidatos que chegaram antes podem seguir.
                let early = self.shared.early_candidates.lock().expect("mutex").take();
                for candidate in early.into_iter().flatten() {
                    self.shared
                        .emit(PeerEvent::Signal(SignalPayload::Candidate { candidate }));
                }
                match (local_fp, remote_fp) {
                    (Some(local), Some(remote)) => {
                        self.shared.emit(PeerEvent::Fingerprints { local, remote });
                    }
                    _ => warn!("SDP sem fingerprint DTLS; a sessão não poderá ser autenticada"),
                }
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
        self.shared.control.send(msg).await
    }

    /// Envio pelo canal de controle sem manter o peer vivo.
    pub fn control_sender(&self) -> ControlSender {
        self.shared.control.clone()
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
