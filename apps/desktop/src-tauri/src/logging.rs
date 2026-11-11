//! Log em arquivo (`%LOCALAPPDATA%\com.krdesk.app\logs\kr-desk.log`; o
//! serviço e o agente escrevem em `logs\` na pasta de instalação).
//!
//! Na versão final não há console; sem este arquivo, um erro ao iniciar
//! fecharia o app sem deixar rastro.

use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;

/// Tamanho a partir do qual o log antigo é renomeado para `.old`.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub fn init(dir: &Path, file: &str) {
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(file);
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(&path, dir.join(format!("{file}.old")));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };

    let filter = EnvFilter::try_from_env("KR_DESK_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,tungstenite=warn,tokio_tungstenite=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .try_init();

    // Registra pânicos no arquivo antes do comportamento padrão.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "pânico");
        default_hook(info);
    }));
}
