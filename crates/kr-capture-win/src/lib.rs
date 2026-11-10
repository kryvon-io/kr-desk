//! Captura da tela principal com DXGI Desktop Duplication.
//!
//! Só existe no Windows; em outras plataformas o crate fica vazio para o
//! workspace continuar compilando.

#[cfg(windows)]
mod dxgi;

#[cfg(windows)]
pub use dxgi::DxgiCapture;
