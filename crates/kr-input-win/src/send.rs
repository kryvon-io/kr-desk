use kr_core::input::InputSink;
use kr_proto::control::{MouseButton, NormCoord};
use tracing::warn;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Marca as entradas injetadas pelo KR Desk (`dwExtraInfo`).
const EXTRA_INFO: usize = 0x4B52_4445; // "KRDE"
const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

/// Converte a posição normalizada na tela principal para a coordenada
/// absoluta (0–65535) da área de trabalho virtual.
fn to_virtual_desk(
    x: NormCoord,
    y: NormCoord,
    primary: (i32, i32),
    virtual_desk: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (pw, ph) = (primary.0.max(1) as i64, primary.1.max(1) as i64);
    let (vx, vy, vw, vh) = virtual_desk;
    let (vw, vh) = ((vw as i64).max(2), (vh as i64).max(2));
    // Pixel no monitor principal (que começa em 0,0 na área virtual).
    let px = x as i64 * (pw - 1) / 65535;
    let py = y as i64 * (ph - 1) / 65535;
    let ax = (px - vx as i64) * 65535 / (vw - 1);
    let ay = (py - vy as i64) * 65535 / (vh - 1);
    (ax.clamp(0, 65535) as i32, ay.clamp(0, 65535) as i32)
}

fn metric(index: windows::Win32::UI::WindowsAndMessaging::SYSTEM_METRICS_INDEX) -> i32 {
    // SAFETY: consulta simples sem ponteiros.
    unsafe { GetSystemMetrics(index) }
}

/// Injeta entrada no Windows local.
#[derive(Debug, Default)]
pub struct SendInputSink {
    failures: u32,
}

impl SendInputSink {
    pub fn new() -> Self {
        Self::default()
    }

    fn send(&mut self, inputs: &[INPUT]) {
        // SAFETY: estruturas INPUT válidas e tamanho correto.
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            self.failures += 1;
            // UIPI (janela elevada em foco) ou área de trabalho segura.
            if self.failures == 1 || self.failures.is_multiple_of(100) {
                warn!(
                    failures = self.failures,
                    "SendInput bloqueado (janela de administrador em foco?)"
                );
            }
        }
    }

    fn mouse(&mut self, dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) {
        self.send(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: EXTRA_INFO,
                },
            },
        }]);
    }
}

impl InputSink for SendInputSink {
    fn move_to(&mut self, x: NormCoord, y: NormCoord) {
        let primary = (metric(SM_CXSCREEN), metric(SM_CYSCREEN));
        let desk = (
            metric(SM_XVIRTUALSCREEN),
            metric(SM_YVIRTUALSCREEN),
            metric(SM_CXVIRTUALSCREEN),
            metric(SM_CYVIRTUALSCREEN),
        );
        let (ax, ay) = to_virtual_desk(x, y, primary, desk);
        self.mouse(
            ax,
            ay,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        );
    }

    fn button(&mut self, button: MouseButton, down: bool) {
        let (flags, data) = match (button, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        };
        self.mouse(0, 0, data, flags);
    }

    fn wheel(&mut self, dx: i32, dy: i32) {
        if dy != 0 {
            self.mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL);
        }
        if dx != 0 {
            self.mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL);
        }
    }

    fn key(&mut self, code: u16, ext: bool, down: bool) {
        let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
        if ext {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        self.send(&[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: code,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: EXTRA_INFO,
                },
            },
        }]);
    }
}

#[cfg(test)]
mod tests {
    use super::to_virtual_desk;

    #[test]
    fn monitor_unico() {
        let desk = (0, 0, 1920, 1080);
        assert_eq!(to_virtual_desk(0, 0, (1920, 1080), desk), (0, 0));
        assert_eq!(
            to_virtual_desk(65535, 65535, (1920, 1080), desk),
            (65535, 65535)
        );
    }

    #[test]
    fn segundo_monitor_a_esquerda() {
        // Monitor secundário de 1280 px à esquerda do principal.
        let desk = (-1280, 0, 3200, 1080);
        let (x, _) = to_virtual_desk(0, 0, (1920, 1080), desk);
        // Pixel 0 do principal = pixel 1280 da área virtual.
        assert_eq!(x, 1280 * 65535 / 3199);
        let (x, _) = to_virtual_desk(65535, 0, (1920, 1080), desk);
        assert_eq!(x, 65535);
    }
}
