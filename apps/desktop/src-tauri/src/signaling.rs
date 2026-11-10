//! Ponte entre o cliente de sinalização (Rust) e a interface (eventos Tauri).

use std::sync::{Arc, Mutex};

use kr_identity::DeviceIdentity;
use kr_proto::ServerMessage;
use kr_signaling::{ConnectionStatus, SignalingConfig, SignalingEvent, SignalingHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, UserAttentionType};

/// Evento com o estado da conexão (payload: [`StatusDto`]).
pub const EVENT_STATUS: &str = "signaling-status";
/// Evento com mensagens do servidor (payload: `ServerMessage` em JSON do protocolo).
pub const EVENT_MESSAGE: &str = "signaling-message";

/// Estado compartilhado com os comandos.
pub struct Bridge {
    pub handle: SignalingHandle,
    pub status: Mutex<StatusDto>,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize)]
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

/// Nome exibido para o outro lado (nome do computador no Windows).
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().take(64).collect())
        .unwrap_or_else(|| "Computador".to_string())
}

/// Endereço do servidor: `KR_DESK_SIGNALING_URL` (para testes) ou produção.
fn server_url() -> String {
    std::env::var("KR_DESK_SIGNALING_URL").unwrap_or_else(|_| kr_signaling::DEFAULT_URL.to_string())
}

pub fn start(app: AppHandle, identity: Arc<DeviceIdentity>) -> Bridge {
    let device_name = device_name();
    let config = SignalingConfig::new(server_url(), device_name.clone(), env!("CARGO_PKG_VERSION"));

    // O cliente usa `tokio::spawn`; entra no runtime do Tauri para iniciá-lo.
    let rt = tauri::async_runtime::handle();
    let _guard = rt.inner().enter();
    let (handle, mut events) = kr_signaling::spawn(config, identity);

    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                SignalingEvent::Status(status) => {
                    let dto = StatusDto::from(status);
                    if let Some(bridge) = app_for_task.try_state::<Bridge>() {
                        *bridge.status.lock().expect("mutex envenenado") = dto.clone();
                    }
                    let _ = app_for_task.emit(EVENT_STATUS, dto);
                }
                SignalingEvent::Message(msg) => {
                    if matches!(msg, ServerMessage::IncomingRequest { .. }) {
                        // Pedido de acesso: traz a janela à frente e chama atenção.
                        crate::show_main_window(&app_for_task);
                        if let Some(w) = app_for_task.get_webview_window(crate::MAIN_WINDOW) {
                            let _ = w.request_user_attention(Some(UserAttentionType::Critical));
                        }
                    }
                    let _ = app_for_task.emit(EVENT_MESSAGE, msg);
                }
            }
        }
    });

    Bridge {
        handle,
        status: Mutex::new(StatusDto::Connecting),
        device_name,
    }
}
