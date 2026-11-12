//! Backend de sessões do KR Desk: sinalização + conexões P2P do lado host.
//!
//! Roda em dois lugares:
//! - **no app** (instalação "só para mim"): host e viewer no mesmo processo;
//! - **no serviço do Windows** (instalação "para todos os usuários"): o
//!   serviço é o host deste computador, inclusive sem ninguém conectado;
//!   o app vira a interface dele (diálogo de pedido, faixa da sessão) por
//!   um pipe ([`ui`]).
//!
//! O que depende do sistema (captura, entrada, área de transferência) entra
//! por [`HostPlatform`]; o que vai para a interface sai por [`BackendEvents`].

pub mod p2p;
pub mod ui;
pub mod unattended;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kr_core::input::InputSink;
use kr_core::media::SharedArea;
use kr_identity::{DeviceIdentity, KnownPeers};
use kr_proto::{ClientMessage, ServerMessage};
use kr_signaling::{ConnectionStatus, SignalingConfig, SignalingEvent, SignalingHandle};
use kr_transport::{ControlSender, VideoSender, VideoStream};
use serde::Serialize;
use tracing::info;

pub use p2p::{P2pManager, P2pStatus, ViewerInfo};
pub use unattended::Unattended;

/// Nomes dos eventos entregues à interface (os mesmos do app).
pub const EVENT_STATUS: &str = "signaling-status";
pub const EVENT_MESSAGE: &str = "signaling-message";
pub const EVENT_P2P: &str = "p2p-status";

/// Estado da conexão com a sinalização, como a interface espera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum StatusDto {
    Connecting,
    #[serde(rename_all = "camelCase")]
    Online {
        device_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Offline {
        reason: String,
        retry_in_ms: u64,
    },
}

impl From<ConnectionStatus> for StatusDto {
    fn from(s: ConnectionStatus) -> Self {
        match s {
            ConnectionStatus::Connecting => Self::Connecting,
            ConnectionStatus::Online { device_id } => Self::Online { device_id },
            ConnectionStatus::Offline {
                reason,
                retry_in_ms,
            } => Self::Offline {
                reason,
                retry_in_ms,
            },
        }
    }
}

/// Saída do backend para a interface.
pub trait BackendEvents: Send + Sync + 'static {
    fn status(&self, status: &StatusDto);
    /// Mensagem do servidor que a interface precisa ver.
    fn message(&self, msg: &ServerMessage);
    /// Chegou um pedido de acesso: entregue-o à interface (em vez de
    /// [`message`](Self::message)). `false` = ninguém pode responder agora (o
    /// pedido é recusado na hora, em vez de o outro lado esperar 30 s).
    fn incoming_request(&self, msg: &ServerMessage) -> bool;
    fn p2p_status(&self, status: &P2pStatus);
}

/// Área de transferência do host durante uma sessão.
pub trait RemoteClipboard: Send {
    /// Escreve aqui o texto copiado no viewer.
    fn set_remote(&self, text: &str);
}

/// Recursos do sistema usados pelo host.
pub trait HostPlatform: Send + Sync + 'static {
    /// Começa a transmitir a tela.
    fn start_video(
        &self,
        video: VideoSender,
        control: ControlSender,
    ) -> Result<VideoStream, String>;
    /// Destino da entrada remota; `area` é o monitor transmitido.
    fn input(&self, area: SharedArea) -> Box<dyn InputSink + Send>;
    /// Observa a área de transferência (texto copiado aqui → `on_change`).
    fn clipboard(&self, on_change: Box<dyn Fn(String) + Send + Sync>) -> Box<dyn RemoteClipboard>;
}

/// Plataforma sem tela nem entrada (testes, Linux).
pub struct NullPlatform;

struct NullClipboard;
impl RemoteClipboard for NullClipboard {
    fn set_remote(&self, _text: &str) {}
}
struct NullSink;
impl InputSink for NullSink {
    fn move_to(&mut self, _x: u16, _y: u16) {}
    fn button(&mut self, _b: kr_proto::control::MouseButton, _d: bool) {}
    fn wheel(&mut self, _dx: i32, _dy: i32) {}
    fn key(&mut self, _c: u16, _e: bool, _d: bool) {}
}

impl HostPlatform for NullPlatform {
    fn start_video(&self, _v: VideoSender, _c: ControlSender) -> Result<VideoStream, String> {
        Err("transmissão de tela indisponível nesta plataforma".into())
    }
    fn input(&self, _area: SharedArea) -> Box<dyn InputSink + Send> {
        Box::new(NullSink)
    }
    fn clipboard(&self, _f: Box<dyn Fn(String) + Send + Sync>) -> Box<dyn RemoteClipboard> {
        Box::new(NullClipboard)
    }
}

