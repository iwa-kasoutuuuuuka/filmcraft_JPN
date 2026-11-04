//! DNGs built here, in code: a tiny TIFF writer, the three compressions (including a lossless JPEG
//! encoder written from T.81 for the tests) and known colour patches.

use super::*;

// --- TIFF writer -------------------------------------------------------------------------------

#[derive(Clone)]
enum V {
    B(Vec<u8>),
    S(Vec<u16>),
    L(Vec<u32>),
    R(Vec<(u32, u32)>),
    Sr(Vec<(i32, i32)>),
}

impl V {
    fn typ(&self) -> u16 {
        match self {
            V::B(_) => 1,
            V::S(_) => 3,
            V::L(_) => 4,
            V::R(_) => 5,
            V::Sr(_) => 10,
        }
    }
    fn count(&self) -> u32 {
        (match self {
            V::B(v) => v.len(),
            V::S(v) => v.len(),
            V::L(v) => v.len(),
            V::R(v) => v.len(),
            V::Sr(v) => v.len(),
        }) as u32
    }
    fn bytes(&self, le: bool) -> Vec<u8> {
        let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        match self {
            V::B(v) => v.clone(),
            V::S(v) => v.iter().flat_map(|&x| w16(x)).collect(),
            V::L(v) => v.iter().flat_map(|&x| w32(x)).collect(),
            V::R(v) => v.iter().flat_map(|&(n, d)| [w32(n), w32(d)].concat()).collect(),
            V::Sr(v) => v.iter().flat_map(|&(n, d)| [w32(n as u32), w32(d as u32)].concat()).collect(),
        }
    }
}

struct W {
    b: Vec<u8>,
    le: bool,
}

impl W {
    fn new(le: bool) -> W {
        let mut b = if le { b"II*\0".to_vec() } else { b"MM\0*".to_vec() };
        b.extend([0; 4]);
        W { b, le }
    }
    fn align(&mut self) {
        if self.b.len() % 2 == 1 {
            self.b.push(0);
        }
    }
    fn blob(&mut self, d: &[u8]) -> u32 {
        self.align();
        let o = self.b.len() as u32;
        self.b.extend_from_slice(d);
        o
    }
    fn u32b(&self, v: u32) -> [u8; 4] {
        if self.le { v.to_le_bytes() } else { v.to_be_bytes() }
    }
    fn ifd(&mut self, mut tags: Vec<(u16, V)>, next: u32) -> u32 {
        tags.sort_by_key(|t| t.0);
        let mut entries = vec![];
        for (tag, v) in &tags {
            let data = v.bytes(self.le);
            let field = if data.len() <= 4 {
                let mut f = data.clone();
                f.resize(4, 0);
                [f[0], f[1], f[2], f[3]]
            } else {
                let o = self.blob(&data);
                self.u32b(o)
            };
            entries.push((*tag, v.typ(), v.count(), field));
        }
        self.align();
        let off = self.b.len() as u32;
        let n = entries.len() as u16;
        self.b.extend(if self.le { n.to_le_bytes() } else { n.to_be_bytes() });
        for (tag, typ, count, field) in entries {
            for x in [tag, typ] {
                self.b.extend(if self.le { x.to_le_bytes() } else { x.to_be_bytes() });
            }
            let c = self.u32b(count);
            self.b.extend(c);
            self.b.extend(field);
        }
        let nx = self.u32b(next);
        self.b.extend(nx);
        off
    }
    fn first(&mut self, off: u32) {
        let o = self.u32b(off);
        self.b[4..8].copy_from_slice(&o);
    }
}

// --- lossless JPEG encoder (T.81 process 14), all 17 categories coded in 5 bits -------------

