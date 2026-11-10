//! Captura de tela com DXGI Desktop Duplication (qualquer monitor ligado à
//! área de trabalho; o principal por padrão).
//!
//! Só existe no Windows; em outras plataformas o crate fica vazio para o
//! workspace continuar compilando.

#[cfg(windows)]
mod dxgi;

#[cfg(windows)]
pub use dxgi::DxgiCapture;
