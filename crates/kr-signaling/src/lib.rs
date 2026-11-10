//! Cliente do servidor de sinalização.
//!
//! Mantém uma conexão WebSocket autenticada o tempo todo:
//! - autentica com a chave do dispositivo (`hello` → `auth` → `auth_ok`);
//! - envia `ping` periódico para manter a presença;
//! - quando o servidor avisa `reconnect_soon` (limite da função na Vercel),
//!   abre uma conexão nova **antes** de largar a antiga, sem lacuna;
//! - se a conexão cair, reconecta com backoff exponencial e jitter.
//!
//! A aplicação recebe [`SignalingEvent`]s por um canal e envia mensagens com
//! [`SignalingHandle::send`].

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use kr_identity::DeviceIdentity;
use kr_proto::{ClientMessage, Envelope, ServerMessage};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Endereço padrão do servidor de produção.
pub const DEFAULT_URL: &str = "wss://kr-desk-server.vercel.app/ws";

#[derive(Debug, Clone)]
pub struct SignalingConfig {
    pub url: String,
    pub device_name: String,
    pub app_version: String,
    pub heartbeat: Duration,
    pub connect_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl SignalingConfig {
    pub fn new(
        url: impl Into<String>,
        device_name: impl Into<String>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            device_name: device_name.into(),
            app_version: app_version.into(),
            heartbeat: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(10),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Estado da conexão com o servidor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Online { device_id: String },
    Offline { reason: String, retry_in_ms: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignalingEvent {
    Status(ConnectionStatus),
    /// Mensagem de aplicação (pedidos, sinais, erros...). `hello`, `auth_ok`,
    /// `pong` e `reconnect_soon` são tratados aqui e não chegam ao app.
    Message(ServerMessage),
}

#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("sem conexão com o servidor")]
    NotConnected,
    #[error("cliente encerrado")]
    Closed,
    #[error("tempo esgotado ao conectar")]
    Timeout,
    #[error("WebSocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("conexão encerrada pelo servidor")]
    ServerClosed,
    #[error("autenticação recusada: {0}")]
    Auth(String),
}

enum Command {
    Send(ClientMessage),
    Shutdown,
}

/// Controle do cliente. Clonável; o cliente para quando todas as cópias caem
/// ou quando [`SignalingHandle::shutdown`] é chamado.
#[derive(Clone)]
pub struct SignalingHandle {
    tx: mpsc::UnboundedSender<Command>,
    online: Arc<AtomicBool>,
}

impl SignalingHandle {
    /// Envia uma mensagem ao servidor. Falha imediatamente se offline.
    pub fn send(&self, msg: ClientMessage) -> Result<(), SignalingError> {
        if !self.online.load(Ordering::SeqCst) {
            return Err(SignalingError::NotConnected);
        }
        self.tx
            .send(Command::Send(msg))
            .map_err(|_| SignalingError::Closed)
    }

    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Inicia o cliente numa task do Tokio.
pub fn spawn(
    config: SignalingConfig,
    identity: Arc<DeviceIdentity>,
) -> (SignalingHandle, mpsc::UnboundedReceiver<SignalingEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let online = Arc::new(AtomicBool::new(false));
    let client = Client {
        config,
        identity,
        events: ev_tx,
        online: online.clone(),
        ping_nonce: AtomicU64::new(0),
    };
    tokio::spawn(client.run(cmd_rx));
    (SignalingHandle { tx: cmd_tx, online }, ev_rx)
}

struct Conn {
    sink: SplitSink<Ws, Message>,
    stream: SplitStream<Ws>,
    device_id: String,
}

type Pending = Pin<Box<dyn Future<Output = Result<Conn, SignalingError>> + Send>>;

struct Client {
    config: SignalingConfig,
    identity: Arc<DeviceIdentity>,
    events: mpsc::UnboundedSender<SignalingEvent>,
    online: Arc<AtomicBool>,
    ping_nonce: AtomicU64,
}

impl Client {
    fn emit(&self, ev: SignalingEvent) {
        let _ = self.events.send(ev);
    }

    fn set_status(&self, status: ConnectionStatus) {
        self.online.store(
            matches!(status, ConnectionStatus::Online { .. }),
            Ordering::SeqCst,
        );
        self.emit(SignalingEvent::Status(status));
    }

    async fn run(self, mut cmd_rx: mpsc::UnboundedReceiver<Command>) {
        let mut attempt: u32 = 0;
        loop {
            self.set_status(ConnectionStatus::Connecting);
            let conn = match establish(&self.config, &self.identity).await {
                Ok(c) => c,
                Err(err) => {
                    let delay = backoff(&self.config, attempt);
                    attempt = attempt.saturating_add(1);
                    warn!(%err, ?delay, "falha ao conectar na sinalização");
                    self.set_status(ConnectionStatus::Offline {
                        reason: err.to_string(),
                        retry_in_ms: delay.as_millis() as u64,
                    });
                    if self.wait_or_shutdown(&mut cmd_rx, delay).await {
                        return;
                    }
                    continue;
                }
            };
            attempt = 0;
            info!(device_id = %conn.device_id, "conectado à sinalização");
            self.set_status(ConnectionStatus::Online {
                device_id: conn.device_id.clone(),
            });

            match self.serve(conn, &mut cmd_rx).await {
                Ended::Shutdown => return,
                Ended::Lost(reason) => {
                    let delay = self.config.backoff_min;
                    warn!(%reason, "conexão com a sinalização perdida");
                    self.set_status(ConnectionStatus::Offline {
                        reason,
                        retry_in_ms: delay.as_millis() as u64,
                    });
                    if self.wait_or_shutdown(&mut cmd_rx, delay).await {
                        return;
                    }
                }
            }
        }
    }

    /// Espera `delay`; retorna true se pediram para encerrar nesse meio-tempo.
    /// Mensagens enviadas enquanto offline são descartadas (`send` já falha).
    async fn wait_or_shutdown(
        &self,
        cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
        delay: Duration,
    ) -> bool {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => return false,
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(Command::Shutdown) => return true,
                    Some(Command::Send(_)) => {}
                },
            }
        }
    }

    async fn serve(
        &self,
        mut current: Conn,
        cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
    ) -> Ended {
        let mut next: Option<Pending> = None;
        let mut heartbeat = tokio::time::interval(self.config.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        heartbeat.tick().await; // o primeiro tick é imediato

        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(Command::Shutdown) => {
                        let _ = current.sink.close().await;
                        self.online.store(false, Ordering::SeqCst);
                        return Ended::Shutdown;
                    }
                    Some(Command::Send(msg)) => {
                        if let Err(err) = send(&mut current.sink, &msg).await {
                            return Ended::Lost(format!("falha ao enviar: {err}"));
                        }
                    }
                },
                msg = recv(&mut current.stream) => match msg {
                    Ok(ServerMessage::ReconnectSoon { in_ms }) => {
                        debug!(in_ms, "servidor pediu rotação; abrindo nova conexão");
                        if next.is_none() {
                            let (cfg, id) = (self.config.clone(), self.identity.clone());
                            next = Some(Box::pin(async move { establish(&cfg, &id).await }));
                        }
                    }
                    Ok(ServerMessage::Pong { .. } | ServerMessage::Hello { .. } | ServerMessage::AuthOk { .. }) => {}
                    Ok(other) => self.emit(SignalingEvent::Message(other)),
                    Err(err) => {
                        // Se a nova conexão já está sendo aberta, espera por ela.
                        if let Some(pending) = next.take() {
                            match pending.await {
                                Ok(new) => {
                                    current = self.swap_in(current, new);
                                    continue;
                                }
                                Err(e) => return Ended::Lost(format!("{err}; rotação falhou: {e}")),
                            }
                        }
                        return Ended::Lost(err.to_string());
                    }
                },
                res = async { next.as_mut().expect("guardado pelo if").as_mut().await }, if next.is_some() => {
                    next = None;
                    match res {
                        Ok(new) => current = self.swap_in(current, new),
                        // Mantém a antiga; se ela cair, o laço externo reconecta.
                        Err(err) => warn!(%err, "falha na rotação da conexão"),
                    }
                },
                _ = heartbeat.tick() => {
                    let nonce = self.ping_nonce.fetch_add(1, Ordering::Relaxed);
                    if let Err(err) = send(&mut current.sink, &ClientMessage::Ping { nonce }).await {
                        return Ended::Lost(format!("falha no heartbeat: {err}"));
                    }
                },
            }
        }
    }

    /// Troca a conexão atual pela nova e fecha a antiga em segundo plano.
    fn swap_in(&self, old: Conn, new: Conn) -> Conn {
        info!("conexão de sinalização rotacionada");
        if new.device_id != old.device_id {
            self.set_status(ConnectionStatus::Online {
                device_id: new.device_id.clone(),
            });
        }
        let mut old_sink = old.sink;
        tokio::spawn(async move {
            let _ = old_sink.close().await;
        });
        new
    }
}