struct BitW {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl BitW {
    fn put(&mut self, v: u32, len: u32) {
        for i in (0..len).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                let b = self.acc as u8;
                self.out.push(b);
                if b == 0xFF {
                    self.out.push(0);
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }
    fn flush(&mut self) {
        while self.n != 0 {
            self.put(1, 1);
        }
    }
}

fn predict(sel: u8, ra: i32, rb: i32, rc: i32) -> i32 {
    match sel {
        1 => ra,
        2 => rb,
        3 => rc,
        4 => ra + rb - rc,
        5 => ra + ((rb - rc) >> 1),
        6 => rb + ((ra - rc) >> 1),
        _ => (ra + rb) >> 1,
    }
}

fn ljpeg(s: &[u16], w: usize, h: usize, nc: usize, precision: u8, sel: u8, restart: usize) -> Vec<u8> {
    let mut o = vec![0xFF, 0xD8];
    let seg = |o: &mut Vec<u8>, m: u8, body: &[u8]| {
        o.extend([0xFF, m]);
        o.extend(((body.len() + 2) as u16).to_be_bytes());
        o.extend_from_slice(body);
    };
    let mut sof = vec![precision];
    sof.extend((h as u16).to_be_bytes());
    sof.extend((w as u16).to_be_bytes());
    sof.push(nc as u8);
    for i in 0..nc {
        sof.extend([i as u8 + 1, 0x11, 0]);
    }
    seg(&mut o, 0xC3, &sof);
    let mut dht = vec![0x00];
    let mut counts = [0u8; 16];
    counts[4] = 17;
    dht.extend(counts);
    dht.extend(0..=16u8);
    seg(&mut o, 0xC4, &dht);
    if restart > 0 {
        seg(&mut o, 0xDD, &(restart as u16).to_be_bytes());
    }
    let mut sos = vec![nc as u8];
    for i in 0..nc {
        sos.extend([i as u8 + 1, 0x00]);
    }
    sos.extend([sel, 0, 0]);
    seg(&mut o, 0xDA, &sos);
    let mut bw = BitW { out: o, acc: 0, n: 0 };
    let init = 1i32 << (precision - 1);
    let row = w * nc;
    let (mut left, mut reset, mut fresh, mut rst) = (restart, true, true, 0u8);
    for y in 0..h {
        if y > 0 {
            fresh = false;
        }
        for x in 0..w {
            if restart > 0 {
                if left == 0 {
                    bw.flush();
                    bw.out.extend([0xFF, 0xD0 + rst % 8]);
                    rst = rst.wrapping_add(1);
                    left = restart;
                    reset = true;
                    fresh = true;
                }
                left -= 1;
            }
            for c in 0..nc {
                let i = (y * w + x) * nc + c;
                let px = |j: usize| i32::from(s[j]);
                let pred = if reset {
                    init
                } else if fresh {
                    px(i - nc)
                } else if x == 0 {
                    px(i - row)
                } else {
                    predict(sel, px(i - nc), px(i - row), px(i - row - nc))
                };
                let mut d = (i32::from(s[i]) - pred) & 0xFFFF;
                if d >= 32768 {
                    d -= 65536;
                }
                if d == -32768 {
                    bw.put(16, 5);
                    continue;
                }
                let cat = 32 - d.unsigned_abs().leading_zeros();
                bw.put(cat, 5);
                if cat > 0 {
                    let extra = if d < 0 { d + (1 << cat) - 1 } else { d };
                    bw.put(extra as u32, cat);
                }
            }
            reset = false;
        }
    }
    bw.flush();
    let mut o = bw.out;
    o.extend([0xFF, 0xD9]);
    o
}

// --- DNG builder -------------------------------------------------------------------------------

#[derive(Clone)]
struct Raw {
    w: usize,
    h: usize,
    spp: usize,
    samples: Vec<u16>,
    bits: u16,
    compression: u16,
    predictor: u16,
    jpeg_predictor: u8,
    restart: usize,
    tile: Option<(usize, usize)>,
    rps: usize,
    photometric: u16,
    tags: Vec<(u16, V)>,
}

impl Raw {
    fn cfa(w: usize, h: usize, samples: Vec<u16>) -> Raw {
        Raw {
            w,
            h,
            spp: 1,
            samples,
            bits: 16,
            compression: 1,
            predictor: 1,
            jpeg_predictor: 1,
            restart: 0,
            tile: None,
            rps: h,
            photometric: 32803,
            tags: vec![(CFA_REPEAT_PATTERN_DIM, V::S(vec![2, 2])), (CFA_PATTERN, V::B(vec![0, 1, 1, 2]))],
        }
    }
    fn tag(mut self, tag: u16, v: V) -> Raw {
        self.tags.retain(|t| t.0 != tag);
        self.tags.push((tag, v));
        self
    }
}

fn pack(s: &[u16], bits: u16, le: bool) -> Vec<u8> {
    match bits {
        8 => s.iter().map(|&v| v as u8).collect(),
        16 => s.iter().flat_map(|&v| if le { v.to_le_bytes() } else { v.to_be_bytes() }).collect(),
        _ => {
            let mut bw = BitW { out: vec![], acc: 0, n: 0 };
            for &v in s {
                // packing has no byte stuffing: write the bits directly
                for i in (0..u32::from(bits)).rev() {
                    bw.acc = (bw.acc << 1) | ((u32::from(v) >> i) & 1);
                    bw.n += 1;
                    if bw.n == 8 {
                        bw.out.push(bw.acc as u8);
                        bw.acc = 0;
                        bw.n = 0;
                    }
                }
            }
            if bw.n > 0 {
                bw.out.push((bw.acc << (8 - bw.n)) as u8);
            }
            bw.out
        }
    }
}

fn encode_tile(r: &Raw, s: &[u16], tw: usize, rows: usize, le: bool) -> Vec<u8> {
    match r.compression {
        1 => pack(s, r.bits, le),
        8 => {
            let mut s = s.to_vec();
            if r.predictor == 2 {
                let row = tw * r.spp;
                let mask = if r.bits >= 16 { 0xFFFF } else { (1u16 << r.bits) - 1 };
                for line in s.chunks_mut(row) {
                    for i in (r.spp..line.len()).rev() {
                        line[i] = line[i].wrapping_sub(line[i - r.spp]) & mask;
                    }
                }
            }
            miniz_oxide::deflate::compress_to_vec_zlib(&pack(&s, r.bits, le), 6)
        }
        7 => {
            let (jw, nc) = if r.spp == 1 && tw.is_multiple_of(2) { (tw / 2, 2) } else { (tw, r.spp) };
            ljpeg(s, jw, rows, nc, r.bits as u8, r.jpeg_predictor, r.restart)
        }
        _ => s.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

/// A DNG holding `r`: in IFD0 itself, or (`sub`) in a SubIFD of an IFD0 that only has metadata.
fn dng(r: &Raw, ifd0_tags: Vec<(u16, V)>, sub: bool, le: bool) -> Vec<u8> {
    let mut wr = W::new(le);
    let tiled = r.tile.is_some();
    let (tw, th) = r.tile.unwrap_or((r.w, r.rps));
    let (across, down) = (r.w.div_ceil(tw), r.h.div_ceil(th));
    let (mut offs, mut counts) = (vec![], vec![]);
    for ty in 0..down {
        for tx in 0..across {
            let rows = if tiled { th } else { th.min(r.h - ty * th) };
            let mut s = vec![0u16; tw * rows * r.spp];
            for y in 0..rows {
                for x in 0..tw {
                    let (ix, iy) = (tx * tw + x, ty * th + y);
                    if ix < r.w && iy < r.h {
                        for c in 0..r.spp {
                            s[(y * tw + x) * r.spp + c] = r.samples[(iy * r.w + ix) * r.spp + c];
                        }
                    }
                }
            }
            let d = encode_tile(r, &s, tw, rows, le);
            offs.push(wr.blob(&d));
            counts.push(d.len() as u32);
        }
    }
    let mut raw_tags = vec![
        (NEW_SUBFILE_TYPE, V::L(vec![0])),
        (IMAGE_WIDTH, V::L(vec![r.w as u32])),
        (IMAGE_LENGTH, V::L(vec![r.h as u32])),
        (BITS_PER_SAMPLE, V::S(vec![r.bits; r.spp])),
        (COMPRESSION, V::S(vec![r.compression])),
        (PHOTOMETRIC, V::S(vec![r.photometric])),
        (SAMPLES_PER_PIXEL, V::S(vec![r.spp as u16])),
        (PLANAR_CONFIGURATION, V::S(vec![1])),
    ];
    if r.predictor != 1 {
        raw_tags.push((PREDICTOR, V::S(vec![r.predictor])));
    }
    if tiled {
        raw_tags.extend([
            (TILE_WIDTH, V::L(vec![tw as u32])),
            (TILE_LENGTH, V::L(vec![th as u32])),
            (TILE_OFFSETS, V::L(offs)),
            (TILE_BYTE_COUNTS, V::L(counts)),
        ]);
    } else {
        raw_tags.extend([(STRIP_OFFSETS, V::L(offs)), (ROWS_PER_STRIP, V::L(vec![r.rps as u32])), (STRIP_BYTE_COUNTS, V::L(counts))]);
    }
    raw_tags.extend(r.tags.iter().cloned());
    let mut ifd0 = vec![(DNG_VERSION, V::B(vec![1, 4, 0, 0]))];
    ifd0.extend(ifd0_tags);
    let off0 = if sub {
        let raw_off = wr.ifd(raw_tags, 0);
        ifd0.extend([(NEW_SUBFILE_TYPE, V::L(vec![1])), (SUB_IFDS, V::L(vec![raw_off]))]);
        wr.ifd(ifd0, 0)
    } else {
        ifd0.extend(raw_tags);
        wr.ifd(ifd0, 0)
    };
    wr.first(off0);
    wr.b
}

// --- colour helpers ----------------------------------------------------------------------------

fn srat(m: &M3) -> V {
    V::Sr(m.iter().flatten().map(|&v| ((v * 1_000_000.0).round() as i32, 1_000_000)).collect())
}

fn rat(v: &[f64]) -> V {
    V::R(v.iter().map(|&x| ((x * 1_000_000.0).round() as u32, 1_000_000)).collect())
}

/// Camera = linear sRGB: XYZ (D50) → camera is the XYZ → sRGB matrix.
fn srgb_camera() -> Vec<(u16, V)> {
    vec![(COLOR_MATRIX_1, srat(&XYZ_D50_TO_SRGB)), (CALIBRATION_ILLUMINANT_1, V::S(vec![23])), (AS_SHOT_NEUTRAL, rat(&[1.0, 1.0, 1.0]))]
}

fn srgb8(v: f64) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let e = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (e * 255.0).round() as u8
}

const BLACK: f64 = 256.0;
const WHITE: f64 = 16383.0;
const RGGB: [usize; 4] = [0, 1, 1, 2];

/// A mosaic of `colour(x, y)` (linear camera RGB, 0–1) through the 2×2 pattern.
fn mosaic(w: usize, h: usize, pattern: [usize; 4], black: f64, white: f64, colour: impl Fn(usize, usize) -> [f64; 3]) -> Vec<u16> {
    let mut v = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let c = pattern[(y & 1) * 2 + (x & 1)];
            v[y * w + x] = (black + colour(x, y)[c] * (white - black)).round() as u16;
        }
    }
    v
}

const PATCHES: [[f64; 3]; 4] = [[0.8, 0.2, 0.1], [0.1, 0.6, 0.2], [0.2, 0.3, 0.9], [0.5, 0.5, 0.5]];

fn quadrants(x: usize, y: usize) -> [f64; 3] {
    PATCHES[(y / 8) * 2 + x / 8]
}

fn patch_raw(pattern: [usize; 4]) -> Raw {
    let mut r = Raw::cfa(16, 16, mosaic(16, 16, pattern, BLACK, WHITE, quadrants));
    r = r.tag(BLACK_LEVEL, V::L(vec![BLACK as u32])).tag(WHITE_LEVEL, V::L(vec![WHITE as u32]));
    r.tag(CFA_PATTERN, V::B(pattern.iter().map(|&c| c as u8).collect()))
}

fn px(r: &Raster, x: usize, y: usize) -> [u8; 4] {
    let i = (y * r.width as usize + x) * 4;
    [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2], r.rgba[i + 3]]
}

