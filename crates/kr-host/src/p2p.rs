//! Conexões P2P do lado host (este computador sendo acessado).
//!
//! Quando o usuário aceita um pedido, o servidor manda `session_ready`; aqui
//! criamos um [`HostPeer`] e ligamos seus sinais à sinalização e seu estado à
//! interface ([`BackendEvents::p2p_status`]). Quando o canal P2P abre, a tela
//! começa a ser transmitida ([`HostPlatform::start_video`]) — mas só depois
//! de o viewer provar a identidade (fingerprints DTLS assinados, ver
//! `kr_proto::session`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kr_core::media::quality::QualityController;
use kr_identity::{DeviceIdentity, KnownPeers, Trust, verification_code, verify_peer};
use kr_proto::control::ControlMessage;
use kr_proto::session::Role;
use kr_proto::{ClientMessage, IceServer};
use kr_signaling::SignalingHandle;
use kr_transport::{HostPeer, PeerEvent, PeerState, Route, VideoStream};
use serde::Serialize;
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{info, warn};

use crate::{BackendEvents, HostPlatform, RemoteClipboard};

/// Intervalo para verificar se a transmissão da tela continua viva.
const VIDEO_CHECK: Duration = Duration::from_secs(2);
/// Faixa de taxa do controle adaptativo (bps).
const MIN_BITRATE: u32 = 400_000;
const MAX_BITRATE: u32 = 8_000_000;

/// Tempo para a conexão voltar (reinício de ICE pelo viewer) antes de
/// encerrar a sessão.
const RECONNECT_GRACE: Duration = Duration::from_secs(180);

/// Prazo para o viewer provar sua identidade depois que o canal abre.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Estado da conexão P2P enviado à interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct P2pStatus {
    pub request_id: String,
    /// ID do viewer.
    pub peer_id: String,
    pub state: PeerState,
    pub rtt_ms: Option<u64>,
    pub route: Option<Route>,
    /// A tela deste computador está sendo transmitida.
    pub sharing: bool,
    /// O viewer pode controlar mouse e teclado.
    pub control: bool,
    /// Código de verificação (igual nas duas telas sem intermediário).
    pub verification_code: Option<String>,
    /// Identidade do viewer verificada.
    pub authenticated: bool,
    /// Primeira conexão com este viewer (chave memorizada agora).
    pub first_contact: bool,
    /// Falha de segurança que encerrou a sessão.
    pub security_error: Option<String>,
}

impl P2pStatus {
    fn new(request_id: &str, peer_id: &str, control: bool) -> Self {
        Self {
            request_id: request_id.to_owned(),
            peer_id: peer_id.to_owned(),
            state: PeerState::Connecting,
            rtt_ms: None,
            route: None,
            sharing: false,
            control,
            verification_code: None,
            authenticated: false,
            first_contact: false,
            security_error: None,
        }
    }
}

/// Quem é o viewer, segundo o servidor (confirmado depois pelo DataChannel).
pub struct ViewerInfo {
    pub peer_id: String,
    pub public_key: String,
}

