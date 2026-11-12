//! Agente SYSTEM na sessão do usuário (`kr-desk.exe --agent`).

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kr_core::agent::{AgentBackend, ClipboardTarget, serve};
use kr_core::input::InputSink;
use kr_core::media::{CaptureError, FrameSource, SharedArea};
use kr_proto::control::{MouseButton, NormCoord};
use tracing::{info, warn};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::desktop::follow_input_desktop;
use crate::pipe::PipeServer;

/// Pede o Ctrl+Alt+Del ao serviço (que lê a nossa saída padrão).
fn request_sas(out: &Mutex<io::Stdout>) {
    let mut out = match out.lock() {
        Ok(o) => o,
        Err(p) => p.into_inner(),
    };
    if writeln!(out, "sas").and_then(|()| out.flush()).is_err() {
        warn!("serviço não recebeu o pedido de Ctrl+Alt+Del");
    } else {
        info!("Ctrl+Alt+Del pedido ao serviço");
    }
}

struct Backend {
    stdout: Arc<Mutex<io::Stdout>>,
}

impl AgentBackend for Backend {
    fn open_source(&mut self) -> Result<Box<dyn FrameSource>, CaptureError> {
        // A captura da área de trabalho errada não recebe quadros.
        follow_input_desktop();
        match kr_capture_win::DxgiCapture::primary() {
            Ok(c) => Ok(Box::new(c)),
            Err(err) => {
                warn!(%err, "DXGI indisponível no agente; tentando Windows.Graphics.Capture");
                kr_capture_win::WgcCapture::primary().map(|c| Box::new(c) as Box<dyn FrameSource>)
            }
        }
    }

    fn open_clipboard(
        &mut self,
        on_change: Box<dyn Fn(String) + Send + Sync>,
    ) -> Box<dyn ClipboardTarget> {
        struct Target(kr_input_win::ClipboardSync);
        impl ClipboardTarget for Target {
            fn set_remote(&mut self, text: &str) {
                self.0.set_remote(text);
            }
        }
        Box::new(Target(kr_input_win::ClipboardSync::start(on_change)))
    }

    fn open_sink(&mut self, area: SharedArea) -> Box<dyn InputSink> {
        Box::new(DesktopSink {
            inner: kr_input_win::SendInputSink::new(area),
            stdout: self.stdout.clone(),
        })
    }
}

/// `SendInput` na área de trabalho ativa (inclusive UAC e tela de bloqueio).
struct DesktopSink {
    inner: kr_input_win::SendInputSink,
    stdout: Arc<Mutex<io::Stdout>>,
}

impl InputSink for DesktopSink {
    fn move_to(&mut self, x: NormCoord, y: NormCoord) {
        follow_input_desktop();
        self.inner.move_to(x, y);
    }
    fn button(&mut self, button: MouseButton, down: bool) {
        follow_input_desktop();
        self.inner.button(button, down);
    }
    fn wheel(&mut self, dx: i32, dy: i32) {
        follow_input_desktop();
        self.inner.wheel(dx, dy);
    }
    fn key(&mut self, code: u16, ext: bool, down: bool) {
        follow_input_desktop();
        self.inner.key(code, ext, down);
    }
    fn secure_attention(&mut self) {
        request_sas(&self.stdout);
    }
}

/// Roda o agente até o serviço encerrar. Não retorna em caso normal.
pub fn run_agent() -> io::Result<()> {
    // Coordenadas reais dos monitores (sem a virtualização de DPI).
    // SAFETY: chamada única, no início do processo.
    if let Err(err) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        warn!(%err, "não foi possível ativar a consciência de DPI");
    }

    // O serviço segura a outra ponta da entrada padrão: quando ela fecha
    // (serviço parou ou caiu), o agente sai. Rodado à mão, sai na hora.
    std::thread::spawn(|| {
        let mut buf = [0u8; 64];
        let mut stdin = io::stdin();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        info!("serviço encerrou; agente saindo");
        std::process::exit(0);
    });

    let stdout = Arc::new(Mutex::new(io::stdout()));
    let mut server = PipeServer::agent()?;
    info!("agente pronto");
    let mut failures = 0u32;
    loop {
        match server.accept() {
            Ok(mut stream) => {
                failures = 0;
                let stdout = stdout.clone();
                std::thread::spawn(move || {
                    let mut backend = Backend { stdout };
                    match serve(&mut stream, &mut backend) {
                        Ok(()) => {}
                        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {}
                        Err(err) => info!(%err, "conexão com o app encerrada"),
                    }
                });
            }
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                warn!(%err, "conexão recusada no pipe do agente");
            }
            Err(err) => {
                failures += 1;
                warn!(%err, failures, "falha no pipe do agente");
                if failures >= 20 {
                    // Ex.: outro programa ocupando o nome do pipe.
                    return Err(err);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}
