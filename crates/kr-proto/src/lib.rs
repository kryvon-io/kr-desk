//! Tipos do protocolo do KR Desk.
//!
//! O protocolo de sinalização é JSON sobre WebSocket. Toda mensagem é um
//! [`Envelope`] com a versão do protocolo (`v`) e um campo `type` em snake_case.
//! O servidor (TypeScript) espelha estes tipos em `apps/server/src/protocol.ts`;
//! os arquivos em `fixtures/` são lidos pelos testes dos dois lados para
//! garantir que continuem compatíveis.
//!
//! Fluxo resumido:
//! 1. servidor → `hello { nonce }`
//! 2. cliente → `auth { public_key, signature(AUTH_CONTEXT + nonce), ... }`
//! 3. servidor → `auth_ok { device_id }`
//! 4. viewer → `connect_request { request_id, target_id }`
//! 5. host ← `incoming_request` ; host → `connect_response { accept }`
//! 6. viewer ← `connect_accepted` ; host ← `session_ready` (ambos com `ice_servers`)
//! 7. troca de `signal { payload }` (SDP/ICE opacos para o servidor)

use serde::{Deserialize, Serialize};

pub mod auth;
pub mod control;

/// Versão do protocolo de sinalização. Incrementar a cada mudança incompatível.
pub const PROTOCOL_VERSION: u16 = 1;

/// Envelope de toda mensagem trocada com o servidor de sinalização.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// Versão do protocolo de quem enviou.
    pub v: u16,
    /// Conteúdo da mensagem.
    #[serde(flatten)]
    pub msg: T,
}

impl<T> Envelope<T> {
    pub fn new(msg: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            msg,
        }
    }
}

/// Servidor STUN/TURN entregue ao cliente para montar o `RTCPeerConnection`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// Mensagens enviadas pelo cliente (app desktop) ao servidor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Prova de posse da chave do dispositivo. Registra o dispositivo na primeira vez.
    Auth {
        /// Chave pública Ed25519 (32 bytes, base64url sem padding).
        public_key: String,
        /// Assinatura Ed25519 de `AUTH_CONTEXT + nonce` (64 bytes, base64url sem padding).
        signature: String,
        device_name: String,
        app_version: String,
    },
    /// Viewer pede para controlar outro dispositivo.
    ConnectRequest {
        /// UUID v4 gerado pelo cliente; identifica a sessão daqui em diante.
        request_id: String,
        /// ID de 9 dígitos do dispositivo alvo.
        target_id: String,
    },
    /// Host aceita ou recusa um pedido recebido.
    ConnectResponse { request_id: String, accept: bool },
    /// Mensagem WebRTC (offer/answer/candidate) repassada sem interpretação.
    Signal {
        request_id: String,
        payload: serde_json::Value,
    },
    /// Encerra (ou cancela) a sessão.
    EndSession { request_id: String },
    /// Heartbeat; renova a presença.
    Ping { nonce: u64 },
}

/// Mensagens enviadas pelo servidor ao cliente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Primeira mensagem após conectar; o cliente deve responder com `auth`.
    Hello {
        nonce: String,
        server_time_ms: u64,
    },
    AuthOk {
        device_id: String,
    },
    /// Chegou um pedido de conexão (somente no host).
    IncomingRequest {
        request_id: String,
        from_id: String,
        from_name: String,
        from_public_key: String,
    },
    /// O pedido foi entregue ao host e aguarda resposta (somente no viewer).
    ConnectPending {
        request_id: String,
    },
    /// O host aceitou (somente no viewer).
    ConnectAccepted {
        request_id: String,
        peer_id: String,
        peer_public_key: String,
        ice_servers: Vec<IceServer>,
    },
    /// Sessão confirmada após o aceite (somente no host).
    SessionReady {
        request_id: String,
        peer_id: String,
        peer_public_key: String,
        ice_servers: Vec<IceServer>,
    },
    ConnectRejected {
        request_id: String,
        reason: RejectReason,
    },
    Signal {
        request_id: String,
        from_id: String,
        payload: serde_json::Value,
    },
    /// O outro lado encerrou a sessão ou desconectou.
    SessionEnded {
        request_id: String,
    },
    Pong {
        nonce: u64,
    },
    /// O servidor vai fechar esta conexão em breve; o cliente deve abrir outra antes.
    ReconnectSoon {
        in_ms: u64,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Declined,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadMessage,
    UnsupportedVersion,
    AuthRequired,
    AuthFailed,
    TargetOffline,
    SelfConnect,
    RateLimited,
    UnknownRequest,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture(name: &str) -> Value {
        let path = format!("{}/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_str(&text).unwrap()
    }

    /// Cada fixture deve desserializar e serializar de volta para o mesmo JSON.
    fn roundtrip<T>(name: &str)
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let original = fixture(name);
        let items = original.as_array().expect("fixture deve ser um array");
        for item in items {
            let parsed: Envelope<T> = serde_json::from_value(item.clone())
                .unwrap_or_else(|e| panic!("{name}: {item} → {e}"));
            let back = serde_json::to_value(&parsed).unwrap();
            assert_eq!(&back, item, "{name}: roundtrip mudou o JSON");
        }
    }

    #[test]
    fn client_messages_match_fixtures() {
        roundtrip::<ClientMessage>("client-messages");
    }

    #[test]
    fn server_messages_match_fixtures() {
        roundtrip::<ServerMessage>("server-messages");
    }

    #[test]
    fn envelope_is_flat() {
        let env = Envelope::new(ClientMessage::Ping { nonce: 7 });
        assert_eq!(
            serde_json::to_value(&env).unwrap(),
            json!({"v": 1, "type": "ping", "nonce": 7})
        );
    }

    #[test]
    fn unknown_type_is_rejected() {
        let r = serde_json::from_value::<Envelope<ClientMessage>>(json!({"v": 1, "type": "nope"}));
        assert!(r.is_err());
    }
}
