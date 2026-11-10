//! Encoder H.264 de software baseado no OpenH264, compilado a partir do
//! código-fonte.
//!
//! **Somente para testes** (pipeline em Linux e interoperabilidade com o
//! Chrome). O app no Windows usa Media Foundation (`kr-codec-win`), que já vem
//! licenciado com o sistema; este crate não entra no instalador.

use kr_core::media::{EncodeError, EncoderConfig, Nv12Frame, VideoEncoder};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig as OhConfig, FrameRate, IntraFramePeriod, Profile,
    RateControlMode, UsageType,
};
use openh264::formats::YUVSlices;

pub struct OpenH264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl OpenH264Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        let oh = OhConfig::new()
            .bitrate(BitRate::from_bps(config.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(config.fps as f32))
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .profile(Profile::Baseline)
            .skip_frames(false)
            .adaptive_quantization(false)
            .background_detection(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(0));
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), oh)
            .map_err(|e| EncodeError(e.to_string()))?;
        let chroma = (config.width as usize / 2) * (config.height as usize / 2);
        Ok(Self {
            encoder,
            width: config.width as usize,
            height: config.height as usize,
            u: vec![0; chroma],
            v: vec![0; chroma],
        })
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, frame: &Nv12Frame, keyframe: bool) -> Result<Vec<u8>, EncodeError> {
        let (w, h) = (self.width, self.height);
        if frame.width as usize != w || frame.height as usize != h {
            return Err(EncodeError("tamanho do quadro mudou".into()));
        }
        // NV12 (UV intercalado) → I420 (planos separados).
        for (i, uv) in frame.uv_plane().chunks_exact(2).enumerate() {
            self.u[i] = uv[0];
            self.v[i] = uv[1];
        }
        let yuv = YUVSlices::new(
            (frame.y_plane(), &self.u, &self.v),
            (w, h),
            (w, w / 2, w / 2),
        );
        if keyframe {
            self.encoder.force_intra_frame();
        }
        let stream = self
            .encoder
            .encode(&yuv)
            .map_err(|e| EncodeError(e.to_string()))?;
        Ok(stream.to_vec())
    }

    fn name(&self) -> String {
        "OpenH264 (software, testes)".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kr_core::media::{BgraFrame, annexb_nal_types, bgra_to_nv12, is_keyframe};

    #[test]
    fn codifica_idr_com_sps_pps_e_depois_p() {
        let config = EncoderConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
        };
        let mut enc = OpenH264Encoder::new(config).unwrap();
        let mut frame = Nv12Frame::new(320, 240);
        bgra_to_nv12(&BgraFrame::new(320, 240), &mut frame);

        let first = enc.encode(&frame, true).unwrap();
        let types = annexb_nal_types(&first);
        assert!(
            types.contains(&7) && types.contains(&8),
            "SPS/PPS: {types:?}"
        );
        assert!(is_keyframe(&first));

        let second = enc.encode(&frame, false).unwrap();
        assert!(!second.is_empty());
        assert!(!is_keyframe(&second));

        let forced = enc.encode(&frame, true).unwrap();
        assert!(is_keyframe(&forced));
    }
}
