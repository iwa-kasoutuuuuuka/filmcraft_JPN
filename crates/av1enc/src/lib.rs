//! AV1 encoder for FilmCraft's export path: a safe, `Result`-returning wrapper around
//! [rav1e](https://github.com/xiph/rav1e) (BSD-2-Clause, pure Rust), built without its `asm`
//! feature so no assembler or C toolchain is involved.
//!
//! - Input: planar 8-bit 4:2:0 ([`YuvFrame`], limited or full range, any size from 16×16 to
//!   65535×65535).
//! - Output: one [`Packet`] per temporal unit in presentation order, in the low-overhead OBU
//!   format with the temporal delimiters removed, as AV1-ISOBMFF (MP4 `av01`) samples carry them.
//! - [`Encoder::av1c`] gives the `AV1CodecConfigurationRecord` for the MP4 `av1C` box once the
//!   first packet (which carries the sequence header OBU) has been produced.
//!
//! rav1e is third-party code: every call into it runs under `catch_unwind`, so an internal panic
//! becomes [`Error::Encoder`] and the encoder refuses further work instead of taking the export
//! (and the app) down.
//!
//! ```no_run
//! use filmcraft_av1enc::{Encoder, EncoderConfig, YuvFrame};
//! let cfg = EncoderConfig::new(1280, 720, 30, 1);
//! let mut enc = Encoder::new(&cfg).unwrap();
//! let (y, u, v) = (vec![128u8; 1280 * 720], vec![128u8; 640 * 360], vec![128u8; 640 * 360]);
//! let frame = YuvFrame { y: &y, u: &u, v: &v, y_stride: 1280, uv_stride: 640 };
//! let mut packets = enc.encode(&frame).unwrap();
//! packets.extend(enc.flush().unwrap());
//! let av1c = enc.av1c();
//! ```

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use std::panic::{AssertUnwindSafe, catch_unwind};

use filmcraft_bitstream::BitReader;
use rav1e::color::{ColorDescription, ColorPrimaries, MatrixCoefficients, PixelRange, TransferCharacteristics};
use rav1e::config::SpeedSettings;
use rav1e::data::{FrameType, Rational};
use rav1e::{Config, Context, EncoderStatus};

/// Encoder errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid AV1 encoder settings: {0}")]
    Config(String),
    #[error("AV1 input frame: {0}")]
    Frame(String),
    #[error("AV1 encoder: {0}")]
    Encoder(String),
    #[error("invalid AV1 bitstream: {0}")]
    Bitstream(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Smallest frame side rav1e accepts.
pub const MIN_SIZE: u32 = 16;
/// Largest frame side AV1 (and rav1e) accepts.
pub const MAX_SIZE: u32 = 65_535;
/// Fastest rav1e speed preset (the default: export previews and delivery files in reasonable
/// time on the pure-Rust code paths).
pub const FASTEST_SPEED: u8 = 10;
/// Longest keyframe interval accepted (rav1e's own limit is far larger; this keeps seeking sane).
pub const MAX_KEYINT: u32 = 10_000;
/// Frame rates accepted (rav1e's rate control misbehaves on absurd rates).
pub const MIN_FPS: f64 = 0.01;
pub const MAX_FPS: f64 = 1000.0;

/// Rate control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RateControl {
    /// One-pass average bitrate (kbps).
    Bitrate { kbps: u32 },
    /// Constant quantizer, 0 (best) ..= 255.
    Quantizer(u8),
}

/// H.273 colour description written into the sequence header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorConfig {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

impl Default for ColorConfig {
    /// BT.709, limited range.
    fn default() -> Self {
        ColorConfig { primaries: 1, transfer: 1, matrix: 1, full_range: false }
    }
}

/// Encoder settings. Every value is checked by [`Encoder::new`].
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    /// Frame rate `fps_num / fps_den` frames per second.
    pub fps_num: u32,
    pub fps_den: u32,
    /// rav1e speed preset 0 (slowest, best) ..= 10 (fastest).
    pub speed: u8,
    pub rate: RateControl,
    /// Maximum distance between keyframes (frames); scene cuts may add more keyframes.
    pub keyint: u32,
    /// Sample aspect ratio (1:1 = square pixels).
    pub sar: (u32, u32),
    pub color: ColorConfig,
    /// Tiles per frame (0 = choose from the frame size). More tiles encode in parallel.
    pub tiles: usize,
}

