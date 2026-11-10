//! Mensagens do DataChannel `control` (ponto a ponto, já cifrado pelo DTLS).
//!
//! JSON em texto, com o campo `t` como discriminador. O viewer (TypeScript no
//! WebView2) espelha estes tipos em `apps/desktop/src/lib/control.ts`.

use serde::{Deserialize, Serialize};

/// Rótulo do DataChannel de controle, criado pelo viewer.
pub const CONTROL_LABEL: &str = "control";

/// DataChannel não confiável (sem retransmissão) para movimentos do mouse:
/// um movimento perdido é substituído pelo próximo, sem atrasar os demais.
pub const INPUT_LABEL: &str = "input";

/// Coordenada normalizada (0 = borda esquerda/superior, 65535 = direita/inferior)
/// relativa à tela transmitida.
pub type NormCoord = u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Monitor do host disponível para transmissão.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorDto {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// Imagem do cursor do host (RGBA 8 bits, sem pré-multiplicação).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorImage {
    pub w: u32,
    pub h: u32,
    /// Ponto ativo (hotspot) em pixels.
    pub hx: u32,
    pub hy: u32,
    /// Pixels RGBA em base64 (`w * h * 4` bytes).
    pub rgba: String,
}

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
    /// Viewer → host: move o ponteiro (canal `input`).
    MouseMove {
        x: NormCoord,
        y: NormCoord,
    },
    /// Viewer → host: botão pressionado/solto na posição dada.
    MouseButton {
        button: MouseButton,
        down: bool,
        x: NormCoord,
        y: NormCoord,
    },
    /// Viewer → host: roda do mouse em unidades do Windows (120 = um "clique";
    /// `dy > 0` rola para cima, `dx > 0` para a direita).
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// Viewer → host: tecla por scancode (conjunto 1) — independe do layout.
    Key {
        code: u16,
        ext: bool,
        down: bool,
    },
    /// Viewer → host: solta todas as teclas e botões (perda de foco, fim).
    ReleaseAll,
    /// Host → viewer: nova forma do cursor.
    CursorShape(CursorImage),
    /// Host → viewer: cursor visível ou oculto (ex.: vídeo em tela cheia).
    CursorVisible {
        visible: bool,
    },
    /// Host → viewer: monitores disponíveis e o que está sendo transmitido.
    Monitors {
        list: Vec<MonitorDto>,
        active: u32,
    },
    /// Viewer → host: transmitir outro monitor.
    SelectMonitor {
        index: u32,
    },
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
