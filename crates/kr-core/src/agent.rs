//! Conversa entre o app e o **agente** do serviço Windows.
//!
//! O agente roda como SYSTEM na sessão do usuário (iniciado pelo serviço) e
//! faz o que um processo comum não consegue: capturar a área de trabalho
//! segura (UAC, tela de bloqueio), injetar entrada em janelas de
//! administrador e pedir o Ctrl+Alt+Del. O app continua com a rede
//! (WebRTC, sinalização) e com a codificação; o agente só entrega quadros
//! BGRA e aplica a entrada.
//!
//! Tudo aqui é independente de plataforma (o transporte é qualquer
//! `Read + Write`; no Windows, um named pipe), para ser testado em Linux.
//!
//! Formato: cada mensagem é `u32` LE com o tamanho + corpo; o primeiro byte
//! do corpo é o tipo. Cada conexão começa com `Hello` e tem um papel só:
//! - **captura:** o app pede `Capture`/`SelectMonitor`; o agente responde com
//!   zero ou mais `Event` seguidos de uma resposta final (`Frame`, `NoFrame`,
//!   `Ok` ou `Error`);
//! - **entrada:** o app manda comandos sem resposta.

use std::collections::{BTreeSet, VecDeque};
use std::io::{self, Read, Write};
use std::time::Duration;

use kr_proto::control::{MouseButton, NormCoord};

use crate::input::InputSink;
use crate::media::{
    BgraFrame, CaptureError, CursorUpdate, DesktopRect, FrameSource, MonitorInfo, SharedArea,
    SourceEvent,
};

/// Versão do protocolo app ↔ agente (app e agente vêm do mesmo instalador,
/// mas o agente antigo pode seguir rodando durante uma atualização).
pub const AGENT_PROTOCOL_VERSION: u16 = 1;

/// Maior mensagem aceita (um quadro 4K BGRA tem ~33 MB).
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// Maior forma de cursor aceita.
const MAX_CURSOR_BYTES: usize = 256 * 256 * 4;

/// Papel de uma conexão com o agente.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Capture,
    Input,
}

/// Pedido do app ao agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Hello {
        version: u16,
        role: Role,
    },
    Capture {
        timeout_ms: u32,
    },
    SelectMonitor {
        index: u32,
    },
    /// Posição normalizada no monitor transmitido (`area`; `None` = principal).
    Move {
        x: NormCoord,
        y: NormCoord,
        area: Option<DesktopRect>,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    Wheel {
        dx: i32,
        dy: i32,
    },
    Key {
        code: u16,
        ext: bool,
        down: bool,
    },
    /// Ctrl+Alt+Del (secure attention sequence).
    Sas,
}

/// Resposta do agente (conexões de captura).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Ok,
    Frame(BgraFrame),
    NoFrame,
    Error { transient: bool, message: String },
    Event(SourceEvent),
}

// --- codificação -------------------------------------------------------------

mod tag {
    pub const HELLO: u8 = 1;
    pub const CAPTURE: u8 = 2;
    pub const SELECT_MONITOR: u8 = 3;
    pub const MOVE: u8 = 4;
    pub const BUTTON: u8 = 5;
    pub const WHEEL: u8 = 6;
    pub const KEY: u8 = 7;
    pub const SAS: u8 = 8;

    pub const OK: u8 = 20;
    pub const FRAME: u8 = 21;
    pub const NO_FRAME: u8 = 22;
    pub const ERROR: u8 = 23;
    pub const CURSOR_SHAPE: u8 = 24;
    pub const CURSOR_VISIBLE: u8 = 25;
    pub const MONITORS: u8 = 26;
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

struct Enc(Vec<u8>);

impl Enc {
    fn new(tag: u8) -> Self {
        Self(vec![tag])
    }
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    fn bool(self, v: bool) -> Self {
        self.u8(v as u8)
    }
    fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(mut self, v: i32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(mut self, v: &[u8]) -> Self {
        self = self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
    fn rect(self, r: DesktopRect) -> Self {
        self.i32(r.left).i32(r.top).u32(r.width).u32(r.height)
    }
}

struct Dec<'a>(&'a [u8]);

impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(bad("mensagem curta"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn bytes(&mut self, max: usize) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(bad("campo grande demais"));
        }
        self.take(n)
    }
    fn string(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(self.bytes(1024)?).into_owned())
    }
    fn rect(&mut self) -> io::Result<DesktopRect> {
        Ok(DesktopRect {
            left: self.i32()?,
            top: self.i32()?,
            width: self.u32()?,
            height: self.u32()?,
        })
    }
}