impl EncoderConfig {
    /// Defaults: fastest preset, 5 Mbps, keyframe every 2 seconds, BT.709 limited range.
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        let fps = if fps_den == 0 { 30.0 } else { fps_num as f64 / fps_den as f64 };
        EncoderConfig {
            width,
            height,
            fps_num,
            fps_den,
            speed: FASTEST_SPEED,
            rate: RateControl::Bitrate { kbps: 5_000 },
            keyint: (fps * 2.0).round().clamp(1.0, MAX_KEYINT as f64) as u32,
            sar: (1, 1),
            color: ColorConfig::default(),
            tiles: 0,
        }
    }

    fn check(&self) -> Result<()> {
        let size_ok = |v: u32| (MIN_SIZE..=MAX_SIZE).contains(&v);
        if !size_ok(self.width) || !size_ok(self.height) {
            return Err(Error::Config(format!("frame size {}x{} (AV1 export needs {MIN_SIZE}..={MAX_SIZE} per side)", self.width, self.height)));
        }
        let fps = self.fps_num as f64 / self.fps_den.max(1) as f64;
        if self.fps_num == 0 || self.fps_den == 0 || !(MIN_FPS..=MAX_FPS).contains(&fps) {
            return Err(Error::Config(format!("frame rate {}/{} ({MIN_FPS}..={MAX_FPS} fps)", self.fps_num, self.fps_den)));
        }
        if self.speed > FASTEST_SPEED {
            return Err(Error::Config(format!("speed {} (0..={FASTEST_SPEED})", self.speed)));
        }
        if self.keyint == 0 || self.keyint > MAX_KEYINT {
            return Err(Error::Config(format!("keyframe interval {} (1..={MAX_KEYINT})", self.keyint)));
        }
        if let RateControl::Bitrate { kbps } = self.rate
            && kbps == 0
        {
            return Err(Error::Config("bitrate 0 kbps".into()));
        }
        if self.sar.0 == 0 || self.sar.1 == 0 {
            return Err(Error::Config(format!("sample aspect ratio {}:{}", self.sar.0, self.sar.1)));
        }
        Ok(())
    }

    fn rav1e(&self) -> rav1e::EncoderConfig {
        let mut e = rav1e::EncoderConfig::with_speed_preset(self.speed);
        e.width = self.width as usize;
        e.height = self.height as usize;
        // time base = 1 / frame rate
        e.time_base = Rational::new(self.fps_den as u64, self.fps_num as u64);
        e.sample_aspect_ratio = Rational::new(self.sar.0 as u64, self.sar.1 as u64);
        e.bit_depth = 8;
        e.chroma_sampling = rav1e::color::ChromaSampling::Cs420;
        e.pixel_range = if self.color.full_range { PixelRange::Full } else { PixelRange::Limited };
        e.color_description = color_description(self.color);
        let keyint = self.keyint as u64;
        e.set_key_frame_interval(keyint.min(12), keyint);
        match self.rate {
            RateControl::Bitrate { kbps } => {
                e.bitrate = i32::try_from(kbps as u64 * 1000).unwrap_or(i32::MAX);
            }
            RateControl::Quantizer(q) => {
                e.bitrate = 0;
                e.quantizer = q as usize;
            }
        }
        e.tiles = if self.tiles > 0 {
            self.tiles.min(64)
        } else {
            // about one tile per 640×360 area, at most 16 (tiles cost a little compression)
            ((self.width as usize * self.height as usize) / (640 * 360)).clamp(1, 16)
        };
        e.speed_settings = SpeedSettings::from_preset(self.speed);
        e
    }
}

fn color_description(c: ColorConfig) -> Option<ColorDescription> {
    let primaries = match c.primaries {
        1 => ColorPrimaries::BT709,
        9 => ColorPrimaries::BT2020,
        _ => return None,
    };
    let transfer = match c.transfer {
        1 => TransferCharacteristics::BT709,
        16 => TransferCharacteristics::SMPTE2084,
        18 => TransferCharacteristics::HLG,
        _ => return None,
    };
    let matrix = match c.matrix {
        1 => MatrixCoefficients::BT709,
        9 => MatrixCoefficients::BT2020NCL,
        _ => return None,
    };
    Some(ColorDescription { color_primaries: primaries, transfer_characteristics: transfer, matrix_coefficients: matrix })
}