enum Ended {
    Shutdown,
    Lost(String),
}

/// Conecta, recebe o `hello` e autentica. Retorna a conexão já autenticada.
async fn establish(
    config: &SignalingConfig,
    identity: &DeviceIdentity,
) -> Result<Conn, SignalingError> {
    let fut = async {
        let (ws, _) = tokio_tungstenite::connect_async(config.url.as_str()).await?;
        let (mut sink, mut stream) = ws.split();

        let nonce = loop {
            if let ServerMessage::Hello { nonce, .. } = recv(&mut stream).await? {
                break nonce;
            }
        };
        let auth = ClientMessage::Auth {
            public_key: identity.public_key_b64(),
            signature: identity.sign_auth(&nonce),
            device_name: config.device_name.clone(),
            app_version: config.app_version.clone(),
        };
        send(&mut sink, &auth).await?;

        loop {
            match recv(&mut stream).await? {
                ServerMessage::AuthOk { device_id } => {
                    return Ok(Conn {
                        sink,
                        stream,
                        device_id,
                    });
                }
                ServerMessage::Error { code, message, .. } => {
                    return Err(SignalingError::Auth(format!("{code:?}: {message}")));
                }
                _ => {}
            }
        }
    };
    tokio::time::timeout(config.connect_timeout, fut)
        .await
        .map_err(|_| SignalingError::Timeout)?
}

