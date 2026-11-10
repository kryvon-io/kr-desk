//! Liga o pipeline de vídeo do `kr-core` (thread bloqueante) à trilha
//! WebRTC (tokio).

use std::sync::Arc;
use std::thread::JoinHandle;

use bytes::Bytes;
use kr_core::media::{
    EncodedFrame, EncoderFactory, PipelineConfig, PipelineControl, PipelineError, PipelineStats,
    SourceFactory, run_pipeline,
};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::VideoSender;

/// Quadros aguardando envio. Pouco, de propósito: atraso acumulado é pior
/// que um quadro descartado.
const QUEUE: usize = 3;

/// Transmissão de vídeo em andamento. Para ao chamar [`VideoStream::stop`] ou
/// ao ser descartada.
pub struct VideoStream {
    control: Arc<PipelineControl>,
    thread: Option<JoinHandle<Result<PipelineStats, PipelineError>>>,
}

impl VideoStream {
    /// Inicia captura e codificação numa thread própria (onde `source` e
    /// `encoder` são criados) e o envio numa task do runtime tokio atual.
    pub fn start(
        video: VideoSender,
        source: SourceFactory,
        encoder: EncoderFactory,
        config: PipelineConfig,
    ) -> std::io::Result<Self> {
        let control = PipelineControl::new();
        let (tx, mut rx) = mpsc::channel::<EncodedFrame>(QUEUE);

        let writer_control = control.clone();
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let data = Bytes::from(frame.data);
                if let Err(err) = video.write(data, frame.duration, frame.captured_at).await {
                    warn!(%err, "falha ao enviar quadro; encerrando o vídeo");
                    writer_control.stop();
                    break;
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
                let result = run_pipeline(source, encoder, config, pipeline_control, |frame| {
                    match tx.try_send(frame) {
                        Ok(()) => true,
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            // Descartar um quadro quebra a referência dos próximos:
                            // o seguinte precisa ser um keyframe.
                            debug!("fila de envio cheia; quadro descartado");
                            keyframes.request_keyframe();
                            true
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => false,
                    }
                });
                if let Err(err) = &result {
                    error!(%err, "pipeline de vídeo parou");
                }
                result
            })?;

        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    pub fn request_keyframe(&self) {
        self.control.request_keyframe();
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