/// One 8-bit 4:2:0 picture. Chroma planes are `ceil(w/2)×ceil(h/2)`.
pub struct YuvFrame<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub uv_stride: usize,
}

/// One temporal unit (one shown frame).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// OBUs of the temporal unit, temporal delimiter removed.
    pub data: Vec<u8>,
    /// A shown key frame: a random access point (MP4 sync sample).
    pub keyframe: bool,
    /// Index of the input frame shown by this unit (0-based, presentation order).
    pub frame_index: u64,
}

/// The rav1e encoder plus the sequence header it wrote.
pub struct Encoder {
    ctx: Context<u8>,
    cfg: EncoderConfig,
    seq_header: Option<Vec<u8>>,
    /// rav1e panicked or failed: the context state is unknown, refuse further work.
    broken: bool,
    flushed: bool,
    sent: u64,
}

/// Run `f` and turn a panic inside rav1e into an error.
fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
        Error::Encoder(format!("{what}: internal error ({msg})"))
    })
}

impl Encoder {
    pub fn new(cfg: &EncoderConfig) -> Result<Self> {
        cfg.check()?;
        let config = Config::new().with_encoder_config(cfg.rav1e());
        config.validate().map_err(|e| Error::Config(e.to_string()))?;
        let ctx = guarded("create", || config.new_context::<u8>())?.map_err(|e| Error::Config(e.to_string()))?;
        Ok(Encoder { ctx, cfg: cfg.clone(), seq_header: None, broken: false, flushed: false, sent: 0 })
    }

    pub fn config(&self) -> &EncoderConfig {
        &self.cfg
    }

    fn alive(&self) -> Result<()> {
        if self.broken { Err(Error::Encoder("the encoder failed earlier; start a new export".into())) } else { Ok(()) }
    }

    /// Check the planes of `f` cover the frame.
    fn check_frame(&self, f: &YuvFrame) -> Result<()> {
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        if f.y_stride < w || f.uv_stride < cw {
            return Err(Error::Frame(format!("strides {} / {} for a {w}-wide frame", f.y_stride, f.uv_stride)));
        }
        let need = |stride: usize, rows: usize, width: usize| stride.checked_mul(rows.saturating_sub(1)).and_then(|n| n.checked_add(width));
        let (ny, nc) = (need(f.y_stride, h, w), need(f.uv_stride, ch, cw));
        if ny.is_none_or(|n| f.y.len() < n) || nc.is_none_or(|n| f.u.len() < n || f.v.len() < n) {
            return Err(Error::Frame(format!("planes too short for {w}x{h} ({} / {} / {} bytes)", f.y.len(), f.u.len(), f.v.len())));
        }
        Ok(())
    }

    /// Queue one picture; returns the temporal units that are ready (rav1e looks ahead, so the
    /// first few calls return none).
    pub fn encode(&mut self, f: &YuvFrame) -> Result<Vec<Packet>> {
        self.alive()?;
        if self.flushed {
            return Err(Error::Encoder("frame sent after flush".into()));
        }
        self.check_frame(f)?;
        let (w, h) = (self.cfg.width as usize, self.cfg.height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut frame = self.ctx.new_frame();
        for (plane, (src, stride, pw, ph)) in frame.planes.iter_mut().zip([(f.y, f.y_stride, w, h), (f.u, f.uv_stride, cw, ch), (f.v, f.uv_stride, cw, ch)]) {
            // rav1e copies whole `stride` rows: hand it exactly `ph` of them, repacking tightly
            // when the last row is short of a full stride (bounds were checked above)
            match src.get(..stride.saturating_mul(ph)) {
                Some(rows) => plane.copy_from_raw_u8(rows, stride, 1),
                None => {
                    let mut tight = Vec::with_capacity(pw.saturating_mul(ph));
                    for r in 0..ph {
                        let start = r.saturating_mul(stride);
                        tight.extend_from_slice(src.get(start..start.saturating_add(pw)).unwrap_or(&[]));
                    }
                    tight.resize(pw.saturating_mul(ph), 0);
                    plane.copy_from_raw_u8(&tight, pw.max(1), 1);
                }
            }
        }
        let ctx = &mut self.ctx;
        let sent = guarded("encode", || ctx.send_frame(frame));
        match sent {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.broken = true;
                return Err(Error::Encoder(format!("send frame: {e}")));
            }
            Err(e) => {
                self.broken = true;
                return Err(e);
            }
        }
        self.sent += 1;
        self.drain()
    }

