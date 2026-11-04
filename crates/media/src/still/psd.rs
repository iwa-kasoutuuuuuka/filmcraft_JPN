//! Photoshop documents (`.psd`, and large-document `.psb`) as their flattened composite, read from
//! Adobe's public "Adobe Photoshop File Formats Specification": a 26-byte header, the colour mode
//! data, image resources and layer and mask sections (skipped by their lengths), then the image data
//! section, which holds the document merged the way Photoshop last saved it. That is what Premiere's
//! "Merge All Layers" import shows; the layers themselves are not read. The composite is planar, one
//! channel after the other, uncompressed, PackBits run-length coded per row, or zlib ("ZIP")
//! compressed with or without per-row delta prediction.
//!
//! Colour modes: bitmap (1 bit, set bits black), grayscale, duotone (its single channel read as
//! grayscale, which is the specification's own fallback; the ink curves are ignored), indexed
//! (through the colour mode data's 768-byte palette, with the transparency index resource 1047),
//! RGB, CMYK (stored inverted, 255 = no ink, converted naively without an ICC profile), Lab (D50,
//! converted to sRGB through the Bradford-adapted matrix) and multichannel (its first channel read
//! as grayscale: channels are stored like CMYK inks, so the preview shows that ink's density with
//! no ink as white; the other channels are ignored). Depths: 1 bit (bitmap), 8 and 16 bits (16-bit
//! kept at 8, as for DPX), and 32-bit float for grayscale and RGB, which Photoshop keeps linear: it
//! is clamped to 0..1 and given the sRGB transfer.
//!
//! The first extra channel is the composite's transparency only when the layer count is negative,
//! which is how the format says so (otherwise extra channels are saved selections). Photoshop stores
//! a transparent composite blended over white, so its colour is unblended back to straight alpha;
//! the specification doesn't say so, it is what psd-tools and ImageMagick do, and Photoshop-written
//! files bear it out (their transparent pixels are white, and a semi-transparent one is its layer's
//! colour blended over white). Documents saved without "Maximize Compatibility" (version info
//! resource 1057 says the image data is not the real composite) are refused, as are depths a mode
//! cannot have (say 32-bit CMYK).

use super::Raster;
use crate::{MediaError, Result};

