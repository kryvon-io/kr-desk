//! Named pipe entre o app e o agente.

use std::io::{self, Read, Write};

use tracing::warn;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeClientSessionId, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows::core::PCWSTR;

use crate::util::{own_image, own_session, process_image, wide, win_err};

/// Direitos do cliente: ler, escrever dados e atributos. Sem
/// `FILE_CREATE_PIPE_INSTANCE` (= `FILE_APPEND_DATA`), para que ninguém além
/// do agente crie instâncias do pipe.
const CLIENT_ACCESS: u32 = 0x0012_018B;

/// SYSTEM: tudo; usuários interativos: só [`CLIENT_ACCESS`].
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;0x12018b;;;IU)";

const BUFFER: u32 = 1024 * 1024;

fn agent_pipe_name(session: u32) -> String {
    format!(r"\\.\pipe\kr-desk-agent-{session}")
}

/// Pipe do serviço (um só, para os apps de todas as sessões).
const SERVICE_PIPE: &str = r"\\.\pipe\kr-desk-service";

/// Quem pode se conectar a um servidor de pipe (além de ser o nosso executável).
#[derive(Clone, Copy)]
pub(crate) enum Clients {
    /// Agente: o app desta sessão ou o serviço (sessão 0).
    SameSessionOrService,
    /// Serviço: apps de qualquer sessão.
    AnySession,
}

/// Uma ponta do pipe (bloqueante). `Read`/`Write` comuns.
pub struct PipeStream {
    handle: HANDLE,
    server: bool,
}

// SAFETY: o handle é de um pipe e pode ser usado de qualquer thread.
unsafe impl Send for PipeStream {}

