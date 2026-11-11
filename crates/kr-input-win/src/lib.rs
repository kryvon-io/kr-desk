//! Entrada remota no Windows.
//!
//! - [`SendInputSink`] (host): aplica mouse e teclado com `SendInput`. O
//!   mouse usa coordenadas absolutas da área de trabalho virtual, mapeadas
//!   para o monitor principal (o que é transmitido); o teclado usa scancodes,
//!   então o layout (ex.: ABNT2) é o do host.
//! - [`KeyboardHook`] (viewer): hook `WH_KEYBOARD_LL` que, com a janela da
//!   sessão em primeiro plano, captura todas as teclas — inclusive Win,
//!   Alt+Tab e Alt+F4 — e as entrega ao app em vez de ao Windows local.
//! - [`ClipboardSync`] (ambos): texto copiado de um lado aparece no outro.
//!
//! Rodando no app, janelas elevadas (administrador) ignoram a entrada por
//! causa do UIPI. Com o serviço do KR Desk, o agente SYSTEM (`kr-service-win`)
//! usa este mesmo [`SendInputSink`] sem essa limitação.

#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod send;

#[cfg(windows)]
pub use clipboard::{ClipboardSync, read_text, write_text};
#[cfg(windows)]
pub use hook::{HookKey, KeyboardHook};
#[cfg(windows)]
pub use send::SendInputSink;
