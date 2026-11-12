//! Comandos chamados pela interface (`invoke`).
//!
//! Com o serviço instalado, o que é do host (responder pedidos, encerrar ou
//! mudar a permissão das sessões dele) vai para o serviço; o resto (acessar
//! outros computadores) fica no backend deste processo.

use kr_core::AppInfo;
use kr_host::StatusDto;
use kr_host::ui::UiCommand;
use serde::Serialize;
use tauri::State;

use crate::backend::AppState;

/// Versão do app e do protocolo.
#[tauri::command]
pub fn app_info() -> AppInfo {
    kr_core::app_info()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalingStatus {
    pub status: StatusDto,
    pub device_name: String,
    /// O serviço do KR Desk é o host deste computador.
    pub service: bool,
}

/// Estado atual (a interface também recebe atualizações pelo evento `signaling-status`).
#[tauri::command]
pub fn signaling_status(state: State<'_, AppState>) -> SignalingStatus {
    match &state.service {
        Some(link) => {
            let (status, device_name) = link.status();
            SignalingStatus {
                status,
                device_name,
                service: true,
            }
        }
        None => SignalingStatus {
            status: state.local.status(),
            device_name: state.local.device_name().to_owned(),
            service: false,
        },
    }
}

/// Pede acesso a outro dispositivo. Retorna o `request_id` da sessão.
/// `unattended`: o usuário informou a senha de acesso não assistido (que vai
/// depois, pelo canal P2P, nunca pelo servidor).
#[tauri::command]
pub fn connect_to(
    target_id: String,
    unattended: Option<bool>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .local
        .connect_to(&target_id, unattended.unwrap_or(false))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnattendedStatus {
    pub enabled: bool,
    /// Este usuário pode mudar a configuração (com o serviço: só administradores).
    pub can_configure: bool,
    /// A configuração vale para o computador (serviço) ou só para este app.
    pub machine_wide: bool,
}

/// Situação do acesso não assistido deste computador.
#[tauri::command]
pub fn unattended_status(state: State<'_, AppState>) -> Result<UnattendedStatus, String> {
    match &state.service {
        Some(link) => {
            let v = link.call(UiCommand::UnattendedStatus)?;
            Ok(UnattendedStatus {
                enabled: v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(false),
                can_configure: v
                    .get("canConfigure")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
                machine_wide: true,
            })
        }
        None => Ok(UnattendedStatus {
            enabled: state.local.unattended_enabled(),
            can_configure: true,
            machine_wide: false,
        }),
    }
}

/// Define a senha de acesso não assistido (e ativa).
#[tauri::command]
pub async fn set_unattended_password(
    password: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    match &state.service {
        Some(link) => {
            let link = link.clone();
            tauri::async_runtime::spawn_blocking(move || {
                link.call(UiCommand::SetUnattendedPassword { password })
            })
            .await
            .map_err(|e| e.to_string())?
            .map(|_| ())
        }
        None => {
            let local = state.local.clone();
            tauri::async_runtime::spawn_blocking(move || local.set_unattended_password(&password))
                .await
                .map_err(|e| e.to_string())?
        }
    }
}

/// Desativa o acesso não assistido.
#[tauri::command]
pub fn disable_unattended(state: State<'_, AppState>) -> Result<(), String> {
    match &state.service {
        Some(link) => link.call(UiCommand::DisableUnattended).map(|_| ()),
        None => state.local.disable_unattended(),
    }
}

/// Responde a um pedido recebido (host).
#[tauri::command]
pub fn respond_request(
    request_id: String,
    accept: bool,
    allow_control: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let allow_control = allow_control.unwrap_or(true);
    match &state.service {
        Some(link) => link
            .call(UiCommand::RespondRequest {
                request_id,
                accept,
                allow_control,
            })
            .map(|_| ()),
        None => state
            .local
            .respond_request(&request_id, accept, allow_control),
    }
}

/// Encerra ou cancela uma sessão/pedido (e a conexão P2P, se houver).
#[tauri::command]
pub async fn end_session(request_id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(link) = state.service.as_ref().filter(|l| l.owns(&request_id)) {
        return link.call(UiCommand::EndSession { request_id }).map(|_| ());
    }
    state.local.end_session(&request_id).await
}

/// Envia um sinal WebRTC (oferta/candidato) do viewer ao host.
#[tauri::command]
pub fn send_signal(
    request_id: String,
    payload: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.local.send_signal(&request_id, payload)
}

/// Liga/desliga a captura nativa do teclado (viewer com a tela remota
/// aberta). Devolve `true` se o hook do Windows está cuidando do teclado.
#[tauri::command]
pub fn set_keyboard_capture(enabled: bool, app: tauri::AppHandle) -> bool {
    use tauri::Manager;
    app.try_state::<crate::remote_input::KeyboardCapture>()
        .is_some_and(|c| c.set_enabled(enabled))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProof {
    pub public_key: String,
    pub signature: String,
}

/// Viewer: assina a mensagem de sessão montada pela interface. Só aceita
/// mensagens com o contexto de sessão (a chave não assina outras coisas).
#[tauri::command]
pub fn sign_session(message: String, state: State<'_, AppState>) -> Result<SessionProof, String> {
    let identity = state.local.identity();
    let signature = identity
        .sign_session_message(&message)
        .map_err(|e| e.to_string())?;
    Ok(SessionProof {
        public_key: identity.public_key_b64(),
        signature,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "result")]
pub enum VerifyResult {
    /// Identidade confirmada; `firstContact` = chave memorizada agora.
    Ok {
        first_contact: bool,
    },
    /// A chave mudou desde a última conexão com este computador.
    Changed,
    Failed {
        reason: String,
    },
}

/// Viewer: verifica a prova do host e aplica TOFU pelo ID dele.
#[tauri::command]
pub fn verify_session(
    peer_id: String,
    expected_public_key: String,
    public_key: String,
    signature: String,
    message: String,
    state: State<'_, AppState>,
) -> VerifyResult {
    if public_key != expected_public_key {
        return VerifyResult::Failed {
            reason: "a chave do outro computador não confere com a do servidor".into(),
        };
    }
    if let Err(err) = kr_identity::verify_session_message(&public_key, &signature, &message) {
        return VerifyResult::Failed {
            reason: format!("verificação falhou: {err}"),
        };
    }
    match state
        .local
        .known()
        .lock()
        .expect("mutex")
        .check(&peer_id, &public_key)
    {
        kr_identity::Trust::Changed => VerifyResult::Changed,
        trust => VerifyResult::Ok {
            first_contact: trust == kr_identity::Trust::New,
        },
    }
}

/// Esquece a chave memorizada de um computador (o usuário confirmou que ele
/// foi reinstalado).
#[tauri::command]
pub fn forget_peer(peer_id: String, state: State<'_, AppState>) {
    state.local.known().lock().expect("mutex").forget(&peer_id);
    tracing::info!(%peer_id, "chave memorizada removida");
}

/// Host: permite ou bloqueia o controle remoto durante a sessão.
#[tauri::command]
pub fn set_session_control(request_id: String, allow: bool, state: State<'_, AppState>) -> bool {
    match &state.service {
        Some(link) => link
            .call(UiCommand::SetSessionControl { request_id, allow })
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        None => state.local.set_session_control(&request_id, allow),
    }
}

/// Viewer: liga/desliga o envio do texto copiado aqui (evento
/// `local-clipboard`).
#[tauri::command]
pub fn set_viewer_clipboard(
    enabled: bool,
    app: tauri::AppHandle,
    clipboard: State<'_, crate::remote_input::ViewerClipboard>,
) {
    clipboard.set_enabled(&app, enabled);
}

/// Viewer: escreve aqui o texto copiado no outro computador.
#[tauri::command]
pub fn write_clipboard(text: String, clipboard: State<'_, crate::remote_input::ViewerClipboard>) {
    if text.chars().count() <= kr_proto::control::MAX_CLIPBOARD_CHARS {
        clipboard.set_remote(&text);
    }
}

/// Abre uma conexão nova com a sinalização (rede mudou durante uma sessão).
#[tauri::command]
pub fn signaling_reconnect(state: State<'_, AppState>) {
    state.local.reconnect();
}
