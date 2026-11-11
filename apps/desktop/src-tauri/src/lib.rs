//! Binário Tauri do KR Desk. Mantido fino: a lógica fica nos crates `kr-*`.

mod commands;
mod logging;
mod p2p;
mod remote_input;
mod screen;
mod signaling;
mod tray;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Precisa ser o primeiro plugin: uma segunda execução só traz a janela à frente.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir()?;
            logging::init(&data_dir.join("logs"));
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "KR Desk iniciando");
            let identity = Arc::new(DeviceIdentity::load_or_create(&data_dir, &SystemProtector)?);
            tracing::info!(public_key = %identity.public_key_b64(), "identidade carregada");

            // Chaves dos computadores já conectados (TOFU), usadas nos dois papéis.
            let known = Arc::new(std::sync::Mutex::new(KnownPeers::load(&data_dir)));
            app.manage(commands::Session {
                identity: identity.clone(),
                known: known.clone(),
            });
            let bridge = signaling::start(app.handle().clone(), identity, known);
            app.manage(bridge);
            tray::create(app.handle())?;
            app.manage(remote_input::KeyboardCapture::install(app.handle()));
            app.manage(remote_input::ViewerClipboard::default());
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
        ])
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o KR Desk");
}