fn button_code(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::X1 => 3,
        MouseButton::X2 => 4,
    }
}

fn button_from(code: u8) -> io::Result<MouseButton> {
    Ok(match code {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::X1,
        4 => MouseButton::X2,
        _ => return Err(bad("botão desconhecido")),
    })
}

impl Request {
    fn encode(&self) -> Vec<u8> {
        match *self {
            Request::Hello { version, role } => {
                Enc::new(tag::HELLO)
                    .u16(version)
                    .u8(match role {
                        Role::Capture => 1,
                        Role::Input => 2,
                    })
                    .0
            }
            Request::Capture { timeout_ms } => Enc::new(tag::CAPTURE).u32(timeout_ms).0,
            Request::SelectMonitor { index } => Enc::new(tag::SELECT_MONITOR).u32(index).0,
            Request::Move { x, y, area } => {
                let e = Enc::new(tag::MOVE).u16(x).u16(y);
                match area {
                    Some(r) => e.bool(true).rect(r).0,
                    None => e.bool(false).0,
                }
            }
            Request::Button { button, down } => {
                Enc::new(tag::BUTTON).u8(button_code(button)).bool(down).0
            }
            Request::Wheel { dx, dy } => Enc::new(tag::WHEEL).i32(dx).i32(dy).0,
            Request::Key { code, ext, down } => Enc::new(tag::KEY).u16(code).bool(ext).bool(down).0,
            Request::Sas => Enc::new(tag::SAS).0,
        }
    }

    fn decode(body: &[u8]) -> io::Result<Self> {
        let mut d = Dec(body);
        Ok(match d.u8()? {
            tag::HELLO => Request::Hello {
                version: d.u16()?,
                role: match d.u8()? {
                    1 => Role::Capture,
                    2 => Role::Input,
                    _ => return Err(bad("papel desconhecido")),
                },
            },
            tag::CAPTURE => Request::Capture {
                timeout_ms: d.u32()?,
            },
            tag::SELECT_MONITOR => Request::SelectMonitor { index: d.u32()? },
            tag::MOVE => Request::Move {
                x: d.u16()?,
                y: d.u16()?,
                area: if d.bool()? { Some(d.rect()?) } else { None },
            },
            tag::BUTTON => Request::Button {
                button: button_from(d.u8()?)?,
                down: d.bool()?,
            },
            tag::WHEEL => Request::Wheel {
                dx: d.i32()?,
                dy: d.i32()?,
            },
            tag::KEY => Request::Key {
                code: d.u16()?,
                ext: d.bool()?,
                down: d.bool()?,
            },
            tag::SAS => Request::Sas,
            t => return Err(bad(format!("pedido desconhecido: {t}"))),
        })
    }
}

fn encode_frame(frame: &BgraFrame, out: &mut Vec<u8>) {
    out.clear();
    out.push(tag::FRAME);
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&(frame.stride as u32).to_le_bytes());
    out.extend_from_slice(&frame.data);
}

impl Reply {
    fn encode(&self) -> Vec<u8> {
        match self {
            Reply::Ok => vec![tag::OK],
            Reply::NoFrame => vec![tag::NO_FRAME],
            Reply::Frame(f) => {
                let mut out = Vec::new();
                encode_frame(f, &mut out);
                out
            }
            Reply::Error { transient, message } => {
                Enc::new(tag::ERROR)
                    .bool(*transient)
                    .bytes(message.as_bytes())
                    .0
            }
            Reply::Event(SourceEvent::Cursor(CursorUpdate::Visible(v))) => {
                Enc::new(tag::CURSOR_VISIBLE).bool(*v).0
            }
            Reply::Event(SourceEvent::Cursor(CursorUpdate::Shape {
                width,
                height,
                hot_x,
                hot_y,
                rgba,
            })) => {
                Enc::new(tag::CURSOR_SHAPE)
                    .u32(*width)
                    .u32(*height)
                    .u32(*hot_x)
                    .u32(*hot_y)
                    .bytes(rgba)
                    .0
            }
            Reply::Event(SourceEvent::Monitors { list, active }) => {
                let mut e = Enc::new(tag::MONITORS).u32(*active).u32(list.len() as u32);
                for m in list {
                    e = e
                        .u32(m.index)
                        .bytes(m.name.as_bytes())
                        .rect(m.rect)
                        .bool(m.primary);
                }
                e.0
            }
        }
    }