fn assert_near(got: [u8; 4], want: [f64; 3], tol: i32, what: &str) {
    for c in 0..3 {
        let e = srgb8(want[c]);
        assert!((i32::from(got[c]) - i32::from(e)).abs() <= tol, "{what}: channel {c}: got {got:?}, want {e} ({want:?})");
    }
    assert_eq!(got[3], 255);
}

fn check_patches(r: &Raster, what: &str) {
    assert_eq!((r.width, r.height), (16, 16));
    for (i, p) in PATCHES.iter().enumerate() {
        let (x, y) = ((i % 2) * 8 + 4, (i / 2) * 8 + 4);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_near(px(r, x + dx, y + dy), *p, 2, &format!("{what} patch {i} at ({}, {})", x + dx, y + dy));
        }
    }
}

fn develop_bytes(b: &[u8]) -> Result<Raster> {
    assert!(sniff(b));
    decode("test.dng", b)
}

// --- tests -------------------------------------------------------------------------------------

#[test]
fn uncompressed_16_bit_bayer_patches() {
    let r = develop_bytes(&dng(&patch_raw(RGGB), srgb_camera(), false, true)).unwrap();
    check_patches(&r, "RGGB");
}

#[test]
fn every_bayer_arrangement_big_endian_in_a_sub_ifd() {
    for pattern in [[0, 1, 1, 2], [2, 1, 1, 0], [1, 0, 2, 1], [1, 2, 0, 1]] {
        let r = develop_bytes(&dng(&patch_raw(pattern), srgb_camera(), true, false)).unwrap();
        check_patches(&r, &format!("{pattern:?}"));
    }
}

