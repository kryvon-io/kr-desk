//! Hook de teclado de baixo nível para o viewer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::thread::JoinHandle;

use tracing::{info, warn};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, MSG, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

/// Tecla capturada pelo hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookKey {
    pub code: u16,
    pub ext: bool,
    pub down: bool,
}

type Callback = Box<dyn Fn(HookKey) + Send + Sync>;

/// Estado global: o procedimento do hook não recebe contexto.
static CALLBACK: Mutex<Option<Callback>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(false);
static TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && ENABLED.load(Ordering::Relaxed) {
        // SAFETY: para WH_KEYBOARD_LL, lparam aponta para um KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = kb.flags.0 & LLKHF_INJECTED.0 != 0;
        // SAFETY: consulta simples.
        let foreground = unsafe { GetForegroundWindow() }.0 as isize;
        if !injected && foreground != 0 && foreground == TARGET.load(Ordering::Relaxed) {
            let message = wparam.0 as u32;
            let key = HookKey {
                code: kb.scanCode as u16,
                ext: kb.flags.0 & LLKHF_EXTENDED.0 != 0,
                down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
            };
            if let Ok(guard) = CALLBACK.lock()
                && let Some(callback) = guard.as_ref()
            {
                callback(key);
                // Consome a tecla: ela vai para o computador remoto.
                return LRESULT(1);
            }
        }
    }
    // SAFETY: repassa ao próximo hook com os mesmos argumentos.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Hook ativo enquanto o valor existir (um por processo).
pub struct KeyboardHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardHook {
    /// Instala o hook numa thread própria (com loop de mensagens). As teclas
    /// só são capturadas com [`KeyboardHook::set_enabled`] ligado e a janela
    /// `target` em primeiro plano.
    pub fn install(
        target: HWND,
        callback: impl Fn(HookKey) + Send + Sync + 'static,
    ) -> Option<Self> {
        *CALLBACK.lock().ok()? = Some(Box::new(callback));
        TARGET.store(target.0 as isize, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("kr-keyboard-hook".into())
            .spawn(move || {
                // SAFETY: hook global instalado e removido nesta thread, que
                // mantém o loop de mensagens exigido pelo WH_KEYBOARD_LL.
                unsafe {
                    let module = GetModuleHandleW(None).ok().map(|m| m.into());
                    let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) {
                        Ok(h) => h,
                        Err(err) => {
                            warn!(%err, "não foi possível instalar o hook de teclado");
                            let _ = tx.send(None);
                            return;
                        }
                    };
                    let _ = tx.send(Some(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
                    let _ = UnhookWindowsHookEx(hook);
                }
            })
            .ok()?;
        let thread_id = rx.recv().ok().flatten()?;
        info!("hook de teclado instalado");
        Some(Self {
            thread_id,
            thread: Some(thread),
        })
    }

    /// Liga/desliga a captura (ex.: só com a tela remota aberta).
    pub fn set_enabled(&self, enabled: bool) {
        ENABLED.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Relaxed);
        // SAFETY: encerra o loop de mensagens da thread do hook.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut cb) = CALLBACK.lock() {
            *cb = None;
        }
    }
}
