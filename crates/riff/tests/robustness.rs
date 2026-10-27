//! Fuzz-style robustness: random multi-byte mutations, splices and truncations of AVI files never
//! panic, and whatever opens reads every chunk without panicking.

use filmcraft_riff::{OpenOptions, open_with};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn list(ltype: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut d = ltype.to_vec();
    d.extend_from_slice(body);
    chunk(b"LIST", &d)
}

/// A two-stream AVI with `rec ` lists, an `idx1`, an `indx` super index and an `AVIX` extension.
fn seed() -> Vec<u8> {
    let mut strh_v = vec![0u8; 56];
    strh_v[..4].copy_from_slice(b"vids");
    strh_v[20..24].copy_from_slice(&1u32.to_le_bytes());
    strh_v[24..28].copy_from_slice(&30u32.to_le_bytes());
    let mut bih = vec![0u8; 40];
    bih[4..8].copy_from_slice(&32u32.to_le_bytes());
    bih[8..12].copy_from_slice(&16u32.to_le_bytes());
    bih[16..20].copy_from_slice(b"MJPG");
    let mut indx = vec![0u8; 24 + 16];
    indx[..2].copy_from_slice(&4u16.to_le_bytes());
    indx[4..8].copy_from_slice(&1u32.to_le_bytes());
    indx[24..32].copy_from_slice(&200u64.to_le_bytes());
    let mut v = chunk(b"strh", &strh_v);
    v.extend(chunk(b"strf", &bih));
    v.extend(chunk(b"indx", &indx));
    let mut strh_a = vec![0u8; 56];
    strh_a[..4].copy_from_slice(b"auds");
    strh_a[20..24].copy_from_slice(&1152u32.to_le_bytes());
    strh_a[24..28].copy_from_slice(&44_100u32.to_le_bytes());
    let mut wfx = vec![0u8; 18 + 22];
    wfx[..2].copy_from_slice(&0xFFFEu16.to_le_bytes());
    wfx[2..4].copy_from_slice(&2u16.to_le_bytes());
    wfx[4..8].copy_from_slice(&44_100u32.to_le_bytes());
    wfx[16..18].copy_from_slice(&22u16.to_le_bytes());
    wfx[24..26].copy_from_slice(&0x55u16.to_le_bytes());
    let mut a = chunk(b"strh", &strh_a);
    a.extend(chunk(b"strf", &wfx));
    let mut hdrl = chunk(b"avih", &[0; 56]);
    hdrl.extend(list(b"strl", &v));
    hdrl.extend(list(b"strl", &a));
    let mut movi = Vec::new();
    let mut idx = Vec::new();
    for k in 0..6u8 {
        let (id, data) = if k % 2 == 0 { (b"00dc", vec![k; 9 + k as usize]) } else { (b"01wb", vec![k; 6]) };
        idx.extend_from_slice(id);
        idx.extend_from_slice(&(if k == 0 { 0x10u32 } else { 0 }).to_le_bytes());
        // offsets relative to the `movi` type id: 4 for it, 12 for the `rec ` list header and type
        idx.extend_from_slice(&(movi.len() as u32 + 16).to_le_bytes());
        idx.extend_from_slice(&(data.len() as u32).to_le_bytes());
        movi.extend(chunk(id, &data));
    }
    let mut riff = b"AVI ".to_vec();
    riff.extend(list(b"hdrl", &hdrl));
    riff.extend(list(b"movi", &list(b"rec ", &movi)));
    riff.extend(chunk(b"idx1", &idx));
    let mut out = chunk(b"RIFF", &riff);
    let mut avix = b"AVIX".to_vec();
    avix.extend(list(b"movi", &chunk(b"00dc", &[7; 12])));
    out.extend(chunk(b"RIFF", &avix));
    out
}

fn exercise(b: &[u8]) {
    for opts in [OpenOptions::default(), OpenOptions { ignore_odml: true, ignore_idx1: false }, OpenOptions { ignore_odml: true, ignore_idx1: true }] {
        if let Ok(f) = open_with(b, opts) {
            for (s, st) in f.streams.iter().enumerate() {
                let _ = st.chunk_starts();
                let _ = st.units_per_second();
                for i in 0..st.chunks.len().min(64) {
                    let _ = f.read_chunk(b, s, i);
                }
            }
        }
    }
}

#[test]
fn seed_opens() {
    let f = filmcraft_riff::open(&seed()).unwrap();
    assert_eq!(f.streams.len(), 2);
    assert_eq!(f.index, filmcraft_riff::IndexSource::Idx1, "the super index points nowhere useful");
    assert_eq!(f.streams[0].chunks.len(), 4, "3 frames by idx1 and one in the AVIX extension");
    assert_eq!(f.streams[1].chunks.len(), 3);
    assert_eq!(f.streams[1].audio.as_ref().map(|w| w.format_tag), Some(0x55), "WAVE_FORMAT_EXTENSIBLE resolves to its sub-format");
}

#[test]
fn random_mutations_never_panic() {
    let base = seed();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..20_000 {
        let mut b = base.clone();
        match round % 5 {
            0 => {
                for _ in 0..1 + rng.below(8) {
                    let i = rng.below(b.len());
                    b[i] ^= 1 << rng.below(8);
                }
            }
            1 => {
                // a 32-bit field overwritten with an extreme value
                let i = rng.below(b.len().saturating_sub(4));
                let v = [0u32, 1, 0x7FFF_FFFF, 0x8000_0000, u32::MAX, b.len() as u32, 8][rng.below(7)];
                b[i..i + 4].copy_from_slice(&v.to_le_bytes());
            }
            2 => b.truncate(rng.below(b.len())),
            3 => {
                // a slice repeated elsewhere (duplicated headers, nested lists)
                let (from, len) = (rng.below(b.len()), 1 + rng.below(64));
                let piece = b[from..(from + len).min(b.len())].to_vec();
                let at = rng.below(b.len());
                b.splice(at..at, piece);
            }
            _ => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
        }
        exercise(&b);
    }
}
