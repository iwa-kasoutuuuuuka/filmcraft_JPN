//! Digital Negative (`.dng`) camera raw files, developed to an sRGB still. Written from the public,
//! royalty-free Adobe "Digital Negative (DNG) Specification", TIFF 6.0 and ITU-T T.81 (lossless
//! JPEG); no other raw decoder was consulted.
//!
//! A DNG is a TIFF file whose first directory carries `DNGVersion`. The full-resolution raw image is
//! the directory (IFD0, its chain or a `SubIFDs` child) with `NewSubFileType` 0 and photometric
//! interpretation CFA (32803) or LinearRaw (34892); the largest one wins. Its strips or tiles are
//! uncompressed (8–16 bits per sample, other depths bit-packed MSB first), lossless JPEG (7,
//! predictors 1–7, restart intervals, the usual "two components of half the tile width" layout),
//! Deflate (8, with horizontal-differencing predictor 2) or lossy JPEG (34892, through the JPEG
//! decoder of the other stills).
//!
//! Developing follows the specification's processing model, simplified:
//! 1. crop to `ActiveArea`, map through `LinearizationTable`, subtract `BlackLevel` (with
//!    `BlackLevelRepeatDim`, `BlackLevelDeltaH`/`V`) and scale by `WhiteLevel` to 0–1;
//! 2. demosaic 2×2 Bayer patterns (any arrangement, `CFAPlaneColor` respected) with the
//!    Malvar–He–Cutler gradient-corrected linear filters ("High-quality linear interpolation for
//!    demosaicing of Bayer-pattern color images", ICASSP 2004); LinearRaw needs none;
//! 3. white balance to `AsShotNeutral` (or `AsShotWhiteXY`, else D65), clipping every channel where
//!    the first one saturates so highlights stay neutral instead of turning magenta;
//! 4. camera to XYZ (D50): `ForwardMatrix` when present, otherwise the inverse of `ColorMatrix`
//!    (with `CameraCalibration` and `AnalogBalance`) plus a Bradford adaptation from the white to
//!    D50. With two calibrations the matrices are interpolated in inverse colour temperature between
//!    `CalibrationIlluminant1`/`2`, iterating the white's temperature (McCamy's approximation), as
//!    the specification describes;
//! 5. `BaselineExposure`, XYZ (D50) to linear sRGB, clip, sRGB transfer curve, 8-bit RGBA;
//! 6. `DefaultCropOrigin`/`Size`, then the IFD0 `Orientation`.
//!
//! Not supported (refused with an "unsupported" error): X-Trans and other non-2×2 colour filter
//! patterns, non-rectangular CFA layouts, floating-point (HDR) DNGs, planar multi-sample data,
//! other compressions and predictors. Only for those does the decoder fall back to the largest RGB
//! preview the file embeds, as a last resort; a corrupt raw is an error, never a preview. Not
//! applied: profile tone curves, hue/sat maps, opcode lists (lens corrections), `DefaultScale`
//! (non-square pixels) and noise reduction.

use std::collections::HashSet;

use rayon::prelude::*;

use super::Raster;
use crate::{MediaError, Result};

mod ljpeg;

/// Largest side TIFF dimensions we accept.
const MAX_SIDE: usize = 65_535;
/// Largest raw image we develop (each pixel needs about 10 bytes while developing).
const MAX_PIXELS: usize = 160_000_000;
/// Most samples (including tile padding) we decode.
const MAX_SAMPLES: usize = 1 << 28;
/// Most image directories we walk, and how deep SubIFDs may nest.
const MAX_IFDS: usize = 64;
const MAX_SUBIFD_DEPTH: u32 = 3;
const MAX_ENTRIES: usize = 4096;
const MAX_TILES: usize = 1 << 20;

const NEW_SUBFILE_TYPE: u16 = 254;
const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const PHOTOMETRIC: u16 = 262;
const STRIP_OFFSETS: u16 = 273;
const ORIENTATION: u16 = 274;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTE_COUNTS: u16 = 279;
const PLANAR_CONFIGURATION: u16 = 284;
const PREDICTOR: u16 = 317;
const TILE_WIDTH: u16 = 322;
const TILE_LENGTH: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const SUB_IFDS: u16 = 330;
const SAMPLE_FORMAT: u16 = 339;
const CFA_REPEAT_PATTERN_DIM: u16 = 33421;
const CFA_PATTERN: u16 = 33422;
const DNG_VERSION: u16 = 50706;
const CFA_PLANE_COLOR: u16 = 50710;
const CFA_LAYOUT: u16 = 50711;
const LINEARIZATION_TABLE: u16 = 50712;
const BLACK_LEVEL_REPEAT_DIM: u16 = 50713;
const BLACK_LEVEL: u16 = 50714;
const BLACK_LEVEL_DELTA_H: u16 = 50715;
const BLACK_LEVEL_DELTA_V: u16 = 50716;
const WHITE_LEVEL: u16 = 50717;
const DEFAULT_CROP_ORIGIN: u16 = 50719;
const DEFAULT_CROP_SIZE: u16 = 50720;
const COLOR_MATRIX_1: u16 = 50721;
const COLOR_MATRIX_2: u16 = 50722;
const CAMERA_CALIBRATION_1: u16 = 50723;
const CAMERA_CALIBRATION_2: u16 = 50724;
const ANALOG_BALANCE: u16 = 50727;
const AS_SHOT_NEUTRAL: u16 = 50728;
const AS_SHOT_WHITE_XY: u16 = 50729;
const BASELINE_EXPOSURE: u16 = 50730;
const CALIBRATION_ILLUMINANT_1: u16 = 50778;
const CALIBRATION_ILLUMINANT_2: u16 = 50779;
const ACTIVE_AREA: u16 = 50829;
const FORWARD_MATRIX_1: u16 = 50964;
const FORWARD_MATRIX_2: u16 = 50965;

const PHOTOMETRIC_RGB: u64 = 2;
const PHOTOMETRIC_YCBCR: u64 = 6;
const PHOTOMETRIC_CFA: u64 = 32803;
const PHOTOMETRIC_LINEAR_RAW: u64 = 34892;

