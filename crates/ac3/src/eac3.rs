//! E-AC-3 (Enhanced AC-3, Dolby Digital Plus): ATSC A/52:2012 Annex E.
//!
//! Decoded:
//! - The E-AC-3 syncframe (E.1.2): strmtyp, substreamid, frmsiz, fscod / fscod2 (including the
//!   24 / 22.05 / 16 kHz rates), 1, 2, 3 or 6 audio blocks per syncframe, the whole bsi (mixing
//!   and informational metadata are parsed and skipped) and audfrm (per-block or frame exponent
//!   strategies with Table E2.10, SNR offset strategies 0-2, block switching / dither / fast gain
//!   / bit allocation / delta bit allocation / skip field enables, converter fields, block start
//!   information).
//! - Audio blocks: dynamic range, spectral extension (strategy, coordinates, translation, noise
//!   blending; E.3.6), standard channel coupling with E-AC-3's default banding structure and
//!   first-block coordinate rules, rematrixing (including the spectral extension banding),
//!   exponents, the A/52 parametric bit allocation with the E-AC-3 defaults (bamode, fast gain
//!   code 4), delta bit allocation, mantissas, dither, 512 / 256-sample transforms — the AC-3
//!   machinery of the parent module is shared.
//! - Independent substream 0 is decoded. Dependent substreams (strmtyp 1, the extra channels of
//!   7.1 programs) and further independent substreams (other programs) decode to no audio: the
//!   5.1 (or smaller) core program is output.
//!
//! Refused with [`Error::Unsupported`] (the syncframe decodes to an error, not to noise):
//! - the adaptive hybrid transform (AHT, E.3.4: chahtinu / cplahtinu / lfeahtinu set);
//! - enhanced coupling (ecplinu set, E.3.5).
//!
//! Not applied (the audio decodes; these are refinements a full Annex E decoder adds):
//! - transient pre-noise processing (E.3.7: a time-domain pre-echo reduction; its parameters are
//!   parsed and skipped);
//! - spectral extension attenuation (the notch filter of E.3.6.4 at translation borders; its
//!   codes are parsed and skipped).

use super::*;

/// Default coupling banding structure (Table E2.x `defcplbndstrc`), by coupling sub-band.
const DEF_CPLBNDSTRC: [bool; 18] = {
    let v = [0u8, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1];
    let mut o = [false; 18];
    let mut i = 0;
    while i < 18 {
        o[i] = v[i] == 1;
        i += 1;
    }
    o
};

/// Default spectral extension banding structure (`defspxbndstrc`), by spectral extension
/// sub-band.
const DEF_SPXBNDSTRC: [bool; 17] = {
    let v = [0u8, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1];
    let mut o = [false; 17];
    let mut i = 0;
    while i < 17 {
        o[i] = v[i] == 1;
        i += 1;
    }
    o
};

/// Frame exponent strategies (Table E2.10): `frmchexpstr` → the strategy of each of the six
/// blocks (0 reuse, 1 D15, 2 D25, 3 D45).
///
/// The table lists every way to split six blocks into runs that share one exponent set: bit
/// 4 - (b - 1) of the code says a new set starts at block b (1..5), and a run's strategy follows
/// from its length (4-6 blocks D15, 2-3 D25, 1 block D45).
const FRM_EXPSTR: [[u8; 6]; 32] = {
    let mut t = [[0u8; 6]; 32];
    let mut k = 0;
    while k < 32 {
        let mut start = 0;
        while start < 6 {
            // end of the run that starts at `start`
            let mut end = start + 1;
            while end < 6 && (k >> (5 - end)) & 1 == 0 {
                end += 1;
            }
            let len = end - start;
            t[k][start] = if len >= 4 {
                1
            } else if len >= 2 {
                2
            } else {
                3
            };
            start = end;
        }
        k += 1;
    }
    t
};