#[test]
fn bit_packed_depths() {
    for bits in [8u16, 10, 12, 14] {
        let white = f64::from((1u32 << bits) - 1);
        let mut raw = Raw::cfa(16, 16, mosaic(16, 16, RGGB, 0.0, white, quadrants));
        raw.bits = bits;
        // odd row counts per strip, so strips end mid-byte
        raw.rps = 5;
        let r = develop_bytes(&dng(&raw, srgb_camera(), false, true)).unwrap();
        check_patches(&r, &format!("{bits}-bit"));
    }
}

#[test]
fn lossless_jpeg_matches_uncompressed() {
    let plain = develop_bytes(&dng(&patch_raw(RGGB), srgb_camera(), false, true)).unwrap();
    for sel in 1..=7u8 {
        for (tile, restart) in [(Some((8, 8)), 0), (Some((16, 4)), 3), (None, 0), (Some((5, 16)), 0)] {
            let mut raw = patch_raw(RGGB);
            raw.compression = 7;
            raw.jpeg_predictor = sel;
            raw.tile = tile;
            raw.restart = restart;
            raw.rps = 6;
            let b = dng(&raw, srgb_camera(), true, true);
            let r = develop_bytes(&b).unwrap_or_else(|e| panic!("predictor {sel} {tile:?}: {e}"));
            assert!(r.rgba == plain.rgba, "predictor {sel}, tiles {tile:?}, restart {restart}");
        }
    }
}