pub struct BackendConfig {
    pub identity: Arc<DeviceIdentity>,
    pub known: Arc<Mutex<KnownPeers>>,
    /// Nome mostrado ao outro lado.
    pub device_name: String,
    pub server_url: String,
    pub version: &'static str,
    /// Onde guardar a configuração de acesso não assistido.
    pub data_dir: PathBuf,
}

/// Sinalização + sessões host. Criado com [`Backend::start`].
pub struct Backend {
    handle: SignalingHandle,
    p2p: Arc<P2pManager>,
    status: Mutex<StatusDto>,
    device_name: String,
    identity: Arc<DeviceIdentity>,
    known: Arc<Mutex<KnownPeers>>,
    unattended: Arc<Unattended>,
}

/// Normaliza o ID digitado ("123 456 789" → "123456789") e valida.
pub fn normalize_id(raw: &str) -> Result<String, String> {
    let id: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    let valid = id.len() == 9 && id.chars().all(|c| c.is_ascii_digit()) && !id.starts_with('0');
    if valid {
        Ok(id)
    } else {
        Err("O ID deve ter 9 dígitos.".into())
    }
}

pub fn is_request_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

/// Nome do computador (Windows: `COMPUTERNAME`).
pub fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().take(64).collect())
        .unwrap_or_else(|| "Computador".to_string())
}

/// Endereço do servidor: `KR_DESK_SIGNALING_URL` (para testes) ou produção.
pub fn server_url() -> String {
    std::env::var("KR_DESK_SIGNALING_URL").unwrap_or_else(|_| kr_signaling::DEFAULT_URL.to_string())
}

