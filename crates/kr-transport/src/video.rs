//! Trilha de vídeo H.264 do host.
//!
//! - [`h264_codec`]: único codec de vídeo registrado (constrained baseline,
//!   `packetization-mode=1`), o mesmo que o WebView2/Chrome decodifica por
//!   hardware.
//! - [`KeyframeRequestInterceptor`]: entrega à aplicação os PLI/FIR que o
//!   viewer manda quando perde um keyframe (o webrtc-rs descarta o RTCP no fim
//!   da cadeia de interceptors se ninguém o marcar).
//! - [`VideoSender`]: escreve quadros Annex-B na trilha.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use rtc::interceptor::{Attribute, Interceptor, Packet, StreamInfo, TaggedPacket};
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtp_transceiver::PayloadType;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters,
    RTCRtpEncodingParameters, RtpCodecKind,
};
use rtc::sansio::Protocol;
use tokio::sync::OnceCell;
use webrtc::media_stream::Track;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::{TrackLocal, TrackLocalEvent};
use webrtc::rtp_transceiver::RtpSender;

use crate::{Result, TransportError};

pub const H264_MIME: &str = "video/H264";
pub const H264_FMTP: &str =
    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f";
const H264_PAYLOAD_TYPE: PayloadType = 102;

/// Codec H.264 oferecido ao viewer.
pub fn h264_codec() -> RTCRtpCodecParameters {
    let fb = |typ: &str, parameter: &str| RTCPFeedback {
        typ: typ.into(),
        parameter: parameter.into(),
    };
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: H264_MIME.into(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: H264_FMTP.into(),
            rtcp_feedback: vec![
                fb("goog-remb", ""),
                fb("ccm", "fir"),
                fb("nack", ""),
                fb("nack", "pli"),
                fb("transport-cc", ""),
            ],
        },
        payload_type: H264_PAYLOAD_TYPE,
    }
}

/// Marca PLI/FIR recebidos para chegarem à aplicação (via `TrackLocal::poll`)
/// e descarta o resto do RTCP, que já foi tratado pelos outros interceptors.
#[derive(Default)]
pub struct KeyframeRequestInterceptor {
    read: VecDeque<TaggedPacket>,
    write: VecDeque<TaggedPacket>,
}

/// Posição na cadeia: depois de todos os interceptors padrão.
pub const KEYFRAME_INTERCEPTOR_SLOT: usize = 14_000;

pub(crate) fn is_keyframe_request(packet: &dyn rtc::rtcp::Packet) -> bool {
    let any = packet.as_any();
    any.is::<PictureLossIndication>() || any.is::<FullIntraRequest>()
}

impl Protocol<TaggedPacket, TaggedPacket, ()> for KeyframeRequestInterceptor {
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Error = rtc::shared::error::Error;
    type Time = Instant;

    fn handle_read(&mut self, mut msg: TaggedPacket) -> std::result::Result<(), Self::Error> {
        if let Packet::Rtcp(packets) = &msg.message.packet {
            let requests: Vec<Box<dyn rtc::rtcp::Packet>> = packets
                .iter()
                .filter(|p| is_keyframe_request(p.as_ref()))
                .cloned()
                .collect();
            if requests.is_empty() {
                return Ok(());
            }
            msg.message.packet = Packet::Rtcp(requests);
            msg.message.add(Attribute::DeliverToApplication);
        }
        self.read.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read.pop_front()
    }

    fn handle_write(&mut self, msg: TaggedPacket) -> std::result::Result<(), Self::Error> {
        self.write.push_back(msg);
        Ok(())
    }

    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write.pop_front()
    }
}

impl Interceptor for KeyframeRequestInterceptor {
    fn bind_local_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_local_stream(&mut self, _info: &StreamInfo) {}
    fn bind_remote_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_remote_stream(&mut self, _info: &StreamInfo) {}
}

/// Cria a trilha local de vídeo (SSRC aleatório).
pub(crate) fn new_track() -> Result<Arc<TrackLocalStaticSample>> {
    let ssrc = loop {
        let v = uuid::Uuid::new_v4().as_u128() as u32;
        if v != 0 {
            break v;
        }
    };
    let track = TrackLocalStaticSample::new(
        Instant::now(),
        MediaStreamTrack::new(
            "kr-desk".into(),
            "screen".into(),
            "tela".into(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(ssrc),
                    ..Default::default()
                },
                codec: h264_codec().rtp_codec,
                ..Default::default()
            }],
        ),
    )?;
    Ok(Arc::new(track))
}

/// Escreve quadros H.264 na trilha. Clonável; barato de compartilhar.
#[derive(Clone)]
pub struct VideoSender {
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    /// (SSRC, payload type) negociados; conhecidos só depois da resposta SDP.
    negotiated: Arc<OnceCell<(u32, PayloadType)>>,
}

impl VideoSender {
    pub(crate) fn new(track: Arc<TrackLocalStaticSample>, sender: Arc<dyn RtpSender>) -> Self {
        Self {
            track,
            sender,
            negotiated: Arc::new(OnceCell::new()),
        }
    }

    async fn negotiated(&self) -> Result<(u32, PayloadType)> {
        self.negotiated
            .get_or_try_init(|| async {
                let ssrc = *self
                    .track
                    .ssrcs()
                    .await
                    .first()
                    .ok_or_else(|| TransportError::Video("trilha sem SSRC".into()))?;
                let pt = self
                    .sender
                    .get_parameters()
                    .await?
                    .rtp_parameters
                    .codecs
                    .first()
                    .map(|c| c.payload_type)
                    .ok_or_else(|| TransportError::Video("H.264 não foi negociado".into()))?;
                Ok((ssrc, pt))
            })
            .await
            .copied()
    }

    /// Envia um quadro Annex-B. `duration` é o tempo desde o quadro anterior.
    pub async fn write(&self, data: Bytes, duration: Duration, captured_at: Instant) -> Result<()> {
        let (ssrc, pt) = self.negotiated().await?;
        self.track
            .sample_writer(ssrc, pt)
            .write_sample(&Sample {
                data,
                duration,
                ..Sample::new(captured_at)
            })
            .await?;
        Ok(())
    }

    /// Espera o próximo pedido de keyframe (PLI/FIR). `None` quando a trilha
    /// é desligada.
    pub(crate) async fn next_keyframe_request(&self) -> Option<()> {
        loop {
            match self.track.poll().await? {
                TrackLocalEvent::OnRtcpPacket(packets)
                    if packets.iter().any(|p| is_keyframe_request(p.as_ref())) =>
                {
                    return Some(());
                }
                _ => {}
            }
        }
    }
}