    /// Finish the stream: returns the remaining temporal units.
    pub fn flush(&mut self) -> Result<Vec<Packet>> {
        self.alive()?;
        if !self.flushed {
            let ctx = &mut self.ctx;
            if let Err(e) = guarded("flush", || ctx.flush()) {
                self.broken = true;
                return Err(e);
            }
            self.flushed = true;
        }
        self.drain()
    }

    /// Collect every packet rav1e can emit now.
    fn drain(&mut self) -> Result<Vec<Packet>> {
        let mut out = Vec::new();
        // each call either emits a packet, does a unit of work or reports it needs input;
        // bound the loop anyway (a few calls per frame sent)
        let budget = self.sent.saturating_mul(8).saturating_add(64);
        for _ in 0..budget {
            let ctx = &mut self.ctx;
            let r = match guarded("receive packet", || ctx.receive_packet()) {
                Ok(r) => r,
                Err(e) => {
                    self.broken = true;
                    return Err(e);
                }
            };
            match r {
                Ok(p) => {
                    let (data, seq) = strip_obus(&p.data)?;
                    if self.seq_header.is_none() {
                        self.seq_header = seq;
                    }
                    let keyframe = p.frame_type == FrameType::KEY && is_shown_key_frame(&data);
                    out.push(Packet { data, keyframe, frame_index: p.input_frameno });
                }
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => return Ok(out),
                Err(e) => {
                    self.broken = true;
                    return Err(Error::Encoder(format!("receive packet: {e}")));
                }
            }
        }
        Ok(out)
    }

    /// The sequence header OBU (with its OBU header and size), once the first packet is out.
    pub fn sequence_header_obu(&self) -> Option<&[u8]> {
        self.seq_header.as_deref()
    }

    /// `AV1CodecConfigurationRecord` (AV1-ISOBMFF §2.3) for the MP4 `av1C` box: the sequence
    /// header OBU as `configOBUs`, profile / level / tier read from it. Before the first packet
    /// (no sequence header yet) the record describes the configured stream with no config OBUs.
    pub fn av1c(&self) -> Vec<u8> {
        let info = self.seq_header.as_deref().and_then(|obu| sequence_info(obu).ok());
        let (profile, level, tier) = info.map_or((0, 31, false), |i| (i.profile, i.level, i.tier));
        let mut b = Vec::with_capacity(4 + self.seq_header.as_ref().map_or(0, Vec::len));
        b.push(0x81); // marker, version 1
        b.push((profile << 5) | (level & 0x1F));
        // tier, high_bitdepth 0, twelve_bit 0, monochrome 0, subsampling_x 1, subsampling_y 1,
        // chroma_sample_position 0 (unknown)
        b.push(((tier as u8) << 7) | 0x08 | 0x04);
        b.push(0); // no initial_presentation_delay
        if let Some(s) = &self.seq_header {
            b.extend_from_slice(s);
        }
        b
    }
}

/// OBU types (AV1 spec §6.2.2).
const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_FRAME: u8 = 6;

/// One OBU: its type, the whole OBU (header included) and its payload.
pub struct Obu<'a> {
    pub obu_type: u8,
    pub bytes: &'a [u8],
    pub payload: &'a [u8],
}

fn leb128(data: &[u8]) -> Result<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..8 {
        let b = *data.get(i).ok_or(Error::Bitstream("truncated leb128"))?;
        v |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Ok((v, i + 1));
        }
    }
    Err(Error::Bitstream("leb128 longer than 8 bytes"))
}