#[test]
fn lossless_jpeg_round_trips_hard_values() {
    // extremes, category 16 differences and 0xFF bytes in the entropy data
    let s: Vec<u16> = (0..64 * 3).map(|i: u32| [0u16, 65535, 32768, 0xFFFF, 1, 0x7FFF][(i % 6) as usize].wrapping_add((i * 977) as u16)).collect();
    for sel in 1..=7 {
        let j = ljpeg(&s, 8, 8, 3, 16, sel, 5);
        let f = ljpeg::decode(&j, 1 << 20).unwrap();
        assert_eq!(f.samples, s, "predictor {sel}");
    }
}

#[test]
fn deflate_with_predictor_matches_uncompressed() {
    let plain = develop_bytes(&dng(&patch_raw(RGGB), srgb_camera(), false, true)).unwrap();
    for (predictor, le) in [(1, true), (2, true), (2, false)] {
        let mut raw = patch_raw(RGGB);
        raw.compression = 8;
        raw.predictor = predictor;
        raw.tile = Some((8, 8));
        let r = develop_bytes(&dng(&raw, srgb_camera(), false, le)).unwrap();
        assert!(r.rgba == plain.rgba, "predictor {predictor}");
    }
}

#[test]
fn white_balance_keeps_greys_and_clipped_highlights_neutral() {
    let n = [0.5, 1.0, 0.25];
    // left half a grey at 40 % (camera values scaled by the neutral), right half saturated
    let raw = Raw::cfa(16, 16, mosaic(16, 16, RGGB, BLACK, WHITE, |x, _| if x < 8 { n.map(|c| c * 0.4) } else { [1.0; 3] }))
        .tag(BLACK_LEVEL, V::L(vec![BLACK as u32]))
        .tag(WHITE_LEVEL, V::L(vec![WHITE as u32]));
    let mut ifd0 = srgb_camera();
    ifd0.retain(|t| t.0 != AS_SHOT_NEUTRAL);
    ifd0.push((AS_SHOT_NEUTRAL, rat(&n)));
    let r = develop_bytes(&dng(&raw, ifd0, false, true)).unwrap();
    assert_near(px(&r, 3, 8), [0.4; 3], 2, "grey");
    assert_eq!(px(&r, 12, 8), [255, 255, 255, 255], "clipped highlight");
}

#[test]
fn forward_matrix_and_baseline_exposure() {
    let n = [0.6, 1.0, 0.7];
    let raw = Raw::cfa(16, 16, mosaic(16, 16, RGGB, BLACK, WHITE, |x, y| quadrants(x, y).map(|v| v * 0.5)))
        .tag(BLACK_LEVEL, V::L(vec![BLACK as u32]))
        .tag(WHITE_LEVEL, V::L(vec![WHITE as u32]));
    // the camera's channels are sRGB scaled by the white balance; ForwardMatrix maps balanced
    // camera values to XYZ (D50)
    let mut raw = raw;
    raw.samples = mosaic(16, 16, RGGB, BLACK, WHITE, |x, y| {
        let q = quadrants(x, y);
        [q[0] * 0.5 * n[0], q[1] * 0.5 * n[1], q[2] * 0.5 * n[2]]
    });
    let ifd0 = vec![
        (COLOR_MATRIX_1, srat(&XYZ_D50_TO_SRGB)),
        (FORWARD_MATRIX_1, srat(&SRGB_TO_XYZ_D50)),
        (CALIBRATION_ILLUMINANT_1, V::S(vec![21])),
        (AS_SHOT_NEUTRAL, rat(&n)),
        (BASELINE_EXPOSURE, V::Sr(vec![(1, 1)])),
    ];
    let r = develop_bytes(&dng(&raw, ifd0, false, true)).unwrap();
    check_patches(&r, "forward matrix");
}

#[test]
fn two_calibrations_interpolate_between_equal_matrices() {
    let mut ifd0 = srgb_camera();
    ifd0.extend([(COLOR_MATRIX_2, srat(&XYZ_D50_TO_SRGB)), (CALIBRATION_ILLUMINANT_2, V::S(vec![17]))]);
    let r = develop_bytes(&dng(&patch_raw(RGGB), ifd0, false, true)).unwrap();
    check_patches(&r, "dual illuminant");
    assert_eq!(interpolation_weight(2856.0, 2856.0, 6504.0), 1.0);
    assert_eq!(interpolation_weight(6504.0, 2856.0, 6504.0), 0.0);
    assert!((cct(D65_XY) - 6504.0).abs() < 30.0);
}

