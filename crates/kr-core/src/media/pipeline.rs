//! Loop de vídeo do host: captura → NV12 → H.264 → `sink`.
//!
//! Roda numa thread dedicada (captura e codificação bloqueiam). O `sink`
//! entrega cada quadro codificado ao transporte; se devolver `false`, o loop
//! termina.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::{
    CaptureError, CursorUpdate, EncodeError, EncoderConfig, FrameSource, Nv12Frame, VideoEncoder,
    bgra_to_nv12, fit_within, is_keyframe,
};

/// Abre a fonte de captura (chamada na thread do pipeline).
pub type SourceFactory = Box<dyn FnOnce() -> Result<Box<dyn FrameSource>, CaptureError> + Send>;

/// Cria um encoder para a resolução pedida (chamado de novo se a tela mudar
/// de tamanho).
pub type EncoderFactory =
    Box<dyn FnMut(EncoderConfig) -> Result<Box<dyn VideoEncoder>, EncodeError> + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineConfig {
    /// Resolução máxima enviada; telas maiores são reduzidas.
    pub max_width: u32,
    pub max_height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    /// Keyframe periódico, para recuperar perdas sem depender de PLI.
    pub keyframe_interval: Duration,
    /// Com a tela parada, reenvia o último quadro neste intervalo.
    pub idle_refresh: Duration,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_width: 1920,
            max_height: 1080,
            fps: 30,
            bitrate_bps: 5_000_000,
            keyframe_interval: Duration::from_secs(10),
            idle_refresh: Duration::from_secs(1),
        }
    }
}

/// Sinais externos para o loop (thread-safe).
#[derive(Debug, Default)]
pub struct PipelineControl {
    keyframe: AtomicBool,
    stop: AtomicBool,
}

impl PipelineControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Pede um keyframe no próximo quadro (PLI/FIR do viewer).
    pub fn request_keyframe(&self) {
        self.keyframe.store(true, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn take_keyframe(&self) -> bool {
        self.keyframe.swap(false, Ordering::Relaxed)
    }
}

/// Quadro H.264 pronto para o transporte.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// Annex-B.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Tempo desde o quadro anterior (define o timestamp RTP).
    pub duration: Duration,
    pub captured_at: Instant,
    pub width: u32,
    pub height: u32,
}

/// Contadores acumulados, para logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PipelineStats {
    pub frames: u64,
    pub keyframes: u64,
    pub bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Encode(#[from] EncodeError),
}

/// Tempo máximo de espera por quadro, para checar `stop` com frequência.
const POLL: Duration = Duration::from_millis(100);
/// Pausa após uma falha temporária de captura.
const TRANSIENT_BACKOFF: Duration = Duration::from_millis(250);
/// Intervalo dos logs de estatística.
const STATS_EVERY: Duration = Duration::from_secs(10);

struct Encoding {
    encoder: Box<dyn VideoEncoder>,
    frame: Nv12Frame,
}

