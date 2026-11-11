//! Área de trabalho ativa (normal, UAC/Winlogon, tela de bloqueio).
//!
//! Captura e `SendInput` só funcionam na área de trabalho que recebe a
//! entrada no momento. O agente (SYSTEM) pode trocar de uma para outra; cada
//! thread que captura ou injeta chama [`follow_input_desktop`] antes.

use std::cell::Cell;

use tracing::{info, warn};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, GetThreadDesktop,
    GetUserObjectInformationW, HDESK, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

/// `GENERIC_ALL`: o agente precisa ler a tela e mandar entrada.
const DESKTOP_ACCESS: DESKTOP_ACCESS_FLAGS = DESKTOP_ACCESS_FLAGS(0x1000_0000);

thread_local! {
    /// Área de trabalho que esta thread abriu (fechada ao trocar).
    static CURRENT: Cell<Option<isize>> = const { Cell::new(None) };
}

fn name(desk: HDESK) -> String {
    let mut buf = [0u16; 128];
    let mut needed = 0u32;
    // SAFETY: buffer com o tamanho informado.
    let ok = unsafe {
        GetUserObjectInformationW(
            HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            std::mem::size_of_val(&buf) as u32,
            Some(&mut needed),
        )
    };
    if ok.is_err() {
        return String::new();
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Põe a thread atual na área de trabalho que recebe a entrada, se mudou.
/// Devolve `true` quando trocou.
pub fn follow_input_desktop() -> bool {
    // SAFETY: handles de área de trabalho válidos; o anterior só é fechado
    // depois que a thread já está na nova.
    unsafe {
        let input = match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS) {
            Ok(d) => d,
            Err(err) => {
                warn!(%err, "não foi possível abrir a área de trabalho ativa");
                return false;
            }
        };
        let target = name(input);
        let current = GetThreadDesktop(GetCurrentThreadId())
            .map(name)
            .unwrap_or_default();
        if !target.is_empty() && target == current {
            let _ = CloseDesktop(input);
            return false;
        }
        match SetThreadDesktop(input) {
            Ok(()) => {
                if let Some(old) = CURRENT.replace(Some(input.0 as isize)) {
                    let _ = CloseDesktop(HDESK(old as *mut _));
                }
                info!(from = %current, to = %target, "área de trabalho ativa mudou");
                true
            }
            Err(err) => {
                warn!(%err, desktop = %target, "não foi possível trocar de área de trabalho");
                let _ = CloseDesktop(input);
                false
            }
        }
    }
}