#[test]
fn linearization_black_pattern_active_area_and_crop() {
    // stored values are half the linear ones (the table doubles them); a 2×2 black level pattern
    // plus a per-row delta; a 2-pixel masked border; then a crop
    let (w, h) = (20, 20);
    let blacks = [100.0, 120.0, 140.0, 160.0];
    let mut s = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let (ax, ay) = (x as isize - 2, y as isize - 2);
            if !(0..16).contains(&ax) || !(0..16).contains(&ay) {
                s[y * w + x] = 7;
                continue;
            }
            let (ax, ay) = (ax as usize, ay as usize);
            let c = RGGB[(ay & 1) * 2 + (ax & 1)];
            let black = blacks[(ay % 2) * 2 + ax % 2] + ay as f64;
            let v = black + quadrants(ax, ay)[c] * (4000.0 - black);
            s[y * w + x] = (v / 2.0).round() as u16;
        }
    }
    let table: Vec<u16> = (0..4096u32).map(|v| (v * 2).min(65535) as u16).collect();
    let raw = Raw::cfa(w, h, s)
        .tag(LINEARIZATION_TABLE, V::S(table))
        .tag(ACTIVE_AREA, V::L(vec![2, 2, 18, 18]))
        .tag(BLACK_LEVEL_REPEAT_DIM, V::S(vec![2, 2]))
        .tag(BLACK_LEVEL, V::R(blacks.iter().map(|&b| (b as u32, 1)).collect()))
        .tag(BLACK_LEVEL_DELTA_V, V::Sr((0..16).map(|y| (y, 1)).collect()))
        .tag(WHITE_LEVEL, V::S(vec![4000]));
    let r = develop_bytes(&dng(&raw, srgb_camera(), false, true)).unwrap();
    check_patches(&r, "linearised");
    let cropped = raw.tag(DEFAULT_CROP_ORIGIN, V::S(vec![4, 2])).tag(DEFAULT_CROP_SIZE, V::S(vec![8, 12]));
    let c = develop_bytes(&dng(&cropped, srgb_camera(), false, true)).unwrap();
    assert_eq!((c.width, c.height), (8, 12));
    assert_eq!(px(&c, 0, 2), px(&r, 4, 4));
    assert_eq!(px(&c, 7, 11), px(&r, 11, 13));
}

#[test]
fn orientation_rotates_the_developed_picture() {
    let plain = develop_bytes(&dng(&patch_raw(RGGB), srgb_camera(), false, true)).unwrap();
    for o in 1..=8u16 {
        let mut ifd0 = srgb_camera();
        ifd0.push((ORIENTATION, V::S(vec![o])));
        let r = develop_bytes(&dng(&patch_raw(RGGB), ifd0, false, true)).unwrap();
        // where the source pixel (2, 5) lands
        let (x, y) = (2usize, 5usize);
        let (ox, oy) = match o {
            1 => (x, y),
            2 => (15 - x, y),
            3 => (15 - x, 15 - y),
            4 => (x, 15 - y),
            5 => (y, x),
            6 => (15 - y, x),
            7 => (15 - y, 15 - x),
            _ => (y, 15 - x),
        };
        assert_eq!(px(&r, ox, oy), px(&plain, x, y), "orientation {o}");
    }
    // 6 turns the top-left corner to the top right
    let mut ifd0 = srgb_camera();
    ifd0.push((ORIENTATION, V::S(vec![6])));
    let tall = Raw::cfa(16, 8, mosaic(16, 8, RGGB, 0.0, 65535.0, |x, _| if x < 8 { PATCHES[0] } else { PATCHES[2] }));
    let r = develop_bytes(&dng(&tall, ifd0, false, true)).unwrap();
    assert_eq!((r.width, r.height), (8, 16));
    assert_near(px(&r, 6, 2), PATCHES[0], 2, "top after rotation");
    assert_near(px(&r, 1, 13), PATCHES[2], 2, "bottom after rotation");
}

#[test]
fn linear_raw_rgb_and_monochrome() {
    let mut s = vec![0u16; 16 * 16 * 3];
    for y in 0..16 {
        for x in 0..16 {
            for c in 0..3 {
                s[(y * 16 + x) * 3 + c] = (quadrants(x, y)[c] * 65535.0).round() as u16;
            }
        }
    }
    let mut raw = Raw::cfa(16, 16, s);
    raw.spp = 3;
    raw.photometric = 34892;
    raw.tags.clear();
    raw.tile = Some((16, 16));
    raw.compression = 7;
    let r = develop_bytes(&dng(&raw, srgb_camera(), false, true)).unwrap();
    // no demosaicing: every pixel is exact, up to the patch edges
    assert_near(px(&r, 7, 7), PATCHES[0], 1, "linear raw");
    assert_near(px(&r, 8, 8), PATCHES[3], 1, "linear raw");
    let mut mono = Raw::cfa(4, 4, vec![16384; 16]);
    mono.spp = 1;
    mono.photometric = 34892;
    mono.tags.clear();
    let r = develop_bytes(&dng(&mono, vec![], false, false)).unwrap();
    assert_near(px(&r, 1, 1), [16384.0 / 65535.0; 3], 1, "monochrome");
}