fn err(name: &str, what: &str) -> MediaError {
    MediaError::Decode(format!("{name}: DNG: {what}"))
}

fn unsupported(name: &str, what: &str) -> MediaError {
    MediaError::Unsupported(format!("{name}: DNG: {what} is not supported"))
}

// ---------------------------------------------------------------------------------------------
// TIFF structure

#[derive(Clone, Copy)]
struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: usize,
    /// Absolute offset of the value(s).
    at: usize,
}

struct Ifd {
    entries: Vec<Entry>,
}

impl Ifd {
    fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

fn type_size(t: u16) -> Option<usize> {
    match t {
        1 | 2 | 6 | 7 => Some(1),
        3 | 8 => Some(2),
        4 | 9 | 11 | 13 => Some(4),
        5 | 10 | 12 => Some(8),
        _ => None,
    }
}

impl<'a> Tiff<'a> {
    fn new(b: &'a [u8]) -> Option<Self> {
        let le = match b.get(..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Self { b, le })
    }

    fn bytes<const N: usize>(&self, at: usize) -> Option<[u8; N]> {
        Some(*self.b.get(at..at.checked_add(N)?)?.first_chunk::<N>()?)
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let a = self.bytes::<2>(at)?;
        Some(if self.le { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let a = self.bytes::<4>(at)?;
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }

    fn u64(&self, at: usize) -> Option<u64> {
        let a = self.bytes::<8>(at)?;
        Some(if self.le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) })
    }

    /// The directory at `off` and the offset of the next one (0 at the end of the chain).
    fn ifd(&self, off: usize) -> Option<(Ifd, usize)> {
        let n = usize::from(self.u16(off)?);
        if n > MAX_ENTRIES {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let p = off.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
            let tag = self.u16(p)?;
            let typ = self.u16(p.checked_add(2)?)?;
            let count = self.u32(p.checked_add(4)?)? as usize;
            let Some(size) = type_size(typ).and_then(|s| s.checked_mul(count)) else { continue };
            let at = if size <= 4 { p.checked_add(8)? } else { self.u32(p.checked_add(8)?)? as usize };
            entries.push(Entry { tag, typ, count, at });
        }
        let next = off.checked_add(2 + n * 12).and_then(|p| self.u32(p)).unwrap_or(0) as usize;
        Some((Ifd { entries }, next))
    }

    /// Value `i` of an unsigned integer entry.
    fn int(&self, e: &Entry, i: usize) -> Option<u64> {
        if i >= e.count {
            return None;
        }
        match e.typ {
            1 | 7 => self.b.get(e.at.checked_add(i)?).map(|&v| u64::from(v)),
            3 => self.u16(e.at.checked_add(i.checked_mul(2)?)?).map(u64::from),
            4 | 13 => self.u32(e.at.checked_add(i.checked_mul(4)?)?).map(u64::from),
            _ => None,
        }
    }

    /// Value `i` of any numeric entry (finite only).
    fn num(&self, e: &Entry, i: usize) -> Option<f64> {
        if i >= e.count {
            return None;
        }
        let at = |s: usize| e.at.checked_add(i.checked_mul(s)?);
        let v = match e.typ {
            1 | 3 | 4 | 7 | 13 => self.int(e, i)? as f64,
            6 => f64::from(*self.b.get(at(1)?)? as i8),
            8 => f64::from(self.u16(at(2)?)? as i16),
            9 => f64::from(self.u32(at(4)?)? as i32),
            5 => {
                let a = at(8)?;
                let (n, d) = (self.u32(a)?, self.u32(a.checked_add(4)?)?);
                if d == 0 {
                    return None;
                }
                f64::from(n) / f64::from(d)
            }
            10 => {
                let a = at(8)?;
                let (n, d) = (self.u32(a)? as i32, self.u32(a.checked_add(4)?)? as i32);
                if d == 0 {
                    return None;
                }
                f64::from(n) / f64::from(d)
            }
            11 => f64::from(f32::from_bits(self.u32(at(4)?)?)),
            12 => f64::from_bits(self.u64(at(8)?)?),
            _ => return None,
        };
        v.is_finite().then_some(v)
    }

    fn tag_int(&self, ifd: &Ifd, tag: u16) -> Option<u64> {
        self.int(ifd.get(tag)?, 0)
    }

    /// All values of an integer tag with at most `max` values.
    fn tag_ints(&self, ifd: &Ifd, tag: u16, max: usize) -> Option<Vec<u64>> {
        let e = ifd.get(tag)?;
        if e.count > max {
            return None;
        }
        (0..e.count).map(|i| self.int(e, i)).collect()
    }