/// Executa o loop até `control.stop()`, o `sink` recusar um quadro ou uma
/// falha definitiva. Devolve as estatísticas acumuladas.
pub fn run_pipeline(
    mut source: Box<dyn FrameSource>,
    mut make_encoder: EncoderFactory,
    config: PipelineConfig,
    control: Arc<PipelineControl>,
    mut sink: impl FnMut(EncodedFrame) -> bool,
    mut on_cursor: impl FnMut(CursorUpdate),
) -> Result<PipelineStats, PipelineError> {
    let frame_interval = Duration::from_secs(1) / config.fps.max(1);
    let mut stats = PipelineStats::default();
    let mut window = (Instant::now(), PipelineStats::default());
    let mut state: Option<Encoding> = None;
    let mut last_sent: Option<Instant> = None;
    let mut last_key: Option<Instant> = None;
    let mut next_capture = Instant::now();
    let mut transient_logged = false;
    info!(source = %source.name(), ?config, "pipeline de vídeo iniciado");

    while !control.is_stopped() {
        // Limita a taxa: telas de 144 Hz não precisam de 144 quadros por segundo.
        let now = Instant::now();
        if next_capture > now {
            std::thread::sleep(next_capture - now);
        }
        let iteration = Instant::now();
        // Mudanças do cursor vistas na captura anterior.
        while let Some(update) = source.take_cursor() {
            on_cursor(update);
        }

        let captured = match source.capture(POLL) {
            Ok(frame) => {
                transient_logged = false;
                frame
            }
            Err(CaptureError::Transient(reason)) => {
                if !transient_logged {
                    warn!(%reason, "captura interrompida; tentando de novo");
                    transient_logged = true;
                }
                std::thread::sleep(TRANSIENT_BACKOFF);
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        let mut keyframe = control.take_keyframe()
            || last_key.is_none_or(|t| t.elapsed() >= config.keyframe_interval);

        match captured {
            Some(bgra) => {
                let (w, h) =
                    fit_within(bgra.width, bgra.height, config.max_width, config.max_height);
                if state
                    .as_ref()
                    .is_none_or(|s| s.frame.width != w || s.frame.height != h)
                {
                    let encoder = make_encoder(EncoderConfig {
                        width: w,
                        height: h,
                        fps: config.fps,
                        bitrate_bps: config.bitrate_bps,
                    })?;
                    info!(
                        encoder = %encoder.name(),
                        src_w = bgra.width,
                        src_h = bgra.height,
                        w,
                        h,
                        "encoder configurado"
                    );
                    state = Some(Encoding {
                        encoder,
                        frame: Nv12Frame::new(w, h),
                    });
                    keyframe = true;
                }
                let s = state.as_mut().expect("estado criado acima");
                bgra_to_nv12(bgra, &mut s.frame);
            }
            None => {
                // Tela parada: só reenvia se pediram keyframe ou passou o intervalo.
                let idle = last_sent.is_none_or(|t| t.elapsed() >= config.idle_refresh);
                if state.is_none() || !(keyframe || idle) {
                    if keyframe {
                        control.request_keyframe(); // guarda o pedido
                    }
                    continue;
                }
            }
        }

        let s = state.as_mut().expect("há quadro para codificar");
        let data = s.encoder.encode(&s.frame, keyframe)?;
        next_capture = iteration + frame_interval;
        if data.is_empty() {
            continue; // encoder ainda sem saída (latência interna)
        }
        let now = Instant::now();
        let is_key = is_keyframe(&data);
        if is_key {
            last_key = Some(now);
            stats.keyframes += 1;
        }
        stats.frames += 1;
        stats.bytes += data.len() as u64;
        let frame = EncodedFrame {
            duration: last_sent.map_or(frame_interval, |t| now - t),
            data,
            keyframe: is_key,
            captured_at: iteration,
            width: s.frame.width,
            height: s.frame.height,
        };
        last_sent = Some(now);
        if !sink(frame) {
            break;
        }

        if window.0.elapsed() >= STATS_EVERY {
            let secs = window.0.elapsed().as_secs_f64();
            info!(
                fps = format!("{:.1}", (stats.frames - window.1.frames) as f64 / secs),
                kbps = ((stats.bytes - window.1.bytes) as f64 * 8.0 / secs / 1000.0) as u64,
                keyframes = stats.keyframes - window.1.keyframes,
                "vídeo"
            );
            window = (Instant::now(), stats);
        }
    }
    info!(?stats, "pipeline de vídeo encerrado");
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::BgraFrame;
    use std::sync::Mutex;

    /// Fonte roteirizada: `Some(tamanho)` gera um quadro, `None` = sem mudança.
    struct Scripted {
        steps: Vec<Option<(u32, u32)>>,
        frame: BgraFrame,
    }

    impl FrameSource for Scripted {
        fn capture(&mut self, _timeout: Duration) -> Result<Option<&BgraFrame>, CaptureError> {
            if self.steps.is_empty() {
                return Err(CaptureError::Fatal("fim do roteiro".into()));
            }
            match self.steps.remove(0) {
                Some((w, h)) => {
                    self.frame = BgraFrame::new(w, h);
                    Ok(Some(&self.frame))
                }
                None => {
                    std::thread::sleep(Duration::from_millis(5));
                    Ok(None)
                }
            }
        }
        fn name(&self) -> String {
            "roteiro".into()
        }
    }

    /// Encoder falso: IDR quando pedido, P caso contrário.
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl VideoEncoder for Fake {
        fn encode(&mut self, f: &Nv12Frame, keyframe: bool) -> Result<Vec<u8>, EncodeError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("{}x{} {}", f.width, f.height, keyframe));
            Ok(if keyframe {
                vec![0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x65]
            } else {
                vec![0, 0, 0, 1, 0x41]
            })
        }
        fn name(&self) -> String {
            "falso".into()
        }
    }

    fn run(steps: Vec<Option<(u32, u32)>>, config: PipelineConfig) -> (Vec<String>, Vec<bool>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let created = log.clone();
        let factory: EncoderFactory = Box::new(move |cfg| {
            created
                .lock()
                .unwrap()
                .push(format!("novo {}x{}", cfg.width, cfg.height));
            Ok(Box::new(Fake {
                log: created.clone(),
            }))
        });
        let source = Box::new(Scripted {
            steps,
            frame: BgraFrame::new(2, 2),
        });
        let mut keys = Vec::new();
        let result = run_pipeline(
            source,
            factory,
            config,
            PipelineControl::new(),
            |f| {
                keys.push(f.keyframe);
                true
            },
            |_| {},
        );
        assert!(matches!(
            result,
            Err(PipelineError::Capture(CaptureError::Fatal(_)))
        ));
        let log = log.lock().unwrap().clone();
        (log, keys)
    }

    fn fast() -> PipelineConfig {
        PipelineConfig {
            fps: 1000,
            max_width: 64,
            max_height: 64,
            idle_refresh: Duration::from_secs(60),
            keyframe_interval: Duration::from_secs(60),
            ..PipelineConfig::default()
        }
    }

    #[test]
    fn primeiro_quadro_e_keyframe_e_tela_parada_nao_envia() {
        let (log, keys) = run(vec![Some((32, 16)), None, None, Some((32, 16))], fast());
        assert_eq!(log, vec!["novo 32x16", "32x16 true", "32x16 false"]);
        assert_eq!(keys, vec![true, false]);
    }

    #[test]
    fn mudanca_de_resolucao_recria_encoder_e_reduz() {
        let (log, keys) = run(vec![Some((32, 16)), Some((128, 64))], fast());
        assert_eq!(
            log,
            vec!["novo 32x16", "32x16 true", "novo 64x32", "64x32 true"]
        );
        assert_eq!(keys, vec![true, true]);
    }

    #[test]
    fn tela_parada_reenvia_apos_idle_refresh() {
        let config = PipelineConfig {
            idle_refresh: Duration::ZERO,
            ..fast()
        };
        let (_, keys) = run(vec![Some((8, 8)), None, None], config);
        assert_eq!(keys, vec![true, false, false]);
    }

    #[test]
    fn pedido_de_keyframe_com_tela_parada() {
        let control = PipelineControl::new();
        let factory: EncoderFactory = Box::new(|_| {
            Ok(Box::new(Fake {
                log: Arc::default(),
            }))
        });
        let source = Box::new(Scripted {
            steps: vec![Some((8, 8)), None, None, None],
            frame: BgraFrame::new(2, 2),
        });
        let mut keys = Vec::new();
        let c = control.clone();
        let _ = run_pipeline(
            source,
            factory,
            fast(),
            control,
            |f| {
                keys.push(f.keyframe);
                if keys.len() == 1 {
                    c.request_keyframe();
                }
                true
            },
            |_| {},
        );
        assert_eq!(keys, vec![true, true]);
    }

    #[test]
    fn sink_recusando_encerra() {
        let factory: EncoderFactory = Box::new(|_| {
            Ok(Box::new(Fake {
                log: Arc::default(),
            }))
        });
        let source = Box::new(crate::media::pattern::TestPattern::new(16, 16, 1000));
        let mut n = 0;
        let stats = run_pipeline(
            source,
            factory,
            fast(),
            PipelineControl::new(),
            |_| {
                n += 1;
                n < 3
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(stats.frames, 3);
        assert_eq!(stats.keyframes, 1);
    }
}
