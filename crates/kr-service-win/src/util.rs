//! Pequenas ajudas Win32 compartilhadas.

use std::io;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

/// Texto UTF-16 terminado em zero.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn win_err(context: &str, err: windows::core::Error) -> io::Error {
    io::Error::other(format!("{context}: {err}"))
}

/// Handle que fecha sozinho.
pub struct Owned(pub HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: handle nosso, fechado uma vez só.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Caminho do executável de um processo (minúsculo, para comparar).
pub fn process_image(pid: u32) -> io::Result<String> {
    // SAFETY: só consulta; o handle é fechado por `Owned`.
    unsafe {
        let process = Owned(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .map_err(|e| win_err("OpenProcess", e))?,
        );
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .map_err(|e| win_err("QueryFullProcessImageNameW", e))?;
        Ok(String::from_utf16_lossy(&buf[..len as usize]).to_lowercase())
    }
}

/// Caminho do nosso próprio executável, no mesmo formato de [`process_image`].
pub fn own_image() -> io::Result<String> {
    // SAFETY: sem ponteiros.
    process_image(unsafe { GetCurrentProcessId() })
}

pub fn session_of(pid: u32) -> io::Result<u32> {
    let mut session = 0u32;
    // SAFETY: `session` é um destino válido.
    unsafe { ProcessIdToSessionId(pid, &mut session) }
        .map_err(|e| win_err("ProcessIdToSessionId", e))?;
    Ok(session)
}

pub fn own_session() -> io::Result<u32> {
    // SAFETY: sem ponteiros.
    session_of(unsafe { GetCurrentProcessId() })
}
