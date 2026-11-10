//! Entrada remota.
//!
//! - **Host:** [`host_controller`] cria o `InputController` que aplica mouse e
//!   teclado do viewer (`SendInput` no Windows) e solta tudo ao encerrar.
//! - **Viewer:** [`KeyboardCapture`] instala o hook de teclado do Windows. Com
//!   a tela remota aberta e a janela em primeiro plano, cada tecla (inclusive
//!   Win e Alt+Tab) vira o evento `viewer-key` para a interface mandar ao host.

use kr_core::input::{InputController, InputSink};
use kr_proto::control::{MouseButton, NormCoord};
use tauri::AppHandle;
#[cfg(windows)]
use tauri::Emitter;

/// Evento com uma tecla capturada pelo hook (viewer).
pub const EVENT_VIEWER_KEY: &str = "viewer-key";

#[cfg(windows)]
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct KeyDto {
    pub code: u16,
    pub ext: bool,
    pub down: bool,
}

/// Destino usado onde não há injeção (desenvolvimento em Linux): só registra.
#[derive(Debug, Default)]
pub struct LogSink;

impl InputSink for LogSink {
    fn move_to(&mut self, _x: NormCoord, _y: NormCoord) {}
    fn button(&mut self, button: MouseButton, down: bool) {
        tracing::debug!(?button, down, "entrada remota ignorada (sem Windows)");
    }
    fn wheel(&mut self, _dx: i32, _dy: i32) {}
    fn key(&mut self, code: u16, _ext: bool, down: bool) {
        tracing::debug!(code, down, "tecla remota ignorada (sem Windows)");
    }
}

#[cfg(windows)]
pub type HostSink = kr_input_win::SendInputSink;
#[cfg(not(windows))]
pub type HostSink = LogSink;

pub fn host_controller() -> InputController<HostSink> {
    InputController::new(HostSink::default())
}

/// Hook de teclado do viewer (só no Windows).
pub struct KeyboardCapture {
    #[cfg(windows)]
    hook: Option<kr_input_win::KeyboardHook>,
}

impl KeyboardCapture {
    #[cfg(windows)]
    pub fn install(app: &AppHandle, window: &tauri::WebviewWindow) -> Self {
        let hook = match window.hwnd() {
            Ok(hwnd) => {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<KeyDto>();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(key) = rx.recv().await {
                        let _ = app.emit(EVENT_VIEWER_KEY, key);
                    }
                });
                // O hook precisa responder rápido: só repassa para a task.
                kr_input_win::KeyboardHook::install(
                    windows::Win32::Foundation::HWND(hwnd.0),
                    move |k| {
                        let _ = tx.send(KeyDto {
                            code: k.code,
                            ext: k.ext,
                            down: k.down,
                        });
                    },
                )
            }
            Err(err) => {
                tracing::warn!(%err, "janela sem HWND; teclado capturado só pela interface");
                None
            }
        };
        Self { hook }
    }

    #[cfg(not(windows))]
    pub fn install(_app: &AppHandle, _window: &tauri::WebviewWindow) -> Self {
        let _ = EVENT_VIEWER_KEY;
        Self {}
    }

    /// Liga/desliga a captura. Devolve `true` se o hook nativo está ativo
    /// (então a interface não deve tratar o teclado por conta própria).
    pub fn set_enabled(&self, enabled: bool) -> bool {
        #[cfg(windows)]
        if let Some(hook) = &self.hook {
            hook.set_enabled(enabled);
            return true;
        }
        let _ = enabled;
        false
    }
}
