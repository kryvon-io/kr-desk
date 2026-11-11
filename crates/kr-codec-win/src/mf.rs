//! MFT H.264 (Media Foundation Transform), síncrono ou assíncrono.
//!
//! - **Assíncrono (hardware):** o MFT avisa por eventos quando quer entrada
//!   (`METransformNeedInput`) e quando há saída (`METransformHaveOutput`).
//!   Como o pipeline chama `encode` quadro a quadro, consultamos a fila de
//!   eventos sem bloquear, com prazo curto.
//! - **Síncrono (software):** `ProcessInput` seguido de `ProcessOutput` até
//!   `MF_E_TRANSFORM_NEED_MORE_INPUT`.
//!
//! Configuração comum: NV12 → H.264 baseline, CBR, sem B-frames,
//! `CODECAPI_AVLowLatencyMode` e keyframe forçado com
//! `CODECAPI_AVEncVideoForceKeyFrame`.

use std::mem::ManuallyDrop;
use std::sync::Once;
use std::time::{Duration, Instant};

use kr_core::media::{
    EncodeError, EncoderConfig, EncoderFactory, Nv12Frame, VideoEncoder, annexb_nal_types,
    is_keyframe,
};
use tracing::{info, warn};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate,
    IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, METransformHaveOutput,
    METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE, MF_E_NOTACCEPTING,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT,
    MF_EVENT_TYPE, MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFSTARTUP_FULL,
    MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT,
    MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_Base,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface, PWSTR};

/// Prazo para o MFT assíncrono pedir entrada.
const NEED_INPUT_TIMEOUT: Duration = Duration::from_secs(1);
/// Prazo para esperar a saída de um quadro (encoders com latência interna
/// devolvem o quadro na próxima chamada).
const OUTPUT_TIMEOUT: Duration = Duration::from_millis(100);
/// GOP longo: keyframes vêm sob demanda (pipeline/PLI).
const GOP_SECONDS: u32 = 10;

fn err(context: &str, e: windows::core::Error) -> EncodeError {
    EncodeError(format!("{context}: {e}"))
}

/// Inicializa COM (MTA) na thread atual e Media Foundation no processo.
fn init_media_foundation() -> Result<(), EncodeError> {
    static STARTUP: Once = Once::new();
    thread_local! {
        static COM: () = {
            // S_FALSE (já inicializado) e RPC_E_CHANGED_MODE são aceitáveis.
            // SAFETY: inicialização COM da thread atual.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        };
    }
    COM.with(|_| ());
    let mut result = Ok(());
    STARTUP.call_once(|| {
        // SAFETY: chamada única por processo; nunca desligamos o MF.
        if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
            result = Err(err("MFStartup", e));
        }
    });
    result
}

/// Lista os encoders H.264 (NV12 → H.264) da categoria pedida.
fn enumerate(flags: MFT_ENUM_FLAG) -> Result<Vec<IMFActivate>, EncodeError> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: ponteiros de saída válidos; o array é liberado com CoTaskMemFree
    // depois de movermos cada IMFActivate para fora dele.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input),
            Some(&output),
            &mut list,
            &mut count,
        )
        .map_err(|e| err("MFTEnumEx", e))?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let items = std::slice::from_raw_parts_mut(list, count as usize)
            .iter_mut()
            .filter_map(Option::take)
            .collect();
        CoTaskMemFree(Some(list as *const _));
        Ok(items)
    }
}

fn friendly_name(activate: &IMFActivate) -> String {
    let mut name = PWSTR::null();
    let mut len = 0u32;
    // SAFETY: a string é alocada pelo MF e liberada com CoTaskMemFree.
    unsafe {
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut len)
            .is_err()
        {
            return "encoder H.264 sem nome".into();
        }
        let text = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.0 as *const _));
        text
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn video_type(subtype: &GUID, cfg: &EncoderConfig) -> windows::core::Result<IMFMediaType> {
    // SAFETY: atributos de um IMFMediaType recém-criado.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(cfg.width, cfg.height))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pack(cfg.fps.max(1), 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        Ok(t)
    }
}

