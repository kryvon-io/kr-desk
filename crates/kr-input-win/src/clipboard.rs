//! Área de transferência de texto (CF_UNICODETEXT).
//!
//! [`ClipboardSync`] observa o número de sequência do clipboard do Windows
//! (barato, sem abrir o clipboard) e, quando muda, lê o texto e chama o
//! callback. Texto escrito por nós (vindo do outro computador) não volta
//! como eco.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tracing::{debug, warn};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;
/// Intervalo de verificação do número de sequência.
const POLL: Duration = Duration::from_millis(300);
/// Tentativas de abrir o clipboard (outro programa pode estar com ele).
const OPEN_ATTEMPTS: u32 = 10;

/// Abre o clipboard, tentando de novo por alguns milissegundos.
fn open() -> bool {
    for _ in 0..OPEN_ATTEMPTS {
        // SAFETY: sem janela dona; fechado por `close`.
        if unsafe { OpenClipboard(None) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn close() {
    // SAFETY: só chamado após `open` bem-sucedido.
    let _ = unsafe { CloseClipboard() };
}

/// Texto atual do clipboard, se houver.
pub fn read_text() -> Option<String> {
    // SAFETY: consulta simples.
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.ok()?;
    if !open() {
        return None;
    }
    // SAFETY: clipboard aberto; o bloco global é travado só durante a cópia
    // e lido até o terminador nulo (dentro do tamanho do bloco).
    let text = unsafe {
        GetClipboardData(CF_UNICODETEXT).ok().and_then(|handle| {
            let mem = HGLOBAL(handle.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let max = windows::Win32::System::Memory::GlobalSize(mem) / 2;
            let len = (0..max).take_while(|&i| *ptr.add(i) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(mem);
            Some(text)
        })
    };
    close();
    text
}

/// Substitui o conteúdo do clipboard por `text`.
pub fn write_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    if !open() {
        return false;
    }
    // SAFETY: bloco alocado com o tamanho exato; após `SetClipboardData` o
    // Windows passa a ser dono dele (só liberamos se a chamada falhar).
    let ok = unsafe {
        let _ = EmptyClipboard();
        match GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            Ok(mem) => {
                let ptr = GlobalLock(mem) as *mut u16;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(mem));
                    false
                } else {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                    let _ = GlobalUnlock(mem);
                    if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_ok() {
                        true
                    } else {
                        let _ = GlobalFree(Some(mem));
                        false
                    }
                }
            }
            Err(_) => false,
        }
    };
    close();
    ok
}

fn sequence() -> u32 {
    // SAFETY: consulta simples.
    unsafe { GetClipboardSequenceNumber() }
}

/// Sincronização com o clipboard local enquanto o valor existir.
pub struct ClipboardSync {
    stop: Arc<AtomicBool>,
    /// Último texto escrito por nós (não deve voltar como eco).
    written: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardSync {
    /// Chama `on_change` com o texto sempre que o usuário copiar algo novo.
    pub fn start(on_change: impl Fn(String) + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let written: Arc<Mutex<Option<String>>> = Arc::default();
        let (thread_stop, thread_written) = (stop.clone(), written.clone());
        let thread = std::thread::Builder::new()
            .name("kr-clipboard".into())
            .spawn(move || {
                let mut last = sequence();
                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL);
                    let current = sequence();
                    if current == last {
                        continue;
                    }
                    last = current;
                    let Some(text) = read_text() else { continue };
                    let echo = thread_written
                        .lock()
                        .map(|mut w| w.take_if(|w| *w == text).is_some())
                        .unwrap_or(false);
                    if echo || text.is_empty() {
                        continue;
                    }
                    debug!(chars = text.chars().count(), "texto copiado localmente");
                    on_change(text);
                }
            })
            .map_err(|err| warn!(%err, "não foi possível observar a área de transferência"))
            .ok();
        Self {
            stop,
            written,
            thread,
        }
    }

    /// Escreve texto vindo do outro computador.
    pub fn set_remote(&self, text: &str) {
        if let Ok(mut w) = self.written.lock() {
            *w = Some(text.to_owned());
        }
        if !write_text(text) {
            warn!("não foi possível escrever na área de transferência");
        }
    }
}

impl Drop for ClipboardSync {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