#[test]
fn still_source_and_import_list() {
    let b = dng(&patch_raw(RGGB), srgb_camera(), false, true);
    assert!(crate::still::sniff(&b));
    let src = crate::still::StillSource::decode("shot.dng", &b).unwrap();
    use crate::MediaSource;
    let v = src.info().video.as_ref().unwrap();
    assert_eq!((v.width, v.height, v.codec.as_str()), (16, 16, "DNG"));
    assert!(crate::STILL_EXTENSIONS.contains(&"dng"));
    assert!(crate::is_importable(std::path::Path::new("A001.DNG")));
    // a plain TIFF is not a DNG and still goes to the TIFF decoder
    let mut plain = W::new(true);
    let data = plain.blob(&[10, 20, 30, 40, 50, 60]);
    let off = plain.ifd(
        vec![
            (IMAGE_WIDTH, V::S(vec![2])),
            (IMAGE_LENGTH, V::S(vec![1])),
            (BITS_PER_SAMPLE, V::S(vec![8, 8, 8])),
            (COMPRESSION, V::S(vec![1])),
            (PHOTOMETRIC, V::S(vec![2])),
            (STRIP_OFFSETS, V::L(vec![data])),
            (SAMPLES_PER_PIXEL, V::S(vec![3])),
            (ROWS_PER_STRIP, V::S(vec![1])),
            (STRIP_BYTE_COUNTS, V::L(vec![6])),
        ],
        0,
    );
    plain.first(off);
    assert!(!sniff(&plain.b));
    let r = crate::still::decode_raster("plain.tif", &plain.b).unwrap();
    assert_eq!(&r.rgba[..4], &[10, 20, 30, 255]);
}

#[test]
fn unsupported_raw_falls_back_to_the_preview_only_then() {
    // an X-Trans-like 6×6 pattern is refused
    let xtrans = patch_raw(RGGB).tag(CFA_REPEAT_PATTERN_DIM, V::S(vec![6, 6])).tag(CFA_PATTERN, V::B(vec![1; 36]));
    assert!(matches!(decode("x.dng", &dng(&xtrans, srgb_camera(), true, true)), Err(MediaError::Unsupported(_))));
    // with an 8-bit RGB preview in IFD0 (the raw as its SubIFD), the preview is shown instead
    let b = dng(&xtrans, vec![], false, true);
    let raw_off = u32::from_le_bytes(b[4..8].try_into().unwrap());
    let mut pw = W { b, le: true };
    let pdata = pw.blob(&[200, 100, 50, 200, 100, 50]);
    let mut ifd0 = vec![
        (DNG_VERSION, V::B(vec![1, 4, 0, 0])),
        (NEW_SUBFILE_TYPE, V::L(vec![1])),
        (IMAGE_WIDTH, V::S(vec![2])),
        (IMAGE_LENGTH, V::S(vec![1])),
        (BITS_PER_SAMPLE, V::S(vec![8, 8, 8])),
        (COMPRESSION, V::S(vec![1])),
        (PHOTOMETRIC, V::S(vec![2])),
        (STRIP_OFFSETS, V::L(vec![pdata])),
        (SAMPLES_PER_PIXEL, V::S(vec![3])),
        (ROWS_PER_STRIP, V::S(vec![1])),
        (STRIP_BYTE_COUNTS, V::L(vec![6])),
        (SUB_IFDS, V::L(vec![raw_off])),
    ];
    ifd0.extend(srgb_camera());
    let off0 = pw.ifd(ifd0, 0);
    pw.first(off0);
    let r = decode("x.dng", &pw.b).unwrap();
    assert_eq!((r.width, r.height), (2, 1));
    assert_eq!(&r.rgba[..4], &[200, 100, 50, 255]);
    // a corrupt raw is an error, never the preview: damage the lossless JPEG's SOI
    let mut t = patch_raw(RGGB);
    t.compression = 7;
    let mut c = dng(&t, srgb_camera(), false, true);
    let at = c.windows(2).position(|w| w == [0xFF, 0xD8]).unwrap();
    c[at + 1] = 0x00;
    assert!(matches!(decode("c.dng", &c), Err(MediaError::Decode(_))));
}

