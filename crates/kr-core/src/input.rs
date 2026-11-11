//! Entrada remota no host: aplica as mensagens do viewer a um [`InputSink`]
//! (no Windows, `SendInput` em `kr-input-win`) e lembra o que está
//! pressionado, para soltar tudo ao fim da sessão — nada de tecla "presa".

use std::collections::BTreeSet;

use kr_proto::control::{ControlMessage, MouseButton, NormCoord};

/// Destino das ações de entrada (implementado por plataforma).
pub trait InputSink {
    /// Move o ponteiro para a posição normalizada da tela transmitida.
    fn move_to(&mut self, x: NormCoord, y: NormCoord);
    fn button(&mut self, button: MouseButton, down: bool);
    /// Roda do mouse (120 = um clique; `dy > 0` para cima).
    fn wheel(&mut self, dx: i32, dy: i32);
    /// Tecla por scancode (conjunto 1); `ext` = tecla estendida (prefixo E0).
    fn key(&mut self, code: u16, ext: bool, down: bool);
    /// Ctrl+Alt+Del (só funciona com o serviço do KR Desk instalado).
    fn secure_attention(&mut self) {
        tracing::info!("Ctrl+Alt+Del ignorado: serviço do KR Desk não disponível");
    }
}

/// Maior scancode aceito (conjunto 1 vai até 0x7F; deixamos folga).
const MAX_SCANCODE: u16 = 0xFF;
/// Limite por mensagem de roda, contra valores absurdos.
const MAX_WHEEL: i32 = 120 * 20;

pub struct InputController<S: InputSink> {
    sink: S,
    keys: BTreeSet<(u16, bool)>,
    buttons: BTreeSet<MouseButton>,
}

impl<S: InputSink> InputController<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
        }
    }

    /// Aplica uma mensagem de entrada. Devolve `false` se não era de entrada.
    pub fn handle(&mut self, msg: &ControlMessage) -> bool {
        match *msg {
            ControlMessage::MouseMove { x, y } => self.sink.move_to(x, y),
            ControlMessage::MouseButton { button, down, x, y } => {
                // Botões vêm pelo canal confiável e podem chegar antes do
                // último movimento (canal não confiável): posiciona antes.
                self.sink.move_to(x, y);
                let changed = if down {
                    self.buttons.insert(button)
                } else {
                    self.buttons.remove(&button)
                };
                if changed {
                    self.sink.button(button, down);
                }
            }
            ControlMessage::Wheel { dx, dy } => {
                let (dx, dy) = (
                    dx.clamp(-MAX_WHEEL, MAX_WHEEL),
                    dy.clamp(-MAX_WHEEL, MAX_WHEEL),
                );
                if dx != 0 || dy != 0 {
                    self.sink.wheel(dx, dy);
                }
            }
            ControlMessage::Key { code, ext, down } => {
                if code == 0 || code > MAX_SCANCODE {
                    return true;
                }
                if down {
                    // Repetição automática chega como novos "down": repassa.
                    self.keys.insert((code, ext));
                    self.sink.key(code, ext, true);
                } else if self.keys.remove(&(code, ext)) {
                    self.sink.key(code, ext, false);
                }
            }
            ControlMessage::ReleaseAll => self.release_all(),
            ControlMessage::CtrlAltDel => {
                // As teclas do atalho não podem ficar presas do lado de cá.
                self.release_all();
                self.sink.secure_attention();
            }
            _ => return false,
        }
        true
    }

    /// Solta tudo que o viewer deixou pressionado.
    pub fn release_all(&mut self) {
        for (code, ext) in std::mem::take(&mut self.keys).into_iter().rev() {
            self.sink.key(code, ext, false);
        }
        for button in std::mem::take(&mut self.buttons) {
            self.sink.button(button, false);
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }
}

impl<S: InputSink> Drop for InputController<S> {
    fn drop(&mut self) {
        self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl Log {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

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
    }

    fn key(code: u16, ext: bool, down: bool) -> ControlMessage {
        ControlMessage::Key { code, ext, down }
    }

    #[test]
    fn botao_posiciona_antes_e_ignora_duplicado() {
        let log = Log::default();
        let mut c = InputController::new(log.clone());
        let down = ControlMessage::MouseButton {
            button: MouseButton::Left,
            down: true,
            x: 10,
            y: 20,
        };
        c.handle(&down);
        c.handle(&down);
        assert_eq!(log.take(), vec!["move 10,20", "Left true", "move 10,20"]);
    }

    #[test]
    fn solta_teclas_e_botoes_pendentes_ao_encerrar() {
        let log = Log::default();
        {
            let mut c = InputController::new(log.clone());
            c.handle(&key(0x1D, false, true)); // Ctrl
            c.handle(&key(0x1E, false, true)); // A
            c.handle(&key(0x1E, false, true)); // repetição
            c.handle(&key(0x1E, false, false));
            c.handle(&ControlMessage::MouseButton {
                button: MouseButton::Right,
                down: true,
                x: 0,
                y: 0,
            });
            log.take();
        } // drop
        assert_eq!(log.take(), vec!["key 0x1d false false", "Right false"]);
    }

    #[test]
    fn release_all_explicito_e_up_sem_down_ignorado() {
        let log = Log::default();
        let mut c = InputController::new(log.clone());
        c.handle(&key(0x4B, true, false)); // up sem down
        c.handle(&key(0x4B, true, true));
        c.handle(&key(0x38, false, true));
        c.handle(&ControlMessage::ReleaseAll);
        assert_eq!(
            log.take(),
            vec![
                "key 0x4b true true",
                "key 0x38 false true",
                "key 0x4b true false",
                "key 0x38 false false"
            ]
        );
        c.handle(&ControlMessage::ReleaseAll);
        assert!(log.take().is_empty());
    }

    #[test]
    fn valida_scancode_e_limita_roda() {
        let log = Log::default();
        let mut c = InputController::new(log.clone());
        c.handle(&key(0, false, true));
        c.handle(&key(0x1FF, false, true));
        c.handle(&ControlMessage::Wheel { dx: 0, dy: 0 });
        c.handle(&ControlMessage::Wheel {
            dx: 0,
            dy: -1_000_000,
        });
        assert_eq!(log.take(), vec!["wheel 0,-2400"]);
        assert!(!c.handle(&ControlMessage::Keyframe));
    }
}