    /// All values of a numeric tag with at most `max` values.
    fn tag_nums(&self, ifd: &Ifd, tag: u16, max: usize) -> Option<Vec<f64>> {
        let e = ifd.get(tag)?;
        if e.count > max {
            return None;
        }
        (0..e.count).map(|i| self.num(e, i)).collect()
    }
}

/// Every directory reachable from the header: IFD0 first, then its SubIFDs and the rest of the
/// chain. Loops and repeats are walked once; the count and the SubIFD depth are capped.
fn directories(name: &str, t: &Tiff) -> Result<Vec<Ifd>> {
    let first = t.u32(4).ok_or_else(|| err(name, "truncated header"))? as usize;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    walk(t, first, 0, &mut seen, &mut out);
    if out.is_empty() {
        return Err(err(name, "no readable image directory"));
    }
    Ok(out)
}

fn walk(t: &Tiff, mut off: usize, depth: u32, seen: &mut HashSet<usize>, out: &mut Vec<Ifd>) {
    while off != 0 && out.len() < MAX_IFDS && seen.insert(off) {
        let Some((ifd, next)) = t.ifd(off) else { return };
        let subs = if depth < MAX_SUBIFD_DEPTH { t.tag_ints(&ifd, SUB_IFDS, MAX_IFDS).unwrap_or_default() } else { Vec::new() };
        out.push(ifd);
        for s in subs {
            walk(t, s as usize, depth + 1, seen, out);
        }
        off = next;
    }
}

/// Whether `b` is a DNG: a TIFF whose first directory has a `DNGVersion` tag.
pub fn sniff(b: &[u8]) -> bool {
    let Some(t) = Tiff::new(b) else { return false };
    let Some(off) = t.u32(4) else { return false };
    t.ifd(off as usize).is_some_and(|(ifd, _)| ifd.get(DNG_VERSION).is_some())
}

// ---------------------------------------------------------------------------------------------
// Strips and tiles

/// The samples of one directory, interleaved, top row first.
struct Samples {
    w: usize,
    h: usize,
    spp: usize,
    bits: u32,
    data: Vec<u16>,
}

fn read_samples(name: &str, t: &Tiff, ifd: &Ifd) -> Result<Samples> {
    let dim = |tag| t.tag_int(ifd, tag).map(|v| v as usize).filter(|v| (1..=MAX_SIDE).contains(v));
    let w = dim(IMAGE_WIDTH).ok_or_else(|| err(name, "missing or invalid image width"))?;
    let h = dim(IMAGE_LENGTH).ok_or_else(|| err(name, "missing or invalid image height"))?;
    if w.saturating_mul(h) > MAX_PIXELS {
        return Err(unsupported(name, "an image over 160 megapixels"));
    }
    let spp = t.tag_int(ifd, SAMPLES_PER_PIXEL).unwrap_or(1) as usize;
    if !(1..=4).contains(&spp) {
        return Err(unsupported(name, &format!("{spp} samples per pixel")));
    }
    let tag_bits = t.tag_int(ifd, BITS_PER_SAMPLE).unwrap_or(1) as u32;
    if t.tag_int(ifd, SAMPLE_FORMAT).unwrap_or(1) == 3 {
        return Err(unsupported(name, "floating-point image data"));
    }
    if spp > 1 && t.tag_int(ifd, PLANAR_CONFIGURATION).unwrap_or(1) != 1 {
        return Err(unsupported(name, "planar image data"));
    }
    let compression = t.tag_int(ifd, COMPRESSION).unwrap_or(1);
    let predictor = t.tag_int(ifd, PREDICTOR).unwrap_or(1);
    if matches!(compression, 1 | 8 | 32946) && !(1..=16).contains(&tag_bits) {
        return Err(unsupported(name, &format!("{tag_bits}-bit image data")));
    }
    let tiled = ifd.get(TILE_WIDTH).is_some();
    let (tw, th, offs_tag, counts_tag) = if tiled {
        let tw = t.tag_int(ifd, TILE_WIDTH).unwrap_or(0) as usize;
        let th = t.tag_int(ifd, TILE_LENGTH).unwrap_or(0) as usize;
        (tw, th, TILE_OFFSETS, TILE_BYTE_COUNTS)
    } else {
        let rps = t.tag_int(ifd, ROWS_PER_STRIP).map_or(h, |r| r as usize);
        (w, rps.clamp(1, h), STRIP_OFFSETS, STRIP_BYTE_COUNTS)
    };
    if !(1..=MAX_SIDE).contains(&tw) || !(1..=MAX_SIDE).contains(&th) {
        return Err(err(name, "invalid tile size"));
    }
    let (across, down) = (w.div_ceil(tw), h.div_ceil(th));
    let n = across.checked_mul(down).filter(|&n| n <= MAX_TILES).ok_or_else(|| err(name, "too many tiles"))?;
    let padded = n.checked_mul(tw).and_then(|v| v.checked_mul(th)).and_then(|v| v.checked_mul(spp));
    if padded.is_none_or(|v| v > MAX_SAMPLES) {
        return Err(unsupported(name, "an image this large"));
    }
    let offs = t.tag_ints(ifd, offs_tag, MAX_TILES).ok_or_else(|| err(name, "missing strip or tile offsets"))?;
    let counts = t.tag_ints(ifd, counts_tag, MAX_TILES).ok_or_else(|| err(name, "missing strip or tile byte counts"))?;
    if offs.len() < n || counts.len() < n {
        return Err(err(name, "fewer strips or tiles than the image needs"));
    }
    let layout = TileLayout { le: t.le, compression, predictor, tw, spp, bits: tag_bits };
    let tiles = (0..n)
        .into_par_iter()
        .map(|i| {
            let ty = i / across;
            let rows = if tiled { th } else { th.min(h - ty * th) };
            let off = offs.get(i).copied().unwrap_or(0) as usize;
            let len = counts.get(i).copied().unwrap_or(0) as usize;
            if off >= t.b.len() {
                return Err(err(name, "truncated file (strip or tile past the end)"));
            }
            let end = off.saturating_add(len).min(t.b.len());
            let data = t.b.get(off..end).unwrap_or(&[]);
            decode_tile(name, &layout, data, rows)
        })
        .collect::<Result<Vec<_>>>()?;
    let bits = tiles.first().and_then(|t| t.1).unwrap_or(tag_bits);
    let mut out = vec![0u16; w * h * spp];
    for (i, (tile, _)) in tiles.iter().enumerate() {
        let (x0, y0) = ((i % across) * tw, (i / across) * th);
        let cols = tw.min(w - x0) * spp;
        for r in 0..th {
            let y = y0 + r;
            if y >= h {
                break;
            }
            let src = tile.get(r * tw * spp..r * tw * spp + cols);
            let dst = out.get_mut((y * w + x0) * spp..(y * w + x0) * spp + cols);
            if let (Some(src), Some(dst)) = (src, dst) {
                dst.copy_from_slice(src);
            }
        }
    }
    Ok(Samples { w, h, spp, bits, data: out })
}

struct TileLayout {
    le: bool,
    compression: u64,
    predictor: u64,
    tw: usize,
    spp: usize,
    bits: u32,
}

/// One strip or tile: `tw × rows × spp` samples, and the bit depth when the data overrides it.
fn decode_tile(name: &str, l: &TileLayout, data: &[u8], rows: usize) -> Result<(Vec<u16>, Option<u32>)> {
    let need = l.tw * rows * l.spp;
    let (mut s, bits) = match l.compression {
        1 => (unpack(name, data, need, l.bits, l.le)?, None),
        8 | 32946 => {
            let bytes = need.checked_mul(l.bits as usize).ok_or_else(|| err(name, "tile too large"))?.div_ceil(8);
            let raw = match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, bytes) {
                Ok(v) => v,
                Err(e) if e.status == miniz_oxide::inflate::TINFLStatus::HasMoreOutput => e.output,
                Err(_) => return Err(err(name, "corrupt Deflate data")),
            };
            (unpack(name, &raw, need, l.bits, l.le)?, None)
        }
        7 | 34892 => match ljpeg::is_lossless(data) {
            Some(true) => {
                let f = ljpeg::decode(data, need.saturating_mul(2)).map_err(|e| err(name, e))?;
                let mut s = f.samples;
                if s.len() < need {
                    return Err(err(name, "lossless JPEG tile smaller than the tile"));
                }
                s.truncate(need);
                return Ok((s, Some(f.precision)));
            }
            Some(false) => return lossy_tile(name, l, data, rows),
            None => return Err(err(name, "tile is not a JPEG stream")),
        },
        c => return Err(unsupported(name, &format!("compression {c}"))),
    };
    match l.predictor {
        1 => {}
        2 => undo_differencing(&mut s, l.tw * l.spp, l.spp, l.bits),
        p => return Err(unsupported(name, &format!("predictor {p}"))),
    }
    Ok((s, bits))
}

