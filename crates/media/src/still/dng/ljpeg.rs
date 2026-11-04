//! Lossless JPEG (ITU-T T.81, process 14: Huffman-coded differences from one of seven predictors),
//! the compression DNG writers use for raw tiles. Written from T.81 Annexes B (syntax), C (Huffman
//! table specification), F.2.2 (decoding a difference) and H (lossless prediction). Only what DNG
//! needs is read: one frame with a single interleaved scan, no subsampling, precision 2–16, point
//! transform and restart intervals. Every input byte is hostile: tables are checked, sample counts
//! are capped by the caller, and running out of data ends the scan with an error instead of
//! decoding zeros forever.

type R<T> = std::result::Result<T, &'static str>;

/// A decoded frame: samples in raster order, components interleaved.
pub(super) struct Frame {
    pub precision: u32,
    pub samples: Vec<u16>,
}

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(*d.get(at..at.checked_add(2)?)?.first_chunk()?))
}

/// What kind of JPEG stream `d` is: `Some(true)` for lossless (SOF3), `Some(false)` for any other
/// frame type (baseline, extended or progressive DCT), `None` when it is not a JPEG stream.
pub(super) fn is_lossless(d: &[u8]) -> Option<bool> {
    if !d.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut pos = 2usize;
    // every step moves at least two bytes forward
    loop {
        while d.get(pos) == Some(&0xFF) && d.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        if d.get(pos) != Some(&0xFF) {
            return None;
        }
        let m = *d.get(pos + 1)?;
        pos += 2;
        match m {
            0xC3 => return Some(true),
            0xC4 | 0xC8 | 0xCC => {}
            0xC0..=0xCF => return Some(false),
            0xD8 | 0xD0..=0xD7 | 0x01 => continue,
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::from(be16(d, pos)?);
        pos = pos.checked_add(len.max(2))?;
    }
}

/// A Huffman table for difference categories (T.81 Annex C, decoding procedure F.2.2.3), with a
/// 9-bit lookup for the short codes.
struct Huff {
    /// `(length << 8) | value` for every 9-bit prefix that starts with a code of 9 bits or fewer; 0
    /// when the code is longer.
    lookup: Vec<u16>,
    maxcode: [i32; 17],
    mincode: [i32; 17],
    valptr: [i32; 17],
    values: Vec<u8>,
}

const LOOKUP_BITS: u32 = 9;

impl Huff {
    fn new(counts: &[u8], values: &[u8]) -> R<Huff> {
        let mut h = Huff { lookup: vec![0; 1 << LOOKUP_BITS], maxcode: [-1; 17], mincode: [0; 17], valptr: [0; 17], values: values.to_vec() };
        let (mut code, mut k) = (0u32, 0usize);
        for len in 1..=16u32 {
            let n = usize::from(*counts.get(len as usize - 1).ok_or("bad Huffman table")?);
            if n > 0 {
                h.valptr[len as usize] = k as i32;
                h.mincode[len as usize] = code as i32;
                for _ in 0..n {
                    if code >= 1 << len {
                        return Err("bad Huffman table");
                    }
                    let v = *values.get(k).ok_or("bad Huffman table")?;
                    if len <= LOOKUP_BITS {
                        let shift = LOOKUP_BITS - len;
                        let base = (code << shift) as usize;
                        let e = ((len as u16) << 8) | u16::from(v);
                        if let Some(slots) = h.lookup.get_mut(base..base + (1 << shift)) {
                            slots.fill(e);
                        }
                    }
                    code += 1;
                    k += 1;
                }
                h.maxcode[len as usize] = code as i32 - 1;
            }
            code <<= 1;
        }
        Ok(h)
    }
}

/// Reads the entropy-coded segment MSB first, removing stuffed zero bytes. At a marker or the end
/// of the data it feeds zero bytes and counts them in `pad`.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
    marker: bool,
    pad: u32,
}