// --- hostile input -----------------------------------------------------------------------------

fn no_panic(b: &[u8]) {
    let r = std::panic::catch_unwind(|| {
        let _ = decode("h.dng", b);
        let _ = sniff(b);
    });
    assert!(r.is_ok(), "panicked on a {}-byte input", b.len());
}

fn samples() -> Vec<Vec<u8>> {
    let mut v = vec![dng(&patch_raw(RGGB), srgb_camera(), true, true)];
    let mut j = patch_raw(RGGB);
    j.compression = 7;
    j.tile = Some((8, 8));
    j.restart = 4;
    v.push(dng(&j, srgb_camera(), false, false));
    let mut d = patch_raw(RGGB);
    d.compression = 8;
    d.predictor = 2;
    d.bits = 12;
    d.samples = mosaic(16, 16, RGGB, 0.0, 4095.0, quadrants);
    v.push(dng(&d, srgb_camera(), false, true));
    v
}

#[test]
fn truncation_never_panics() {
    for b in samples() {
        for len in 0..b.len() {
            no_panic(&b[..len]);
        }
    }
}

#[test]
fn bit_flips_never_panic() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for b in samples() {
        for _ in 0..400 {
            let mut m = b.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() as usize) % m.len();
                m[i] ^= 1 << (next() % 8);
            }
            no_panic(&m);
        }
    }
}

#[test]
fn hostile_sizes_and_loops_are_errors() {
    // huge dimensions with almost no data
    let mut huge = Raw::cfa(2, 2, vec![0; 4]);
    huge.compression = 7;
    let mut wr = W { b: dng(&huge, srgb_camera(), false, true), le: true };
    let off = wr.ifd(
        vec![
            (DNG_VERSION, V::B(vec![1, 4, 0, 0])),
            (IMAGE_WIDTH, V::L(vec![60_000])),
            (IMAGE_LENGTH, V::L(vec![60_000])),
            (BITS_PER_SAMPLE, V::S(vec![16])),
            (COMPRESSION, V::S(vec![7])),
            (PHOTOMETRIC, V::S(vec![32803])),
            (CFA_REPEAT_PATTERN_DIM, V::S(vec![2, 2])),
            (CFA_PATTERN, V::B(vec![0, 1, 1, 2])),
            (STRIP_OFFSETS, V::L(vec![8])),
            (STRIP_BYTE_COUNTS, V::L(vec![4])),
        ],
        0,
    );
    wr.first(off);
    assert!(decode("huge.dng", &wr.b).is_err());
    // a lossless JPEG tile that claims far more samples than its tile
    let mut lj = Raw::cfa(16, 16, vec![0; 256]);
    lj.compression = 7;
    let mut b = dng(&lj, srgb_camera(), false, true);
    let sof = b.windows(2).position(|w| w == [0xFF, 0xC3]).unwrap();
    b[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(decode("big-jpeg.dng", &b).is_err());
    // a lossless JPEG whose entropy data stops early
    let grad: Vec<u16> = (0..64 * 64).map(|i| (i * 1031 % 65536) as u16).collect();
    let j = ljpeg(&grad, 64, 64, 1, 16, 1, 0);
    assert!(ljpeg::decode(&j, 1 << 20).is_ok());
    assert!(ljpeg::decode(&j[..j.len() / 2], 1 << 20).is_err());
    // IFD chains and SubIFDs that point back at themselves
    let mut wr = W::new(true);
    let first = wr.b.len() as u32;
    let looped = wr.ifd(vec![(DNG_VERSION, V::B(vec![1, 4, 0, 0])), (SUB_IFDS, V::L(vec![first]))], first);
    assert_eq!(looped, first);
    wr.first(looped);
    assert!(sniff(&wr.b));
    assert!(decode("loop.dng", &wr.b).is_err());
    // tile layouts sized to exhaust memory
    let mut wr = W::new(true);
    let off = wr.ifd(
        vec![
            (DNG_VERSION, V::B(vec![1, 4, 0, 0])),
            (IMAGE_WIDTH, V::L(vec![65_535])),
            (IMAGE_LENGTH, V::L(vec![2_000])),
            (BITS_PER_SAMPLE, V::S(vec![16])),
            (PHOTOMETRIC, V::S(vec![32803])),
            (CFA_PATTERN, V::B(vec![0, 1, 1, 2])),
            (TILE_WIDTH, V::L(vec![65_535])),
            (TILE_LENGTH, V::L(vec![65_535])),
            (TILE_OFFSETS, V::L(vec![8])),
            (TILE_BYTE_COUNTS, V::L(vec![u32::MAX])),
        ],
        0,
    );
    wr.first(off);
    assert!(decode("tiles.dng", &wr.b).is_err());
}
