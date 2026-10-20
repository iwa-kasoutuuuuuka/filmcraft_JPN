//! AV1 export (MP4 `av01`): straight sRGB RGBA8 → 8-bit limited-range BT.709 4:2:0 → rav1e
//! ([`filmcraft_av1enc`]) at its fastest speed preset, one-pass average bitrate at the requested
//! target (Match Source presets derive it from bits per pixel). Samples are temporal units
//! without temporal delimiters; the `av1C` carries the sequence header OBU. HDR sequences are
//! tone mapped to SDR (8-bit only for now).

use filmcraft_isobmff::SampleEntry;
use filmcraft_time::FrameRate;

use crate::{EncodedPacket, EncoderFrame, ExportError, ExportSettings, Format, InputConverter, Result, VideoEncoder, rgba_to_i420, rgba_to_yuv420_8};

/// rav1e speed preset of exports (fastest; the pure-Rust kernels are several times slower than
/// rav1e's assembly).
pub const AV1_SPEED: u8 = filmcraft_av1enc::FASTEST_SPEED;

struct Av1Encoder {
    enc: filmcraft_av1enc::Encoder,
    w: u32,
    h: u32,
    rate: FrameRate,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    sar: Option<(u32, u32)>,
}

impl Av1Encoder {
    fn packets(&self, ps: Vec<filmcraft_av1enc::Packet>) -> Vec<EncodedPacket> {
        let dur = u32::try_from(self.rate.den).unwrap_or(1).max(1);
        ps.into_iter().map(|p| EncodedPacket { data: p.data, key: p.keyframe, duration: dur, composition_offset: 0 }).collect()
    }
}

impl VideoEncoder for Av1Encoder {
    fn sample_entry(&self) -> SampleEntry {
        let cfg = filmcraft_isobmff::Av1Config::parse(&self.enc.av1c()).unwrap_or(filmcraft_isobmff::Av1Config {
            seq_level_idx_0: 31,
            chroma_subsampling_x: true,
            chroma_subsampling_y: true,
            ..Default::default()
        });
        let mut e = SampleEntry::av1(cfg, self.w.min(u16::MAX as u32) as u16, self.h.min(u16::MAX as u32) as u16);
        if let Some(v) = e.video.as_mut() {
            v.compressor_name = "AV1 (rav1e)".into();
            v.color = Some(filmcraft_isobmff::ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: false });
            v.pixel_aspect = self.sar;
        }
        e
    }
    fn timescale(&self) -> u32 {
        u32::try_from(self.rate.num).unwrap_or(u32::MAX).max(1)
    }
    fn encode(&mut self, f: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        let (w, h) = (f.width as usize, f.height as usize);
        let need = w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or_else(|| ExportError::Encode("frame size".into()))?;
        if f.rgba.len() < need || (f.width, f.height) != (self.w, self.h) {
            return Err(ExportError::Encode(format!("AV1: frame {}x{} with {} bytes for a {}x{} stream", f.width, f.height, f.rgba.len(), self.w, self.h)));
        }
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let ps = match f.prepared.filter(|p| p.len() == w * h + 2 * cw * ch) {
            // converted while rendering (`input_converter`): the three planes back to back
            Some(p) => {
                let (y, uv) = p.split_at(w * h);
                let (u, v) = uv.split_at(cw * ch);
                self.enc.encode(&filmcraft_av1enc::YuvFrame { y, u, v, y_stride: w, uv_stride: cw })
            }
            None => {
                rgba_to_yuv420_8(f.rgba, w, h, &mut self.y, &mut self.u, &mut self.v);
                self.enc.encode(&filmcraft_av1enc::YuvFrame { y: &self.y, u: &self.u, v: &self.v, y_stride: w, uv_stride: cw })
            }
        };
        Ok(self.packets(ps.map_err(|e| ExportError::Encode(e.to_string()))?))
    }
    fn input_converter(&self) -> Option<InputConverter> {
        Some(InputConverter { id: "i420", convert: rgba_to_i420 })
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        let ps = self.enc.flush().map_err(|e| ExportError::Encode(e.to_string()))?;
        Ok(self.packets(ps))
    }
}

/// The rav1e configuration of an export (exposed for tests and the Summary).
pub(crate) fn av1_config(w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Result<filmcraft_av1enc::EncoderConfig> {
    let (num, den) = (u32::try_from(rate.num).ok().filter(|n| *n > 0), u32::try_from(rate.den).ok().filter(|d| *d > 0));
    let (Some(num), Some(den)) = (num, den) else {
        return Err(ExportError::Unsupported(format!("AV1: frame rate {}/{}", rate.num, rate.den)));
    };
    let mut cfg = filmcraft_av1enc::EncoderConfig::new(w, h, num, den);
    cfg.speed = AV1_SPEED;
    cfg.rate = filmcraft_av1enc::RateControl::Bitrate { kbps: s.bitrate_kbps.max(100) };
    if let Some(k) = s.keyframe_distance.filter(|k| *k > 0) {
        cfg.keyint = k.min(filmcraft_av1enc::MAX_KEYINT);
    }
    // The pixel aspect ratio is signalled in the MP4 `pasp` only: rav1e would express it as a
    // render size, which decoders treat inconsistently.
    Ok(cfg)
}

pub(crate) fn av1_factory(format: Format, w: u32, h: u32, rate: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    if format != Format::Av1 {
        return None;
    }
    Some(av1_config(w, h, rate, s).and_then(|cfg| {
        let sar = s.pixel_aspect.map(|(n, d)| (n.max(1), d.max(1)));
        let enc = filmcraft_av1enc::Encoder::new(&cfg).map_err(|e| ExportError::Unsupported(e.to_string()))?;
        Ok(Box::new(Av1Encoder { enc, w, h, rate, y: Vec::new(), u: Vec::new(), v: Vec::new(), sar }) as Box<dyn VideoEncoder>)
    }))
}
