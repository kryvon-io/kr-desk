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
    bridge: State<'_, Bridge>,
) -> Result<(), String> {
    if !is_request_id(&request_id) {
        return Err("Pedido inválido.".into());
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
