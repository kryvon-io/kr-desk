//! Instalação do serviço (chamada pelo instalador, como administrador).

use std::io;
use std::time::{Duration, Instant};

use tracing::info;
use windows::Win32::Foundation::{ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
    CreateServiceW, DeleteService, ENUM_SERVICE_TYPE, OpenSCManagerW, OpenServiceW,
    QueryServiceStatus, SC_ACTION, SC_ACTION_RESTART, SC_HANDLE, SC_MANAGER_ALL_ACCESS,
    SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONFIG_FAILURE_ACTIONS, SERVICE_CONTROL_STOP, SERVICE_DESCRIPTIONW,
    SERVICE_ERROR_NORMAL, SERVICE_FAILURE_ACTIONSW, SERVICE_NO_CHANGE, SERVICE_STATUS,
    SERVICE_STOPPED, SERVICE_WIN32_OWN_PROCESS, StartServiceW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::SERVICE_NAME;
use crate::util::{wide, win_err};

struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        // SAFETY: handle do SCM nosso.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager() -> io::Result<Sc> {
    // SAFETY: sem ponteiros além dos nulos.
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ALL_ACCESS) }
        .map(Sc)
        .map_err(|e| win_err("OpenSCManager (precisa de administrador)", e))
}

fn open(scm: &Sc) -> io::Result<Option<Sc>> {
    let name = wide(SERVICE_NAME);
    // SAFETY: nome terminado em zero.
    match unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), SERVICE_ALL_ACCESS) } {
        Ok(h) => Ok(Some(Sc(h))),
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => Ok(None),
        Err(e) => Err(win_err("OpenService", e)),
    }
}

/// Cria (ou atualiza) e inicia o serviço apontando para este executável.
pub fn install_service() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let bin = wide(&format!("\"{}\" --service", exe.display()));
    let scm = manager()?;
    let service = match open(&scm)? {
        Some(s) => {
            // SAFETY: strings terminadas em zero; o resto sem mudança.
            unsafe {
                ChangeServiceConfigW(
                    s.0,
                    ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(bin.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
            }
            .map_err(|e| win_err("ChangeServiceConfig", e))?;
            s
        }
        None => {
            let name = wide(SERVICE_NAME);
            let display = wide("KR Desk");
            // SAFETY: strings terminadas em zero; conta nula = LocalSystem.
            unsafe {
                CreateServiceW(
                    scm.0,
                    PCWSTR(name.as_ptr()),
                    PCWSTR(display.as_ptr()),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(bin.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
            }
            .map(Sc)
            .map_err(|e| win_err("CreateService", e))?
        }
    };

    let mut description = wide(
        "Permite ao KR Desk controlar janelas de administrador (UAC), a tela de bloqueio \
         e enviar Ctrl+Alt+Del durante uma sessão de acesso remoto.",
    );
    let info = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(description.as_mut_ptr()),
    };
    // Se cair, o Windows reinicia o serviço após 5 s (até 3 vezes por dia).
    let mut actions = [SC_ACTION {
        Type: SC_ACTION_RESTART,
        Delay: 5000,
    }; 3];
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 24 * 60 * 60,
        lpRebootMsg: PWSTR::null(),
        lpCommand: PWSTR::null(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    // SAFETY: estruturas válidas durante as chamadas.
    unsafe {
        let _ = ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some((&info as *const SERVICE_DESCRIPTIONW).cast()),
        );
        let _ = ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            Some((&failure as *const SERVICE_FAILURE_ACTIONSW).cast()),
        );
    }

    // SAFETY: handle válido, sem argumentos.
    match unsafe { StartServiceW(service.0, None) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => {}
        Err(e) => return Err(win_err("StartService", e)),
    }
    info!("serviço instalado e iniciado");
    Ok(())
}

fn stop(service: &Sc) {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: destino válido.
    let _ = unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        // SAFETY: destino válido.
        if unsafe { QueryServiceStatus(service.0, &mut status) }.is_err()
            || status.dwCurrentState == SERVICE_STOPPED
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Para o serviço (antes de atualizar os arquivos). Sem erro se não existir.
pub fn stop_service() -> io::Result<()> {
    let scm = manager()?;
    if let Some(service) = open(&scm)? {
        stop(&service);
        info!("serviço parado");
    }
    Ok(())
}

/// Para e remove o serviço. Sem erro se não existir.
pub fn uninstall_service() -> io::Result<()> {
    let scm = manager()?;
    if let Some(service) = open(&scm)? {
        stop(&service);
        // SAFETY: handle válido.
        unsafe { DeleteService(service.0) }.map_err(|e| win_err("DeleteService", e))?;
        info!("serviço removido");
    }
    Ok(())
}
