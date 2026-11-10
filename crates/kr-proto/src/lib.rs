//! Tipos do protocolo do KR Desk.
//!
//! Nesta etapa existe apenas o envelope versionado e mensagens de diagnóstico.
//! As mensagens de sinalização reais (`hello`, `auth`, `connect_request`, ...)
//! entram na Etapa 1.

use serde::{Deserialize, Serialize};

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

/// Mensagens de diagnóstico (usadas para testar a conexão).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticMessage {
    Ping { nonce: u64 },
    Pong { nonce: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_uses_flat_json() {
        let env = Envelope::new(DiagnosticMessage::Ping { nonce: 42 });
        let json = serde_json::to_string(&env).unwrap();
        assert_eq!(json, r#"{"v":1,"type":"ping","nonce":42}"#);

        let back: Envelope<DiagnosticMessage> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn unknown_message_type_is_rejected() {
        let err = serde_json::from_str::<Envelope<DiagnosticMessage>>(r#"{"v":1,"type":"nope"}"#);
        assert!(err.is_err());
    }
}
