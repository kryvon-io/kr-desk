//! Atualização automática.
//!
//! O app consulta `latest.json` na última release do GitHub (ver
//! `.github/workflows/release.yml`). O instalador novo só é aceito se a
//! assinatura dele bater com a chave pública embutida em `tauri.conf.json`
//! (a privada fica só nos segredos do GitHub). A instalação pede a
//! confirmação do usuário na interface; na instalação "para todos os
//! usuários", o Windows ainda pede permissão de administrador.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Evento para a interface quando há versão nova.
pub const EVENT_UPDATE: &str = "update-available";
/// Primeira consulta logo depois de abrir; depois, a cada 6 horas.
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
}

async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone(),
    });
    *app.state::<PendingUpdate>().0.lock().expect("mutex") = update;
    Ok(info)
}

/// Consulta periódica (só em builds de release: em desenvolvimento não há
/// versão publicada para comparar).
pub fn spawn_checks(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            match check(&app).await {
                Ok(Some(info)) => {
                    tracing::info!(version = %info.version, "atualização disponível");
                    let _ = app.emit(EVENT_UPDATE, info);
                }
                Ok(None) => tracing::debug!("nenhuma atualização"),
                Err(err) => tracing::warn!(%err, "não foi possível consultar atualizações"),
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Consulta agora (a interface chama ao abrir).
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    check(&app).await
}

/// Baixa, confere a assinatura, instala e reinicia o app.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    let update = pending
        .0
        .lock()
        .expect("mutex")
        .take()
        .ok_or("Nenhuma atualização pendente.")?;
    tracing::info!(version = %update.version, "instalando atualização");
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("Falha ao instalar a atualização: {e}"))?;
    app.restart();
}