/// E-AC-3 header fields from the start of `b` (the caller checked the syncword, `b.len() >= 7`
/// and bsid 11-16).
pub(crate) fn parse_header(b: &[u8]) -> Result<Header> {
    let (b2, b3, b4, b5) = match b.get(2..6) {
        Some(&[b2, b3, b4, b5]) => (b2, b3, b4, b5),
        _ => return Err(Error::NoSync),
    };
    let strmtyp = b2 >> 6;
    if strmtyp == 3 {
        return Err(Error::Invalid("reserved E-AC-3 stream type"));
    }
    let substreamid = (b2 >> 3) & 7;
    let frmsiz = (((b2 & 7) as usize) << 8) | b3 as usize;
    let fscod = (b4 >> 6) as usize;
    let code = ((b4 >> 4) & 3) as usize;
    let (sample_rate, blocks) = if fscod == 3 {
        let rate = *[24_000u32, 22_050, 16_000].get(code).ok_or(Error::Invalid("reserved fscod2"))?;
        (rate, 6usize)
    } else {
        ([48_000u32, 44_100, 32_000][fscod], [1usize, 2, 3, 6][code])
    };
    let frame_bytes = (frmsiz + 1) * 2;
    if frame_bytes < 8 {
        return Err(Error::Invalid("E-AC-3 syncframe too short"));
    }
    let samples = 256 * blocks;
    let bitrate_kbps = (frame_bytes as u64 * 8 * sample_rate as u64 / samples as u64 / 1000) as u32;
    Ok(Header {
        sample_rate,
        bitrate_kbps,
        frame_bytes,
        bsid: b5 >> 3,
        acmod: (b4 >> 1) & 7,
        lfeon: b4 & 1 == 1,
        samples,
        dependent: strmtyp == 1,
        substreamid,
    })
}

/// ceil(log2(n)) for n ≥ 1 (0 for n ≤ 1).
fn ceil_log2(n: usize) -> u32 {
    if n <= 1 { 0 } else { usize::BITS - (n - 1).leading_zeros() }
}

/// audfrm(): per-syncframe side information (E.1.2.3).
struct AudFrm {
    acmod: usize,
    lfeon: bool,
    nf: usize,
    strmtyp: u32,
    snroffststr: u32,
    blkswe: bool,
    dithflage: bool,
    bamode: bool,
    frmfgaincode: bool,
    dbaflde: bool,
    skipflde: bool,
    cplstre: [bool; 6],
    cplinu: [bool; 6],
    cplexpstr: [u8; 6],
    chexpstr: [[u8; 5]; 6],
    lfeexpstr: [u8; 6],
    frmcsnroffst: i32,
    frmfsnroffst: i32,
}

/// E-AC-3 state carried from block to block within a syncframe (beyond the AC-3 [`Frame`]).
struct EState {
    spxinu: bool,
    chinspx: [bool; 5],
    /// Copy start, begin and end of the extension region, in spectral extension sub-bands
    /// (sub-band s starts at bin 25 + 12 s).
    spxstrt: usize,
    spxbegf: usize,
    spxbegin: usize,
    spxend: usize,
    spxbndstrc: [bool; 17],
    /// Bins in each spectral extension band.
    spxbndsz: [usize; 17],
    nspxbnds: usize,
    firstspxcos: [bool; 5],
    spxblnd: [f32; 5],
    spxco: [[f32; 17]; 5],
    cplbndstrc: [bool; 18],
    firstcplcos: [bool; 5],
    firstcplleak: bool,
}

impl EState {
    fn new() -> EState {
        EState {
            spxinu: false,
            chinspx: [false; 5],
            spxstrt: 0,
            spxbegf: 0,
            spxbegin: 0,
            spxend: 0,
            spxbndstrc: DEF_SPXBNDSTRC,
            spxbndsz: [0; 17],
            nspxbnds: 0,
            firstspxcos: [true; 5],
            spxblnd: [0.0; 5],
            spxco: [[0.0; 17]; 5],
            cplbndstrc: DEF_CPLBNDSTRC,
            firstcplcos: [true; 5],
            firstcplleak: true,
        }
    }

    /// Band sizes from the banding structure over [spxbegin, spxend).
    fn spx_bands(&mut self) {
        self.spxbndsz = [0; 17];
        self.spxbndsz[0] = 12;
        let mut n = 1;
        for s in self.spxbegin + 1..self.spxend.min(17) {
            if self.spxbndstrc[s] {
                self.spxbndsz[n - 1] += 12;
            } else if n < 17 {
                self.spxbndsz[n] = 12;
                n += 1;
            }
        }
        self.nspxbnds = n;
    }
}

/// Bit allocation defaults when `bamode` is 0 (E.1.3.3.x).
const BA_DEFAULTS: (usize, usize, usize, usize, usize) = (2, 1, 1, 2, 7);