/// Largest side a PSD allows.
const MAX_SIDE: u32 = 30_000;
/// Largest side a PSB (large document) allows; [`MAX_PIXELS`] still bounds the picture.
const MAX_SIDE_PSB: u32 = 300_000;
/// Largest picture we decode (each pixel takes 4 bytes as RGBA8).
const MAX_PIXELS: usize = 1 << 28;
/// Most channels the format allows.
const MAX_CHANNELS: usize = 56;
/// Largest ZIP-compressed composite we inflate (the used channels only).
const MAX_INFLATED: u64 = 1 << 30;
/// Deflate expands at most about 1032 to 1: no shorter stream can hold the picture.
const ZIP_RATIO: u64 = 1032;
/// Tagged blocks whose length takes 8 bytes in a PSB.
const PSB_LONG_KEYS: [&[u8; 4]; 13] = [b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2", b"FEid", b"FXid", b"PxSD"];

/// Whether `b` starts like a Photoshop document.
pub fn sniff(b: &[u8]) -> bool {
    b.starts_with(b"8BPS")
}

fn err(name: &str, what: &str) -> MediaError {
    MediaError::Decode(format!("{name}: PSD: {what}"))
}

fn unsupported(name: &str, what: &str) -> MediaError {
    MediaError::Unsupported(format!("{name}: PSD: {what} documents are not supported"))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(*b.get(at..at.checked_add(2)?)?.first_chunk()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(*b.get(at..at.checked_add(4)?)?.first_chunk()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(*b.get(at..at.checked_add(8)?)?.first_chunk()?))
}

/// A length field of 4 bytes, or 8 when `wide`, as a `usize`.
fn length(b: &[u8], at: usize, wide: bool) -> Option<usize> {
    if wide { usize::try_from(be64(b, at)?).ok() } else { usize::try_from(be32(b, at)?).ok() }
}

/// The section that starts at `pos` (a 4-byte length, or 8 when `wide`, then the data) and the
/// position after it.
fn section<'a>(name: &str, b: &'a [u8], pos: usize, wide: bool) -> Result<(&'a [u8], usize)> {
    let truncated = || err(name, "truncated file");
    let len = length(b, pos, wide).ok_or_else(truncated)?;
    let start = pos.checked_add(if wide { 8 } else { 4 }).ok_or_else(truncated)?;
    let end = start.checked_add(len).ok_or_else(truncated)?;
    let data = b.get(start..end).ok_or_else(truncated)?;
    Ok((data, end))
}

/// The data of image resource `want`. Resources are `8BIM`, an id, an even-padded Pascal name, then
/// a length and the even-padded data.
fn resource(s: &[u8], want: u16) -> Option<&[u8]> {
    let mut p = 0usize;
    // each resource takes at least 12 bytes, so this walks the section once
    while let Some(head) = s.get(p..p.checked_add(7)?) {
        if !head.starts_with(b"8BIM") {
            return None;
        }
        let id = be16(head, 4)?;
        let name = (usize::from(*head.get(6)?) + 2) & !1;
        let len_at = p.checked_add(6)?.checked_add(name)?;
        let len = be32(s, len_at)? as usize;
        let data = len_at.checked_add(4)?;
        if id == want {
            return s.get(data..data.checked_add(len)?);
        }
        p = data.checked_add(len)?.checked_add(len & 1)?;
    }
    None
}

/// What the version info resource (1057) says about the image data: `Some(false)` when it is not
/// the real composite.
fn real_merged_data(s: &[u8]) -> Option<bool> {
    resource(s, 1057).map(|d| d.get(4).is_none_or(|&v| v != 0))
}

/// Whether the layer and mask section has a negative layer count, which marks the first extra
/// channel of the composite as its transparency. 16-bit documents leave the layer info empty and
/// keep the layers in an `Lr16` (or `Lr32`) tagged block after the global layer mask. In a PSB the
/// layer info length and some tagged block lengths take 8 bytes.
fn merged_alpha(s: &[u8], psb: bool) -> bool {
    let negative = |at: usize| be16(s, at).is_some_and(|n| (n as i16) < 0);
    let wide = if psb { 8 } else { 4 };
    let Some(info) = length(s, 0, psb) else { return false };
    if info >= 2 {
        return negative(wide);
    }
    let Some(mut p) = info.checked_add(wide) else { return false };
    let Some(mask) = be32(s, p) else { return false };
    let Some(next) = p.checked_add(4).and_then(|q| q.checked_add(mask as usize)) else { return false };
    p = next;
    // each block takes at least 12 bytes, so this walks the section once
    while let Some(head) = p.checked_add(12).and_then(|end| s.get(p..end)) {
        let (sig, key) = head.split_at(4);
        if sig != b"8BIM" && sig != b"8B64" {
            return false;
        }
        let long = psb && PSB_LONG_KEYS.iter().any(|k| key.starts_with(*k));
        let Some(data) = p.checked_add(if long { 16 } else { 12 }) else { return false };
        if key.starts_with(b"Lr16") || key.starts_with(b"Lr32") {
            return negative(data);
        }
        // global tagged blocks pad their data to a multiple of 4 bytes (the specification says even;
        // 4 is how psd-tools reads them)
        let Some(len) = p.checked_add(8).and_then(|at| length(s, at, long)) else { return false };
        let Some(next) = len.checked_add(3).and_then(|l| data.checked_add(l & !3)) else { return false };
        p = next;
    }
    false
}

/// Expand one PackBits-coded row into `out`, which it must fill exactly.
fn unpack(name: &str, src: &[u8], out: &mut [u8]) -> Result<()> {
    let (mut i, mut o) = (0usize, 0usize);
    let truncated = || err(name, "truncated run-length row");
    let overlong = || err(name, "run-length row longer than the picture");
    while o < out.len() {
        let n = *src.get(i).ok_or_else(truncated)? as i8;
        i += 1;
        if n >= 0 {
            let k = n as usize + 1;
            let lit = src.get(i..i + k).ok_or_else(truncated)?;
            out.get_mut(o..o + k).ok_or_else(overlong)?.copy_from_slice(lit);
            i += k;
            o += k;
        } else if n != -128 {
            let k = usize::from(n.unsigned_abs()) + 1;
            let v = *src.get(i).ok_or_else(truncated)?;
            out.get_mut(o..o + k).ok_or_else(overlong)?.fill(v);
            i += 1;
            o += k;
        }
    }
    Ok(())
}

/// Inflate the zlib data of the composite into exactly `len` bytes. One stream normally holds every
/// channel; streams that end early are followed by the next one (one per channel, as in layer data).
fn inflate(name: &str, data: &[u8], len: usize) -> Result<Vec<u8>> {
    use miniz_oxide::inflate::stream::{InflateState, inflate};
    use miniz_oxide::{DataFormat, MZFlush, MZStatus};
    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut out = vec![0u8; len];
    let (mut i, mut o) = (0usize, 0usize);
    while o < len {
        let input = data.get(i..).unwrap_or_default();
        if input.is_empty() {
            return Err(err(name, "truncated ZIP data"));
        }
        let r = inflate(&mut state, input, out.get_mut(o..).unwrap_or_default(), MZFlush::None);
        i = i.saturating_add(r.bytes_consumed);
        o = o.saturating_add(r.bytes_written);
        match r.status {
            Ok(MZStatus::StreamEnd) => state.reset(DataFormat::Zlib),
            Ok(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => {}
            _ => return Err(err(name, "bad ZIP data")),
        }
    }
    Ok(out)
}

/// Undo ZIP prediction in one row: each byte (8 bits and less) or big-endian 16-bit sample is stored
/// as the difference from the one before it. 32-bit rows are delta-coded bytewise over the row, then
/// hold the samples' first bytes, then their second bytes, and so on.
fn unpredict(row: &mut [u8], depth: u16, scratch: &mut Vec<u8>) {
    match depth {
        16 => {
            let mut prev = 0u16;
            for s in row.as_chunks_mut::<2>().0 {
                prev = prev.wrapping_add(u16::from_be_bytes(*s));
                *s = prev.to_be_bytes();
            }
        }
        _ => {
            let mut prev = 0u8;
            for v in row.iter_mut() {
                prev = prev.wrapping_add(*v);
                *v = prev;
            }
            if depth == 32 {
                let w = row.len() / 4;
                scratch.clear();
                scratch.extend_from_slice(row);
                for (x, s) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    for (k, d) in s.iter_mut().enumerate() {
                        *d = scratch.get(k * w + x).copied().unwrap_or(0);
                    }
                }
            }
        }
    }
}

/// How a mode's channels make colour.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Bitmap,
    Gray,
    Indexed,
    Rgb,
    Cmyk,
    Lab,
}

