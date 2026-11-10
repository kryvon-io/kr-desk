//! Comandos chamados pela interface (`invoke`).

use kr_core::AppInfo;
use kr_proto::ClientMessage;
use serde::Serialize;
use tauri::State;

use crate::signaling::{Bridge, StatusDto};

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
}

/// Estado atual (a interface também recebe atualizações pelo evento `signaling-status`).
#[tauri::command]
pub fn signaling_status(bridge: State<'_, Bridge>) -> SignalingStatus {
    SignalingStatus {
        status: bridge.status.lock().expect("mutex envenenado").clone(),
        device_name: bridge.device_name.clone(),
    }
}

/// Normaliza o ID digitado ("123 456 789" → "123456789") e valida.
fn normalize_id(raw: &str) -> Result<String, String> {
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

fn is_request_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

/// Pede acesso a outro dispositivo. Retorna o `request_id` da sessão.
#[tauri::command]
pub fn connect_to(target_id: String, bridge: State<'_, Bridge>) -> Result<String, String> {
    let target_id = normalize_id(&target_id)?;
    let request_id = uuid::Uuid::new_v4().to_string();
    bridge
        .handle
        .send(ClientMessage::ConnectRequest {
            request_id: request_id.clone(),
            target_id,
        })
        .map_err(|e| e.to_string())?;
    Ok(request_id)
}

/// Responde a um pedido recebido (host).
#[tauri::command]
pub fn respond_request(
    request_id: String,
    accept: bool,
    allow_control: Option<bool>,
    bridge: State<'_, Bridge>,
) -> Result<(), String> {
    if !is_request_id(&request_id) {
        return Err("Pedido inválido.".into());
    }
    if accept {
        bridge
            .p2p
            .set_pending_control(&request_id, allow_control.unwrap_or(true));
    }
    bridge
        .handle
        .send(ClientMessage::ConnectResponse { request_id, accept })
        .map_err(|e| e.to_string())
}

/// Encerra ou cancela uma sessão/pedido (e a conexão P2P, se houver).
#[tauri::command]
pub async fn end_session(request_id: String, bridge: State<'_, Bridge>) -> Result<(), String> {
    if !is_request_id(&request_id) {
        return Err("Sessão inválida.".into());
    }
    bridge.p2p.close(&request_id).await;
    bridge
        .handle
        .send(ClientMessage::EndSession { request_id })
        .map_err(|e| e.to_string())
}

/// Envia um sinal WebRTC (oferta/candidato) do viewer ao host.
#[tauri::command]
pub fn send_signal(
    request_id: String,
    payload: serde_json::Value,
    bridge: State<'_, Bridge>,
) -> Result<(), String> {
    if !is_request_id(&request_id) {
        return Err("Sessão inválida.".into());
    }
    if !payload.is_object() {
        return Err("Sinal inválido.".into());
    }
    bridge
        .handle
        .send(ClientMessage::Signal {
            request_id,
            payload,
        })
        .map_err(|e| e.to_string())
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
}

/// Liga/desliga a captura nativa do teclado (viewer com a tela remota
/// aberta). Devolve `true` se o hook do Windows está cuidando do teclado.
#[tauri::command]
pub fn set_keyboard_capture(enabled: bool, app: tauri::AppHandle) -> bool {
    use tauri::Manager;
    app.try_state::<crate::remote_input::KeyboardCapture>()
        .is_some_and(|c| c.set_enabled(enabled))
}

/// Chaves usadas na autenticação da sessão (viewer e host).
pub struct Session {
    pub identity: std::sync::Arc<kr_identity::DeviceIdentity>,
    pub known: std::sync::Arc<std::sync::Mutex<kr_identity::KnownPeers>>,
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
pub fn sign_session(message: String, session: State<'_, Session>) -> Result<SessionProof, String> {
    let signature = session
        .identity
        .sign_session_message(&message)
        .map_err(|e| e.to_string())?;
    Ok(SessionProof {
        public_key: session.identity.public_key_b64(),
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
    session: State<'_, Session>,
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
    match session
        .known
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
pub fn forget_peer(peer_id: String, session: State<'_, Session>) {
    session.known.lock().expect("mutex").forget(&peer_id);
    tracing::info!(%peer_id, "chave memorizada removida");
}

/// Host: permite ou bloqueia o controle remoto durante a sessão.
#[tauri::command]
pub fn set_session_control(request_id: String, allow: bool, bridge: State<'_, Bridge>) -> bool {
    bridge.p2p.set_control(&request_id, allow)
}
