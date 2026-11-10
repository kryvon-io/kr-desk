//! Entrada remota no Windows.
//!
//! - [`SendInputSink`] (host): aplica mouse e teclado com `SendInput`. O
//!   mouse usa coordenadas absolutas da área de trabalho virtual, mapeadas
//!   para o monitor principal (o que é transmitido); o teclado usa scancodes,
//!   então o layout (ex.: ABNT2) é o do host.
//! - [`KeyboardHook`] (viewer): hook `WH_KEYBOARD_LL` que, com a janela da
//!   sessão em primeiro plano, captura todas as teclas — inclusive Win,
//!   Alt+Tab e Alt+F4 — e as entrega ao app em vez de ao Windows local.
//!
//! Limitações conhecidas: janelas elevadas (administrador) ignoram a entrada
//! por causa do UIPI, e Ctrl+Alt+Del não pode ser enviado; ambos ficam para o
//! serviço da Etapa 8.

#[cfg(windows)]
mod hook;
#[cfg(windows)]
mod send;

#[cfg(windows)]
pub use hook::{HookKey, KeyboardHook};
#[cfg(windows)]
pub use send::SendInputSink;