    fn decode(body: &[u8]) -> io::Result<Self> {
        let mut d = Dec(body);
        Ok(match d.u8()? {
            tag::OK => Reply::Ok,
            tag::NO_FRAME => Reply::NoFrame,
            tag::FRAME => {
                let (width, height, stride) = (d.u32()?, d.u32()?, d.u32()? as usize);
                if stride < width as usize * 4 || d.0.len() != stride * height as usize {
                    return Err(bad("quadro com tamanho inconsistente"));
                }
                Reply::Frame(BgraFrame {
                    width,
                    height,
                    stride,
                    data: d.0.to_vec(),
                })
            }
            tag::ERROR => Reply::Error {
                transient: d.bool()?,
                message: d.string()?,
            },
            tag::CURSOR_VISIBLE => {
                Reply::Event(SourceEvent::Cursor(CursorUpdate::Visible(d.bool()?)))
            }
            tag::CURSOR_SHAPE => {
                let (width, height, hot_x, hot_y) = (d.u32()?, d.u32()?, d.u32()?, d.u32()?);
                let rgba = d.bytes(MAX_CURSOR_BYTES)?.to_vec();
                if rgba.len() != width as usize * height as usize * 4 {
                    return Err(bad("cursor com tamanho inconsistente"));
                }
                Reply::Event(SourceEvent::Cursor(CursorUpdate::Shape {
                    width,
                    height,
                    hot_x,
                    hot_y,
                    rgba,
                }))
            }
            tag::MONITORS => {
                let active = d.u32()?;
                let n = d.u32()?.min(64);
                let mut list = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    list.push(MonitorInfo {
                        index: d.u32()?,
                        name: d.string()?,
                        rect: d.rect()?,
                        primary: d.bool()?,
                    });
                }
                Reply::Event(SourceEvent::Monitors { list, active })
            }
            t => return Err(bad(format!("resposta desconhecida: {t}"))),
        })
    }
}

fn write_message(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_MESSAGE {
        return Err(bad("mensagem grande demais"));
    }
    // Um `write_all` só: em named pipes cada escrita é uma chamada ao kernel.
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    w.write_all(&out)?;
    w.flush()
}

/// Lê uma mensagem inteira para `buf` (reaproveitado entre chamadas).
fn read_message(r: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<()> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(bad("tamanho de mensagem inválido"));
    }
    buf.resize(len, 0);
    r.read_exact(buf)
}

pub fn write_request(w: &mut impl Write, req: &Request) -> io::Result<()> {
    write_message(w, &req.encode())
}