/// Split a temporal unit (low-overhead format) into OBUs.
pub fn obus(data: &[u8]) -> Result<Vec<Obu<'_>>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let h = *data.get(pos).ok_or(Error::Bitstream("truncated OBU header"))?;
        if h & 0x80 != 0 {
            return Err(Error::Bitstream("OBU forbidden bit set"));
        }
        let obu_type = (h >> 3) & 0x0F;
        let mut p = pos + 1 + usize::from(h & 0x04 != 0);
        let size = if h & 0x02 != 0 {
            let (v, n) = leb128(data.get(p..).ok_or(Error::Bitstream("truncated OBU header"))?)?;
            p += n;
            usize::try_from(v).map_err(|_| Error::Bitstream("OBU size"))?
        } else {
            data.len().saturating_sub(p)
        };
        let end = p.checked_add(size).filter(|e| *e <= data.len()).ok_or(Error::Bitstream("OBU larger than the temporal unit"))?;
        let (Some(bytes), Some(payload)) = (data.get(pos..end), data.get(p..end)) else {
            return Err(Error::Bitstream("OBU bounds"));
        };
        out.push(Obu { obu_type, bytes, payload });
        pos = end;
    }
    Ok(out)
}

/// The temporal unit without temporal delimiters, plus its sequence header OBU (if any).
fn strip_obus(data: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let mut out = Vec::with_capacity(data.len());
    let mut seq = None;
    for o in obus(data)? {
        match o.obu_type {
            OBU_TEMPORAL_DELIMITER => {}
            t => {
                if t == OBU_SEQUENCE_HEADER && seq.is_none() {
                    seq = Some(o.bytes.to_vec());
                }
                out.extend_from_slice(o.bytes);
            }
        }
    }
    Ok((out, seq))
}

/// Whether the first frame (header) of a temporal unit is a shown key frame.
fn is_shown_key_frame(data: &[u8]) -> bool {
    let Ok(list) = obus(data) else { return false };
    for o in list {
        if matches!(o.obu_type, OBU_FRAME_HEADER | OBU_FRAME) {
            let b = o.payload.first().copied().unwrap_or(0);
            // show_existing_frame = 0, frame_type = KEY_FRAME (0), show_frame = 1
            // (no reduced_still_picture_header: rav1e writes full frame headers)
            return b >> 7 == 0 && (b >> 5) & 3 == 0 && (b >> 4) & 1 == 1;
        }
    }
    false
}

/// Profile / level / tier of operating point 0 of a sequence header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceInfo {
    pub profile: u8,
    pub level: u8,
    pub tier: bool,
}

/// Read [`SequenceInfo`] from a sequence header OBU (AV1 spec §5.5.1).
pub fn sequence_info(obu: &[u8]) -> Result<SequenceInfo> {
    let o = obus(obu)?.into_iter().find(|o| o.obu_type == OBU_SEQUENCE_HEADER).ok_or(Error::Bitstream("no sequence header OBU"))?;
    let mut r = BitReader::new(o.payload);
    let bad = |_| Error::Bitstream("truncated sequence header");
    let profile = r.read_bits(3).map_err(bad)? as u8;
    let _still_picture = r.read_flag().map_err(bad)?;
    let reduced = r.read_flag().map_err(bad)?;
    if reduced {
        let level = r.read_bits(5).map_err(bad)? as u8;
        return Ok(SequenceInfo { profile, level, tier: false });
    }
    // timing_info_present_flag
    if r.read_flag().map_err(bad)? {
        r.read_bits(32).map_err(bad)?; // num_units_in_display_tick
        r.read_bits(32).map_err(bad)?; // time_scale
        if r.read_flag().map_err(bad)? {
            r.read_ue().map_err(bad)?; // num_ticks_per_picture_minus_1 (uvlc)
        }
        // decoder_model_info_present_flag
        if r.read_flag().map_err(bad)? {
            r.read_bits(5).map_err(bad)?; // buffer_delay_length_minus_1
            r.read_bits(32).map_err(bad)?; // num_units_in_decoding_tick
            r.read_bits(5).map_err(bad)?; // buffer_removal_time_length_minus_1
            r.read_bits(5).map_err(bad)?; // frame_presentation_time_length_minus_1
        }
    }
    r.read_flag().map_err(bad)?; // initial_display_delay_present_flag
    r.read_bits(5).map_err(bad)?; // operating_points_cnt_minus_1
    r.read_bits(12).map_err(bad)?; // operating_point_idc[0]
    let level = r.read_bits(5).map_err(bad)? as u8;
    let tier = if level > 7 { r.read_flag().map_err(bad)? } else { false };
    Ok(SequenceInfo { profile, level, tier })
}

#[cfg(test)]
mod tests;
