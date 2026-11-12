//! Binário Tauri do KR Desk. Mantido fino: a lógica fica nos crates `kr-*`.

mod backend;
mod commands;
mod logging;
mod remote_input;
mod screen;
mod tray;
mod updater;

use std::sync::Arc;

use kr_identity::{DeviceIdentity, KnownPeers, SystemProtector};
use tauri::{Manager, WindowEvent};

/// Rótulo da janela principal (definido implicitamente pelo tauri.conf.json).
pub const MAIN_WINDOW: &str = "main";

/// Mostra, restaura e foca a janela principal.
pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Modos do executável usados pelo serviço e pelo instalador
/// (`--service`, `--agent`, `--install-service`, `--stop-service`,
/// `--uninstall-service`). Devolve o código de saída, ou `None` para abrir o
/// app normalmente.
#[cfg(windows)]
fn service_mode() -> Option<i32> {
    let arg = std::env::args().nth(1)?;
    let (file, action): (&str, fn() -> std::io::Result<()>) = match arg.as_str() {
        "--service" => ("service.log", kr_service_win::run_service),
        "--agent" => ("agent.log", kr_service_win::run_agent),
        "--install-service" => ("install.log", kr_service_win::install_service),
        "--stop-service" => ("install.log", kr_service_win::stop_service),
        "--uninstall-service" => ("install.log", kr_service_win::uninstall_service),
        _ => return None,
    };
    // Pasta de instalação (Arquivos de Programas): só administradores escrevem,
    // então um usuário comum não consegue redirecionar o log do SYSTEM.
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("logs")))
    {
        logging::init(&dir, file);
    }
    match action() {
        Ok(()) => Some(0),
        Err(err) => {
            tracing::error!(%err, mode = %arg, "falha");
            Some(1)
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    if let Some(code) = service_mode() {
        std::process::exit(code);
    }
    tauri::Builder::default()
        // Precisa ser o primeiro plugin: uma segunda execução só traz a janela à frente.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::PendingUpdate::default())
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir()?;
            logging::init(&data_dir.join("logs"), "kr-desk.log");
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "KR Desk iniciando");
            let identity = Arc::new(DeviceIdentity::load_or_create(&data_dir, &SystemProtector)?);
            tracing::info!(public_key = %identity.public_key_b64(), "identidade carregada");

            // Chaves dos computadores já conectados (TOFU), usadas nos dois papéis.
            let known = Arc::new(std::sync::Mutex::new(KnownPeers::load(&data_dir)));
            app.manage(backend::AppState::start(
                app.handle(),
                identity,
                known,
                data_dir.clone(),
            ));
            tray::create(app.handle())?;
            app.manage(remote_input::KeyboardCapture::install(app.handle()));
            app.manage(remote_input::ViewerClipboard::default());
            updater::spawn_checks(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Fechar a janela só esconde: o app segue na bandeja, disponível para receber pedidos.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::signaling_status,
            commands::connect_to,
            commands::respond_request,
            commands::end_session,
            commands::send_signal,
            commands::set_keyboard_capture,
            commands::sign_session,
            commands::verify_session,
            commands::forget_peer,
            commands::set_session_control,
            commands::set_viewer_clipboard,
            commands::write_clipboard,
            commands::signaling_reconnect,
            commands::unattended_status,
            commands::set_unattended_password,
            commands::disable_unattended,
            updater::check_update,
            updater::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o KR Desk");
}