/// L*a*b* (D50; L in 0..100, a and b in -128..127) to sRGB.
fn lab_to_srgb(l: f32, a: f32, b: f32) -> [u8; 3] {
    let f = |t: f32| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f32 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    let fy = (l + 16.0) / 116.0;
    let (x, y, z) = (0.964_22 * f(fy + a / 500.0), f(fy), 0.825_21 * f(fy - b / 200.0));
    // XYZ (D50) to linear sRGB (D65), Bradford adapted
    let rgb = [
        3.133_856 * x - 1.616_866_7 * y - 0.490_614_6 * z,
        -0.978_768_4 * x + 1.916_141_5 * y + 0.033_454 * z,
        0.071_945_3 * x - 0.228_991_4 * y + 1.405_242_7 * z,
    ];
    rgb.map(|c| (filmcraft_color::linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0 + 0.5).clamp(0.0, 255.0) as u8)
}

/// Linear light at 16 bits to sRGB-encoded 8 bits.
fn srgb16() -> &'static [u8] {
    static T: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    T.get_or_init(|| (0..=65_535u32).map(|i| (filmcraft_color::linear_to_srgb(i as f32 / 65_535.0) * 255.0 + 0.5).clamp(0.0, 255.0) as u8).collect())
}

/// `stored` blended as `colour * a + white * (1 - a)`, back to the colour (rounded, clamped).
fn unmatte(stored: u32, white: u32, a: u32, max: u32) -> u32 {
    let (v, w, a, m) = (u64::from(stored), u64::from(white), u64::from(a), u64::from(max));
    ((v * m).saturating_sub(w * m.saturating_sub(a)) + a / 2).checked_div(a).unwrap_or(0).min(m) as u32
}

