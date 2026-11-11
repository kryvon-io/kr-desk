//! Liga o pipeline de vídeo do `kr-core` (thread bloqueante) à trilha
//! WebRTC (tokio).

use std::sync::Arc;
use std::thread::JoinHandle;

use base64::Engine;
use bytes::Bytes;
use kr_core::media::{
    CursorUpdate, EncodedFrame, EncoderFactory, PipelineConfig, PipelineControl, PipelineError,
    PipelineStats, SharedArea, SourceEvent, SourceFactory, run_pipeline,
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use kr_proto::control::{ControlMessage, CursorImage, MonitorDto};

use crate::{ControlSender, VideoSender};

fn cursor_message(update: CursorUpdate) -> ControlMessage {
    match update {
        CursorUpdate::Visible(visible) => ControlMessage::CursorVisible { visible },
        CursorUpdate::Shape {
            width,
            height,
            hot_x,
            hot_y,
            rgba,
        } => ControlMessage::CursorShape(CursorImage {
            w: width,
            h: height,
            hx: hot_x,
            hy: hot_y,
            rgba: base64::engine::general_purpose::STANDARD.encode(rgba),
        }),
    }
}

/// Traduz um evento da captura; atualiza a área ativa ao trocar de monitor.
fn event_message(event: SourceEvent, area: &SharedArea) -> ControlMessage {
    match event {
        SourceEvent::Cursor(update) => cursor_message(update),
        SourceEvent::Monitors { list, active } => {
            let rect = list.iter().find(|m| m.index == active).map(|m| m.rect);
            *area.lock().expect("mutex") = rect;
            ControlMessage::Monitors {
                list: list
                    .into_iter()
                    .map(|m| MonitorDto {
                        index: m.index,
                        name: m.name,
                        width: m.rect.width,
                        height: m.rect.height,
                        primary: m.primary,
                    })
                    .collect(),
                active,
            }
        }
    }
}

/// Quadros aguardando envio. Pouco, de propósito: atraso acumulado é pior
/// que um quadro descartado.
const QUEUE: usize = 3;

/// Transmissão de vídeo em andamento. Para ao chamar [`VideoStream::stop`] ou
/// ao ser descartada.
pub struct VideoStream {
    control: Arc<PipelineControl>,
    thread: Option<JoinHandle<Result<PipelineStats, PipelineError>>>,
    area: SharedArea,
}

impl VideoStream {
    /// Inicia captura e codificação numa thread própria (onde `source` e
    /// `encoder` são criados) e o envio numa task do runtime tokio atual.
    /// Cursor e lista de monitores vão para `control_channel`, se informado.
    pub fn start(
        video: VideoSender,
        source: SourceFactory,
        encoder: EncoderFactory,
        config: PipelineConfig,
        control_channel: Option<ControlSender>,
    ) -> std::io::Result<Self> {
        let control = PipelineControl::new();
        let area = SharedArea::default();
        let (tx, mut rx) = mpsc::channel::<EncodedFrame>(QUEUE);
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SourceEvent>();
        let event_area = area.clone();
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let msg = event_message(event, &event_area);
                if let Some(sender) = &control_channel {
                    sender.send(&msg).await;
                }
            }
        });

        let writer_control = control.clone();
        tokio::spawn(async move {
            // Durante uma queda de rede (antes do reinício de ICE) o envio pode
            // falhar: seguimos tentando e, quando voltar, pedimos um keyframe.
            let mut failing = 0u32;
            while let Some(frame) = rx.recv().await {
                let data = Bytes::from(frame.data);
                match video.write(data, frame.duration, frame.captured_at).await {
                    Ok(()) => {
                        if failing > 0 {
                            info!(failing, "envio de vídeo voltou");
                            writer_control.request_keyframe();
                            failing = 0;
                        }
                    }
                    Err(err) => {
                        if failing == 0 {
                            warn!(%err, "falha ao enviar quadro; tentando de novo");
                        }
                        failing += 1;
                    }
                }
            }
        });

        let pipeline_control = control.clone();
        let thread = std::thread::Builder::new()
            .name("kr-video".into())
            .spawn(move || {
                let keyframes = pipeline_control.clone();
                let source = match source() {
                    Ok(s) => s,
                    Err(err) => {
                        error!(%err, "não foi possível abrir a captura");
                        pipeline_control.stop();
                        return Err(err.into());
                    }
                };
                let result = run_pipeline(
                    source,
                    encoder,
                    config,
                    pipeline_control,
                    |frame| match tx.try_send(frame) {
                        Ok(()) => true,
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            // Descartar um quadro quebra a referência dos próximos:
                            // o seguinte precisa ser um keyframe.
                            debug!("fila de envio cheia; quadro descartado");
                            keyframes.request_keyframe();
                            true
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => false,
                    },
                    |event| {
                        let _ = event_tx.send(event);
                    },
                );
                if let Err(err) = &result {
                    error!(%err, "pipeline de vídeo parou");
                }
                result
            })?;

        Ok(Self {
            control,
            thread: Some(thread),
            area,
        })
    }

    pub fn request_keyframe(&self) {
        self.control.request_keyframe();
    }

    /// Ajusta taxa e resolução máxima (controle adaptativo).
    pub fn set_quality(&self, target: kr_core::media::quality::QualityTarget) {
        self.control.request_quality(target);
    }

    /// Passa a transmitir outro monitor.
    pub fn select_monitor(&self, index: u32) {
        self.control.request_monitor(index);
    }

    /// Área da tela transmitida (para mirar a entrada remota).
    pub fn area(&self) -> SharedArea {
        self.area.clone()
    }

    /// O pipeline terminou (erro de captura/codificação ou envio)?
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Para o pipeline sem esperar a thread terminar.
    pub fn stop(&mut self) {
        self.control.stop();
        self.thread.take();
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.stop();
    }
}