impl PipeStream {
    fn raw(&self) -> HANDLE {
        self.handle
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = 0u32;
        let len = buf.len().min(u32::MAX as usize);
        // SAFETY: buffer válido; operação síncrona.
        match unsafe { ReadFile(self.handle, Some(&mut buf[..len]), Some(&mut n), None) } {
            Ok(()) => Ok(n as usize),
            Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
            Err(e) => Err(win_err("ReadFile", e)),
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut n = 0u32;
        let len = buf.len().min(u32::MAX as usize);
        // SAFETY: buffer válido; operação síncrona.
        match unsafe { WriteFile(self.handle, Some(&buf[..len]), Some(&mut n), None) } {
            Ok(()) => Ok(n as usize),
            Err(e)
                if e.code() == ERROR_BROKEN_PIPE.to_hresult()
                    || e.code() == ERROR_NO_DATA.to_hresult() =>
            {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe fechado"))
            }
            Err(e) => Err(win_err("WriteFile", e)),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        // SAFETY: handle nosso, fechado uma vez só.
        unsafe {
            if self.server {
                let _ = DisconnectNamedPipe(self.handle);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Descritor de segurança do pipe (liberado no `Drop`).
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn new() -> io::Result<Self> {
        let sddl = wide(PIPE_SDDL);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: string terminada em zero; `sd` recebe memória do sistema.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(|e| win_err("descritor de segurança do pipe", e))?;
        Ok(Self(sd))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: memória alocada pelo sistema acima.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// Servidor do agente: aceita conexões verificadas.
pub(crate) struct PipeServer {
    name: Vec<u16>,
    descriptor: Descriptor,
    first: bool,
    session: u32,
    image: String,
    clients: Clients,
}

// SAFETY: o descritor de segurança é memória comum do processo, usada por
// uma thread de cada vez.
unsafe impl Send for PipeServer {}

impl PipeServer {
    /// Servidor do agente desta sessão.
    pub fn agent() -> io::Result<Self> {
        let session = own_session()?;
        Self::new(&agent_pipe_name(session), Clients::SameSessionOrService)
    }

    /// Servidor do serviço, para os apps.
    pub fn service() -> io::Result<Self> {
        Self::new(SERVICE_PIPE, Clients::AnySession)
    }

    fn new(name: &str, clients: Clients) -> io::Result<Self> {
        Ok(Self {
            name: wide(name),
            descriptor: Descriptor::new()?,
            first: true,
            session: own_session()?,
            image: own_image()?,
            clients,
        })
    }

    /// Espera o próximo cliente. Recusa (e devolve erro) quem não for o
    /// `kr-desk.exe` instalado rodando nesta mesma sessão.
    pub fn accept(&mut self) -> io::Result<PipeStream> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0.0,
            bInheritHandle: false.into(),
        };
        let mut mode = PIPE_ACCESS_DUPLEX;
        if self.first {
            // Se alguém criou o pipe antes de nós, falha em vez de dividir o nome.
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: nome terminado em zero; atributos válidos durante a chamada.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(self.name.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(mode.0),
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                Some(&sa),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        self.first = false;
        let stream = PipeStream {
            handle,
            server: true,
        };
        // SAFETY: handle válido; operação síncrona.
        if let Err(e) = unsafe { ConnectNamedPipe(stream.raw(), None) }
            && e.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            return Err(win_err("ConnectNamedPipe", e));
        }
        self.verify_client(&stream)?;
        Ok(stream)
    }

    fn verify_client(&self, stream: &PipeStream) -> io::Result<()> {
        let (mut pid, mut session) = (0u32, 0u32);
        // SAFETY: destinos válidos.
        unsafe {
            GetNamedPipeClientProcessId(stream.raw(), &mut pid)
                .map_err(|e| win_err("GetNamedPipeClientProcessId", e))?;
            GetNamedPipeClientSessionId(stream.raw(), &mut session)
                .map_err(|e| win_err("GetNamedPipeClientSessionId", e))?;
        }
        let session_ok = match self.clients {
            Clients::SameSessionOrService => session == self.session || session == 0,
            Clients::AnySession => true,
        };
        if !session_ok {
            return Err(denied(format!("cliente de outra sessão ({session})")));
        }
        let image = process_image(pid)?;
        if image != self.image {
            return Err(denied(format!("cliente não é o KR Desk: {image}")));
        }
        Ok(())
    }
}

fn denied(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, msg)
}

/// Conecta ao agente desta sessão. Falha se o serviço não estiver instalado
/// (ou o agente ainda não subiu) — o app então captura por conta própria.
pub fn connect_agent() -> io::Result<PipeStream> {
    connect(&agent_pipe_name(own_session()?))
}

/// Serviço: conecta ao agente da sessão que está no console agora.
pub fn connect_console_agent() -> io::Result<PipeStream> {
    // SAFETY: sem ponteiros.
    let session = unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() };
    if session == 0xFFFF_FFFF {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "nenhuma sessão no console",
        ));
    }
    connect(&agent_pipe_name(session))
}

/// App: conecta ao serviço (falha se ele não estiver rodando).
pub fn connect_service() -> io::Result<PipeStream> {
    connect(SERVICE_PIPE)
}

fn connect(name: &str) -> io::Result<PipeStream> {
    let name = wide(name);
    let mut waited = false;
    let stream = loop {
        // SAFETY: nome terminado em zero; `SECURITY_IDENTIFICATION` impede o
        // servidor de agir em nosso nome.
        match unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                CLIENT_ACCESS,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        } {
            Ok(handle) => {
                break PipeStream {
                    handle,
                    server: false,
                };
            }
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && !waited => {
                waited = true;
                // SAFETY: nome terminado em zero.
                let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 2000) };
            }
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("agente indisponível: {e}"),
                ));
            }
        }
    };
    // O servidor precisa ser o nosso executável (e não um impostor que criou
    // o pipe antes do agente).
    let mut pid = 0u32;
    // SAFETY: destino válido.
    unsafe { GetNamedPipeServerProcessId(stream.raw(), &mut pid) }
        .map_err(|e| win_err("GetNamedPipeServerProcessId", e))?;
    let image = process_image(pid)?;
    if image != own_image()? {
        warn!(%image, "pipe do agente aberto por outro programa; ignorado");
        return Err(denied(format!("servidor do pipe não é o KR Desk: {image}")));
    }
    Ok(stream)
}