impl Bits<'_> {
    fn next_byte(&mut self) -> u8 {
        if self.marker {
            self.pad += 1;
            return 0;
        }
        match self.d.get(self.pos) {
            Some(&0xFF) => {
                if self.d.get(self.pos + 1) == Some(&0) {
                    self.pos += 2;
                    0xFF
                } else {
                    self.marker = true;
                    self.pad += 1;
                    0
                }
            }
            Some(&b) => {
                self.pos += 1;
                b
            }
            None => {
                self.pad += 1;
                0
            }
        }
    }

    /// Make at least 57 bits available.
    fn refill(&mut self) {
        while self.n <= 56 {
            let b = self.next_byte();
            self.acc |= u64::from(b) << (56 - self.n);
            self.n += 8;
        }
    }

    fn consume(&mut self, k: u32) {
        self.acc = self.acc.checked_shl(k).unwrap_or(0);
        self.n = self.n.saturating_sub(k);
    }

    /// `k` (1–16) bits as an unsigned number.
    fn take(&mut self, k: u32) -> u32 {
        self.refill();
        let v = (self.acc >> (64 - k)) as u32;
        self.consume(k);
        v
    }

    fn symbol(&mut self, h: &Huff) -> Option<u8> {
        self.refill();
        let peek = (self.acc >> (64 - LOOKUP_BITS)) as usize;
        let e = h.lookup.get(peek).copied().unwrap_or(0);
        if e != 0 {
            self.consume(u32::from(e >> 8));
            return Some(e as u8);
        }
        for l in LOOKUP_BITS + 1..=16 {
            let c = (self.acc >> (64 - l)) as i32;
            let li = l as usize;
            if c <= *h.maxcode.get(li)? {
                let idx = h.valptr.get(li)? + c - h.mincode.get(li)?;
                let v = *h.values.get(usize::try_from(idx).ok()?)?;
                self.consume(l);
                return Some(v);
            }
        }
        None
    }

    /// One decoded difference (F.2.2.1 and the lossless category 16).
    fn diff(&mut self, h: &Huff) -> R<i32> {
        let s = self.symbol(h).ok_or("corrupt Huffman code")?;
        match s {
            0 => Ok(0),
            16 => Ok(32768),
            1..=15 => {
                let s = u32::from(s);
                let v = self.take(s) as i32;
                Ok(if v < 1 << (s - 1) { v - (1 << s) + 1 } else { v })
            }
            _ => Err("corrupt difference category"),
        }
    }

    /// Drop the buffered bits and skip the RSTn marker that ends the restart interval.
    fn restart(&mut self) {
        self.acc = 0;
        self.n = 0;
        self.marker = false;
        self.pad = 0;
        while self.d.get(self.pos) == Some(&0xFF) && self.d.get(self.pos + 1) == Some(&0xFF) {
            self.pos += 1;
        }
        if self.d.get(self.pos) == Some(&0xFF) && matches!(self.d.get(self.pos + 1), Some(0xD0..=0xD7)) {
            self.pos += 2;
        }
    }
}

/// Zero bytes the decoder may read past the data (its look-ahead) before the scan counts as
/// truncated.
const MAX_PAD: u32 = 64;

struct FrameHeader {
    precision: u32,
    height: usize,
    width: usize,
    ids: Vec<u8>,
}

fn frame_header(seg: &[u8]) -> R<FrameHeader> {
    let precision = u32::from(*seg.first().ok_or("truncated frame header")?);
    let height = usize::from(be16(seg, 1).ok_or("truncated frame header")?);
    let width = usize::from(be16(seg, 3).ok_or("truncated frame header")?);
    let nf = usize::from(*seg.get(5).ok_or("truncated frame header")?);
    if !(2..=16).contains(&precision) {
        return Err("lossless JPEG precision out of range");
    }
    if height == 0 {
        return Err("lossless JPEG with a DNL height is not supported");
    }
    if width == 0 || !(1..=4).contains(&nf) {
        return Err("invalid lossless JPEG frame header");
    }
    let mut ids = Vec::with_capacity(nf);
    for i in 0..nf {
        let c = seg.get(6 + 3 * i..9 + 3 * i).ok_or("truncated frame header")?;
        if c.get(1) != Some(&0x11) {
            return Err("subsampled lossless JPEG is not supported");
        }
        ids.push(*c.first().ok_or("truncated frame header")?);
    }
    Ok(FrameHeader { precision, height, width, ids })
}

fn huffman_tables(seg: &[u8], tables: &mut [Option<Huff>; 4]) -> R<()> {
    let mut p = 0usize;
    while let Some(&tcth) = seg.get(p) {
        let counts = seg.get(p + 1..p + 17).ok_or("truncated Huffman table")?;
        let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
        if total > 256 {
            return Err("bad Huffman table");
        }
        let values = seg.get(p + 17..p + 17 + total).ok_or("truncated Huffman table")?;
        p += 17 + total;
        let slot = usize::from(tcth & 15);
        if slot > 3 {
            return Err("bad Huffman table id");
        }
        // lossless JPEG uses DC tables only
        if tcth >> 4 == 0
            && let Some(t) = tables.get_mut(slot)
        {
            *t = Some(Huff::new(counts, values)?);
        }
    }
    Ok(())
}