impl Backend {
    /// Conecta à sinalização e passa a atender pedidos. Precisa rodar dentro
    /// de um runtime Tokio.
    pub fn start(
        config: BackendConfig,
        events: Arc<dyn BackendEvents>,
        platform: Arc<dyn HostPlatform>,
    ) -> Arc<Self> {
        let signaling = SignalingConfig::new(
            config.server_url,
            config.device_name.clone(),
            config.version,
        );
        let (handle, mut rx) = kr_signaling::spawn(signaling, config.identity.clone());
        let unattended = Arc::new(Unattended::load(&config.data_dir));
        let p2p = Arc::new(P2pManager::new(
            config.identity.clone(),
            config.known.clone(),
            events.clone(),
            platform,
            unattended.clone(),
        ));
        let backend = Arc::new(Self {
            handle,
            p2p,
            status: Mutex::new(StatusDto::Connecting),
            device_name: config.device_name,
            identity: config.identity,
            known: config.known,
            unattended,
        });
        let this = backend.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                this.on_event(event, &events).await;
            }
        });
        backend
    }

    async fn on_event(self: &Arc<Self>, event: SignalingEvent, events: &Arc<dyn BackendEvents>) {
        let msg = match event {
            SignalingEvent::Status(status) => {
                let dto = StatusDto::from(status);
                *self.status.lock().expect("mutex") = dto.clone();
                events.status(&dto);
                return;
            }
            SignalingEvent::Message(msg) => msg,
        };
        match &msg {
            ServerMessage::IncomingRequest {
                request_id,
                unattended: true,
                ..
            } if self.unattended.enabled() => {
                // O viewer diz ter a senha: aceita já e exige a senha pelo
                // canal P2P antes de mostrar qualquer coisa.
                info!(%request_id, "pedido de acesso não assistido; a senha será exigida");
                self.p2p.require_password(request_id);
                self.p2p.set_pending_control(request_id, true);
                let _ = self.handle.send(ClientMessage::ConnectResponse {
                    request_id: request_id.clone(),
                    accept: true,
                });
                return;
            }
            ServerMessage::IncomingRequest { request_id, .. } => {
                // A interface recebe o pedido por `incoming_request`.
                if !events.incoming_request(&msg) {
                    info!(%request_id, "pedido recusado: ninguém para responder aqui");
                    let _ = self.handle.send(ClientMessage::ConnectResponse {
                        request_id: request_id.clone(),
                        accept: false,
                    });
                }
                return;
            }
            ServerMessage::SessionReady {
                request_id,
                ice_servers,
                peer_id,
                peer_public_key,
            } => {
                // Aceitamos o pedido: este computador é o host da conexão P2P.
                let (handle, p2p) = (self.handle.clone(), self.p2p.clone());
                let (rid, ice) = (request_id.clone(), ice_servers.clone());
                let viewer = ViewerInfo {
                    peer_id: peer_id.clone(),
                    public_key: peer_public_key.clone(),
                };
                tokio::spawn(async move {
                    p2p.start_host(handle, rid, ice, viewer).await;
                });
            }
            ServerMessage::Signal {
                request_id,
                payload,
                ..
            } => {
                // Sinais de sessões em que somos host ficam aqui; os demais
                // (somos viewer) seguem para a interface.
                if self.p2p.handle_signal(request_id, payload.clone()).await {
                    return;
                }
            }
            ServerMessage::SessionEnded { request_id } => {
                self.p2p.close(request_id).await;
            }
            _ => {}
        }
        events.message(&msg);
    }

    pub fn status(&self) -> StatusDto {
        self.status.lock().expect("mutex").clone()
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn identity(&self) -> &Arc<DeviceIdentity> {
        &self.identity
    }

    pub fn known(&self) -> &Arc<Mutex<KnownPeers>> {
        &self.known
    }

    /// Pede acesso a outro dispositivo. Retorna o `request_id` da sessão.
    /// `unattended`: temos a senha de acesso não assistido do outro lado.
    pub fn connect_to(&self, target_id: &str, unattended: bool) -> Result<String, String> {
        let target_id = normalize_id(target_id)?;
        let request_id = uuid::Uuid::new_v4().to_string();
        self.handle
            .send(ClientMessage::ConnectRequest {
                request_id: request_id.clone(),
                target_id,
                unattended,
            })
            .map_err(|e| e.to_string())?;
        Ok(request_id)
    }

    /// Responde a um pedido recebido (host).
    pub fn respond_request(
        &self,
        request_id: &str,
        accept: bool,
        allow_control: bool,
    ) -> Result<(), String> {
        if !is_request_id(request_id) {
            return Err("Pedido inválido.".into());
        }
        if accept {
            self.p2p.set_pending_control(request_id, allow_control);
        }
        self.handle
            .send(ClientMessage::ConnectResponse {
                request_id: request_id.to_owned(),
                accept,
            })
            .map_err(|e| e.to_string())
    }

    /// Encerra ou cancela uma sessão/pedido (e a conexão P2P, se houver).
    pub async fn end_session(&self, request_id: &str) -> Result<(), String> {
        if !is_request_id(request_id) {
            return Err("Sessão inválida.".into());
        }
        self.p2p.close(request_id).await;
        self.handle
            .send(ClientMessage::EndSession {
                request_id: request_id.to_owned(),
            })
            .map_err(|e| e.to_string())
    }

    /// Envia um sinal WebRTC (oferta/candidato) do viewer ao host.
    pub fn send_signal(&self, request_id: &str, payload: serde_json::Value) -> Result<(), String> {
        if !is_request_id(request_id) {
            return Err("Sessão inválida.".into());
        }
        if !payload.is_object() {
            return Err("Sinal inválido.".into());
        }
        self.handle
            .send(ClientMessage::Signal {
                request_id: request_id.to_owned(),
                payload,
            })
            .map_err(|e| e.to_string())
    }

    /// Host: permite ou bloqueia o controle remoto durante a sessão.
    pub fn set_session_control(&self, request_id: &str, allow: bool) -> bool {
        self.p2p.set_control(request_id, allow)
    }

    /// O acesso não assistido está ativo?
    pub fn unattended_enabled(&self) -> bool {
        self.unattended.enabled()
    }

    /// Define a senha de acesso não assistido (e ativa). Lento (Argon2).
    pub fn set_unattended_password(&self, password: &str) -> Result<(), String> {
        self.unattended.set_password(password)
    }

    pub fn disable_unattended(&self) -> Result<(), String> {
        self.unattended.disable()
    }

    /// Desconecta da sinalização (o app está fechando).
    pub fn shutdown(&self) {
        self.handle.shutdown();
    }

    /// Abre uma conexão nova com a sinalização (rede mudou).
    pub fn reconnect(&self) {
        self.handle.reconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normaliza_ids() {
        assert_eq!(normalize_id("123 456 789").unwrap(), "123456789");
        assert_eq!(normalize_id("123-456-789").unwrap(), "123456789");
        assert!(normalize_id("12345678").is_err());
        assert!(normalize_id("012345678").is_err());
        assert!(normalize_id("12345678a").is_err());
    }

    #[test]
    fn status_no_formato_da_interface() {
        let s = serde_json::to_value(StatusDto::Online {
            device_id: "123456789".into(),
        })
        .unwrap();
        assert_eq!(
            s,
            serde_json::json!({"state": "online", "deviceId": "123456789"})
        );
    }
}
