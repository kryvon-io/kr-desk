//! Transmissão da tela deste computador (lado host).
//!
//! No Windows: DXGI Desktop Duplication + H.264 do Media Foundation
//! (hardware, com reserva em software). Em outras plataformas o app compila
//! (para desenvolvimento), mas não transmite vídeo.

use kr_transport::{VideoSender, VideoStream};

/// Variável de ambiente para forçar o encoder de software (diagnóstico):
/// `KR_DESK_ENCODER=software`.
pub const ENV_ENCODER: &str = "KR_DESK_ENCODER";

/// Começa a transmitir a tela pela trilha de vídeo.
#[cfg(windows)]
pub fn start(video: VideoSender) -> Result<VideoStream, String> {
    use kr_capture_win::DxgiCapture;
    use kr_codec_win::{EncoderPreference, auto_encoder};
    use kr_core::media::{FrameSource, PipelineConfig};

    let preference = match std::env::var(ENV_ENCODER).as_deref() {
        Ok("software") => EncoderPreference::Software,
        _ => EncoderPreference::Auto,
    };
    VideoStream::start(
        video,
        Box::new(|| DxgiCapture::primary().map(|c| Box::new(c) as Box<dyn FrameSource>)),
        auto_encoder(preference),
        PipelineConfig::default(),
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn start(_video: VideoSender) -> Result<VideoStream, String> {
    let _ = ENV_ENCODER;
    Err("transmissão de tela só existe no Windows".into())
}
