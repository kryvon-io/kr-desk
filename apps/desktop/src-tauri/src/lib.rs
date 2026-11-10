//! Binário Tauri do KR Desk. Mantido fino: a lógica fica nos crates `kr-*`.

use kr_core::AppInfo;

/// Retorna versão do app e do protocolo para a interface.
#[tauri::command]
fn app_info() -> AppInfo {
    kr_core::app_info()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("falha ao iniciar o KR Desk");
}