/// Decode a lossless JPEG stream holding at most `max_samples` samples.
pub(super) fn decode(d: &[u8], max_samples: usize) -> R<Frame> {
    if !d.starts_with(&[0xFF, 0xD8]) {
        return Err("not a JPEG stream");
    }
    let mut pos = 2usize;
    let mut tables: [Option<Huff>; 4] = [None, None, None, None];
    let mut frame = None;
    let mut restart = 0usize;
    // every step moves at least two bytes forward
    loop {
        while d.get(pos) == Some(&0xFF) && d.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        if d.get(pos) != Some(&0xFF) {
            return Err("corrupt JPEG marker");
        }
        let m = *d.get(pos + 1).ok_or("truncated JPEG stream")?;
        pos += 2;
        match m {
            0xD8 | 0xD0..=0xD7 | 0x01 => continue,
            0xD9 => return Err("JPEG stream without a scan"),
            _ => {}
        }
        let len = usize::from(be16(d, pos).ok_or("truncated JPEG stream")?);
        if len < 2 {
            return Err("corrupt JPEG segment length");
        }
        let seg = d.get(pos + 2..pos + len).ok_or("truncated JPEG stream")?;
        pos += len;
        match m {
            0xC3 => frame = Some(frame_header(seg)?),
            0xC4 => huffman_tables(seg, &mut tables)?,
            0xC8 | 0xCC => {}
            0xC0..=0xCF => return Err("not a lossless JPEG stream"),
            0xDD => restart = usize::from(be16(seg, 0).ok_or("truncated restart interval")?),
            0xDA => {
                let f = frame.ok_or("JPEG scan before the frame header")?;
                return scan(d.get(pos..).unwrap_or(&[]), seg, &f, &tables, restart, max_samples);
            }
            _ => {}
        }
    }
}

fn scan(data: &[u8], seg: &[u8], f: &FrameHeader, tables: &[Option<Huff>; 4], restart: usize, max_samples: usize) -> R<Frame> {
    let nc = f.ids.len();
    if usize::from(*seg.first().ok_or("truncated scan header")?) != nc {
        return Err("lossless JPEG with several scans is not supported");
    }
    let mut huff = Vec::with_capacity(nc);
    for j in 0..nc {
        let td = usize::from(*seg.get(2 + 2 * j).ok_or("truncated scan header")? >> 4);
        huff.push(tables.get(td).and_then(Option::as_ref).ok_or("missing Huffman table")?);
    }
    let predictor = *seg.get(1 + 2 * nc).ok_or("truncated scan header")?;
    let al = u32::from(*seg.get(3 + 2 * nc).ok_or("truncated scan header")? & 15);
    if !(1..=7).contains(&predictor) {
        return Err("invalid lossless JPEG predictor");
    }
    if al >= f.precision {
        return Err("invalid lossless JPEG point transform");
    }
    let (w, h) = (f.width, f.height);
    let row = w.checked_mul(nc).ok_or("lossless JPEG too large")?;
    let n = row.checked_mul(h).ok_or("lossless JPEG too large")?;
    if n > max_samples {
        return Err("lossless JPEG frame larger than its tile");
    }
    let mut out = vec![0u16; n];
    let mut br = Bits { d: data, pos: 0, acc: 0, n: 0, marker: false, pad: 0 };
    let init = 1i32 << (f.precision - al - 1);
    let mut left = restart;
    let mut reset = true;
    let mut fresh_row = true;
    for y in 0..h {
        if y > 0 {
            fresh_row = false;
        }
        for x in 0..w {
            if restart > 0 {
                if left == 0 {
                    br.restart();
                    left = restart;
                    reset = true;
                    fresh_row = true;
                }
                left -= 1;
            }
            let base = (y * w + x) * nc;
            for (c, table) in huff.iter().enumerate() {
                let i = base + c;
                let diff = br.diff(table)?;
                let px = |at: Option<usize>| at.and_then(|a| out.get(a)).map_or(0, |&v| i32::from(v));
                let pred = if reset {
                    init
                } else if fresh_row {
                    px(i.checked_sub(nc))
                } else if x == 0 {
                    px(i.checked_sub(row))
                } else {
                    let (ra, rb, rc) = (px(i.checked_sub(nc)), px(i.checked_sub(row)), px(i.checked_sub(row + nc)));
                    match predictor {
                        1 => ra,
                        2 => rb,
                        3 => rc,
                        4 => ra + rb - rc,
                        5 => ra + ((rb - rc) >> 1),
                        6 => rb + ((ra - rc) >> 1),
                        _ => (ra + rb) >> 1,
                    }
                };
                if let Some(o) = out.get_mut(i) {
                    *o = ((pred + diff) & 0xFFFF) as u16;
                }
            }
            reset = false;
        }
        if br.pad > MAX_PAD {
            return Err("truncated lossless JPEG data");
        }
    }
    if al > 0 {
        for v in &mut out {
            *v = v.wrapping_shl(al);
        }
    }
    Ok(Frame { precision: f.precision, samples: out })
}