/// A baseline / extended DCT JPEG tile (lossy DNG, or a JPEG preview).
fn lossy_tile(name: &str, l: &TileLayout, data: &[u8], rows: usize) -> Result<(Vec<u16>, Option<u32>)> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).map_err(|e| err(name, &format!("JPEG tile: {e}")))?;
    if img.width() as usize != l.tw || (img.height() as usize) < rows {
        return Err(err(name, "JPEG tile does not match the tile size"));
    }
    let raw = match l.spp {
        1 => img.to_luma8().into_raw(),
        3 => img.to_rgb8().into_raw(),
        n => return Err(unsupported(name, &format!("a {n}-sample JPEG tile"))),
    };
    let need = l.tw * rows * l.spp;
    Ok((raw.iter().take(need).map(|&v| u16::from(v)).collect(), Some(8)))
}

/// `n` samples of `bits` bits: bytes, 16-bit words in the file's byte order, or MSB-first packed
/// bits for other depths (no padding between rows).
fn unpack(name: &str, d: &[u8], n: usize, bits: u32, le: bool) -> Result<Vec<u16>> {
    let need = n.checked_mul(bits as usize).ok_or_else(|| err(name, "tile too large"))?.div_ceil(8);
    if d.len() < need {
        return Err(err(name, "truncated image data"));
    }
    Ok(match bits {
        8 => d.iter().take(n).map(|&v| u16::from(v)).collect(),
        16 => d.as_chunks::<2>().0.iter().take(n).map(|&c| if le { u16::from_le_bytes(c) } else { u16::from_be_bytes(c) }).collect(),
        _ => {
            let mask = (1u32 << bits) - 1;
            let mut out = Vec::with_capacity(n);
            let (mut acc, mut have) = (0u32, 0u32);
            let mut it = d.iter();
            for _ in 0..n {
                while have < bits {
                    acc = (acc << 8) | u32::from(*it.next().unwrap_or(&0));
                    have += 8;
                }
                have -= bits;
                out.push(((acc >> have) & mask) as u16);
                acc &= (1u32 << have) - 1;
            }
            out
        }
    })
}

