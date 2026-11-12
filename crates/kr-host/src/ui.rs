//! Conversa entre o serviço (host deste computador) e o app (interface).
//!
//! Linhas JSON sobre qualquer `Read + Write` (no Windows, um named pipe).
//! Cada conexão começa com `{"hello":"commands"|"events","version":1}`:
//! - **commands:** o app manda um comando por linha e recebe uma resposta
//!   (`{"ok":true,"value":…}` ou `{"ok":false,"error":"…"}`);
//! - **events:** o serviço manda `{"event":"…","payload":…}` (os mesmos
//!   eventos que a interface recebe do backend local).
//!
//! São duas conexões porque um pipe síncrono do Windows não lê e escreve ao
//! mesmo tempo em threads diferentes.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Mutex;
use std::sync::mpsc;

use kr_proto::ServerMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BackendEvents, EVENT_MESSAGE, EVENT_P2P, EVENT_STATUS, P2pStatus, StatusDto};

pub const UI_PROTOCOL_VERSION: u32 = 1;
/// Maior linha aceita (mensagens do servidor são pequenas).
const MAX_LINE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Commands,
    Events,
}

#[derive(Debug, Serialize, Deserialize)]
struct Hello {
    hello: Channel,
    version: u32,
}

/// Comandos que o app pode mandar ao serviço.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum UiCommand {
    /// Estado da sinalização e nome do computador.
    Status,
    RespondRequest {
        request_id: String,
        accept: bool,
        allow_control: bool,
    },
    EndSession {
        request_id: String,
    },
    SetSessionControl {
        request_id: String,
        allow: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Reply {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Evento para a interface (nome do evento Tauri + conteúdo).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiEvent {
    pub event: String,
    pub payload: Value,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn write_line<W: Write>(w: &mut W, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(|e| bad(e.to_string()))?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

fn read_line<R: BufRead>(r: &mut R, buf: &mut String) -> io::Result<()> {
    buf.clear();
    let n = r.take(MAX_LINE as u64).read_line(buf)?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "conexão fechada",
        ));
    }
    if !buf.ends_with('\n') {
        return Err(bad("linha grande demais"));
    }
    Ok(())
}

// --- lado do serviço ------------------------------------------------------------

/// Lê o `hello` de uma conexão nova.
pub fn accept<S: Read>(reader: &mut BufReader<S>) -> io::Result<Channel> {
    let mut buf = String::new();
    read_line(reader, &mut buf)?;
    let hello: Hello = serde_json::from_str(&buf).map_err(|e| bad(e.to_string()))?;
    if hello.version != UI_PROTOCOL_VERSION {
        return Err(bad(format!(
            "versão diferente (serviço {UI_PROTOCOL_VERSION}, app {})",
            hello.version
        )));
    }
    Ok(hello.hello)
}

/// Atende comandos até a conexão fechar.
pub fn serve_commands<S: Read + Write>(
    mut reader: BufReader<S>,
    mut handle: impl FnMut(UiCommand) -> Result<Value, String>,
) -> io::Result<()> {
    let mut buf = String::new();
    loop {
        read_line(&mut reader, &mut buf)?;
        let reply = match serde_json::from_str::<UiCommand>(&buf) {
            Ok(cmd) => match handle(cmd) {
                Ok(value) => Reply {
                    ok: true,
                    value: Some(value),
                    error: None,
                },
                Err(error) => Reply {
                    ok: false,
                    value: None,
                    error: Some(error),
                },
            },
            Err(e) => Reply {
                ok: false,
                value: None,
                error: Some(format!("comando inválido: {e}")),
            },
        };
        write_line(reader.get_mut(), &reply)?;
    }
}

/// Repassa eventos até a conexão ou o canal fecharem.
pub fn serve_events<W: Write>(mut stream: W, rx: mpsc::Receiver<UiEvent>) -> io::Result<()> {
    for event in rx {
        write_line(&mut stream, &event)?;
    }
    Ok(())
}

/// Distribui os eventos do backend do serviço para os apps conectados.
#[derive(Default)]
pub struct UiHub {
    clients: Mutex<Vec<mpsc::Sender<UiEvent>>>,
}

impl UiHub {
    /// Novo app conectado (canal de eventos).
    pub fn subscribe(&self) -> mpsc::Receiver<UiEvent> {
        let (tx, rx) = mpsc::channel();
        self.clients.lock().expect("mutex").push(tx);
        rx
    }

    /// Envia a todos; quem desconectou sai da lista. Devolve quantos receberam.
    pub fn broadcast(&self, event: &str, payload: Value) -> usize {
        let event = UiEvent {
            event: event.to_owned(),
            payload,
        };
        let mut clients = self.clients.lock().expect("mutex");
        clients.retain(|c| c.send(event.clone()).is_ok());
        clients.len()
    }
}

impl BackendEvents for UiHub {
    fn status(&self, status: &StatusDto) {
        self.broadcast(
            EVENT_STATUS,
            serde_json::to_value(status).expect("serializável"),
        );
    }

    fn message(&self, msg: &ServerMessage) {
        self.broadcast(
            EVENT_MESSAGE,
            serde_json::to_value(msg).expect("serializável"),
        );
    }

    fn incoming_request(&self, msg: &ServerMessage) -> bool {
        // Sem app aberto (ex.: tela de login), ninguém pode aceitar.
        self.broadcast(
            EVENT_MESSAGE,
            serde_json::to_value(msg).expect("serializável"),
        ) > 0
    }

    fn p2p_status(&self, status: &P2pStatus) {
        self.broadcast(
            EVENT_P2P,
            serde_json::to_value(status).expect("serializável"),
        );
    }
}

// --- lado do app ----------------------------------------------------------------

fn hello<S: Write>(stream: &mut S, channel: Channel) -> io::Result<()> {
    write_line(
        stream,
        &Hello {
            hello: channel,
            version: UI_PROTOCOL_VERSION,
        },
    )
}

/// Canal de comandos do app para o serviço.
pub struct UiClient<S: Read + Write> {
    conn: Mutex<BufReader<S>>,
}

impl<S: Read + Write> UiClient<S> {
    pub fn connect(mut stream: S) -> io::Result<Self> {
        hello(&mut stream, Channel::Commands)?;
        Ok(Self {
            conn: Mutex::new(BufReader::new(stream)),
        })
    }

    /// Envia um comando e espera a resposta. Erro de E/S = serviço caiu.
    pub fn call(&self, cmd: &UiCommand) -> io::Result<Result<Value, String>> {
        let mut conn = self.conn.lock().expect("mutex");
        write_line(conn.get_mut(), cmd)?;
        let mut buf = String::new();
        read_line(&mut *conn, &mut buf)?;
        let reply: Reply = serde_json::from_str(&buf).map_err(|e| bad(e.to_string()))?;
        Ok(if reply.ok {
            Ok(reply.value.unwrap_or(Value::Null))
        } else {
            Err(reply.error.unwrap_or_else(|| "erro desconhecido".into()))
        })
    }
}

/// Lê eventos do serviço até a conexão fechar.
pub fn read_events<S: Read + Write>(
    mut stream: S,
    mut on_event: impl FnMut(UiEvent),
) -> io::Result<()> {
    hello(&mut stream, Channel::Events)?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    loop {
        read_line(&mut reader, &mut buf)?;
        match serde_json::from_str::<UiEvent>(&buf) {
            Ok(e) => on_event(e),
            Err(e) => tracing::warn!(%e, "evento inválido do serviço"),
        }
    }
}

// Os testes usam pares de sockets Unix.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    fn spawn_server(hub: Arc<UiHub>) -> impl Fn() -> UnixStream {
        move || {
            let (client, server) = UnixStream::pair().unwrap();
            let hub = hub.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(server);
                match accept(&mut reader).unwrap() {
                    Channel::Commands => {
                        let _ = serve_commands(reader, |cmd| match cmd {
                            UiCommand::Status => Ok(serde_json::json!({"deviceName": "PC"})),
                            UiCommand::EndSession { request_id } => {
                                Err(format!("sessão {request_id} não existe"))
                            }
                            other => Ok(serde_json::to_value(format!("{other:?}")).unwrap()),
                        });
                    }
                    Channel::Events => {
                        let rx = hub.subscribe();
                        let _ = serve_events(reader.into_inner(), rx);
                    }
                }
            });
            client
        }
    }

    #[test]
    fn comandos_vao_e_voltam() {
        let connect = spawn_server(Arc::new(UiHub::default()));
        let client = UiClient::connect(connect()).unwrap();
        assert_eq!(
            client.call(&UiCommand::Status).unwrap().unwrap(),
            serde_json::json!({"deviceName": "PC"})
        );
        assert_eq!(
            client
                .call(&UiCommand::EndSession {
                    request_id: "x".into()
                })
                .unwrap()
                .unwrap_err(),
            "sessão x não existe"
        );
        let r = client
            .call(&UiCommand::RespondRequest {
                request_id: "r".into(),
                accept: true,
                allow_control: false,
            })
            .unwrap()
            .unwrap();
        assert!(r.as_str().unwrap().contains("allow_control: false"));
    }

    #[test]
    fn eventos_chegam_a_todos_e_pedidos_dependem_de_alguem_ouvindo() {
        let hub = Arc::new(UiHub::default());
        let request = ServerMessage::IncomingRequest {
            request_id: "r1".into(),
            from_id: "123456789".into(),
            from_name: "PC".into(),
            from_public_key: "k".into(),
        };
        // Ninguém conectado: o pedido seria recusado.
        assert!(!hub.incoming_request(&request));

        let connect = spawn_server(hub.clone());
        let (tx, rx) = mpsc::channel();
        for _ in 0..2 {
            let stream = connect();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let _ = read_events(stream, |e| {
                    let _ = tx.send(e);
                });
            });
        }
        // Espera os dois se inscreverem.
        for _ in 0..100 {
            if hub.clients.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(hub.incoming_request(&request));
        hub.status(&StatusDto::Connecting);
        let got: Vec<UiEvent> = (0..4)
            .map(|_| rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap())
            .collect();
        assert_eq!(got.iter().filter(|e| e.event == EVENT_MESSAGE).count(), 2);
        assert_eq!(got.iter().filter(|e| e.event == EVENT_STATUS).count(), 2);
        assert_eq!(got[0].payload["type"], "incoming_request");
    }

    #[test]
    fn versao_diferente_e_recusada() {
        let (mut client, server) = UnixStream::pair().unwrap();
        write_line(
            &mut client,
            &Hello {
                hello: Channel::Commands,
                version: 99,
            },
        )
        .unwrap();
        assert!(accept(&mut BufReader::new(server)).is_err());
    }
}
