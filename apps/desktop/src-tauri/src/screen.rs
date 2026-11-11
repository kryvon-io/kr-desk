//! Transmissão da tela deste computador (lado host).
//!
//! No Windows: a captura vem do agente do serviço (SYSTEM, vê UAC e tela de
//! bloqueio) quando ele está instalado; sem ele, DXGI Desktop Duplication
//! (ou Windows.Graphics.Capture, se a DXGI não abrir) neste processo. Depois,
//! H.264 do Media Foundation (hardware, com reserva em software). Em outras plataformas o app compila
//! (para desenvolvimento), mas não transmite vídeo.

use kr_transport::{ControlSender, VideoSender, VideoStream};

/// Variável de ambiente para forçar o encoder de software (diagnóstico):
/// `KR_DESK_ENCODER=software`.
pub const ENV_ENCODER: &str = "KR_DESK_ENCODER";

/// Variável de ambiente para escolher a captura (diagnóstico):
/// `KR_DESK_CAPTURE=wgc` (WGC neste processo) ou `local` (sem o serviço).
pub const ENV_CAPTURE: &str = "KR_DESK_CAPTURE";

#[cfg(windows)]
#[derive(Clone, Copy)]
enum CapturePreference {
    /// Agente do serviço; sem ele, DXGI; se não abrir, WGC.
    Auto,
    /// DXGI (ou WGC) neste processo, sem o serviço.
    Local,
    Wgc,
}

/// Abre a captura (na thread do pipeline).
#[cfg(windows)]
fn open_capture(
    preference: CapturePreference,
) -> Result<Box<dyn kr_core::media::FrameSource>, kr_core::media::CaptureError> {
    use kr_capture_win::{DxgiCapture, WgcCapture};
    if let CapturePreference::Auto = preference {
        match kr_core::agent::AgentSource::connect(Box::new(kr_service_win::connect_agent)) {
            Ok(c) => {
                tracing::info!("captura pelo serviço do KR Desk (UAC e tela de bloqueio)");
                return Ok(Box::new(c));
            }
            Err(err) => tracing::info!(%err, "serviço indisponível; capturando neste processo"),
        }
    }
    if let CapturePreference::Auto | CapturePreference::Local = preference {
        match DxgiCapture::primary() {
            Ok(c) => return Ok(Box::new(c)),
            Err(err) => {
                tracing::warn!(%err, "DXGI indisponível; tentando Windows.Graphics.Capture")
            }
        }
    }
    WgcCapture::primary().map(|c| Box::new(c) as Box<dyn kr_core::media::FrameSource>)
}

/// Começa a transmitir a tela pela trilha de vídeo.
#[cfg(windows)]
pub fn start(video: VideoSender, cursor: ControlSender) -> Result<VideoStream, String> {
    use kr_codec_win::{EncoderPreference, auto_encoder};
    use kr_core::media::PipelineConfig;

    let capture = match std::env::var(ENV_CAPTURE).as_deref() {
        Ok("wgc") => CapturePreference::Wgc,
        Ok("local") => CapturePreference::Local,
        _ => CapturePreference::Auto,
    };
    let preference = match std::env::var(ENV_ENCODER).as_deref() {
        Ok("software") => EncoderPreference::Software,
        _ => EncoderPreference::Auto,
    };
    VideoStream::start(
        video,
        Box::new(move || open_capture(capture)),
        auto_encoder(preference),
        PipelineConfig::default(),
        Some(cursor),
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn start(_video: VideoSender, _cursor: ControlSender) -> Result<VideoStream, String> {
    let _ = (ENV_ENCODER, ENV_CAPTURE);
    Err("transmissão de tela só existe no Windows".into())
}