/// TIFF predictor 2: each sample was stored as the difference from the one to its left.
fn undo_differencing(s: &mut [u16], row: usize, spp: usize, bits: u32) {
    let mask = if bits >= 16 { 0xFFFF } else { (1u16 << bits) - 1 };
    if row == 0 {
        return;
    }
    for r in s.chunks_mut(row) {
        for i in spp..r.len() {
            let prev = r[i - spp];
            r[i] = r[i].wrapping_add(prev) & mask;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Developing

/// Decode a DNG to straight-alpha RGBA8 (opaque).
pub fn decode(name: &str, b: &[u8]) -> Result<Raster> {
    let t = Tiff::new(b).ok_or_else(|| err(name, "not a TIFF file"))?;
    let ifds = directories(name, &t)?;
    let ifd0 = ifds.first().ok_or_else(|| err(name, "no image directory"))?;
    if ifd0.get(DNG_VERSION).is_none() {
        return Err(err(name, "not a DNG file (no DNGVersion)"));
    }
    let orientation = t.tag_int(ifd0, ORIENTATION).unwrap_or(1);
    let raw = ifds
        .iter()
        .filter(|i| t.tag_int(i, NEW_SUBFILE_TYPE).unwrap_or(0) == 0 && matches!(t.tag_int(i, PHOTOMETRIC), Some(PHOTOMETRIC_CFA | PHOTOMETRIC_LINEAR_RAW)))
        .max_by_key(|i| t.tag_int(i, IMAGE_WIDTH).unwrap_or(0).saturating_mul(t.tag_int(i, IMAGE_LENGTH).unwrap_or(0)));
    let developed = match raw {
        Some(raw) => develop(name, &t, raw, ifd0),
        None => Err(unsupported(name, "a file without a raw image")),
    };
    let raster = match developed {
        Ok(r) => r,
        // last resort for raw data we cannot develop: the embedded preview, if any
        Err(e @ MediaError::Unsupported(_)) => preview(name, &t, &ifds).ok_or(e)?,
        Err(e) => return Err(e),
    };
    Ok(orient(raster, orientation))
}

/// The largest RGB preview the file embeds (uncompressed 8-bit RGB or JPEG).
fn preview(name: &str, t: &Tiff, ifds: &[Ifd]) -> Option<Raster> {
    let mut candidates: Vec<&Ifd> = ifds
        .iter()
        .filter(|i| {
            let p = t.tag_int(i, PHOTOMETRIC);
            let c = t.tag_int(i, COMPRESSION).unwrap_or(1);
            p == Some(PHOTOMETRIC_RGB) || (p == Some(PHOTOMETRIC_YCBCR) && c == 7)
        })
        .collect();
    candidates.sort_by_key(|i| std::cmp::Reverse(t.tag_int(i, IMAGE_WIDTH).unwrap_or(0).saturating_mul(t.tag_int(i, IMAGE_LENGTH).unwrap_or(0))));
    candidates.into_iter().find_map(|ifd| {
        let s = read_samples(name, t, ifd).ok()?;
        if s.spp < 3 {
            return None;
        }
        let shift = s.bits.saturating_sub(8);
        let mut rgba = vec![255u8; s.w * s.h * 4];
        for (px, src) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(s.data.chunks_exact(s.spp)) {
            for c in 0..3 {
                px[c] = (src.get(c).copied().unwrap_or(0) >> shift).min(255) as u8;
            }
        }
        Some(Raster { width: s.w as u32, height: s.h as u32, rgba, has_alpha: false })
    })
}

/// The colour (0 red, 1 green, 2 blue) of each cell of a 2×2 Bayer pattern, row by row.
fn bayer(name: &str, t: &Tiff, raw: &Ifd) -> Result<[u8; 4]> {
    let dims = t.tag_ints(raw, CFA_REPEAT_PATTERN_DIM, 2).unwrap_or_else(|| vec![2, 2]);
    if dims != [2, 2] {
        let d = |i: usize| dims.get(i).copied().unwrap_or(0);
        return Err(unsupported(name, &format!("a {}×{} colour filter pattern (only 2×2 Bayer)", d(0), d(1))));
    }
    if t.tag_int(raw, CFA_LAYOUT).unwrap_or(1) != 1 {
        return Err(unsupported(name, "a non-rectangular colour filter layout"));
    }
    let planes = t.tag_ints(raw, CFA_PLANE_COLOR, 4).unwrap_or_else(|| vec![0, 1, 2]);
    let pattern = t.tag_ints(raw, CFA_PATTERN, 4).filter(|p| p.len() == 4).ok_or_else(|| err(name, "missing CFAPattern"))?;
    let mut out = [0u8; 4];
    for (o, &p) in out.iter_mut().zip(&pattern) {
        let c = planes.get(p as usize).copied().filter(|&c| c <= 2).ok_or_else(|| unsupported(name, "a colour filter with non-RGB colours"))?;
        *o = c as u8;
    }
    let count = |c: u8| out.iter().filter(|&&v| v == c).count();
    let greens_diagonal = (out[0] == 1 && out[3] == 1) || (out[1] == 1 && out[2] == 1);
    if count(0) != 1 || count(1) != 2 || count(2) != 1 || !greens_diagonal {
        return Err(unsupported(name, "a non-Bayer colour filter pattern"));
    }
    Ok(out)
}

/// Black and white levels of the active area.
struct Levels {
    black: Vec<f32>,
    rows: usize,
    cols: usize,
    spp: usize,
    dh: Vec<f32>,
    dv: Vec<f32>,
    white: Vec<f32>,
}

impl Levels {
    fn read(t: &Tiff, raw: &Ifd, spp: usize, w: usize, h: usize, bits: u32) -> Levels {
        let dims = t.tag_ints(raw, BLACK_LEVEL_REPEAT_DIM, 2).filter(|d| d.len() == 2 && d.iter().all(|&v| (1..=64).contains(&v)));
        let (rows, cols) = dims.map_or((1, 1), |d| (d[0] as usize, d[1] as usize));
        let n = rows * cols * spp;
        let black = match t.tag_nums(raw, BLACK_LEVEL, n) {
            Some(v) if v.len() == n => v.iter().map(|&b| b as f32).collect(),
            Some(v) if v.len() == 1 => vec![v[0] as f32; n],
            _ => vec![0.0; n],
        };
        let delta = |tag, len: usize| t.tag_nums(raw, tag, len).filter(|v| v.len() == len).map(|v| v.iter().map(|&d| d as f32).collect()).unwrap_or_default();
        let default_white = ((1u32 << bits.clamp(1, 16)) - 1) as f32;
        let white = match t.tag_nums(raw, WHITE_LEVEL, spp) {
            Some(v) if v.len() == spp => v.iter().map(|&w| w as f32).collect(),
            Some(v) if v.len() == 1 => vec![v[0] as f32; spp],
            _ => vec![default_white; spp],
        };
        Levels { black, rows, cols, spp, dh: delta(BLACK_LEVEL_DELTA_H, w), dv: delta(BLACK_LEVEL_DELTA_V, h), white }
    }

    fn black(&self, x: usize, y: usize, c: usize) -> f32 {
        let i = ((y % self.rows) * self.cols + x % self.cols) * self.spp + c;
        self.black.get(i).copied().unwrap_or(0.0) + self.dh.get(x).copied().unwrap_or(0.0) + self.dv.get(y).copied().unwrap_or(0.0)
    }
}

fn develop(name: &str, t: &Tiff, raw: &Ifd, ifd0: &Ifd) -> Result<Raster> {
    let cfa = t.tag_int(raw, PHOTOMETRIC) == Some(PHOTOMETRIC_CFA);
    // refuse unsupported patterns before decoding anything
    let pattern = if cfa { Some(bayer(name, t, raw)?) } else { None };
    let s = read_samples(name, t, raw)?;
    let spp = s.spp;
    if (cfa && spp != 1) || (!cfa && spp != 1 && spp != 3) {
        return Err(unsupported(name, &format!("raw data with {spp} samples per pixel")));
    }
    // ActiveArea: top, left, bottom, right
    let area = t.tag_nums(raw, ACTIVE_AREA, 4).filter(|a| a.len() == 4).map(|a| indices4(&a));
    let (top, left, bottom, right) = match area {
        Some([t0, l0, b0, r0]) if t0 < b0 && l0 < r0 && b0 <= s.h && r0 <= s.w => (t0, l0, b0, r0),
        _ => (0, 0, s.h, s.w),
    };
    let (w, h) = (right - left, bottom - top);
    let lin: Option<Vec<u16>> =
        t.tag_ints(raw, LINEARIZATION_TABLE, 65_536).filter(|v| !v.is_empty()).map(|v| v.iter().map(|&x| x.min(65_535) as u16).collect());
    let levels = Levels::read(t, raw, spp, w, h, s.bits);
    let mut plane = vec![0f32; w * h * spp];
    plane.par_chunks_mut(w * spp).enumerate().for_each(|(y, row)| {
        for (i, v) in row.iter_mut().enumerate() {
            let (x, c) = (i / spp, i % spp);
            let mut r = s.data.get(((y + top) * s.w + x + left) * spp + c).copied().unwrap_or(0);
            if let Some(lin) = &lin {
                r = lin.get(usize::from(r)).or(lin.last()).copied().unwrap_or(r);
            }
            let black = levels.black(x, y, c);
            let range = levels.white.get(c).copied().unwrap_or(1.0) - black;
            *v = if range > 0.0 { ((f32::from(r) - black) / range).max(0.0) } else { 0.0 };
        }
    });
    drop(s);
    // the CFA pattern is anchored at the stored image's origin
    let pattern = pattern.map(|p| {
        let mut q = [0u8; 4];
        for (i, o) in q.iter_mut().enumerate() {
            let (yy, xx) = ((i / 2 + top) & 1, (i % 2 + left) & 1);
            *o = p[yy * 2 + xx];
        }
        q
    });
    let (cx, cy, cw, ch) = crop(t, raw, w, h);
    let colour = Colour::read(t, raw, ifd0);
    let lut = srgb_lut();
    let mut rgba = vec![0u8; cw * ch * 4];
    rgba.par_chunks_mut(cw * 4).enumerate().for_each(|(oy, row)| {
        let y = oy + cy;
        for (ox, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = ox + cx;
            let cam = match pattern {
                Some(p) => mhc(&plane, w, h, p, x, y),
                None if spp == 3 => {
                    let at = (y * w + x) * 3;
                    let g = |c: usize| plane.get(at + c).copied().unwrap_or(0.0);
                    [g(0), g(1), g(2)]
                }
                None => {
                    let g = plane.get(y * w + x).copied().unwrap_or(0.0);
                    [g, g, g]
                }
            };
            let rgb = colour.apply(cam);
            px.copy_from_slice(&[encode(&lut, rgb[0]), encode(&lut, rgb[1]), encode(&lut, rgb[2]), 255]);
        }
    });
    Ok(Raster { width: cw as u32, height: ch as u32, rgba, has_alpha: false })
}

/// Four non-negative numbers as indices.
fn indices4(v: &[f64]) -> [usize; 4] {
    let g = |i: usize| v.get(i).copied().unwrap_or(0.0).max(0.0) as usize;
    [g(0), g(1), g(2), g(3)]
}

/// `DefaultCropOrigin` / `DefaultCropSize` within the active area: x, y, width, height.
fn crop(t: &Tiff, raw: &Ifd, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let origin = t.tag_nums(raw, DEFAULT_CROP_ORIGIN, 2).filter(|v| v.len() == 2);
    let size = t.tag_nums(raw, DEFAULT_CROP_SIZE, 2).filter(|v| v.len() == 2);
    match (origin, size) {
        (Some(o), Some(s)) => {
            let ox = (o[0].max(0.0) as usize).min(w - 1);
            let oy = (o[1].max(0.0) as usize).min(h - 1);
            let cw = (s[0].max(1.0) as usize).clamp(1, w - ox);
            let ch = (s[1].max(1.0) as usize).clamp(1, h - oy);
            (ox, oy, cw, ch)
        }
        _ => (0, 0, w, h),
    }
}

/// Reflect an index into `0..n` keeping its parity (so CFA colours are preserved).
fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let mut i = i;
    if i < 0 {
        i = -i;
    }
    if i >= n {
        i = 2 * (n - 1) - i;
    }
    i.clamp(0, (n - 1).max(0)) as usize
}

/// Malvar–He–Cutler demosaicing of the pixel at (`x`, `y`): camera RGB.
fn mhc(plane: &[f32], w: usize, h: usize, pattern: [u8; 4], x: usize, y: usize) -> [f32; 3] {
    let at = |dx: isize, dy: isize| {
        let (xx, yy) = (reflect(x as isize + dx, w), reflect(y as isize + dy, h));
        plane.get(yy * w + xx).copied().unwrap_or(0.0)
    };
    let colour = pattern[(y & 1) * 2 + (x & 1)];
    let c = at(0, 0);
    let cross1 = at(-1, 0) + at(1, 0) + at(0, -1) + at(0, 1);
    let cross2 = at(-2, 0) + at(2, 0) + at(0, -2) + at(0, 2);
    let diag = at(-1, -1) + at(1, -1) + at(-1, 1) + at(1, 1);
    let out = if colour == 1 {
        let (lr, ud) = (at(-1, 0) + at(1, 0), at(0, -1) + at(0, 1));
        let (lr2, ud2) = (at(-2, 0) + at(2, 0), at(0, -2) + at(0, 2));
        // the colour of the horizontal neighbours, interpolated from them; the other one vertically
        let along_row = (5.0 * c + 4.0 * lr - lr2 - diag + 0.5 * ud2) / 8.0;
        let along_col = (5.0 * c + 4.0 * ud - ud2 - diag + 0.5 * lr2) / 8.0;
        let row_colour = pattern[(y & 1) * 2 + ((x + 1) & 1)];
        if row_colour == 0 { [along_row, c, along_col] } else { [along_col, c, along_row] }
    } else {
        let g = (4.0 * c + 2.0 * cross1 - cross2) / 8.0;
        let other = (6.0 * c + 2.0 * diag - 1.5 * cross2) / 8.0;
        if colour == 0 { [c, g, other] } else { [other, g, c] }
    };
    out.map(|v| v.max(0.0))
}

// ---------------------------------------------------------------------------------------------
// Colour

type M3 = [[f64; 3]; 3];

const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
/// XYZ (D50) to linear sRGB (Bradford-adapted to D65), and its inverse.
const XYZ_D50_TO_SRGB: M3 = [[3.1338561, -1.6168667, -0.4906146], [-0.9787684, 1.9161415, 0.0334540], [0.0719453, -0.2289914, 1.4052427]];
const SRGB_TO_XYZ_D50: M3 = [[0.4360747, 0.3850649, 0.1430804], [0.2225045, 0.7168786, 0.0606169], [0.0139322, 0.0971045, 0.7141733]];
const BRADFORD: M3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
const D50_XYZ: [f64; 3] = [0.96422, 1.0, 0.82521];
const D50_XY: [f64; 2] = [0.34567, 0.35850];
const D65_XY: [f64; 2] = [0.31271, 0.32902];

fn mul(a: M3, b: M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn mulv(a: M3, v: [f64; 3]) -> [f64; 3] {
    a.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2])
}

fn diag(v: [f64; 3]) -> M3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

fn lerp(a: M3, b: M3, g: f64) -> M3 {
    let mut o = a;
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][j] * g + b[i][j] * (1.0 - g);
        }
    }
    o
}