pub fn read_request(r: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<Request> {
    read_message(r, buf)?;
    Request::decode(buf)
}

pub fn write_reply(w: &mut impl Write, reply: &Reply) -> io::Result<()> {
    write_message(w, &reply.encode())
}

pub fn read_reply(r: &mut impl Read, buf: &mut Vec<u8>) -> io::Result<Reply> {
    read_message(r, buf)?;
    Reply::decode(buf)
}

// --- lado do app ---------------------------------------------------------------

/// Abre uma conexão nova com o agente (ou falha se ele não estiver rodando).
pub type Connector<S> = Box<dyn FnMut() -> io::Result<S> + Send>;

fn hello<S: Read + Write>(stream: &mut S, role: Role, buf: &mut Vec<u8>) -> io::Result<()> {
    write_request(
        stream,
        &Request::Hello {
            version: AGENT_PROTOCOL_VERSION,
            role,
        },
    )?;
    match read_reply(stream, buf)? {
        Reply::Ok => Ok(()),
        Reply::Error { message, .. } => Err(io::Error::other(message)),
        other => Err(bad(format!("resposta inesperada ao Hello: {other:?}"))),
    }
}

/// Captura feita pelo agente. Se a conexão cair (agente reiniciado), a
/// próxima chamada reconecta; enquanto isso a captura fica "indisponível no
/// momento" e o pipeline tenta de novo.
pub struct AgentSource<S: Read + Write> {
    connect: Connector<S>,
    stream: Option<S>,
    frame: BgraFrame,
    events: VecDeque<SourceEvent>,
    buf: Vec<u8>,
    /// Monitor escolhido, reaplicado ao reconectar.
    selected: Option<u32>,
}

impl<S: Read + Write> AgentSource<S> {
    /// Conecta já (para o app saber se o agente está disponível).
    pub fn connect(mut connect: Connector<S>) -> io::Result<Self> {
        let mut buf = Vec::new();
        let mut stream = connect()?;
        hello(&mut stream, Role::Capture, &mut buf)?;
        Ok(Self {
            connect,
            stream: Some(stream),
            frame: BgraFrame::new(2, 2),
            events: VecDeque::new(),
            buf,
            selected: None,
        })
    }

    fn ensure(&mut self) -> Result<&mut S, CaptureError> {
        if self.stream.is_none() {
            let mut stream = (self.connect)().map_err(|e| {
                CaptureError::Transient(format!("agente do serviço indisponível: {e}"))
            })?;
            hello(&mut stream, Role::Capture, &mut self.buf)
                .map_err(|e| CaptureError::Transient(format!("agente recusou: {e}")))?;
            self.stream = Some(stream);
            if let Some(index) = self.selected {
                let _ = self.call(&Request::SelectMonitor { index });
            }
        }
        Ok(self.stream.as_mut().expect("aberta acima"))
    }

    /// Envia o pedido e junta os eventos até a resposta final.
    fn call(&mut self, req: &Request) -> Result<Reply, CaptureError> {
        let result = (|| {
            let stream = self.ensure()?;
            write_request(stream, req).map_err(io_lost)?;
            loop {
                let stream = self.stream.as_mut().expect("aberta");
                match read_reply(stream, &mut self.buf).map_err(io_lost)? {
                    Reply::Event(e) => self.events.push_back(e),
                    other => return Ok(other),
                }
            }
        })();
        if result.is_err() {
            // Conexão em estado desconhecido: começa outra na próxima vez.
            self.stream = None;
        }
        result
    }
}

fn io_lost(err: io::Error) -> CaptureError {
    CaptureError::Transient(format!("conexão com o agente caiu: {err}"))
}

fn reply_error(transient: bool, message: String) -> CaptureError {
    if transient {
        CaptureError::Transient(message)
    } else {
        CaptureError::Fatal(message)
    }
}

impl<S: Read + Write> FrameSource for AgentSource<S> {
    fn capture(&mut self, timeout: Duration) -> Result<Option<&BgraFrame>, CaptureError> {
        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        match self.call(&Request::Capture { timeout_ms })? {
            Reply::Frame(f) => {
                self.frame = f;
                Ok(Some(&self.frame))
            }
            Reply::NoFrame => Ok(None),
            Reply::Error { transient, message } => Err(reply_error(transient, message)),
            other => Err(CaptureError::Fatal(format!(
                "resposta inesperada do agente: {other:?}"
            ))),
        }
    }

    fn name(&self) -> String {
        "agente do serviço (SYSTEM)".into()
    }

    fn take_event(&mut self) -> Option<SourceEvent> {
        self.events.pop_front()
    }

    fn select_monitor(&mut self, index: u32) -> Result<(), CaptureError> {
        self.selected = Some(index);
        match self.call(&Request::SelectMonitor { index })? {
            Reply::Ok => Ok(()),
            Reply::Error { transient, message } => Err(reply_error(transient, message)),
            other => Err(CaptureError::Fatal(format!(
                "resposta inesperada do agente: {other:?}"
            ))),
        }
    }
}

/// Entrada aplicada pelo agente (funciona em janelas de administrador e na
/// área de trabalho segura). Reconecta sozinha se o agente reiniciar.
pub struct AgentInputSink<S: Read + Write> {
    connect: Connector<S>,
    stream: Option<S>,
    /// Monitor transmitido (atualizado pela captura no app).
    area: SharedArea,
    failures: u32,
}

impl<S: Read + Write> AgentInputSink<S> {
    pub fn connect(mut connect: Connector<S>, area: SharedArea) -> io::Result<Self> {
        let mut stream = connect()?;
        hello(&mut stream, Role::Input, &mut Vec::new())?;
        Ok(Self {
            connect,
            stream: Some(stream),
            area,
            failures: 0,
        })
    }

    fn send(&mut self, req: Request) {
        if self.stream.is_none() {
            let opened = (self.connect)()
                .and_then(|mut s| hello(&mut s, Role::Input, &mut Vec::new()).map(|()| s));
            match opened {
                Ok(s) => self.stream = Some(s),
                Err(_) => {
                    self.failures += 1;
                    if self.failures == 1 || self.failures.is_multiple_of(100) {
                        tracing::warn!(
                            failures = self.failures,
                            "agente indisponível para entrada"
                        );
                    }
                    return;
                }
            }
        }
        let stream = self.stream.as_mut().expect("aberta acima");
        if let Err(err) = write_request(stream, &req) {
            tracing::warn!(%err, "conexão de entrada com o agente caiu");
            self.stream = None;
        }
    }
}

impl<S: Read + Write> InputSink for AgentInputSink<S> {
    fn move_to(&mut self, x: NormCoord, y: NormCoord) {
        let area = self.area.lock().ok().and_then(|a| *a);
        self.send(Request::Move { x, y, area });
    }
    fn button(&mut self, button: MouseButton, down: bool) {
        self.send(Request::Button { button, down });
    }
    fn wheel(&mut self, dx: i32, dy: i32) {
        self.send(Request::Wheel { dx, dy });
    }
    fn key(&mut self, code: u16, ext: bool, down: bool) {
        self.send(Request::Key { code, ext, down });
    }
    fn secure_attention(&mut self) {
        self.send(Request::Sas);
    }
}

// --- lado do agente -------------------------------------------------------------

/// O que o agente sabe fazer; a implementação real fica no `kr-service-win`.
pub trait AgentBackend {
    /// Abre a captura (na thread da conexão).
    fn open_source(&mut self) -> Result<Box<dyn FrameSource>, CaptureError>;
    /// Destino da entrada; `area` é atualizada a cada movimento.
    fn open_sink(&mut self, area: SharedArea) -> Box<dyn InputSink>;
}

const MAX_SCANCODE: u16 = 0xFF;
const MAX_WHEEL: i32 = 120 * 20;

/// Atende uma conexão até ela fechar. Erros de E/S encerram a conexão.
pub fn serve<S: Read + Write>(stream: &mut S, backend: &mut dyn AgentBackend) -> io::Result<()> {
    let mut buf = Vec::new();
    let role = match read_request(stream, &mut buf)? {
        Request::Hello { version, role } if version == AGENT_PROTOCOL_VERSION => role,
        Request::Hello { version, .. } => {
            let message = format!(
                "versão do agente diferente (agente {AGENT_PROTOCOL_VERSION}, app {version})"
            );
            write_reply(
                stream,
                &Reply::Error {
                    transient: false,
                    message: message.clone(),
                },
            )?;
            return Err(bad(message));
        }
        _ => return Err(bad("conexão sem Hello")),
    };
    write_reply(stream, &Reply::Ok)?;
    match role {
        Role::Capture => serve_capture(stream, backend, buf),
        Role::Input => serve_input(stream, backend, buf),
    }
}

fn flush_events<S: Write>(stream: &mut S, source: &mut dyn FrameSource) -> io::Result<()> {
    while let Some(e) = source.take_event() {
        write_reply(stream, &Reply::Event(e))?;
    }
    Ok(())
}

fn error_reply(err: CaptureError) -> Reply {
    match err {
        CaptureError::Transient(message) => Reply::Error {
            transient: true,
            message,
        },
        CaptureError::Fatal(message) => Reply::Error {
            transient: false,
            message,
        },
    }
}

fn serve_capture<S: Read + Write>(
    stream: &mut S,
    backend: &mut dyn AgentBackend,
    mut buf: Vec<u8>,
) -> io::Result<()> {
    let mut source: Option<Box<dyn FrameSource>> = None;
    let mut selected: Option<u32> = None;
    let mut frame_msg = Vec::new();
    loop {
        match read_request(stream, &mut buf)? {
            Request::Capture { timeout_ms } => {
                if source.is_none() {
                    match backend.open_source() {
                        Ok(mut s) => {
                            if let Some(i) = selected
                                && let Err(err) = s.select_monitor(i)
                            {
                                tracing::warn!(%err, "monitor escolhido indisponível");
                            }
                            source = Some(s);
                        }
                        Err(err) => {
                            write_reply(stream, &error_reply(err))?;
                            continue;
                        }
                    }
                }
                let s = source.as_mut().expect("aberta acima");
                let timeout = Duration::from_millis(timeout_ms.min(5000) as u64);
                let result = match s.capture(timeout) {
                    Ok(Some(f)) => {
                        encode_frame(f, &mut frame_msg);
                        Ok(true)
                    }
                    Ok(None) => Ok(false),
                    Err(err) => Err(err),
                };
                flush_events(stream, s.as_mut())?;
                match result {
                    Ok(true) => write_message(stream, &frame_msg)?,
                    Ok(false) => write_reply(stream, &Reply::NoFrame)?,
                    Err(err) => {
                        // Troca de área de trabalho (UAC, bloqueio) derruba a
                        // captura: a próxima chamada abre de novo, já na nova.
                        source = None;
                        write_reply(stream, &error_reply(err))?;
                    }
                }
            }
            Request::SelectMonitor { index } => {
                selected = Some(index);
                let reply = match source.as_mut() {
                    Some(s) => {
                        let r = s.select_monitor(index);
                        flush_events(stream, s.as_mut())?;
                        match r {
                            Ok(()) => Reply::Ok,
                            Err(err) => error_reply(err),
                        }
                    }
                    // Aplicado quando a captura abrir.
                    None => Reply::Ok,
                };
                write_reply(stream, &reply)?;
            }
            other => return Err(bad(format!("pedido inválido na captura: {other:?}"))),
        }
    }
}

fn serve_input<S: Read + Write>(
    stream: &mut S,
    backend: &mut dyn AgentBackend,
    mut buf: Vec<u8>,
) -> io::Result<()> {
    let area: SharedArea = Default::default();
    let mut sink = backend.open_sink(area.clone());
    let mut keys = BTreeSet::new();
    let mut buttons = BTreeSet::new();
    let result = loop {
        let req = match read_request(stream, &mut buf) {
            Ok(r) => r,
            Err(err) => break Err(err),
        };
        match req {
            Request::Move { x, y, area: rect } => {
                if let Ok(mut a) = area.lock() {
                    *a = rect;
                }
                sink.move_to(x, y);
            }
            Request::Button { button, down } => {
                if down {
                    buttons.insert(button);
                } else {
                    buttons.remove(&button);
                }
                sink.button(button, down);
            }
            Request::Wheel { dx, dy } => sink.wheel(
                dx.clamp(-MAX_WHEEL, MAX_WHEEL),
                dy.clamp(-MAX_WHEEL, MAX_WHEEL),
            ),
            Request::Key { code, ext, down } => {
                if code == 0 || code > MAX_SCANCODE {
                    continue;
                }
                if down {
                    keys.insert((code, ext));
                } else {
                    keys.remove(&(code, ext));
                }
                sink.key(code, ext, down);
            }
            Request::Sas => sink.secure_attention(),
            other => break Err(bad(format!("pedido inválido na entrada: {other:?}"))),
        }
    };
    // O app caiu no meio de um atalho: nada fica pressionado.
    for (code, ext) in keys.into_iter().rev() {
        sink.key(code, ext, false);
    }
    for b in buttons {
        sink.button(b, false);
    }
    result
}

// Os testes usam pares de sockets Unix (no Windows, o CI cobre o resto).
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::media::pattern::TestPattern;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl InputSink for Log {
        fn move_to(&mut self, x: u16, y: u16) {
            self.0.lock().unwrap().push(format!("move {x},{y}"));
        }
        fn button(&mut self, b: MouseButton, down: bool) {
            self.0.lock().unwrap().push(format!("{b:?} {down}"));
        }
        fn wheel(&mut self, dx: i32, dy: i32) {
            self.0.lock().unwrap().push(format!("wheel {dx},{dy}"));
        }
        fn key(&mut self, code: u16, ext: bool, down: bool) {
            self.0
                .lock()
                .unwrap()
                .push(format!("key {code:#x} {ext} {down}"));
        }
        fn secure_attention(&mut self) {
            self.0.lock().unwrap().push("sas".into());
        }
    }

    struct Backend {
        log: Log,
        areas: Arc<Mutex<Vec<Option<DesktopRect>>>>,
        opens: Arc<Mutex<u32>>,
    }

    impl AgentBackend for Backend {
        fn open_source(&mut self) -> Result<Box<dyn FrameSource>, CaptureError> {
            *self.opens.lock().unwrap() += 1;
            Ok(Box::new(TestPattern::with_monitors(
                &[(64, 48), (32, 24)],
                30,
            )))
        }
        fn open_sink(&mut self, area: SharedArea) -> Box<dyn InputSink> {
            struct Recording(Log, SharedArea, Arc<Mutex<Vec<Option<DesktopRect>>>>);
            impl InputSink for Recording {
                fn move_to(&mut self, x: u16, y: u16) {
                    self.2.lock().unwrap().push(*self.1.lock().unwrap());
                    self.0.move_to(x, y)
                }
                fn button(&mut self, b: MouseButton, d: bool) {
                    self.0.button(b, d)
                }
                fn wheel(&mut self, dx: i32, dy: i32) {
                    self.0.wheel(dx, dy)
                }
                fn key(&mut self, c: u16, e: bool, d: bool) {
                    self.0.key(c, e, d)
                }
                fn secure_attention(&mut self) {
                    self.0.secure_attention()
                }
            }
            Box::new(Recording(self.log.clone(), area, self.areas.clone()))
        }
    }

    fn backend() -> Backend {
        Backend {
            log: Log::default(),
            areas: Default::default(),
            opens: Default::default(),
        }
    }

    /// Conector que entrega um lado de um par de sockets ligado a um agente
    /// rodando numa thread.
    fn agent(b: &Arc<Mutex<Backend>>) -> Connector<UnixStream> {
        let b = b.clone();
        Box::new(move || {
            let (client, mut server) = UnixStream::pair()?;
            let b = b.clone();
            std::thread::spawn(move || {
                let mut guard = b.lock().unwrap();
                let _ = serve(&mut server, &mut *guard);
            });
            Ok(client)
        })
    }

    #[test]
    fn pedidos_e_respostas_vao_e_voltam() {
        let reqs = [
            Request::Hello {
                version: 1,
                role: Role::Input,
            },
            Request::Capture { timeout_ms: 33 },
            Request::SelectMonitor { index: 2 },
            Request::Move {
                x: 1,
                y: 65535,
                area: Some(DesktopRect {
                    left: -1920,
                    top: 0,
                    width: 1920,
                    height: 1080,
                }),
            },
            Request::Move {
                x: 5,
                y: 6,
                area: None,
            },
            Request::Button {
                button: MouseButton::X2,
                down: true,
            },
            Request::Wheel { dx: -120, dy: 240 },
            Request::Key {
                code: 0x5B,
                ext: true,
                down: false,
            },
            Request::Sas,
        ];
        for r in reqs {
            assert_eq!(Request::decode(&r.encode()).unwrap(), r);
        }
        let mut frame = BgraFrame::new(3, 2);
        frame.data[5] = 9;
        let replies = [
            Reply::Ok,
            Reply::NoFrame,
            Reply::Frame(frame),
            Reply::Error {
                transient: true,
                message: "acesso perdido".into(),
            },
            Reply::Event(SourceEvent::Cursor(CursorUpdate::Visible(false))),
            Reply::Event(SourceEvent::Cursor(CursorUpdate::Shape {
                width: 1,
                height: 2,
                hot_x: 0,
                hot_y: 1,
                rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
            })),
            Reply::Event(SourceEvent::Monitors {
                list: vec![MonitorInfo {
                    index: 0,
                    name: "Tela 1".into(),
                    rect: DesktopRect {
                        left: 0,
                        top: 0,
                        width: 10,
                        height: 10,
                    },
                    primary: true,
                }],
                active: 0,
            }),
        ];
        for r in replies {
            assert_eq!(Reply::decode(&r.encode()).unwrap(), r);
        }
    }

    #[test]
    fn rejeita_mensagens_malformadas() {
        assert!(Request::decode(&[99]).is_err());
        assert!(Request::decode(&[tag::MOVE, 1]).is_err());
        // Quadro com dados a menos que width × height × 4.
        let mut bad_frame = Enc::new(tag::FRAME).u32(4).u32(4).u32(16).0;
        bad_frame.extend_from_slice(&[0; 10]);
        assert!(Reply::decode(&bad_frame).is_err());
        let mut r = io::Cursor::new(vec![0, 0, 0, 0]);
        assert!(read_request(&mut r, &mut Vec::new()).is_err());
    }

    #[test]
    fn captura_pelo_agente_entrega_quadros_e_eventos() {
        let b = Arc::new(Mutex::new(backend()));
        let mut source = AgentSource::connect(agent(&b)).unwrap();
        let frame = source
            .capture(Duration::from_millis(50))
            .unwrap()
            .expect("quadro")
            .clone();
        assert_eq!((frame.width, frame.height), (64, 48));
        let mut events = Vec::new();
        while let Some(e) = source.take_event() {
            events.push(e);
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SourceEvent::Monitors { list, .. } if list.len() == 2)),
            "{events:?}"
        );
        source.select_monitor(1).unwrap();
        let mut active = None;
        while let Some(e) = source.take_event() {
            if let SourceEvent::Monitors { active: a, .. } = e {
                active = Some(a);
            }
        }
        assert_eq!(active, Some(1));
    }

    #[test]
    fn captura_reconecta_quando_o_agente_reinicia() {
        let b = Arc::new(Mutex::new(backend()));
        let opens = b.lock().unwrap().opens.clone();
        let drops = Arc::new(Mutex::new(0u32));
        let inner = agent(&b);
        let inner = Arc::new(Mutex::new(inner));
        let d = drops.clone();
        // A primeira conexão é encerrada depois do Hello (agente "reiniciou").
        let connector: Connector<UnixStream> = Box::new(move || {
            let s = (inner.lock().unwrap())()?;
            let mut n = d.lock().unwrap();
            *n += 1;
            if *n == 1 {
                let dup = s.try_clone()?;
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(50));
                    let _ = dup.shutdown(std::net::Shutdown::Both);
                });
            }
            Ok(s)
        });
        let mut source = AgentSource::connect(connector).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(matches!(
            source.capture(Duration::from_millis(10)),
            Err(CaptureError::Transient(_))
        ));
        assert!(source.capture(Duration::from_millis(10)).unwrap().is_some());
        assert_eq!(*drops.lock().unwrap(), 2);
        assert!(*opens.lock().unwrap() >= 1);
    }

    #[test]
    fn entrada_pelo_agente_e_solta_tudo_ao_cair() {
        let b = Arc::new(Mutex::new(backend()));
        let (log, areas) = {
            let g = b.lock().unwrap();
            (g.log.clone(), g.areas.clone())
        };
        let area: SharedArea = Default::default();
        let rect = DesktopRect {
            left: 1920,
            top: 0,
            width: 1280,
            height: 1024,
        };
        {
            let mut sink = AgentInputSink::connect(agent(&b), area.clone()).unwrap();
            sink.move_to(10, 20);
            *area.lock().unwrap() = Some(rect);
            sink.move_to(30, 40);
            sink.button(MouseButton::Left, true);
            sink.key(0x1D, false, true);
            sink.key(0x0, false, true); // inválida: ignorada
            sink.wheel(0, 99_999);
            sink.secure_attention();
        } // conexão fecha com Ctrl e botão esquerdo pressionados
        std::thread::sleep(Duration::from_millis(200));
        let log = log.0.lock().unwrap().clone();
        assert_eq!(
            log,
            vec![
                "move 10,20",
                "move 30,40",
                "Left true",
                "key 0x1d false true",
                "wheel 0,2400",
                "sas",
                "key 0x1d false false",
                "Left false",
            ]
        );
        assert_eq!(*areas.lock().unwrap(), vec![None, Some(rect)]);
    }

    #[test]
    fn versao_diferente_e_recusada() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || serve(&mut server, &mut backend()));
        write_request(
            &mut client,
            &Request::Hello {
                version: 999,
                role: Role::Capture,
            },
        )
        .unwrap();
        let reply = read_reply(&mut client, &mut Vec::new()).unwrap();
        assert!(matches!(
            reply,
            Reply::Error {
                transient: false,
                ..
            }
        ));
        assert!(t.join().unwrap().is_err());
    }
}