impl Decoder {
    /// Decode an E-AC-3 syncframe (`frame` is exactly `h.frame_bytes` long).
    pub(crate) fn decode_eac3(&mut self, frame: &[u8], h: Header) -> Result<Decoded> {
        if !h.decodes_audio() {
            return Ok(Decoded { header: h, channels: Vec::new() });
        }
        let mut r = Bits::new(frame);
        r.skip(16); // syncword
        // bsi
        let strmtyp = r.read(2);
        r.skip(3 + 11); // substreamid, frmsiz
        let fscod = r.read(2) as usize;
        let (numblkscod, hth) = if fscod == 3 { (3usize, r.read(2) as usize) } else { (r.read(2) as usize, fscod) };
        if hth > 2 {
            return Err(Error::Invalid("reserved fscod2"));
        }
        let numblks = [1usize, 2, 3, 6][numblkscod & 3];
        let acmod = r.read(3) as usize;
        let lfeon = r.bit();
        r.skip(5); // bsid
        r.skip(5); // dialnorm
        if r.bit() {
            r.skip(8); // compr
        }
        if acmod == 0 {
            r.skip(5); // dialnorm2
            if r.bit() {
                r.skip(8); // compr2
            }
        }
        if strmtyp == 1 && r.bit() {
            r.skip(16); // chanmap
        }
        if r.bit() {
            // mixmdate
            if acmod > 2 {
                r.skip(2); // dmixmod
            }
            if acmod & 1 != 0 && acmod > 2 {
                r.skip(6); // ltrtcmixlev, lorocmixlev
            }
            if acmod & 4 != 0 {
                r.skip(6); // ltrtsurmixlev, lorosurmixlev
            }
            if lfeon && r.bit() {
                r.skip(5); // lfemixlevcod
            }
            if strmtyp == 0 {
                if r.bit() {
                    r.skip(6); // pgmscl
                }
                if acmod == 0 && r.bit() {
                    r.skip(6); // pgmscl2
                }
                if r.bit() {
                    r.skip(6); // extpgmscl
                }
                match r.read(2) {
                    1 => r.skip(5), // premixcmpsel, drcsrc, premixcmpscl
                    2 => r.skip(12),
                    3 => {
                        let l = r.read(5) as usize;
                        r.skip((l + 2) * 8);
                    }
                    _ => {}
                }
                if acmod < 2 {
                    if r.bit() {
                        r.skip(14); // panmean, paninfo
                    }
                    if acmod == 0 && r.bit() {
                        r.skip(14);
                    }
                }
                if r.bit() {
                    // frmmixcfginfoe
                    if numblkscod == 0 {
                        r.skip(5);
                    } else {
                        for _ in 0..numblks {
                            if r.bit() {
                                r.skip(5);
                            }
                        }
                    }
                }
            }
        }
        if r.bit() {
            // infomdate
            r.skip(5); // bsmod, copyrightb, origbs
            if acmod == 2 {
                r.skip(4); // dsurmod, dheadphonmod
            }
            if acmod >= 6 {
                r.skip(2); // dsurexmod
            }
            if r.bit() {
                r.skip(8); // mixlevel, roomtyp, adconvtyp
            }
            if acmod == 0 && r.bit() {
                r.skip(8);
            }
            if fscod < 3 {
                r.skip(1); // sourcefscod
            }
        }
        if strmtyp == 0 && numblkscod != 3 {
            r.skip(1); // convsync
        }
        if strmtyp == 2 && (numblkscod == 3 || r.bit()) {
            r.skip(6); // frmsizecod
        }
        if r.bit() {
            let l = r.read(6) as usize;
            r.skip((l + 1) * 8); // addbsi
        }
        // audfrm
        let nf = NFCHANS[acmod & 7];
        let (expstre, ahte) = if numblkscod == 3 { (r.bit(), r.bit()) } else { (true, false) };
        let mut a = AudFrm {
            acmod,
            lfeon,
            nf,
            strmtyp,
            snroffststr: r.read(2),
            blkswe: false,
            dithflage: false,
            bamode: false,
            frmfgaincode: false,
            dbaflde: false,
            skipflde: false,
            cplstre: [false; 6],
            cplinu: [false; 6],
            cplexpstr: [0; 6],
            chexpstr: [[0; 5]; 6],
            lfeexpstr: [0; 6],
            frmcsnroffst: 0,
            frmfsnroffst: 0,
        };
        let transproce = r.bit();
        a.blkswe = r.bit();
        a.dithflage = r.bit();
        a.bamode = r.bit();
        a.frmfgaincode = r.bit();
        a.dbaflde = r.bit();
        a.skipflde = r.bit();
        let spxattene = r.bit();
        if acmod > 1 {
            a.cplstre[0] = true;
            a.cplinu[0] = r.bit();
            for blk in 1..numblks {
                a.cplstre[blk] = r.bit();
                a.cplinu[blk] = if a.cplstre[blk] { r.bit() } else { a.cplinu[blk - 1] };
            }
        }
        let ncplblks = a.cplinu[..numblks].iter().filter(|&&c| c).count();
        if expstre {
            for blk in 0..numblks {
                if a.cplinu[blk] {
                    a.cplexpstr[blk] = r.read(2) as u8;
                }
                for ch in 0..nf {
                    a.chexpstr[blk][ch] = r.read(2) as u8;
                }
            }
        } else {
            // six blocks (expstre is only coded with numblkscod 3)
            if acmod > 1 && ncplblks > 0 {
                a.cplexpstr = FRM_EXPSTR[r.read(5) as usize & 31];
            }
            for ch in 0..nf {
                let s = FRM_EXPSTR[r.read(5) as usize & 31];
                for (blk, &e) in s.iter().enumerate() {
                    a.chexpstr[blk][ch] = e;
                }
            }
        }
        if lfeon {
            for blk in 0..numblks {
                a.lfeexpstr[blk] = r.read(1) as u8;
            }
        }
        // converter exponent strategies: always present in six-block frames
        if strmtyp == 0 && (numblkscod == 3 || r.bit()) {
            r.skip(5 * nf); // convexpstr
        }
        if ahte {
            // AHT is signalled for exponent sets sent once and reused in blocks 1-5
            let mut aht = false;
            if ncplblks == 6 && (1..6).all(|b| !a.cplstre[b] && a.cplexpstr[b] == 0) {
                aht |= r.bit();
            }
            for ch in 0..nf {
                if (1..6).all(|b| a.chexpstr[b][ch] == 0) {
                    aht |= r.bit();
                }
            }
            if lfeon && (1..6).all(|b| a.lfeexpstr[b] == 0) {
                aht |= r.bit();
            }
            if aht {
                return Err(Error::Unsupported("E-AC-3 adaptive hybrid transform (AHT)".into()));
            }
        }
        if a.snroffststr == 0 {
            a.frmcsnroffst = r.read(6) as i32;
            a.frmfsnroffst = r.read(4) as i32;
        }
        if transproce {
            // transient pre-noise processing: not applied
            for _ in 0..nf {
                if r.bit() {
                    r.skip(10 + 8); // transprocloc, transproclen
                }
            }
        }
        if spxattene {
            // spectral extension attenuation: not applied
            for _ in 0..nf {
                if r.bit() {
                    r.skip(5);
                }
            }
        }
        if numblkscod != 0 && r.bit() {
            // blkstrtinfo
            let words = frame.len() / 2;
            r.skip((numblks - 1) * (4 + ceil_log2(words) as usize));
        }
        if r.overrun() {
            return Err(Error::Invalid("E-AC-3 header runs past the frame"));
        }
        // audio blocks
        let mut f = Frame::new(nf, hth);
        let mut e = EState::new();
        let nout = nf + lfeon as usize;
        let mut pcm: Vec<Vec<f32>> = vec![Vec::with_capacity(256 * numblks); nout];
        for blk in 0..numblks {
            let mut coefs = [[0f32; 256]; 6];
            self.eac3_block(&mut r, &mut f, &mut e, &a, blk, &mut coefs)?;
            if r.overrun() {
                return Err(Error::Invalid("audio block runs past the frame"));
            }
            self.output_block(&f, &coefs, &mut pcm);
        }
        Ok(Decoded { header: h, channels: wav_order(pcm, acmod, lfeon) })
    }

