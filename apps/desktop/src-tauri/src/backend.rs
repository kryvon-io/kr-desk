//! Liga o backend de sessões (`kr-host`) à interface.
//!
//! - **Sem o serviço** (instalação "só para mim"): o backend deste processo é
//!   host e viewer.
//! - **Com o serviço**: o serviço é o host deste computador (o ID mostrado é o
//!   dele) e este app vira a interface dele — pedidos, faixa e controle das
//!   sessões chegam pelo pipe ([`ServiceLink`]). O backend deste processo
//!   continua só para acessar outros computadores (viewer) e recusa pedidos
//!   no próprio ID.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use kr_host::{
    Backend, BackendConfig, BackendEvents, EVENT_MESSAGE, EVENT_P2P, EVENT_STATUS, HostPlatform,
    P2pStatus, RemoteClipboard, StatusDto,
};
use kr_identity::{DeviceIdentity, KnownPeers};
use kr_proto::ServerMessage;
use kr_transport::{ControlSender, VideoSender, VideoStream};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, UserAttentionType};

/// Traz a janela à frente e chama atenção (pedido de acesso).
fn attention(app: &AppHandle) {
    crate::show_main_window(app);
    if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = w.request_user_attention(Some(UserAttentionType::Critical));
    }
}

/// Eventos do backend local para a interface.
struct TauriEvents {
    app: AppHandle,
    /// Com o serviço, o host é ele: o estado mostrado e os pedidos vêm de lá.
    service: bool,
}

impl BackendEvents for TauriEvents {
    fn status(&self, status: &StatusDto) {
        if !self.service {
            let _ = self.app.emit(EVENT_STATUS, status);
        }
    }

    fn message(&self, msg: &ServerMessage) {
        let _ = self.app.emit(EVENT_MESSAGE, msg);
    }

    fn incoming_request(&self, msg: &ServerMessage) -> bool {
        if self.service {
            return false;
        }
        attention(&self.app);
        let _ = self.app.emit(EVENT_MESSAGE, msg);
        true
    }

    fn p2p_status(&self, status: &P2pStatus) {
        let _ = self.app.emit(EVENT_P2P, status);
    }
}

/// Tela, entrada e área de transferência deste processo.
struct AppPlatform;

struct Clipboard(crate::remote_input::ClipboardSync);
impl RemoteClipboard for Clipboard {
    fn set_remote(&self, text: &str) {
        self.0.set_remote(text);
    }
}

impl HostPlatform for AppPlatform {
    fn start_video(
        &self,
        video: VideoSender,
        control: ControlSender,
    ) -> Result<VideoStream, String> {
        crate::screen::start(video, control)
    }

    fn input(&self, area: kr_core::media::SharedArea) -> Box<dyn kr_core::input::InputSink + Send> {
        Box::new(crate::remote_input::host_sink(area))
    }

    fn clipboard(&self, on_change: Box<dyn Fn(String) + Send + Sync>) -> Box<dyn RemoteClipboard> {
        Box::new(Clipboard(crate::remote_input::ClipboardSync::start(
            on_change,
        )))
    }
}

/// Estado usado pelos comandos.
pub struct AppState {
    pub local: Arc<Backend>,
    pub service: Option<Arc<ServiceLink>>,
}

impl AppState {
    pub fn start(
        app: &AppHandle,
        identity: Arc<DeviceIdentity>,
        known: Arc<Mutex<KnownPeers>>,
    ) -> Self {
        let service = service_installed();
        if service {
            tracing::info!("serviço do KR Desk instalado: ele é o host deste computador");
        }
        // O backend usa `tokio::spawn`; entra no runtime do Tauri para iniciá-lo.
        let rt = tauri::async_runtime::handle();
        let _guard = rt.inner().enter();
        let local = Backend::start(
            BackendConfig {
                identity,
                known,
                device_name: kr_host::device_name(),
                server_url: kr_host::server_url(),
                version: env!("CARGO_PKG_VERSION"),
            },
            Arc::new(TauriEvents {
                app: app.clone(),
                service,
            }),
            Arc::new(AppPlatform),
        );
        let service = service.then(|| ServiceLink::start(app.clone()));
        Self { local, service }
    }
}

#[cfg(windows)]
fn service_installed() -> bool {
    std::env::var("KR_DESK_NO_SERVICE").is_err() && kr_service_win::service_installed()
}

#[cfg(not(windows))]
fn service_installed() -> bool {
    false
}

#[cfg(windows)]
type ServiceStream = kr_service_win::PipeStream;
#[cfg(not(windows))]
type ServiceStream = std::os::unix::net::UnixStream;

