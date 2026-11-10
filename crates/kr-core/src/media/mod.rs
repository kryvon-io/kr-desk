//! Vídeo do host: tipos de quadro, traits de captura e codificação e o loop
//! que liga as duas pontas.
//!
//! As implementações reais ficam nos crates `kr-capture-win` (DXGI) e
//! `kr-codec-win` (Media Foundation). Aqui tudo é independente de plataforma,
//! para que o pipeline seja testado em Linux com [`pattern::TestPattern`] e um
//! encoder de software.

pub mod convert;
pub mod pattern;
pub mod pipeline;

use std::time::Duration;

pub use convert::{bgra_to_nv12, fit_within};
pub use pipeline::{
    EncodedFrame, EncoderFactory, PipelineConfig, PipelineControl, PipelineError, PipelineStats,
    SourceFactory, run_pipeline,
};

/// Quadro capturado em BGRA 8 bits (formato nativo da Desktop Duplication).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgraFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes por linha (pode ser maior que `width * 4`).
    pub stride: usize,
    pub data: Vec<u8>,
}

impl BgraFrame {
    pub fn new(width: u32, height: u32) -> Self {
        let stride = width as usize * 4;
        Self {
            width,
            height,
            stride,
            data: vec![0; stride * height as usize],
        }
    }

    /// Linha `y` (somente os pixels, sem o preenchimento do stride).
    pub fn row(&self, y: u32) -> &[u8] {
        let start = y as usize * self.stride;
        &self.data[start..start + self.width as usize * 4]
    }
}

/// Quadro NV12 (entrada dos encoders H.264): plano Y seguido do plano UV
/// intercalado, ambos sem preenchimento. Largura e altura são pares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Nv12Frame {
    pub fn new(width: u32, height: u32) -> Self {
        assert!(
            width.is_multiple_of(2) && height.is_multiple_of(2),
            "NV12 exige dimensões pares"
        );
        Self {
            width,
            height,
            data: vec![0; Self::byte_len(width, height)],
        }
    }

    pub fn byte_len(width: u32, height: u32) -> usize {
        width as usize * height as usize * 3 / 2
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.data[..self.width as usize * self.height as usize]
    }

    pub fn uv_plane(&self) -> &[u8] {
        &self.data[self.width as usize * self.height as usize..]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// A captura caiu mas pode voltar (troca de resolução, UAC, bloqueio de
    /// tela). O pipeline tenta de novo após uma pausa.
    #[error("captura indisponível no momento: {0}")]
    Transient(String),
    #[error("falha na captura: {0}")]
    Fatal(String),
}

/// Mudança no cursor do host, enviada ao viewer pelo DataChannel (o viewer
/// desenha o cursor localmente, sem esperar o vídeo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorUpdate {
    /// Nova forma em RGBA (não pré-multiplicado).
    Shape {
        width: u32,
        height: u32,
        hot_x: u32,
        hot_y: u32,
        rgba: Vec<u8>,
    },
    Visible(bool),
}

/// Fonte de quadros da tela. Não exige `Send`: implementações com COM/DXGI
/// são criadas e usadas na própria thread do pipeline (ver [`SourceFactory`]).
pub trait FrameSource {
    /// Espera até `timeout` por um quadro novo. `Ok(None)` significa que a
    /// tela não mudou nesse intervalo.
    fn capture(&mut self, timeout: Duration) -> Result<Option<&BgraFrame>, CaptureError>;

    /// Nome para logs (ex.: "DXGI Desktop Duplication").
    fn name(&self) -> String;

    /// Próxima mudança do cursor observada desde a última chamada.
    fn take_cursor(&mut self) -> Option<CursorUpdate> {
        None
    }
}

#[derive(Debug, thiserror::Error)]
#[error("falha na codificação: {0}")]
pub struct EncodeError(pub String);

/// Parâmetros de um encoder H.264.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

/// Encoder H.264 que produz Annex-B (start codes `00 00 00 01`), com SPS/PPS
/// antes de cada keyframe. Criado na thread do pipeline por uma
/// [`EncoderFactory`].
pub trait VideoEncoder {
    fn encode(&mut self, frame: &Nv12Frame, keyframe: bool) -> Result<Vec<u8>, EncodeError>;

    /// Nome para logs (ex.: "NVIDIA H.264 Encoder MFT").
    fn name(&self) -> String;
}

/// Tipo de cada unidade NAL de um fluxo Annex-B (útil em testes e logs).
pub fn annexb_nal_types(data: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(&header) = data.get(i + 3) {
                types.push(header & 0x1f);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    types
}

/// O fluxo contém um quadro IDR (NAL tipo 5)?
pub fn is_keyframe(data: &[u8]) -> bool {
    annexb_nal_types(data).contains(&5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nal_types_de_annexb() {
        let stream = [
            0, 0, 0, 1, 0x67, 1, 2, // SPS
            0, 0, 0, 1, 0x68, 3, // PPS
            0, 0, 1, 0x65, 9, 9, // IDR (start code de 3 bytes)
        ];
        assert_eq!(annexb_nal_types(&stream), vec![7, 8, 5]);
        assert!(is_keyframe(&stream));
        assert!(!is_keyframe(&[0, 0, 0, 1, 0x41, 7]));
    }

    #[test]
    fn nv12_tamanho_e_planos() {
        let f = Nv12Frame::new(4, 2);
        assert_eq!(f.data.len(), 12);
        assert_eq!(f.y_plane().len(), 8);
        assert_eq!(f.uv_plane().len(), 4);
    }
}
