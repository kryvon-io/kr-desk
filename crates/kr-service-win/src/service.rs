//! O serviço (`kr-desk.exe --service`, LocalSystem): mantém o agente na
//! sessão do console e atende o Ctrl+Alt+Del.

use std::io::{self, Read};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{error, info, warn};
use windows::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, NO_ERROR,
    SetHandleInformation, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authentication::Identity::SendSAS;
use windows::Win32::Security::{
    DuplicateTokenEx, SECURITY_ATTRIBUTES, SecurityImpersonation, SetTokenInformation,
    TOKEN_ALL_ACCESS, TokenPrimary, TokenSessionId,
};
use windows::Win32::Storage::FileSystem::ReadFile;
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE,
    SERVICE_STOP_PENDING, SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    SetServiceStatus, StartServiceCtrlDispatcherW,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

use crate::SERVICE_NAME;
use crate::util::{Owned, wide, win_err};

/// Sem sessão no console (ex.: durante a troca de usuário).
const NO_SESSION: u32 = 0xFFFF_FFFF;
/// Intervalo mínimo entre duas partidas do agente.
const RESPAWN_DELAY: Duration = Duration::from_secs(2);

static STOP: AtomicBool = AtomicBool::new(false);

struct StatusHandle(SERVICE_STATUS_HANDLE);
// SAFETY: o handle de status pode ser usado de qualquer thread.
unsafe impl Send for StatusHandle {}
unsafe impl Sync for StatusHandle {}

static STATUS: OnceLock<StatusHandle> = OnceLock::new();

fn set_status(state: SERVICE_STATUS_CURRENT_STATE) {
    let Some(handle) = STATUS.get() else { return };
    let accepted = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
    } else {
        0
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: NO_ERROR.0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_RUNNING || state == SERVICE_STOPPED {
            0
        } else {
            5000
        },
    };
    // SAFETY: handle registrado pelo SCM; estrutura válida.
    if let Err(err) = unsafe { SetServiceStatus(handle.0, &status) } {
        warn!(%err, "SetServiceStatus falhou");
    }
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event: u32,
    _data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOP.store(true, Ordering::SeqCst);
            set_status(SERVICE_STOP_PENDING);
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(SERVICE_NAME);
    // SAFETY: nome terminado em zero; o handler é uma função estática.
    let handle = match unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(control_handler), None)
    } {
        Ok(h) => h,
        Err(err) => {
            error!(%err, "não foi possível registrar o serviço");
            return;
        }
    };
    let _ = STATUS.set(StatusHandle(handle));
    set_status(SERVICE_START_PENDING);
    allow_software_sas();
    set_status(SERVICE_RUNNING);
    info!(version = env!("CARGO_PKG_VERSION"), "serviço iniciado");
    supervise();
    info!("serviço parado");
    set_status(SERVICE_STOPPED);
}

/// Entra no despachante do SCM. Só retorna quando o serviço para.
pub fn run_service() -> io::Result<()> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::null(),
            lpServiceProc: None,
        },
    ];
    // SAFETY: tabela terminada pela entrada nula, válida durante a chamada.
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }
        .map_err(|e| win_err("StartServiceCtrlDispatcherW (rode pelo SCM)", e))
}

/// `SendSAS` só funciona se a política permitir SAS por software
/// (`SoftwareSASGeneration`: 1 = serviços, 3 = serviços e acessibilidade).
fn allow_software_sas() {
    let key = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System");
    let value = wide("SoftwareSASGeneration");
    let mut current = 0u32;
    let mut size = 4u32;
    // SAFETY: destinos válidos com o tamanho informado.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut current as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if read.is_ok() && (current == 1 || current == 3) {
        return;
    }
    let one = 1u32;
    // SAFETY: valor DWORD de 4 bytes.
    let written = unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            REG_DWORD.0,
            Some((&one as *const u32).cast()),
            4,
        )
    };
    if written.is_ok() {
        info!("SAS por software liberado para serviços (Ctrl+Alt+Del remoto)");
    } else {
        warn!(
            code = written.0,
            "não foi possível liberar o Ctrl+Alt+Del remoto"
        );
    }
}

/// Processo do agente e a ponta da entrada padrão que o mantém vivo.
struct Agent {
    process: Owned,
    session: u32,
    _stdin: Owned,
}

impl Agent {
    fn exited(&self) -> bool {
        // SAFETY: handle de processo válido.
        unsafe { WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0 }
    }

    fn kill(self) {
        // SAFETY: handle de processo válido.
        let _ = unsafe { TerminateProcess(self.process.0, 0) };
    }
}

/// Handle que pode ir para outra thread.
struct SendHandle(Owned);
// SAFETY: handles de pipe podem ser usados de qualquer thread.
unsafe impl Send for SendHandle {}