fn inv(m: M3) -> Option<M3> {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let k = 1.0 / det;
    Some([
        [(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k],
        [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k],
        [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k],
    ])
}

/// Bradford chromatic adaptation from white `src` to white `dst` (XYZ).
fn bradford(src: [f64; 3], dst: [f64; 3]) -> Option<M3> {
    let (s, d) = (mulv(BRADFORD, src), mulv(BRADFORD, dst));
    if s.iter().any(|&v| v.abs() < 1e-9) {
        return None;
    }
    Some(mul(mul(inv(BRADFORD)?, diag([d[0] / s[0], d[1] / s[1], d[2] / s[2]])), BRADFORD))
}

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let y = xy[1].max(1e-6);
    [xy[0] / y, 1.0, (1.0 - xy[0] - xy[1]) / y]
}

/// Correlated colour temperature of a white (McCamy's cubic approximation).
fn cct(xy: [f64; 2]) -> f64 {
    let d = 0.1858 - xy[1];
    if d.abs() < 1e-9 {
        return 5000.0;
    }
    let n = (xy[0] - 0.3320) / d;
    let t = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
    if t.is_finite() { t.clamp(1000.0, 50_000.0) } else { 5000.0 }
}

/// Colour temperature of an EXIF light source (`CalibrationIlluminant`).
fn illuminant_temperature(code: u64) -> Option<f64> {
    Some(match code {
        1 | 4 | 9 => 5500.0,
        2 | 14 => 4150.0,
        3 => 2850.0,
        10 => 6500.0,
        11 => 7500.0,
        12 => 6430.0,
        13 => 5000.0,
        15 => 3450.0,
        16 => 2940.0,
        17 => 2856.0,
        18 => 4874.0,
        19 => 6774.0,
        20 => 5503.0,
        21 => 6504.0,
        22 => 7504.0,
        23 => 5003.0,
        24 => 3200.0,
        _ => return None,
    })
}