/// Qual encoder tentar primeiro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderPreference {
    /// Hardware se houver; software caso contrário.
    Auto,
    /// Só o MFT de software (útil para diagnosticar drivers).
    Software,
}

/// Encoder H.264 baseado num MFT.
pub struct MfEncoder {
    activate: IMFActivate,
    transform: IMFTransform,
    codec: Option<ICodecAPI>,
    /// `Some` para MFTs assíncronos (hardware).
    events: Option<IMFMediaEventGenerator>,
    name: String,
    hardware: bool,
    config: EncoderConfig,
    input_id: u32,
    output_id: u32,
    provides_samples: bool,
    output_size: u32,
    /// SPS/PPS do tipo de saída, para encoders que não os repetem no IDR.
    sequence_header: Vec<u8>,
    need_input: u32,
    pending: Vec<u8>,
    frame_index: i64,
}

impl MfEncoder {
    /// Cria o melhor encoder disponível conforme a preferência.
    pub fn new(config: EncoderConfig, preference: EncoderPreference) -> Result<Self, EncodeError> {
        init_media_foundation()?;
        let mut last_error = None;
        let mut categories = Vec::new();
        if preference == EncoderPreference::Auto {
            categories.push((
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                true,
            ));
        }
        categories.push((MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, false));

        for (flags, hardware) in categories {
            let candidates = match enumerate(flags) {
                Ok(c) => c,
                Err(e) => {
                    warn!(%e, hardware, "falha ao listar encoders");
                    continue;
                }
            };
            for activate in candidates {
                let name = friendly_name(&activate);
                match Self::open(activate, name.clone(), hardware, config) {
                    Ok(encoder) => {
                        info!(encoder = %name, hardware, ?config, "encoder H.264 pronto");
                        return Ok(encoder);
                    }
                    Err(e) => {
                        warn!(encoder = %name, %e, "encoder recusado");
                        last_error = Some(e);
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            EncodeError("nenhum encoder H.264 do Media Foundation disponível".into())
        }))
    }

    fn open(
        activate: IMFActivate,
        name: String,
        hardware: bool,
        config: EncoderConfig,
    ) -> Result<Self, EncodeError> {
        // SAFETY: chamadas COM no MFT recém-ativado, todas na thread atual.
        unsafe {
            let transform: IMFTransform = activate
                .ActivateObject()
                .map_err(|e| err("ActivateObject", e))?;
            if let Ok(attrs) = transform.GetAttributes() {
                if hardware {
                    attrs
                        .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .map_err(|e| err("MF_TRANSFORM_ASYNC_UNLOCK", e))?;
                }
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }

            let (mut input_id, mut output_id) = ([0u32], [0u32]);
            if transform
                .GetStreamIDs(&mut input_id, &mut output_id)
                .is_err()
            {
                // E_NOTIMPL: os IDs são 0..n.
                (input_id, output_id) = ([0], [0]);
            }

            let mut encoder = Self {
                codec: transform.cast::<ICodecAPI>().ok(),
                events: None,
                activate,
                transform,
                name,
                hardware,
                config,
                input_id: input_id[0],
                output_id: output_id[0],
                provides_samples: false,
                output_size: 0,
                sequence_header: Vec::new(),
                need_input: 0,
                pending: Vec::new(),
                frame_index: 0,
            };
            encoder.configure()?;
            if hardware {
                encoder.events = Some(
                    encoder
                        .transform
                        .cast::<IMFMediaEventGenerator>()
                        .map_err(|e| err("IMFMediaEventGenerator", e))?,
                );
            }
            for msg in [
                MFT_MESSAGE_COMMAND_FLUSH,
                MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
                MFT_MESSAGE_NOTIFY_START_OF_STREAM,
            ] {
                encoder
                    .transform
                    .ProcessMessage(msg, 0)
                    .map_err(|e| err("ProcessMessage", e))?;
            }
            Ok(encoder)
        }
    }

    fn set_codec(&self, api: &GUID, value: VARIANT, label: &str) -> bool {
        let Some(codec) = &self.codec else {
            return false;
        };
        // SAFETY: GUID e VARIANT válidos durante a chamada.
        match unsafe { codec.SetValue(api, &value) } {
            Ok(()) => true,
            Err(e) => {
                // Nem todo encoder aceita todas as opções; seguimos sem ela.
                warn!(encoder = %self.name, option = label, %e, "opção do encoder ignorada");
                false
            }
        }
    }

    /// Muda a taxa média durante a codificação (CBR).
    pub fn change_bitrate(&mut self, bps: u32) -> bool {
        let ok = self.set_codec(
            &CODECAPI_AVEncCommonMeanBitRate,
            VARIANT::from(bps),
            "MeanBitRate (dinâmico)",
        );
        if ok {
            self.config.bitrate_bps = bps;
        }
        ok
    }

    fn configure(&mut self) -> Result<(), EncodeError> {
        let cfg = self.config;
        self.set_codec(
            &CODECAPI_AVLowLatencyMode,
            VARIANT::from(true),
            "LowLatencyMode",
        );
        self.set_codec(
            &CODECAPI_AVEncCommonRateControlMode,
            VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32),
            "RateControlMode",
        );
        self.set_codec(
            &CODECAPI_AVEncCommonMeanBitRate,
            VARIANT::from(cfg.bitrate_bps),
            "MeanBitRate",
        );
        self.set_codec(
            &CODECAPI_AVEncMPVDefaultBPictureCount,
            VARIANT::from(0u32),
            "BPictureCount",
        );
        self.set_codec(
            &CODECAPI_AVEncMPVGOPSize,
            VARIANT::from(cfg.fps.max(1) * GOP_SECONDS),
            "GOPSize",
        );

        // SAFETY: tipos de mídia válidos; o encoder exige a saída antes da entrada.
        unsafe {
            let output = video_type(&MFVideoFormat_H264, &cfg).map_err(|e| err("tipo H.264", e))?;
            output
                .SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_bps)
                .map_err(|e| err("MF_MT_AVG_BITRATE", e))?;
            output
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
                .map_err(|e| err("MF_MT_MPEG2_PROFILE", e))?;
            self.transform
                .SetOutputType(self.output_id, &output, 0)
                .map_err(|e| err("SetOutputType", e))?;

            let input = video_type(&MFVideoFormat_NV12, &cfg).map_err(|e| err("tipo NV12", e))?;
            self.transform
                .SetInputType(self.input_id, &input, 0)
                .map_err(|e| err("SetInputType", e))?;
        }
        self.refresh_output_info()
    }

    fn refresh_output_info(&mut self) -> Result<(), EncodeError> {
        // SAFETY: consultas ao MFT configurado.
        unsafe {
            let info = self
                .transform
                .GetOutputStreamInfo(self.output_id)
                .map_err(|e| err("GetOutputStreamInfo", e))?;
            let provides =
                MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0;
            self.provides_samples = info.dwFlags & provides as u32 != 0;
            self.output_size = info
                .cbSize
                .max(self.config.width * self.config.height * 3 / 2);

            self.sequence_header.clear();
            if let Ok(current) = self.transform.GetOutputCurrentType(self.output_id)
                && let Ok(size) = current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)
                && size > 0
            {
                let mut blob = vec![0u8; size as usize];
                if current
                    .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)
                    .is_ok()
                {
                    self.sequence_header = blob;
                }
            }
        }
        Ok(())
    }

    /// Copia o quadro NV12 para uma amostra do MF.
    fn make_sample(&mut self, frame: &Nv12Frame) -> Result<IMFSample, EncodeError> {
        let len = frame.data.len() as u32;
        let fps = self.config.fps.max(1) as i64;
        // SAFETY: o buffer tem `len` bytes; travado só durante a cópia.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).map_err(|e| err("MFCreateMemoryBuffer", e))?;
            let mut ptr = std::ptr::null_mut();
            buffer
                .Lock(&mut ptr, None, None)
                .map_err(|e| err("Lock (entrada)", e))?;
            std::ptr::copy_nonoverlapping(frame.data.as_ptr(), ptr, frame.data.len());
            buffer.Unlock().map_err(|e| err("Unlock (entrada)", e))?;
            buffer
                .SetCurrentLength(len)
                .map_err(|e| err("SetCurrentLength", e))?;
            let sample = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
            sample.AddBuffer(&buffer).map_err(|e| err("AddBuffer", e))?;
            // Tempos em unidades de 100 ns.
            sample
                .SetSampleTime(self.frame_index * 10_000_000 / fps)
                .map_err(|e| err("SetSampleTime", e))?;
            sample
                .SetSampleDuration(10_000_000 / fps)
                .map_err(|e| err("SetSampleDuration", e))?;
            self.frame_index += 1;
            Ok(sample)
        }
    }

    /// Uma chamada a `ProcessOutput`. Devolve `false` quando o MFT precisa de
    /// mais entrada.
    fn process_output(&mut self) -> Result<bool, EncodeError> {
        // SAFETY: a estrutura de saída segue o contrato do MFT: se ele não
        // fornece amostras, passamos uma com espaço suficiente; os campos
        // ManuallyDrop são recolhidos logo após a chamada.
        unsafe {
            let sample = if self.provides_samples {
                None
            } else {
                let sample = MFCreateSample().map_err(|e| err("MFCreateSample (saída)", e))?;
                let buffer = MFCreateMemoryBuffer(self.output_size)
                    .map_err(|e| err("MFCreateMemoryBuffer (saída)", e))?;
                sample
                    .AddBuffer(&buffer)
                    .map_err(|e| err("AddBuffer (saída)", e))?;
                Some(sample)
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.output_id,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let result = self.transform.ProcessOutput(0, &mut buffers, &mut status);
            let sample = ManuallyDrop::take(&mut buffers[0].pSample);
            drop(ManuallyDrop::take(&mut buffers[0].pEvents));

            match result {
                Ok(()) => {
                    if let Some(sample) = sample {
                        self.append_sample(&sample)?;
                    }
                    Ok(true)
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(false),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // O encoder mudou o formato de saída: aceita o que ele propõe.
                    let proposed = self
                        .transform
                        .GetOutputAvailableType(self.output_id, 0)
                        .map_err(|e| err("GetOutputAvailableType", e))?;
                    self.transform
                        .SetOutputType(self.output_id, &proposed, 0)
                        .map_err(|e| err("SetOutputType (mudança)", e))?;
                    self.refresh_output_info()?;
                    Ok(true)
                }
                Err(e) => Err(err("ProcessOutput", e)),
            }
        }
    }

    fn append_sample(&mut self, sample: &IMFSample) -> Result<(), EncodeError> {
        // SAFETY: o buffer contíguo fica travado só durante a cópia.
        unsafe {
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| err("ConvertToContiguousBuffer", e))?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(|e| err("Lock (saída)", e))?;
            self.pending
                .extend_from_slice(std::slice::from_raw_parts(ptr, len as usize));
            buffer.Unlock().map_err(|e| err("Unlock (saída)", e))?;
        }
        Ok(())
    }

    /// Lê um evento do MFT assíncrono, se houver. Devolve `false` se a fila
    /// estava vazia.
    fn pump_event(&mut self) -> Result<bool, EncodeError> {
        let events = self.events.clone().expect("MFT assíncrono");
        // SAFETY: consulta não bloqueante da fila de eventos.
        let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(ev) => ev,
            Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(false),
            Err(e) => return Err(err("GetEvent", e)),
        };
        // SAFETY: evento válido.
        let kind = unsafe { event.GetType() }.map_err(|e| err("IMFMediaEvent::GetType", e))?;
        let kind = MF_EVENT_TYPE(kind as i32);
        if kind == METransformNeedInput {
            self.need_input += 1;
        } else if kind == METransformHaveOutput {
            // Uma mudança de formato pede uma segunda chamada.
            if self.process_output()? && self.pending.is_empty() {
                self.process_output()?;
            }
        }
        Ok(true)
    }

    fn encode_async(&mut self, sample: &IMFSample) -> Result<(), EncodeError> {
        let deadline = Instant::now() + NEED_INPUT_TIMEOUT;
        while self.need_input == 0 {
            if !self.pump_event()? {
                if Instant::now() > deadline {
                    return Err(EncodeError(format!(
                        "{} não pediu entrada em {NEED_INPUT_TIMEOUT:?}",
                        self.name
                    )));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // SAFETY: o MFT pediu entrada (METransformNeedInput).
        unsafe { self.transform.ProcessInput(self.input_id, sample, 0) }
            .map_err(|e| err("ProcessInput", e))?;
        self.need_input -= 1;

        let before = self.pending.len();
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        loop {
            if self.pump_event()? {
                continue;
            }
            if self.pending.len() > before || Instant::now() > deadline {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn encode_sync(&mut self, sample: &IMFSample) -> Result<(), EncodeError> {
        // SAFETY: MFT síncrono configurado.
        let mut input = unsafe { self.transform.ProcessInput(self.input_id, sample, 0) };
        if let Err(e) = &input
            && e.code() == MF_E_NOTACCEPTING
        {
            while self.process_output()? {}
            // SAFETY: idem; a saída pendente foi drenada.
            input = unsafe { self.transform.ProcessInput(self.input_id, sample, 0) };
        }
        input.map_err(|e| err("ProcessInput", e))?;
        while self.process_output()? {}
        Ok(())
    }

    pub fn is_hardware(&self) -> bool {
        self.hardware
    }
}

impl VideoEncoder for MfEncoder {
    fn encode(&mut self, frame: &Nv12Frame, keyframe: bool) -> Result<Vec<u8>, EncodeError> {
        if frame.width != self.config.width || frame.height != self.config.height {
            return Err(EncodeError("tamanho do quadro mudou".into()));
        }
        if keyframe {
            self.set_codec(
                &CODECAPI_AVEncVideoForceKeyFrame,
                VARIANT::from(1u32),
                "ForceKeyFrame",
            );
        }
        let sample = self.make_sample(frame)?;
        if self.events.is_some() {
            self.encode_async(&sample)?;
        } else {
            self.encode_sync(&sample)?;
        }
        let mut out = std::mem::take(&mut self.pending);
        if is_keyframe(&out)
            && !annexb_nal_types(&out).contains(&7)
            && !self.sequence_header.is_empty()
        {
            let mut with_header = self.sequence_header.clone();
            with_header.extend_from_slice(&out);
            out = with_header;
        }
        Ok(out)
    }

    fn name(&self) -> String {
        let kind = if self.hardware {
            "hardware"
        } else {
            "software"
        };
        format!("{} ({kind})", self.name)
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        // SAFETY: encerramento do MFT na mesma thread que o usou.
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            if self.hardware {
                let _ = self.activate.ShutdownObject();
            }
        }
    }
}

/// Encoder que troca o hardware pelo software se o hardware falhar no meio
/// da sessão (drivers instáveis, GPU removida etc.).
struct FallbackEncoder {
    inner: MfEncoder,
    config: EncoderConfig,
}

impl VideoEncoder for FallbackEncoder {
    fn encode(&mut self, frame: &Nv12Frame, keyframe: bool) -> Result<Vec<u8>, EncodeError> {
        match self.inner.encode(frame, keyframe) {
            Err(e) if self.inner.is_hardware() => {
                warn!(%e, encoder = %self.inner.name(), "encoder de hardware falhou; usando software");
                self.inner = MfEncoder::new(self.config, EncoderPreference::Software)?;
                self.inner.encode(frame, true)
            }
            other => other,
        }
    }

    fn set_bitrate(&mut self, bps: u32) -> bool {
        // Mantém a taxa nova se for preciso recriar (troca para software).
        self.config.bitrate_bps = bps;
        self.inner.change_bitrate(bps)
    }

    fn name(&self) -> String {
        self.inner.name()
    }
}

/// Fábrica para o pipeline: hardware com reserva em software.
pub fn auto_encoder(preference: EncoderPreference) -> EncoderFactory {
    Box::new(move |config| {
        let inner = MfEncoder::new(config, preference)?;
        Ok(Box::new(FallbackEncoder { inner, config }) as Box<dyn VideoEncoder>)
    })
}