    /// One E-AC-3 audblk() (E.1.2.4) into `coefs`.
    fn eac3_block(&mut self, r: &mut Bits, f: &mut Frame, e: &mut EState, a: &AudFrm, blk: usize, coefs: &mut [[f32; 256]; 6]) -> Result<()> {
        let (nf, acmod, lfeon) = (a.nf, a.acmod, a.lfeon);
        for ch in 0..nf {
            f.blksw[ch] = a.blkswe && r.bit();
        }
        for ch in 0..nf {
            f.dithflag[ch] = !a.dithflage || r.bit();
        }
        for k in 0..if acmod == 0 { 2 } else { 1 } {
            if r.bit() {
                f.dynrng[k] = dynrng_gain(r.read(8));
            } else if blk == 0 {
                f.dynrng[k] = 1.0;
            }
        }
        // spectral extension strategy
        if blk == 0 || r.bit() {
            e.spxinu = r.bit();
            if e.spxinu {
                if acmod == 1 {
                    e.chinspx[0] = true;
                } else {
                    for ch in 0..nf {
                        e.chinspx[ch] = r.bit();
                    }
                }
                e.spxstrt = r.read(2) as usize;
                let begf = r.read(3) as usize;
                let endf = r.read(3) as usize;
                e.spxbegf = begf;
                e.spxbegin = if begf < 6 { begf + 2 } else { begf * 2 - 3 };
                e.spxend = if endf < 3 { endf + 5 } else { endf * 2 + 3 };
                if r.bit() {
                    for s in e.spxbegin + 1..e.spxend.min(17) {
                        e.spxbndstrc[s] = r.bit();
                    }
                }
                if e.spxend <= e.spxbegin || e.spxstrt >= e.spxbegin {
                    return Err(Error::Invalid("spectral extension bands out of order"));
                }
                e.spx_bands();
            } else {
                e.chinspx = [false; 5];
                e.firstspxcos = [true; 5];
            }
        }
        // spectral extension coordinates
        if e.spxinu {
            for ch in 0..nf {
                if !e.chinspx[ch] {
                    e.firstspxcos[ch] = true;
                    continue;
                }
                let coe = if e.firstspxcos[ch] {
                    e.firstspxcos[ch] = false;
                    true
                } else {
                    r.bit()
                };
                if coe {
                    e.spxblnd[ch] = r.read(5) as f32 / 32.0;
                    let mstr = r.read(2) as i32;
                    for bnd in 0..e.nspxbnds {
                        let ex = r.read(4) as i32;
                        let m = r.read(2) as f32;
                        let t = if ex == 15 { m / 4.0 } else { (m + 4.0) / 8.0 };
                        e.spxco[ch][bnd] = t * 2f32.powi(-(ex + 3 * mstr));
                    }
                }
            }
        }
        // coupling strategy
        if a.cplstre[blk] {
            f.cplinu = a.cplinu[blk];
            if f.cplinu {
                if r.bit() {
                    return Err(Error::Unsupported("E-AC-3 enhanced coupling".into()));
                }
                if acmod == 2 {
                    f.chincpl[0] = true;
                    f.chincpl[1] = true;
                } else {
                    for ch in 0..nf {
                        f.chincpl[ch] = r.bit();
                    }
                }
                f.phsflginu = acmod == 2 && r.bit();
                f.cplbegf = r.read(4) as usize;
                // with spectral extension coupling ends where the extension begins
                f.cplend = if e.spxinu { e.spxbegin.saturating_sub(1) } else { r.read(4) as usize + 3 };
                if f.cplend <= f.cplbegf {
                    return Err(Error::Invalid("coupling end below start"));
                }
                if r.bit() {
                    for s in f.cplbegf + 1..f.cplend.min(18) {
                        e.cplbndstrc[s] = r.bit();
                    }
                }
                let mut band = 0;
                f.sub_to_band[0] = 0;
                for s in 1..f.ncplsubnd() {
                    if !e.cplbndstrc.get(f.cplbegf + s).copied().unwrap_or(false) {
                        band += 1;
                    }
                    f.sub_to_band[s] = band;
                }
                f.ncplbnd = band + 1;
            } else {
                f.chincpl = [false; 5];
                e.firstcplcos = [true; 5];
                e.firstcplleak = true;
                f.phsflginu = false;
            }
        }
        // coupling coordinates
        if f.cplinu {
            let mut cplcoe = [false; 5];
            for ch in 0..nf {
                if !f.chincpl[ch] {
                    e.firstcplcos[ch] = true;
                    continue;
                }
                cplcoe[ch] = if e.firstcplcos[ch] {
                    e.firstcplcos[ch] = false;
                    true
                } else {
                    r.bit()
                };
                if cplcoe[ch] {
                    let mstr = r.read(2) as i32;
                    for bnd in 0..f.ncplbnd {
                        let ex = r.read(4) as i32;
                        let m = r.read(4) as f32;
                        let v = if ex == 15 { m / 16.0 } else { (m + 16.0) / 32.0 };
                        let co = v * 2f32.powi(-(ex + 3 * mstr));
                        for s in 0..f.ncplsubnd() {
                            if f.sub_to_band[s] == bnd {
                                f.cplco[ch][s] = co;
                            }
                        }
                    }
                }
            }
            if acmod == 2 && f.phsflginu && (cplcoe[0] || cplcoe[1]) {
                for bnd in 0..f.ncplbnd {
                    let flag = r.bit();
                    for s in 0..f.ncplsubnd() {
                        if f.sub_to_band[s] == bnd {
                            f.phsflg[s] = flag;
                        }
                    }
                }
            }
        }
        // rematrixing
        if acmod == 2 && (blk == 0 || r.bit()) {
            let n = if f.cplinu {
                if f.cplbegf > 2 {
                    4
                } else if f.cplbegf > 0 {
                    3
                } else {
                    2
                }
            } else if e.spxinu && e.spxbegf < 2 {
                3
            } else {
                4
            };
            f.rematflg = [false; 4];
            for k in 0..n {
                f.rematflg[k] = r.bit();
            }
        }
        // exponent strategies (from audfrm) and channel bandwidths
        if f.cplinu {
            f.expstr[CPL] = a.cplexpstr[blk];
        }
        for ch in 0..nf {
            f.expstr[ch] = a.chexpstr[blk][ch];
        }
        if lfeon {
            f.expstr[LFE] = a.lfeexpstr[blk];
        }
        for ch in 0..nf {
            if f.expstr[ch] != 0 {
                if f.cplinu && f.chincpl[ch] {
                    f.endmant[ch] = f.cplstrtmant();
                } else if e.spxinu && e.chinspx[ch] {
                    f.endmant[ch] = 25 + 12 * e.spxbegin;
                } else {
                    let bw = r.read(6) as usize;
                    if bw > 60 {
                        return Err(Error::Invalid("chbwcod > 60"));
                    }
                    f.endmant[ch] = (bw + 12) * 3 + 37;
                }
            } else if blk == 0 {
                return Err(Error::Invalid("exponent reuse in the first block"));
            }
        }
        if f.cplinu && blk == 0 && f.expstr[CPL] == 0 {
            return Err(Error::Invalid("coupling exponent reuse in the first block"));
        }
        if lfeon && blk == 0 && f.expstr[LFE] == 0 {
            return Err(Error::Invalid("LFE exponent reuse in the first block"));
        }
        // exponents
        if f.cplinu && f.expstr[CPL] != 0 {
            let abs = (r.read(4) as i32) << 1;
            let grpsize = 1 << (f.expstr[CPL] - 1);
            let ngrps = (f.cplendmant() - f.cplstrtmant()) / (3 * grpsize);
            let first = f.cplstrtmant();
            decode_exponents(r, abs, ngrps, grpsize, &mut f.exps[CPL], first, true)?;
        }
        for ch in 0..nf {
            if f.expstr[ch] != 0 {
                let abs = r.read(4) as i32;
                let grpsize = 1usize << (f.expstr[ch] - 1);
                let ngrps = (f.endmant[ch] - 1 + 3 * grpsize - 3) / (3 * grpsize);
                decode_exponents(r, abs, ngrps, grpsize, &mut f.exps[ch], 0, false)?;
                r.skip(2); // gainrng
            }
        }
        if lfeon && f.expstr[LFE] != 0 {
            let abs = r.read(4) as i32;
            decode_exponents(r, abs, 2, 1, &mut f.exps[LFE], 0, false)?;
        }
        // bit allocation parametric information
        if a.bamode {
            if r.bit() {
                f.g.sdcycod = r.read(2) as usize;
                f.g.fdcycod = r.read(2) as usize;
                f.g.sgaincod = r.read(2) as usize;
                f.g.dbpbcod = r.read(2) as usize;
                f.g.floorcod = r.read(3) as usize;
            } else if blk == 0 {
                (f.g.sdcycod, f.g.fdcycod, f.g.sgaincod, f.g.dbpbcod, f.g.floorcod) = BA_DEFAULTS;
            }
        } else {
            (f.g.sdcycod, f.g.fdcycod, f.g.sgaincod, f.g.dbpbcod, f.g.floorcod) = BA_DEFAULTS;
        }
        if a.snroffststr == 0 {
            f.csnroffst = a.frmcsnroffst;
            f.fsnroffst = [a.frmfsnroffst; 7];
        } else if blk == 0 || r.bit() {
            f.csnroffst = r.read(6) as i32;
            if a.snroffststr == 1 {
                f.fsnroffst = [r.read(4) as i32; 7];
            } else {
                if f.cplinu {
                    f.fsnroffst[CPL] = r.read(4) as i32;
                }
                for ch in 0..nf {
                    f.fsnroffst[ch] = r.read(4) as i32;
                }
                if lfeon {
                    f.fsnroffst[LFE] = r.read(4) as i32;
                }
            }
        }
        if a.frmfgaincode && r.bit() {
            if f.cplinu {
                f.fgaincod[CPL] = r.read(3) as usize;
            }
            for ch in 0..nf {
                f.fgaincod[ch] = r.read(3) as usize;
            }
            if lfeon {
                f.fgaincod[LFE] = r.read(3) as usize;
            }
        } else {
            f.fgaincod = [4; 7];
        }
        if a.strmtyp == 0 && r.bit() {
            r.skip(10); // convsnroffst
        }
        if f.cplinu {
            let leak = if e.firstcplleak {
                e.firstcplleak = false;
                true
            } else {
                r.bit()
            };
            if leak {
                f.cplleak = (((r.read(3) as i32) << 8) + 768, ((r.read(3) as i32) << 8) + 768);
            }
        }
        // delta bit allocation
        if blk == 0 {
            f.deltbae = [2; 7];
        }
        if a.dbaflde && r.bit() {
            if f.cplinu {
                f.deltbae[CPL] = r.read(2) as u8;
            }
            for ch in 0..nf {
                f.deltbae[ch] = r.read(2) as u8;
            }
            let order: Vec<usize> = f.cplinu.then_some(CPL).into_iter().chain(0..nf).collect();
            for ch in order {
                if f.deltbae[ch] == 1 {
                    let n = r.read(3) as usize + 1;
                    f.deltsegs[ch] = (0..n).map(|_| (r.read(5) as u8, r.read(4) as u8, r.read(3) as u8)).collect();
                } else if f.deltbae[ch] == 3 {
                    return Err(Error::Invalid("reserved delta bit allocation state"));
                }
            }
        }
        // skip field
        if a.skipflde && r.bit() {
            let l = r.read(9) as usize;
            r.skip(l * 8);
        }
        self.reconstruct(r, f, acmod, lfeon, coefs);
        self.spectral_extension(e, nf, coefs);
        Ok(())
    }