/// How much of calibration 1 to use for a white of temperature `t`: linear in inverse temperature
/// between the two calibrations, clamped.
fn interpolation_weight(t: f64, t1: f64, t2: f64) -> f64 {
    let (i, i1, i2) = (1.0 / t, 1.0 / t1, 1.0 / t2);
    if (i1 - i2).abs() < 1e-12 {
        return 1.0;
    }
    ((i - i2) / (i1 - i2)).clamp(0.0, 1.0)
}

/// Camera to linear sRGB.
struct Colour {
    /// Applied to white-balanced camera values.
    m: [[f32; 3]; 3],
    inv_neutral: [f32; 3],
    /// Where the first white-balanced channel saturates.
    clip: f32,
}

struct Calibration {
    cm: [Option<M3>; 2],
    cc: [Option<M3>; 2],
    fm: [Option<M3>; 2],
    temps: [Option<f64>; 2],
    ab: M3,
}

impl Calibration {
    /// Weight of calibration 1 for a white `xy`. Without two known illuminants, calibration 2
    /// (usually D65) is used when it exists.
    fn weight(&self, xy: [f64; 2]) -> f64 {
        match (self.temps, self.cm[1].is_some()) {
            ([Some(t1), Some(t2)], true) => interpolation_weight(cct(xy), t1, t2),
            (_, true) => 0.0,
            _ => 1.0,
        }
    }

    fn pick(pair: [Option<M3>; 2], g: f64) -> Option<M3> {
        match pair {
            [Some(a), Some(b)] => Some(lerp(a, b, g)),
            [a, b] => a.or(b),
        }
    }

    /// XYZ to camera for weight `g`.
    fn xyz_to_camera(&self, g: f64) -> Option<M3> {
        let cc = Self::pick(self.cc, g).unwrap_or(IDENTITY);
        Some(mul(mul(self.ab, cc), Self::pick(self.cm, g)?))
    }

    /// Camera to XYZ (D50) for weight `g`, mapping `neutral` to the D50 white.
    fn camera_to_xyz(&self, g: f64, neutral: [f64; 3]) -> Option<M3> {
        let abcc = mul(self.ab, Self::pick(self.cc, g).unwrap_or(IDENTITY));
        if let Some(fm) = Self::pick(self.fm, g) {
            let inv_abcc = inv(abcc)?;
            let r = mulv(inv_abcc, neutral);
            if r.iter().any(|&v| v <= 1e-9) {
                return None;
            }
            return Some(mul(mul(fm, diag([1.0 / r[0], 1.0 / r[1], 1.0 / r[2]])), inv_abcc));
        }
        let c2x = inv(self.xyz_to_camera(g)?)?;
        Some(mul(bradford(mulv(c2x, neutral), D50_XYZ)?, c2x))
    }
}

