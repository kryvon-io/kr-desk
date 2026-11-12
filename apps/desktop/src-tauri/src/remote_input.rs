//! Entrada remota.
//!
//! - **Host:** [`host_sink`] é o destino da entrada do viewer (`SendInput` no
//!   Windows, ou o agente do serviço).
//! - **Viewer:** [`KeyboardCapture`] instala o hook de teclado do Windows. Com
//!   a tela remota aberta e a janela em primeiro plano, cada tecla (inclusive
//!   Win e Alt+Tab) vira o evento `viewer-key` para a interface mandar ao host.

#[cfg(not(windows))]
use kr_core::input::InputSink;
#[cfg(not(windows))]
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
#[cfg(not(windows))]
#[derive(Debug, Default)]
pub struct LogSink;

#[cfg(not(windows))]
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

/// Entrada no host: pelo agente do serviço (alcança janelas de administrador,
/// UAC e tela de bloqueio) ou, sem ele, `SendInput` neste processo.
#[cfg(windows)]
pub enum HostSink {
    Agent(kr_core::agent::AgentInputSink<kr_service_win::PipeStream>),
    Local(kr_input_win::SendInputSink),
}

#[cfg(windows)]
impl kr_core::input::InputSink for HostSink {
    fn move_to(&mut self, x: u16, y: u16) {
        match self {
            Self::Agent(s) => s.move_to(x, y),
            Self::Local(s) => s.move_to(x, y),
        }
    }
    fn button(&mut self, button: kr_proto::control::MouseButton, down: bool) {
        match self {
            Self::Agent(s) => s.button(button, down),
            Self::Local(s) => s.button(button, down),
        }
    }
    fn wheel(&mut self, dx: i32, dy: i32) {
        match self {
            Self::Agent(s) => s.wheel(dx, dy),
            Self::Local(s) => s.wheel(dx, dy),
        }
    }
    fn key(&mut self, code: u16, ext: bool, down: bool) {
        match self {
            Self::Agent(s) => s.key(code, ext, down),
            Self::Local(s) => s.key(code, ext, down),
        }
    }
    fn secure_attention(&mut self) {
        match self {
            Self::Agent(s) => s.secure_attention(),
            Self::Local(s) => s.secure_attention(),
        }
    }
}

#[cfg(not(windows))]
pub type HostSink = LogSink;

/// Destino da entrada no host; `area` diz qual monitor está sendo transmitido.
#[cfg(windows)]
pub fn host_sink(area: kr_core::media::SharedArea) -> HostSink {
    match kr_core::agent::AgentInputSink::connect(
        Box::new(kr_service_win::connect_agent),
        area.clone(),
    ) {
        Ok(s) => {
            tracing::info!("entrada remota pelo serviço do KR Desk");
            HostSink::Agent(s)
        }
        Err(err) => {
            tracing::info!(%err, "serviço indisponível; entrada remota neste processo");
            HostSink::Local(kr_input_win::SendInputSink::new(area))
        }
    }
}

#[cfg(not(windows))]
pub fn host_sink(_area: kr_core::media::SharedArea) -> HostSink {
    HostSink::default()
}

/// Hook de teclado do viewer (só no Windows).
pub struct KeyboardCapture {
    #[cfg(windows)]
    hook: Option<kr_input_win::KeyboardHook>,
}

impl KeyboardCapture {
    #[cfg(windows)]
    pub fn install(app: &AppHandle) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<KeyDto>();
        let emitter = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(key) = rx.recv().await {
                if let Err(err) = emitter.emit(EVENT_VIEWER_KEY, key) {
                    tracing::warn!(%err, "não foi possível repassar a tecla à interface");
                }
            }
        });
        // O hook precisa responder rápido: só repassa para a task.
        let hook = kr_input_win::KeyboardHook::install(move |k| {
            let _ = tx.send(KeyDto {
                code: k.code,
                ext: k.ext,
                down: k.down,
            });
        });
        Self { hook }
    }

    #[cfg(not(windows))]
    pub fn install(_app: &AppHandle) -> Self {
        let _ = EVENT_VIEWER_KEY;
        Self {}
    }

    /// Liga/desliga a captura. Devolve `true` se o hook nativo está ativo.
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

/// Área de transferência de texto (no Windows, a real; fora dele, nada).
#[cfg(windows)]
pub use kr_input_win::ClipboardSync;

#[cfg(not(windows))]
pub struct ClipboardSync;

#[cfg(not(windows))]
impl ClipboardSync {
    pub fn start(_on_change: impl Fn(String) + Send + 'static) -> Self {
        Self
    }

    pub fn set_remote(&self, _text: &str) {}
}

/// Evento com texto copiado neste computador durante uma sessão como viewer.
pub const EVENT_LOCAL_CLIPBOARD: &str = "local-clipboard";

/// Clipboard do viewer: ligado só com a sessão autenticada e com controle.
#[derive(Default)]
pub struct ViewerClipboard(std::sync::Mutex<Option<ClipboardSync>>);

impl ViewerClipboard {
    pub fn set_enabled(&self, app: &AppHandle, enabled: bool) {
        let Ok(mut slot) = self.0.lock() else { return };
        if !enabled {
            *slot = None;
            return;
        }
        if slot.is_none() {
            let app = app.clone();
            *slot = Some(ClipboardSync::start(move |text| {
                use tauri::Emitter as _;
                let _ = app.emit(EVENT_LOCAL_CLIPBOARD, text);
            }));
        }
    }

    pub fn set_remote(&self, text: &str) {
        if let Ok(slot) = self.0.lock()
            && let Some(sync) = slot.as_ref()
        {
            sync.set_remote(text);
        }
    }
}
