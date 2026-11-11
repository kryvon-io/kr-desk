//! O serviço como host deste computador: sinalização e sessões P2P rodam
//! aqui (sessão 0, sempre ativa), com a tela, a entrada e a área de
//! transferência vindas do agente da sessão do console. O app, quando
//! aberto, é a interface (pipe `kr-desk-service`, ver [`kr_host::ui`]).

use std::io::{self, BufReader};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kr_core::agent::{AgentClipboard, AgentInputSink, AgentSource};
use kr_core::input::InputSink;
use kr_core::media::{FrameSource, PipelineConfig, SharedArea};
use kr_host::ui::{self, Channel, UiCommand, UiHub};
use kr_host::{Backend, BackendConfig, HostPlatform, RemoteClipboard};
use kr_identity::{DeviceIdentity, KnownPeers, SystemProtector};
use kr_transport::{ControlSender, VideoSender, VideoStream};
use serde_json::{Value, json};
use tracing::{info, warn};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::core::PCWSTR;

use crate::pipe::{PipeServer, connect_console_agent};
use crate::util::{wide, win_err};

/// Só SYSTEM e administradores (a chave do computador fica aqui).
const DATA_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// `<instalação>\data`, criada com acesso só para SYSTEM e administradores.
fn data_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::other("executável sem pasta"))?
        .join("data");
    if dir.is_dir() {
        return Ok(dir);
    }
    let sddl = wide(DATA_SDDL);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: string terminada em zero; memória liberada abaixo.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
    }
    .map_err(|e| win_err("descritor da pasta de dados", e))?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let path = wide(&dir.to_string_lossy());
    // SAFETY: caminho terminado em zero; atributos válidos durante a chamada.
    let created = unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&sa)) };
    // SAFETY: memória alocada pelo sistema acima.
    unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    created.map_err(|e| win_err("CreateDirectory(data)", e))?;
    Ok(dir)
}

/// Tela, entrada e área de transferência pelo agente do console.
struct ServicePlatform;

struct Clipboard(AgentClipboard);
impl RemoteClipboard for Clipboard {
    fn set_remote(&self, text: &str) {
        self.0.set_remote(text);
    }
}

impl HostPlatform for ServicePlatform {
    fn start_video(
        &self,
        video: VideoSender,
        control: ControlSender,
    ) -> Result<VideoStream, String> {
        // O agente pode estar subindo (ex.: troca de sessão): a captura
        // conecta na primeira chamada e o pipeline tenta de novo até ele vir.
        VideoStream::start(
            video,
            Box::new(|| {
                Ok(Box::new(AgentSource::lazy(Box::new(connect_console_agent)))
                    as Box<dyn FrameSource>)
            }),
            kr_codec_win::auto_encoder(kr_codec_win::EncoderPreference::Auto),
            PipelineConfig::default(),
            Some(control),
        )
        .map_err(|e| e.to_string())
    }

    fn input(&self, area: SharedArea) -> Box<dyn InputSink + Send> {
        Box::new(AgentInputSink::lazy(Box::new(connect_console_agent), area))
    }

    fn clipboard(&self, on_change: Box<dyn Fn(String) + Send + Sync>) -> Box<dyn RemoteClipboard> {
        Box::new(Clipboard(AgentClipboard::start(
            Box::new(connect_console_agent),
            on_change,
        )))
    }
}

fn handle_command(
    backend: &Backend,
    rt: &tokio::runtime::Handle,
    cmd: UiCommand,
) -> Result<Value, String> {
    match cmd {
        UiCommand::Status => Ok(json!({
            "status": backend.status(),
            "deviceName": backend.device_name(),
        })),
        UiCommand::RespondRequest {
            request_id,
            accept,
            allow_control,
        } => backend
            .respond_request(&request_id, accept, allow_control)
            .map(|()| Value::Null),
        UiCommand::EndSession { request_id } => rt
            .block_on(backend.end_session(&request_id))
            .map(|()| Value::Null),
        UiCommand::SetSessionControl { request_id, allow } => {
            Ok(Value::Bool(backend.set_session_control(&request_id, allow)))
        }
    }
}

/// Sobe o backend e o pipe dos apps em segundo plano.
pub(crate) fn start() -> io::Result<()> {
    let data = data_dir()?;
    let identity = Arc::new(
        DeviceIdentity::load_or_create(&data, &SystemProtector)
            .map_err(|e| io::Error::other(format!("identidade do computador: {e}")))?,
    );
    info!(public_key = %identity.public_key_b64(), "identidade do computador carregada");
    let known = Arc::new(Mutex::new(KnownPeers::load(&data)));
    let hub = Arc::new(UiHub::default());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("kr-host")
        .build()?;
    let handle = rt.handle().clone();
    let backend = {
        let _guard = rt.enter();
        Backend::start(
            BackendConfig {
                identity,
                known,
                device_name: kr_host::device_name(),
                server_url: kr_host::server_url(),
                version: env!("CARGO_PKG_VERSION"),
            },
            hub.clone(),
            Arc::new(ServicePlatform),
        )
    };
    // O runtime vive enquanto o processo do serviço viver.
    std::thread::spawn(move || rt.block_on(std::future::pending::<()>()));

    let mut server = PipeServer::service()?;
    std::thread::spawn(move || {
        loop {
            let stream = match server.accept() {
                Ok(s) => s,
                Err(err) => {
                    warn!(%err, "conexão recusada no pipe do serviço");
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                }
            };
            let (backend, hub, handle) = (backend.clone(), hub.clone(), handle.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let result = match ui::accept(&mut reader) {
                    Ok(Channel::Commands) => {
                        ui::serve_commands(reader, |cmd| handle_command(&backend, &handle, cmd))
                    }
                    Ok(Channel::Events) => ui::serve_events(reader.into_inner(), hub.subscribe()),
                    Err(err) => Err(err),
                };
                if let Err(err) = result
                    && err.kind() != io::ErrorKind::UnexpectedEof
                {
                    info!(%err, "conexão com o app encerrada");
                }
            });
        }
    });
    info!("host do serviço pronto");
    Ok(())
}
