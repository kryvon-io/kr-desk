//! Núcleo do KR Desk, independente de plataforma.
//!
//! Contém as traits implementadas pelos crates `*-win` e a lógica que não
//! depende do Windows (ex.: o pipeline de vídeo em [`media`]).

pub mod input;
pub mod media;

use serde::Serialize;

/// Informações de versão exibidas na interface e enviadas no handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub protocol_version: u16,
    pub target_os: &'static str,
    pub target_arch: &'static str,
}

/// Retorna as informações da build atual.
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "KR Desk",
        version: env!("CARGO_PKG_VERSION"),
        protocol_version: kr_proto::PROTOCOL_VERSION,
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_matches_workspace() {
        let info = app_info();
        assert_eq!(info.name, "KR Desk");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.protocol_version, kr_proto::PROTOCOL_VERSION);
    }

    #[test]
    fn app_info_serializes_camel_case() {
        let json = serde_json::to_value(app_info()).unwrap();
        assert!(json.get("protocolVersion").is_some());
        assert!(json.get("targetOs").is_some());
    }
}