async fn send(
    sink: &mut SplitSink<Ws, Message>,
    msg: &ClientMessage,
) -> Result<(), SignalingError> {
    let text = serde_json::to_string(&Envelope::new(msg)).expect("serialização não falha");
    sink.send(Message::text(text)).await?;
    Ok(())
}

/// Próxima mensagem de aplicação. Ignora frames de controle e JSON inválido.
/// Seguro para cancelamento: o único ponto de espera é `stream.next()`.
async fn recv(stream: &mut SplitStream<Ws>) -> Result<ServerMessage, SignalingError> {
    loop {
        match stream.next().await {
            None => return Err(SignalingError::ServerClosed),
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<Envelope<ServerMessage>>(&text) {
                    Ok(env) if env.v == kr_proto::PROTOCOL_VERSION => return Ok(env.msg),
                    Ok(env) => warn!(v = env.v, "versão de protocolo inesperada"),
                    Err(err) => warn!(%err, "mensagem do servidor não reconhecida"),
                }
            }
            Some(Ok(Message::Close(_))) => return Err(SignalingError::ServerClosed),
            Some(Ok(_)) => {}
        }
    }
}

/// Backoff exponencial com jitter (50–100% do valor).
fn backoff(config: &SignalingConfig, attempt: u32) -> Duration {
    let base = config
        .backoff_min
        .saturating_mul(2u32.saturating_pow(attempt.min(16)))
        .min(config.backoff_max);
    let jitter = getrandom::u32().unwrap_or(u32::MAX / 2) as f64 / u32::MAX as f64;
    base.mul_f64(0.5 + 0.5 * jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        let cfg = SignalingConfig::new("ws://x", "pc", "0");
        for attempt in 0..20 {
            let d = backoff(&cfg, attempt);
            assert!(d <= cfg.backoff_max, "{attempt}: {d:?}");
            assert!(d >= cfg.backoff_min / 2, "{attempt}: {d:?}");
        }
        assert!(backoff(&cfg, 10) >= Duration::from_secs(15));
    }
}