pub struct P2pManager {
    events: Arc<dyn BackendEvents>,
    platform: Arc<dyn HostPlatform>,
    identity: Arc<DeviceIdentity>,
    known: Arc<std::sync::Mutex<KnownPeers>>,
    peers: Mutex<HashMap<String, Arc<HostPeer>>>,
    /// Permissão escolhida no diálogo, até a sessão começar.
    pending_control: std::sync::Mutex<HashMap<String, bool>>,
    /// Permissão de controle de cada sessão ativa (pode mudar durante a sessão).
    controls: std::sync::Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl P2pManager {
    pub fn new(
        identity: Arc<DeviceIdentity>,
        known: Arc<std::sync::Mutex<KnownPeers>>,
        events: Arc<dyn BackendEvents>,
        platform: Arc<dyn HostPlatform>,
    ) -> Self {
        Self {
            events,
            platform,
            identity,
            known,
            peers: Mutex::default(),
            pending_control: Default::default(),
            controls: Default::default(),
        }
    }

    /// Guarda a permissão escolhida ao aceitar o pedido.
    pub fn set_pending_control(&self, request_id: &str, allow: bool) {
        self.pending_control
            .lock()
            .expect("mutex")
            .insert(request_id.to_owned(), allow);
    }

    /// Liga/desliga o controle remoto de uma sessão ativa.
    pub fn set_control(&self, request_id: &str, allow: bool) -> bool {
        match self.controls.lock().expect("mutex").get(request_id) {
            Some(tx) => {
                tx.send_replace(allow);
                true
            }
            None => false,
        }
    }

    /// Cria a conexão do host para uma sessão aceita.
    pub async fn start_host(
        &self,
        signaling: SignalingHandle,
        request_id: String,
        ice_servers: Vec<IceServer>,
        viewer: ViewerInfo,
    ) {
        let allow = self
            .pending_control
            .lock()
            .expect("mutex")
            .remove(&request_id)
            .unwrap_or(true);
        let (tx, mut events) = mpsc::unbounded_channel();
        let peer = match HostPeer::new(&ice_servers, tx).await {
            Ok(p) => Arc::new(p),
            Err(err) => {
                warn!(%err, "falha ao criar a conexão P2P");
                let mut status = P2pStatus::new(&request_id, &viewer.peer_id, allow);
                status.state = PeerState::Failed;
                self.events.p2p_status(&status);
                return;
            }
        };
        // A task guarda só a trilha de vídeo (não o peer), para não criar um
        // ciclo que manteria o canal de eventos aberto depois do fim da sessão.
        let video_sender = peer.video();
        let control_sender = peer.control_sender();
        if let Some(old) = self.peers.lock().await.insert(request_id.clone(), peer) {
            old.close().await;
        }
        let (control_tx, mut control_rx) = watch::channel(allow);
        self.controls
            .lock()
            .expect("mutex")
            .insert(request_id.clone(), control_tx);
        info!(%request_id, viewer = %viewer.peer_id, control = allow, "conexão P2P (host) criada; aguardando oferta");

        let identity = self.identity.clone();
        let known = self.known.clone();
        let app = self.events.clone();
        let platform = self.platform.clone();
        tokio::spawn(async move {
            let mut status = P2pStatus::new(&request_id, &viewer.peer_id, allow);
            app.p2p_status(&status);
            let mut fingerprints: Option<(String, String)> = None;
            let mut video: Option<VideoStream> = None;
            // Criado após autenticar; solta teclas/botões ao ser descartado.
            let mut input: Option<
                kr_core::input::InputController<Box<dyn kr_core::input::InputSink + Send>>,
            > = None;
            let mut auth_deadline: Option<tokio::time::Instant> = None;
            let mut reconnect_deadline: Option<tokio::time::Instant> = None;
            let mut quality = QualityController::new(
                kr_core::media::PipelineConfig::default().bitrate_bps,
                MIN_BITRATE,
                MAX_BITRATE,
            );
            // Texto copiado aqui → viewer (só com controle permitido).
            let (clip_tx, mut clip_rx) = mpsc::unbounded_channel::<String>();
            let mut clipboard: Option<Box<dyn RemoteClipboard>> = None;
            let start_clipboard = |tx: &mpsc::UnboundedSender<String>| {
                let tx = tx.clone();
                platform.clipboard(Box::new(move |text| {
                    let _ = tx.send(text);
                }))
            };
            let mut check = tokio::time::interval(VIDEO_CHECK);

            // Encerra a sessão por falha de segurança.
            let fail = |status: &mut P2pStatus, reason: String| {
                warn!(%request_id, %reason, "sessão recusada por segurança");
                status.security_error = Some(reason);
                status.sharing = false;
                let _ = signaling.send(ClientMessage::EndSession {
                    request_id: request_id.clone(),
                });
            };

            loop {
                let ev = tokio::select! {
                    ev = events.recv() => match ev {
                        Some(ev) => ev,
                        None => break,
                    },
                    Some(text) = clip_rx.recv() => {
                        if status.control && status.authenticated
                            && text.chars().count() <= kr_proto::control::MAX_CLIPBOARD_CHARS
                        {
                            control_sender.send(&ControlMessage::Clipboard { text }).await;
                        }
                        continue;
                    }
                    changed = control_rx.changed() => {
                        if changed.is_err() {
                            continue;
                        }
                        let allow = *control_rx.borrow_and_update();
                        status.control = allow;
                        if !allow {
                            // Solta o que estiver pressionado e para de aceitar entrada.
                            if let Some(c) = input.as_mut() {
                                c.release_all();
                            }
                            clipboard = None;
                        } else if status.authenticated && clipboard.is_none() {
                            clipboard = Some(start_clipboard(&clip_tx));
                        }
                        control_sender.send(&ControlMessage::Permissions { control: allow }).await;
                        info!(%request_id, control = allow, "permissão de controle alterada");
                        app.p2p_status(&status);
                        continue;
                    }
                    _ = check.tick() => {
                        if reconnect_deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                            warn!(%request_id, "conexão não voltou; encerrando a sessão");
                            let _ = signaling.send(ClientMessage::EndSession {
                                request_id: request_id.clone(),
                            });
                            status.state = PeerState::Failed;
                            status.sharing = false;
                            app.p2p_status(&status);
                            break;
                        }
                        if !status.authenticated
                            && status.security_error.is_none()
                            && auth_deadline.is_some_and(|d| tokio::time::Instant::now() >= d)
                        {
                            fail(&mut status, "o outro computador não comprovou a identidade".into());
                            app.p2p_status(&status);
                            break;
                        }
                        // O pipeline pode parar sozinho (erro de captura/encoder).
                        if status.sharing && video.as_ref().is_none_or(VideoStream::is_finished) {
                            warn!(%request_id, "transmissão da tela parou");
                            video = None;
                            status.sharing = false;
                            app.p2p_status(&status);
                        }
                        continue;
                    }
                };
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
                    PeerEvent::Fingerprints { local, remote } => {
                        status.verification_code =
                            Some(verification_code(&request_id, &local, &remote));
                        fingerprints = Some((local, remote));
                    }
                    PeerEvent::State(s) => {
                        match s {
                            PeerState::Disconnected | PeerState::Failed => {
                                if reconnect_deadline.is_none() {
                                    info!(%request_id, ?s, "conexão caiu; aguardando reconexão");
                                    // A conexão com a sinalização pode ter morrido junto
                                    // (rede caiu): abre uma nova para receber a oferta.
                                    signaling.reconnect();
                                    reconnect_deadline =
                                        Some(tokio::time::Instant::now() + RECONNECT_GRACE);
                                }
                                // Nada fica pressionado enquanto a rede está fora.
                                if let Some(c) = input.as_mut() {
                                    c.release_all();
                                }
                            }
                            PeerState::Connected if reconnect_deadline.take().is_some() => {
                                info!(%request_id, "conexão restabelecida");
                                if let Some(v) = &video {
                                    v.request_keyframe();
                                }
                            }
                            _ => {}
                        }
                        status.state = s;
                    }
                    PeerEvent::ControlOpen => {
                        status.state = PeerState::Connected;
                        // Prova a nossa identidade; vídeo e entrada só depois de
                        // verificar a do viewer.
                        match &fingerprints {
                            Some((local, remote)) => {
                                let sig =
                                    identity.sign_session(&request_id, Role::Host, local, remote);
                                control_sender
                                    .send(&ControlMessage::Auth {
                                        pk: identity.public_key_b64(),
                                        sig,
                                    })
                                    .await;
                                let allow_now = *control_rx.borrow();
                                control_sender
                                    .send(&ControlMessage::Permissions { control: allow_now })
                                    .await;
                                auth_deadline = Some(tokio::time::Instant::now() + AUTH_TIMEOUT);
                            }
                            None => {
                                fail(&mut status, "conexão sem fingerprint DTLS".into());
                                app.p2p_status(&status);
                                break;
                            }
                        }
                    }
                    PeerEvent::PeerAuth { pk, sig } => {
                        if status.authenticated || status.security_error.is_some() {
                            continue;
                        }
                        let Some((local, remote)) = &fingerprints else {
                            continue;
                        };
                        let verdict = if pk != viewer.public_key {
                            Err("a chave do outro computador não confere com a do servidor"
                                .to_string())
                        } else {
                            verify_peer(&pk, &sig, &request_id, Role::Viewer, remote, local)
                                .map_err(|e| format!("verificação falhou: {e}"))
                                .and_then(|()| {
                                    match known.lock().expect("mutex").check(&viewer.peer_id, &pk) {
                                        Trust::Changed => Err(
                                            "a identidade do outro computador mudou desde a última conexão"
                                                .to_string(),
                                        ),
                                        trust => Ok(trust),
                                    }
                                })
                        };
                        match verdict {
                            Ok(trust) => {
                                info!(%request_id, ?trust, "viewer autenticado");
                                status.authenticated = true;
                                status.first_contact = trust == Trust::New;
                                if video.is_none() {
                                    match platform
                                        .start_video(video_sender.clone(), control_sender.clone())
                                    {
                                        Ok(stream) => {
                                            info!(%request_id, "transmissão da tela iniciada");
                                            video = Some(stream);
                                            status.sharing = true;
                                        }
                                        Err(err) => warn!(%err, "tela não será transmitida"),
                                    }
                                }
                                if status.control && clipboard.is_none() {
                                    clipboard = Some(start_clipboard(&clip_tx));
                                }
                                if input.is_none() {
                                    // A entrada mira o monitor que a captura estiver transmitindo.
                                    let area =
                                        video.as_ref().map(VideoStream::area).unwrap_or_default();
                                    input = Some(kr_core::input::InputController::new(
                                        platform.input(area),
                                    ));
                                }
                            }
                            Err(reason) => {
                                fail(&mut status, reason);
                                app.p2p_status(&status);
                                break;
                            }
                        }
                    }
                    PeerEvent::Rtt(d) => status.rtt_ms = Some(d.as_millis() as u64),
                    PeerEvent::Route(r) => status.route = Some(r),
                    PeerEvent::RxReport(report) => {
                        if let (Some(target), Some(v)) = (quality.on_report(report), &video) {
                            info!(%request_id, ?target, ?report, "qualidade ajustada à rede");
                            v.set_quality(target);
                        }
                        continue;
                    }
                    PeerEvent::Clipboard(text) => {
                        if status.control
                            && let Some(c) = &clipboard
                        {
                            c.set_remote(&text);
                        }
                        continue;
                    }
                    PeerEvent::SelectMonitor(index) => {
                        if let Some(v) = &video {
                            info!(%request_id, index, "viewer pediu outro monitor");
                            v.select_monitor(index);
                        }
                        continue;
                    }
                    PeerEvent::Input(msg) => {
                        // `release_all` sempre passa; o resto só com permissão.
                        let allowed =
                            *control_rx.borrow() || matches!(msg, ControlMessage::ReleaseAll);
                        if let Some(c) = input.as_mut().filter(|_| allowed) {
                            c.handle(&msg);
                        }
                        continue;
                    }
                    PeerEvent::KeyframeRequest => {
                        if let Some(v) = &video {
                            v.request_keyframe();
                        }
                        continue;
                    }
                }
                if status.state == PeerState::Closed {
                    video = None;
                    input = None;
                    clipboard = None;
                    status.sharing = false;
                }
                app.p2p_status(&status);
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
        self.controls.lock().expect("mutex").remove(request_id);
        self.pending_control
            .lock()
            .expect("mutex")
            .remove(request_id);
        if let Some(peer) = self.peers.lock().await.remove(request_id) {
            peer.close().await;
            info!(%request_id, "conexão P2P (host) encerrada");
        }
    }
}