/// Ligação com o serviço (host deste computador).
pub struct ServiceLink {
    client: Mutex<Option<Arc<kr_host::ui::UiClient<ServiceStream>>>>,
    /// Pedidos e sessões que pertencem ao serviço.
    requests: Mutex<HashSet<String>>,
    status: Mutex<(StatusDto, String)>,
}

#[cfg(windows)]
fn connect_service() -> std::io::Result<ServiceStream> {
    kr_service_win::connect_service()
}

#[cfg(not(windows))]
fn connect_service() -> std::io::Result<ServiceStream> {
    Err(std::io::Error::other("sem serviço fora do Windows"))
}

impl ServiceLink {
    fn start(app: AppHandle) -> Arc<Self> {
        let link = Arc::new(Self {
            client: Mutex::new(None),
            requests: Mutex::default(),
            status: Mutex::new((StatusDto::Connecting, kr_host::device_name())),
        });
        let this = link.clone();
        std::thread::spawn(move || this.run(app));
        link
    }

    /// Conecta (e reconecta) ao serviço e repassa os eventos à interface.
    fn run(self: Arc<Self>, app: AppHandle) {
        let mut logged = false;
        loop {
            let opened = connect_service()
                .and_then(kr_host::ui::UiClient::connect)
                .and_then(|client| connect_service().map(|events| (client, events)));
            match opened {
                Ok((client, events)) => {
                    logged = false;
                    let client = Arc::new(client);
                    if let Ok(Ok(v)) = client.call(&kr_host::ui::UiCommand::Status) {
                        self.set_status_from(&app, &v);
                    }
                    *self.client.lock().expect("mutex") = Some(client);
                    tracing::info!("conectado ao serviço do KR Desk");
                    let result = kr_host::ui::read_events(events, |e| self.on_event(&app, e));
                    tracing::warn!(?result, "conexão com o serviço caiu");
                    *self.client.lock().expect("mutex") = None;
                    self.set_status(&app, StatusDto::Connecting);
                }
                Err(err) => {
                    if !logged {
                        tracing::warn!(%err, "serviço do KR Desk indisponível; tentando de novo");
                        logged = true;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    fn set_status(&self, app: &AppHandle, status: StatusDto) {
        self.status.lock().expect("mutex").0 = status.clone();
        let _ = app.emit(EVENT_STATUS, status);
    }

    fn set_status_from(&self, app: &AppHandle, v: &Value) {
        if let Some(name) = v.get("deviceName").and_then(Value::as_str) {
            self.status.lock().expect("mutex").1 = name.to_owned();
        }
        if let Some(s) = v.get("status").and_then(status_from_json) {
            self.set_status(app, s);
        }
    }

    fn on_event(&self, app: &AppHandle, e: kr_host::ui::UiEvent) {
        if e.event == EVENT_STATUS {
            if let Some(s) = status_from_json(&e.payload) {
                self.set_status(app, s);
            }
            return;
        }
        if e.event == EVENT_MESSAGE {
            let kind = e.payload.get("type").and_then(Value::as_str).unwrap_or("");
            let rid = e.payload.get("request_id").and_then(Value::as_str);
            if let Some(rid) = rid {
                let mut requests = self.requests.lock().expect("mutex");
                match kind {
                    "incoming_request" | "session_ready" => {
                        requests.insert(rid.to_owned());
                    }
                    "session_ended" => {
                        requests.remove(rid);
                    }
                    _ => {}
                }
            }
            if kind == "incoming_request" {
                attention(app);
            }
        }
        let _ = app.emit(&e.event, e.payload);
    }

    /// O pedido/sessão é do serviço?
    pub fn owns(&self, request_id: &str) -> bool {
        self.requests.lock().expect("mutex").contains(request_id)
    }

    pub fn status(&self) -> (StatusDto, String) {
        self.status.lock().expect("mutex").clone()
    }

    pub fn call(&self, cmd: kr_host::ui::UiCommand) -> Result<Value, String> {
        let client = self.client.lock().expect("mutex").clone();
        let client = client.ok_or("O serviço do KR Desk não está respondendo.")?;
        client
            .call(&cmd)
            .map_err(|e| format!("O serviço do KR Desk não respondeu: {e}"))?
    }
}

fn status_from_json(v: &Value) -> Option<StatusDto> {
    Some(match v.get("state")?.as_str()? {
        "connecting" => StatusDto::Connecting,
        "online" => StatusDto::Online {
            device_id: v.get("deviceId")?.as_str()?.to_owned(),
        },
        "offline" => StatusDto::Offline {
            reason: v.get("reason")?.as_str()?.to_owned(),
            retry_in_ms: v.get("retryInMs")?.as_u64()?,
        },
        _ => return None,
    })
}