impl Colour {
    fn read(t: &Tiff, raw: &Ifd, ifd0: &Ifd) -> Colour {
        let nums = |tag, n: usize| t.tag_nums(raw, tag, n).or_else(|| t.tag_nums(ifd0, tag, n)).filter(|v| v.len() == n);
        let mat = |tag| nums(tag, 9).map(|v| [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]);
        let int = |tag| t.tag_int(raw, tag).or_else(|| t.tag_int(ifd0, tag));
        let cal = Calibration {
            cm: [mat(COLOR_MATRIX_1), mat(COLOR_MATRIX_2)],
            cc: [mat(CAMERA_CALIBRATION_1), mat(CAMERA_CALIBRATION_2)],
            fm: [mat(FORWARD_MATRIX_1), mat(FORWARD_MATRIX_2)],
            temps: [int(CALIBRATION_ILLUMINANT_1).and_then(illuminant_temperature), int(CALIBRATION_ILLUMINANT_2).and_then(illuminant_temperature)],
            ab: nums(ANALOG_BALANCE, 3).filter(|v| v.iter().all(|&a| a > 0.0)).map_or(IDENTITY, |v| diag([v[0], v[1], v[2]])),
        };
        let positive = |v: [f64; 3]| {
            let max = v.iter().fold(0.0f64, |a, &b| a.max(b));
            (v.iter().all(|&c| c.is_finite() && c > 1e-6) && max > 0.0).then(|| v.map(|c| c / max))
        };
        let as_shot = nums(AS_SHOT_NEUTRAL, 3).and_then(|v| positive([v[0], v[1], v[2]]));
        let (neutral, g) = match as_shot {
            Some(n) => {
                // find the white's temperature: the calibration depends on it and it on the calibration
                let mut xy = D50_XY;
                let mut g = cal.weight(xy);
                for _ in 0..8 {
                    g = cal.weight(xy);
                    let Some(c2x) = cal.xyz_to_camera(g).and_then(inv) else { break };
                    let xyz = mulv(c2x, n);
                    let sum = xyz[0] + xyz[1] + xyz[2];
                    if !(sum.is_finite() && sum > 1e-9) {
                        break;
                    }
                    xy = [xyz[0] / sum, xyz[1] / sum];
                }
                (n, g)
            }
            None => {
                let xy = nums(AS_SHOT_WHITE_XY, 2).filter(|v| v[0] > 0.0 && v[1] > 0.0 && v[0] + v[1] < 1.0).map_or(D65_XY, |v| [v[0], v[1]]);
                let g = cal.weight(xy);
                let n = cal.xyz_to_camera(g).and_then(|m| positive(mulv(m, xy_to_xyz(xy))));
                (n.unwrap_or([1.0; 3]), g)
            }
        };
        let c2x = cal.camera_to_xyz(g, neutral).unwrap_or_else(|| mul(SRGB_TO_XYZ_D50, diag([1.0 / neutral[0], 1.0 / neutral[1], 1.0 / neutral[2]])));
        let exposure = nums(BASELINE_EXPOSURE, 1).map_or(0.0, |v| v[0].clamp(-10.0, 10.0)).exp2();
        let mut m = mul(mul(XYZ_D50_TO_SRGB, c2x), diag(neutral)).map(|r| r.map(|v| v * exposure));
        if m.iter().flatten().any(|v| !v.is_finite()) {
            m = diag([exposure; 3]);
        }
        let max = neutral.iter().fold(0.0f64, |a, &b| a.max(b));
        Colour { m: m.map(|r| r.map(|v| v as f32)), inv_neutral: neutral.map(|v| (1.0 / v) as f32), clip: (1.0 / max) as f32 }
    }

    fn apply(&self, cam: [f32; 3]) -> [f32; 3] {
        let mut wb = [0.0f32; 3];
        for c in 0..3 {
            wb[c] = (cam[c] * self.inv_neutral[c]).clamp(0.0, self.clip);
        }
        self.m.map(|r| r[0] * wb[0] + r[1] * wb[1] + r[2] * wb[2])
    }
}

const LUT_SIZE: usize = 4096;

/// Linear 0–1 to 8-bit sRGB.
fn srgb_lut() -> Vec<u8> {
    (0..LUT_SIZE)
        .map(|i| {
            let v = i as f64 / (LUT_SIZE - 1) as f64;
            let e = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            (e * 255.0 + 0.5).clamp(0.0, 255.0) as u8
        })
        .collect()
}

fn encode(lut: &[u8], v: f32) -> u8 {
    let i = (v.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32 + 0.5) as usize;
    lut.get(i).copied().unwrap_or(255)
}

/// Apply a TIFF orientation (1–8) so the picture displays upright.
fn orient(r: Raster, orientation: u64) -> Raster {
    if !(2..=8).contains(&orientation) {
        return r;
    }
    let (w, h) = (r.width as usize, r.height as usize);
    let swap = orientation >= 5;
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; r.rgba.len()];
    for oy in 0..oh {
        for ox in 0..ow {
            let (sx, sy) = match orientation {
                2 => (w - 1 - ox, oy),
                3 => (w - 1 - ox, h - 1 - oy),
                4 => (ox, h - 1 - oy),
                5 => (oy, ox),
                6 => (oy, h - 1 - ox),
                7 => (w - 1 - oy, h - 1 - ox),
                _ => (w - 1 - oy, ox),
            };
            let (s, d) = ((sy * w + sx) * 4, (oy * ow + ox) * 4);
            if let (Some(src), Some(dst)) = (r.rgba.get(s..s + 4), out.get_mut(d..d + 4)) {
                dst.copy_from_slice(src);
            }
        }
    }
    Raster { width: ow as u32, height: oh as u32, rgba: out, has_alpha: r.has_alpha }
}

#[cfg(test)]
mod tests;
