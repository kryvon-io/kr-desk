//! Mensagens do DataChannel `control` (ponto a ponto, já cifrado pelo DTLS).
//!
//! JSON em texto, com o campo `t` como discriminador. O viewer (TypeScript no
//! WebView2) espelha estes tipos em `apps/desktop/src/lib/control.ts`.

use serde::{Deserialize, Serialize};

/// Rótulo do DataChannel de controle, criado pelo viewer.
pub const CONTROL_LABEL: &str = "control";

/// Tipo de candidato ICE usado pela conexão (RFC 8445).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CandidateType {
    Host,
    Srflx,
    Prflx,
    Relay,
}

impl CandidateType {
    /// Conexão passa por um servidor TURN?
    pub fn is_relay(self) -> bool {
        self == Self::Relay
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Medição de latência: o outro lado responde `pong` com o mesmo `n`.
    Ping {
        n: u64,
    },
    Pong {
        n: u64,
    },
    /// Rota escolhida pelo ICE, informada pelo viewer (que lê `getStats()`).
    Route {
        local: CandidateType,
        remote: CandidateType,
        protocol: String,
    },
    /// Viewer → host: pede um keyframe (ex.: o `<video>` acabou de montar).
    Keyframe,
}

impl ControlMessage {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("serialização não falha")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_match_fixtures() {
        let text = include_str!("../fixtures/control-messages.json");
        let items: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        for item in items {
            let parsed: ControlMessage = serde_json::from_value(item.clone()).unwrap();
            assert_eq!(serde_json::to_value(&parsed).unwrap(), item);
        }
    }
}
