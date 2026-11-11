//! Captura de tela no Windows (qualquer monitor ligado à área de trabalho; o
//! principal por padrão):
//!
//! - [`DxgiCapture`]: DXGI Desktop Duplication — o caminho padrão, com a forma
//!   do cursor separada.
//! - [`WgcCapture`]: Windows.Graphics.Capture — reserva para onde a DXGI
//!   falha (VMs, GPUs híbridas).
//!
//! Só existe no Windows; em outras plataformas o crate fica vazio para o
//! workspace continuar compilando.

#[cfg(windows)]
mod dxgi;

#[cfg(windows)]
mod wgc;

#[cfg(windows)]
pub use dxgi::DxgiCapture;
#[cfg(windows)]
pub use wgc::WgcCapture;
