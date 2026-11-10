//! Encoder H.264 com Media Foundation.
//!
//! Tenta primeiro um encoder de hardware (NVENC, Quick Sync ou AMF, expostos
//! como MFT assíncrono) e cai para o MFT de software da Microsoft se o
//! hardware não existir ou falhar. Só existe no Windows.

#[cfg(windows)]
mod mf;

#[cfg(windows)]
pub use mf::{EncoderPreference, MfEncoder, auto_encoder};