fn supervise() {
    let mut agent: Option<Agent> = None;
    let mut last_spawn: Option<Instant> = None;
    let mut quick_exits = 0u32;
    while !STOP.load(Ordering::SeqCst) {
        // SAFETY: sem ponteiros.
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if let Some(a) = &agent {
            let moved = a.session != session;
            if moved || a.exited() {
                if moved {
                    info!(from = a.session, to = session, "sessão do console mudou");
                } else {
                    warn!("agente encerrou; reiniciando");
                    if last_spawn.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                        quick_exits += 1;
                    }
                }
                agent.take().expect("checado acima").kill();
            }
        }
        // Um agente que cai logo depois de subir espera cada vez mais.
        let delay = RESPAWN_DELAY * (1 << quick_exits.min(4));
        if agent.is_none()
            && session != NO_SESSION
            && last_spawn.is_none_or(|t| t.elapsed() >= delay)
        {
            last_spawn = Some(Instant::now());
            match spawn_agent(session) {
                Ok(a) => {
                    info!(session, "agente iniciado");
                    agent = Some(a);
                }
                Err(err) => warn!(%err, session, "não foi possível iniciar o agente"),
            }
        }
        if agent
            .as_ref()
            .is_some_and(|_| last_spawn.is_some_and(|t| t.elapsed() > Duration::from_secs(60)))
        {
            quick_exits = 0;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if let Some(a) = agent {
        a.kill();
    }
}

fn inheritable() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    }
}

/// Pipe anônimo; `parent_end` não é herdado pelo filho.
fn pipe(parent_reads: bool) -> io::Result<(Owned, Owned)> {
    let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
    let sa = inheritable();
    // SAFETY: destinos válidos.
    unsafe { CreatePipe(&mut read, &mut write, Some(&sa), 0) }
        .map_err(|e| win_err("CreatePipe", e))?;
    let (read, write) = (Owned(read), Owned(write));
    let parent = if parent_reads { &read } else { &write };
    // SAFETY: handle válido.
    unsafe { SetHandleInformation(parent.0, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
        .map_err(|e| win_err("SetHandleInformation", e))?;
    Ok((read, write))
}

/// Inicia `kr-desk.exe --agent` como SYSTEM na sessão `session`.
fn spawn_agent(session: u32) -> io::Result<Agent> {
    let exe = std::env::current_exe()?;
    let exe_w = wide(&exe.to_string_lossy());
    let mut cmd = wide(&format!("\"{}\" --agent", exe.display()));
    let mut desktop = wide(r"winsta0\default");

    // SAFETY: chamadas Win32 com destinos válidos; handles fechados por `Owned`.
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)
            .map_err(|e| win_err("OpenProcessToken", e))?;
        let own = Owned(own);
        let mut token = HANDLE::default();
        DuplicateTokenEx(
            own.0,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut token,
        )
        .map_err(|e| win_err("DuplicateTokenEx", e))?;
        let token = Owned(token);
        SetTokenInformation(token.0, TokenSessionId, (&session as *const u32).cast(), 4)
            .map_err(|e| win_err("SetTokenInformation(sessão)", e))?;

        let (stdin_read, stdin_write) = pipe(false)?;
        let (stdout_read, stdout_write) = pipe(true)?;

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: stdin_read.0,
            hStdOutput: stdout_write.0,
            hStdError: stdout_write.0,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            Some(token.0),
            PCWSTR(exe_w.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
        .map_err(|e| win_err("CreateProcessAsUserW", e))?;
        drop(Owned(pi.hThread));
        // As pontas do filho ficam só com ele.
        drop(stdin_read);
        drop(stdout_write);

        let reader = SendHandle(stdout_read);
        std::thread::spawn(move || read_agent_output(reader));

        Ok(Agent {
            process: Owned(pi.hProcess),
            session,
            _stdin: stdin_write,
        })
    }
}

/// Lê a saída do agente: `sas` = Ctrl+Alt+Del; o resto vai para o log.
fn read_agent_output(handle: SendHandle) {
    struct Reader(SendHandle);
    impl Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut n = 0u32;
            // SAFETY: buffer válido; leitura síncrona.
            match unsafe { ReadFile(self.0.0.0, Some(buf), Some(&mut n), None) } {
                Ok(()) => Ok(n as usize),
                Err(_) => Ok(0),
            }
        }
    }
    let reader = io::BufReader::new(Reader(handle));
    for line in io::BufRead::lines(reader) {
        let Ok(line) = line else { break };
        match line.trim() {
            "sas" => {
                info!("Ctrl+Alt+Del enviado");
                // SAFETY: sem ponteiros; exige a política liberada acima.
                unsafe { SendSAS(false) };
            }
            "" => {}
            other => info!(saida = %other, "agente"),
        }
    }
}