/// Decode the composite image of a Photoshop document to straight-alpha RGBA8.
pub fn decode(name: &str, b: &[u8]) -> Result<Raster> {
    if !sniff(b) {
        return Err(err(name, "not a Photoshop document"));
    }
    let h = b.get(..26).ok_or_else(|| err(name, "truncated header"))?;
    let field16 = |at| be16(h, at).unwrap_or(0);
    let field32 = |at| be32(h, at).unwrap_or(0);
    let psb = match field16(4) {
        1 => false,
        2 => true,
        v => return Err(err(name, &format!("unknown version {v}"))),
    };
    let (channels, height, width, depth, mode) = (usize::from(field16(12)), field32(14), field32(18), field16(22), field16(24));
    let (model, colours, mode_name) = match mode {
        0 => (Model::Bitmap, 1, "bitmap"),
        1 => (Model::Gray, 1, "grayscale"),
        2 => (Model::Indexed, 1, "indexed colour"),
        3 => (Model::Rgb, 3, "RGB"),
        4 => (Model::Cmyk, 4, "CMYK"),
        7 => (Model::Gray, 1, "multichannel"),
        8 => (Model::Gray, 1, "duotone"),
        9 => (Model::Lab, 3, "Lab"),
        m => return Err(err(name, &format!("unknown colour mode {m}"))),
    };
    match (model, depth) {
        (Model::Bitmap, 1) | (Model::Indexed, 8) | (Model::Gray | Model::Rgb, 8 | 16 | 32) | (Model::Cmyk | Model::Lab, 8 | 16) => {}
        (_, 1 | 8 | 16 | 32) => return Err(unsupported(name, &format!("{depth}-bit {mode_name}"))),
        (_, d) => return Err(err(name, &format!("unknown bit depth {d}"))),
    }
    if channels < colours || channels > MAX_CHANNELS {
        return Err(err(name, &format!("{channels} channels")));
    }
    let max_side = if psb { MAX_SIDE_PSB } else { MAX_SIDE };
    if width == 0 || height == 0 || width > max_side || height > max_side {
        return Err(err(name, "picture size out of range"));
    }
    let (w, ht) = (width as usize, height as usize);
    let pixels = w.checked_mul(ht).filter(|&n| n <= MAX_PIXELS).ok_or_else(|| err(name, "picture too large"))?;

    let (mode_data, pos) = section(name, b, 26, false)?;
    let (resources, pos) = section(name, b, pos, false)?;
    if real_merged_data(resources) == Some(false) {
        return Err(MediaError::Unsupported(format!(
            "{name}: PSD: saved without Maximize Compatibility, so the file holds no composite image (resave it with that option on)"
        )));
    }
    // the palette: 256 reds, then 256 greens, then 256 blues
    let palette = if model == Model::Indexed { mode_data.get(..768).ok_or_else(|| err(name, "missing colour palette"))? } else { &[] };
    let transparent = resource(resources, 1047).and_then(|d| be16(d, 0)).filter(|&i| i < 256).map(usize::from);
    let (layers, pos) = section(name, b, pos, psb)?;
    let alpha = channels > colours && !matches!(model, Model::Bitmap | Model::Indexed) && merged_alpha(layers, psb);
    let used = colours + usize::from(alpha);
    let compression = be16(b, pos).ok_or_else(|| err(name, "truncated file"))?;
    let data = b.get(pos + 2..).unwrap_or_default();
    let row_bytes = if depth == 1 { w.div_ceil(8) } else { w * usize::from(depth / 8) };
    let needed = (row_bytes as u64) * (ht as u64) * (used as u64);
    // a PackBits pair expands to at most 128 bytes: no shorter file can hold the picture
    let ratio = match compression {
        0 => 1,
        1 => 64,
        2 | 3 => ZIP_RATIO,
        c => return Err(err(name, &format!("unknown compression {c}"))),
    };
    if (data.len() as u64).saturating_mul(ratio) < needed {
        return Err(err(name, "truncated image data"));
    }

    // ZIP is inflated whole (the used channels only), then read as uncompressed
    let inflated;
    let (data, rle) = match compression {
        0 => (data, false),
        1 => (data, true),
        _ => {
            if needed > MAX_INFLATED {
                return Err(err(name, "ZIP-compressed picture too large"));
            }
            let len = usize::try_from(needed).map_err(|_| err(name, "picture too large"))?;
            let mut out = inflate(name, data, len)?;
            if compression == 3 && row_bytes > 0 {
                let mut scratch = Vec::with_capacity(row_bytes);
                out.chunks_exact_mut(row_bytes).for_each(|row| unpredict(row, depth, &mut scratch));
            }
            inflated = out;
            (inflated.as_slice(), false)
        }
    };

    // where each row of the used channels starts, and how long it is; PSB row lengths take 4 bytes
    let mut rows = Vec::with_capacity(ht * used);
    if rle {
        let count = if psb { 4 } else { 2 };
        let mut at = ht.checked_mul(channels).and_then(|n| n.checked_mul(count)).ok_or_else(|| err(name, "truncated row lengths"))?;
        for i in 0..ht * used {
            let len = if psb { be32(data, i * 4).map(|n| n as usize) } else { be16(data, i * 2).map(usize::from) };
            let len = len.ok_or_else(|| err(name, "truncated row lengths"))?;
            rows.push((at, len));
            at = at.saturating_add(len);
        }
    } else {
        rows.extend((0..ht * used).map(|i| (i * row_bytes, row_bytes)));
    }
    let read = |i: usize, row: &mut [u8]| -> Result<()> {
        let &(at, len) = rows.get(i).ok_or_else(|| err(name, "truncated image data"))?;
        let src = at.checked_add(len).and_then(|end| data.get(at..end)).ok_or_else(|| err(name, "truncated image data"))?;
        if rle {
            unpack(name, src, row)
        } else {
            row.copy_from_slice(src);
            Ok(())
        }
    };

    // samples on a 0..=max scale: 32-bit float is clamped and kept linear at 16 bits, then encoded
    let max: u32 = if depth <= 8 { 255 } else { 65_535 };
    let to8 = |v: u32| ((u64::from(v) * 255 + u64::from(max / 2)) / u64::from(max)) as u8;
    let encode = |v: u32| if depth == 32 { srgb16().get(v as usize).copied().unwrap_or(255) } else { to8(v) };
    let samples = |row: &[u8], out: &mut [u32]| match depth {
        1 => out.iter_mut().enumerate().for_each(|(x, s)| {
            let set = row.get(x / 8).is_some_and(|&v| v & (0x80 >> (x % 8)) != 0);
            *s = if set { 0 } else { 255 };
        }),
        8 => out.iter_mut().zip(row).for_each(|(s, &v)| *s = u32::from(v)),
        16 => out.iter_mut().zip(row.as_chunks::<2>().0).for_each(|(s, &v)| *s = u32::from(u16::from_be_bytes(v))),
        _ => out.iter_mut().zip(row.as_chunks::<4>().0).for_each(|(s, &v)| {
            // NaN casts to 0
            *s = (f32::from_be_bytes(v).clamp(0.0, 1.0) * 65_535.0 + 0.5) as u32;
        }),
    };
    // the colour each channel takes on where there is nothing (Lab's a and b are neutral half way)
    let white = |c: usize| if model == Model::Lab && c > 0 { max / 2 + 1 } else { max };

    let mut row = vec![0u8; row_bytes];
    let mut planes = vec![vec![0u32; w]; used];
    let mut rgba = vec![255u8; pixels * 4];
    let mut any_transparent = false;
    for (y, out) in rgba.chunks_exact_mut(w * 4).enumerate() {
        for (c, plane) in planes.iter_mut().enumerate() {
            read(c * ht + y, &mut row)?;
            samples(&row, plane);
        }
        for (x, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut v = [0u32; 4];
            for (c, d) in v.iter_mut().enumerate().take(colours) {
                *d = planes.get(c).and_then(|p| p.get(x)).copied().unwrap_or(0);
            }
            if alpha {
                let a = planes.get(colours).and_then(|p| p.get(x)).copied().unwrap_or(max);
                // stored = colour * a + white * (1 - a)
                if a > 0 && a < max {
                    for (c, d) in v.iter_mut().enumerate().take(colours) {
                        *d = unmatte(*d, white(c), a, max);
                    }
                }
                px[3] = to8(a);
            }
            let [c0, c1, c2, c3] = v;
            let rgb = match model {
                Model::Bitmap | Model::Gray => [encode(c0); 3],
                Model::Rgb => [encode(c0), encode(c1), encode(c2)],
                // inverted inks: 255 is none, so each primary is its ink's remainder times black's
                Model::Cmyk => [c0, c1, c2].map(|i| to8(((u64::from(i) * u64::from(c3) + u64::from(max / 2)) / u64::from(max)) as u32)),
                Model::Lab => {
                    // a* and b* are neutral at the middle code (128, or 32768 at 16 bits)
                    let (s, mid) = (255.0 / max as f32, white(1) as f32);
                    lab_to_srgb(c0 as f32 * 100.0 / max as f32, (c1 as f32 - mid) * s, (c2 as f32 - mid) * s)
                }
                Model::Indexed => {
                    let i = c0 as usize;
                    if transparent == Some(i) {
                        px[3] = 0;
                    }
                    [0, 256, 512].map(|o| palette.get(o + i).copied().unwrap_or(0))
                }
            };
            px[..3].copy_from_slice(&rgb);
            any_transparent |= px[3] < 255;
        }
    }
    Ok(Raster { width, height, rgba, has_alpha: any_transparent })
}

#[cfg(test)]
mod tests;