    /// Uniform noise in [-1, 1) for spectral extension.
    fn noise(&mut self) -> f32 {
        self.dither = self.dither.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.dither >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }

    /// Spectral extension (E.3.6): translate the band below the extension region upwards, then
    /// blend each band with noise and scale it by its coordinate.
    fn spectral_extension(&mut self, e: &EState, nf: usize, coefs: &mut [[f32; 256]; 6]) {
        if !e.spxinu {
            return;
        }
        let copystart = 25 + 12 * e.spxstrt;
        let copyend = 25 + 12 * e.spxbegin;
        let endbin = (25 + 12 * e.spxend) as f32;
        let bands = &e.spxbndsz[..e.nspxbnds.min(17)];
        for ch in 0..nf.min(5) {
            if !e.chinspx[ch] {
                continue;
            }
            let c = &mut coefs[ch];
            // translation, wrapping back to the copy start when the source runs out
            let (mut src, mut dst) = (copystart, copyend);
            for &size in bands {
                if src + size > copyend {
                    src = copystart;
                }
                for _ in 0..size {
                    if src >= copyend {
                        src = copystart;
                    }
                    if let (Some(&v), true) = (c.get(src), dst < 256) {
                        c[dst] = v;
                    }
                    src += 1;
                    dst += 1;
                }
            }
            // noise blending and scaling
            let mut bin = copyend;
            for (bnd, &size) in bands.iter().enumerate() {
                let hi = (bin + size).min(256);
                let Some(band) = c.get_mut(bin..hi) else { break };
                let nratio = ((bin as f32 + size as f32 / 2.0) / endbin - e.spxblnd[ch]).clamp(0.0, 1.0);
                let nblend = (3.0 * nratio).sqrt(); // unit-variance uniform noise
                let sblend = (1.0 - nratio).sqrt();
                let rms = (band.iter().map(|v| v * v).sum::<f32>() / size.max(1) as f32).sqrt();
                let co = e.spxco[ch][bnd] * 32.0;
                for v in band.iter_mut() {
                    *v = (*v * sblend + self.noise() * nblend * rms) * co;
                }
                bin += size;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_exponent_strategy_table() {
        const R: u8 = 0;
        const D15: u8 = 1;
        const D25: u8 = 2;
        const D45: u8 = 3;
        // spot checks against Table E2.10
        assert_eq!(FRM_EXPSTR[0], [D15, R, R, R, R, R]);
        assert_eq!(FRM_EXPSTR[1], [D15, R, R, R, R, D45]);
        assert_eq!(FRM_EXPSTR[3], [D15, R, R, R, D45, D45]);
        assert_eq!(FRM_EXPSTR[6], [D25, R, R, D45, D25, R]);
        assert_eq!(FRM_EXPSTR[8], [D25, R, D15, R, R, R]);
        assert_eq!(FRM_EXPSTR[13], [D25, R, D45, D25, R, D45]);
        assert_eq!(FRM_EXPSTR[16], [D45, D15, R, R, R, R]);
        assert_eq!(FRM_EXPSTR[22], [D45, D25, R, D45, D25, R]);
        assert_eq!(FRM_EXPSTR[26], [D45, D45, D25, R, D25, R]);
        assert_eq!(FRM_EXPSTR[31], [D45; 6]);
        // every row starts with new exponents
        assert!(FRM_EXPSTR.iter().all(|r| r[0] != R));
    }

    #[test]
    fn eac3_headers() {
        // strmtyp 0, substream 0, frmsiz 767 (1536 bytes), 48 kHz, 6 blocks, 3/2 + LFE, bsid 16
        let b = [0x0B, 0x77, 0x02, 0xFF, 0b0011_1111, 16 << 3, 0, 0];
        let h = super::super::parse_header(&b).unwrap();
        assert_eq!((h.sample_rate, h.frame_bytes, h.samples, h.channels(), h.bitrate_kbps), (48_000, 1536, 1536, 6, 384));
        assert!(h.is_eac3() && h.decodes_audio());
        // dependent substream, 1 block, 2/0
        let b = [0x0B, 0x77, 0x40, 0x3F, 0b0000_0100, 16 << 3, 0, 0];
        let h = super::super::parse_header(&b).unwrap();
        assert_eq!((h.samples, h.dependent, h.decodes_audio()), (256, true, false));
        // 24 kHz (fscod 3, fscod2 0)
        let b = [0x0B, 0x77, 0x01, 0x7F, 0b1100_0010, 16 << 3, 0, 0];
        assert_eq!(super::super::parse_header(&b).unwrap().sample_rate, 24_000);
        // reserved fscod2, reserved strmtyp, bsid 17
        assert!(super::super::parse_header(&[0x0B, 0x77, 0x01, 0x7F, 0b1111_0010, 16 << 3, 0, 0]).is_err());
        assert!(super::super::parse_header(&[0x0B, 0x77, 0xC1, 0x7F, 0b0011_0010, 16 << 3, 0, 0]).is_err());
        assert_eq!(super::super::parse_header(&[0x0B, 0x77, 0, 0, 0, 17 << 3, 0, 0]), Err(Error::Unsupported("bsid 17".into())));
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(768), 10);
        assert_eq!(ceil_log2(1024), 10);
        assert_eq!(ceil_log2(1025), 11);
    }

    #[test]
    fn dependent_substreams_decode_to_nothing() {
        let mut f = vec![0u8; 128];
        f[..6].copy_from_slice(&[0x0B, 0x77, 0x40, 63, 0b0011_0100, 16 << 3]);
        let d = Decoder::new().decode(&f).unwrap();
        assert!(d.channels.is_empty() && d.header.dependent);
    }

    #[test]
    fn hostile_eac3_frames_never_panic() {
        let mut d = Decoder::new();
        let mut x = 0xEAC3u32;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for len in [7usize, 8, 16, 64, 300, 1000, 4096] {
            for _ in 0..400 {
                let mut f: Vec<u8> = (0..len).map(|_| next() as u8).collect();
                f[0] = 0x0B;
                f[1] = 0x77;
                f[2] &= 0x07; // independent substream 0
                f[5] = (f[5] & 7) | (16 << 3);
                // frame size within the buffer most of the time
                let words = (len / 2).clamp(1, 2048) - 1;
                if next() & 3 != 0 {
                    f[2] = (f[2] & 0xF8) | ((words >> 8) as u8 & 7);
                    f[3] = words as u8;
                }
                let _ = d.decode(&f);
            }
        }
    }
}
