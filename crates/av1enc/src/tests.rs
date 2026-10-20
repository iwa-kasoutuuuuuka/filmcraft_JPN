use super::*;

/// A moving gradient with a bright square: planes (y, u, v) of frame `i`.
fn picture(w: usize, h: usize, i: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            let g = 16 + ((c + r + i * 3) * 200 / (w + h)) as u32;
            let sq = (c + w - (i * 2) % w) % w < w / 4 && r > h / 3 && r < h / 3 + h / 4;
            y[r * w + c] = if sq { 230 } else { g.min(235) as u8 };
        }
    }
    let u = (0..cw * ch).map(|k| (100 + (k % cw) * 40 / cw) as u8).collect();
    let v = (0..cw * ch).map(|k| (150 - (k / cw) * 40 / ch) as u8).collect();
    (y, u, v)
}

fn psnr(a: &[u8], b: &[u16]) -> f64 {
    let n = a.len().min(b.len()).max(1);
    let mse = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / n as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

fn encode_all(cfg: &EncoderConfig, frames: usize) -> (Encoder, Vec<Packet>, Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>) {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut enc = Encoder::new(cfg).unwrap();
    let mut pics = Vec::new();
    let mut packets = Vec::new();
    for i in 0..frames {
        let (y, u, v) = picture(w, h, i);
        packets.extend(enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: w, uv_stride: w.div_ceil(2) }).unwrap());
        pics.push((y, u, v));
    }
    packets.extend(enc.flush().unwrap());
    (enc, packets, pics)
}

/// Encode with rav1e, decode with our own AV1 decoder (filmcraft-av1): one shown frame per
/// packet, in order, every frame close to its source.
#[test]
fn roundtrip_through_our_decoder() {
    let mut cfg = EncoderConfig::new(96, 64, 24, 1);
    cfg.rate = RateControl::Quantizer(60);
    cfg.keyint = 8;
    let (enc, packets, pics) = encode_all(&cfg, 12);
    assert_eq!(packets.len(), 12);
    for (i, p) in packets.iter().enumerate() {
        assert_eq!(p.frame_index, i as u64);
        assert!(!p.data.is_empty());
        // no temporal delimiters in MP4 samples
        assert!(obus(&p.data).unwrap().iter().all(|o| o.obu_type != OBU_TEMPORAL_DELIMITER));
    }
    assert!(packets[0].keyframe, "first unit is a key frame");
    assert!(packets.iter().skip(1).any(|p| !p.keyframe), "inter frames follow");
    assert!(packets[8].keyframe, "keyframe interval 8");
    let seq = enc.sequence_header_obu().unwrap().to_vec();
    let info = sequence_info(&seq).unwrap();
    assert_eq!(info.profile, 0);
    let av1c = enc.av1c();
    assert_eq!(av1c[0], 0x81);
    assert_eq!(av1c[1] >> 5, 0);
    assert_eq!(av1c[1] & 0x1F, info.level);
    assert_eq!(&av1c[4..], &seq[..]);
    // our decoder agrees with the parsed sequence header
    let parsed = filmcraft_av1::SequenceHeader::parse(obus(&seq).unwrap()[0].payload).unwrap();
    assert_eq!((parsed.profile, parsed.seq_level_idx[0]), (info.profile, info.level));
    assert_eq!(parsed.color.color_primaries, 1);
    assert!(!parsed.color.color_range);

    let mut dec = filmcraft_av1::Decoder::new();
    let mut shown = Vec::new();
    // feed the configOBUs first, as a demuxer does from av1C
    dec.decode(&av1c[4..]).unwrap();
    for p in &packets {
        shown.extend(dec.decode(&p.data).unwrap());
    }
    shown.extend(dec.flush());
    assert_eq!(shown.len(), 12);
    for (i, (pic, (y, u, v))) in shown.iter().zip(&pics).enumerate() {
        assert_eq!((pic.width, pic.height, pic.bit_depth), (96, 64, 8));
        let (py, pu, pv) = (psnr(y, &pic.planes[0]), psnr(u, &pic.planes[1]), psnr(v, &pic.planes[2]));
        assert!(py > 30.0 && pu > 30.0 && pv > 30.0, "frame {i}: PSNR {py:.1} / {pu:.1} / {pv:.1}");
    }
}

/// Bitrate mode, odd sizes (4:2:0 chroma rounds up) and padded strides.
#[test]
fn bitrate_mode_odd_size_and_strides() {
    let (w, h) = (35usize, 21usize);
    let mut cfg = EncoderConfig::new(w as u32, h as u32, 30000, 1001);
    cfg.rate = RateControl::Bitrate { kbps: 500 };
    let mut enc = Encoder::new(&cfg).unwrap();
    let (y, u, v) = picture(w, h, 0);
    let (ys, cs) = (w + 13, w.div_ceil(2) + 5);
    let pad = |src: &[u8], pw: usize, stride: usize, rows: usize, last_short: bool| -> Vec<u8> {
        let mut out = Vec::new();
        for r in 0..rows {
            out.extend_from_slice(&src[r * pw..(r + 1) * pw]);
            if r + 1 < rows || !last_short {
                out.extend(std::iter::repeat_n(0u8, stride - pw));
            }
        }
        out
    };
    let mut packets = Vec::new();
    for short in [false, true, false] {
        let (py, pu, pv) = (pad(&y, w, ys, h, short), pad(&u, w.div_ceil(2), cs, h.div_ceil(2), short), pad(&v, w.div_ceil(2), cs, h.div_ceil(2), short));
        packets.extend(enc.encode(&YuvFrame { y: &py, u: &pu, v: &pv, y_stride: ys, uv_stride: cs }).unwrap());
    }
    packets.extend(enc.flush().unwrap());
    assert_eq!(packets.len(), 3);
    let mut dec = filmcraft_av1::Decoder::new();
    let mut shown = Vec::new();
    for p in &packets {
        shown.extend(dec.decode(&p.data).unwrap());
    }
    shown.extend(dec.flush());
    assert_eq!(shown.len(), 3);
    for pic in &shown {
        assert_eq!((pic.width, pic.height), (w as u32, h as u32));
        assert!(psnr(&y, &pic.planes[0]) > 25.0);
    }
}

/// Hostile settings and frames are errors, never panics.
#[test]
fn hostile_settings_are_errors() {
    let base = EncoderConfig::new(64, 64, 30, 1);
    let bad = [
        EncoderConfig { width: 0, ..base.clone() },
        EncoderConfig { height: 15, ..base.clone() },
        EncoderConfig { width: 70_000, ..base.clone() },
        EncoderConfig { fps_num: 0, ..base.clone() },
        EncoderConfig { fps_den: 0, ..base.clone() },
        EncoderConfig { fps_num: u32::MAX, fps_den: 1, ..base.clone() },
        EncoderConfig { fps_num: 1, fps_den: u32::MAX, ..base.clone() },
        EncoderConfig { speed: 11, ..base.clone() },
        EncoderConfig { keyint: 0, ..base.clone() },
        EncoderConfig { keyint: u32::MAX, ..base.clone() },
        EncoderConfig { rate: RateControl::Bitrate { kbps: 0 }, ..base.clone() },
        EncoderConfig { sar: (0, 1), ..base.clone() },
        EncoderConfig { sar: (1, 0), ..base.clone() },
        // render width 64 × 2³²−1 does not fit AV1's 16-bit render size
        EncoderConfig { sar: (u32::MAX, 1), ..base.clone() },
    ];
    for cfg in bad {
        assert!(Encoder::new(&cfg).is_err(), "{cfg:?}");
    }
    // extreme but valid numbers still work
    let ok = EncoderConfig { rate: RateControl::Bitrate { kbps: u32::MAX }, fps_num: 1000, fps_den: 1, tiles: usize::MAX, ..base.clone() };
    let mut enc = Encoder::new(&ok).unwrap();
    // short planes / bad strides
    let (y, u, v) = (vec![0u8; 64 * 64], vec![0u8; 32 * 32], vec![0u8; 32 * 32]);
    for f in [
        YuvFrame { y: &y[..100], u: &u, v: &v, y_stride: 64, uv_stride: 32 },
        YuvFrame { y: &y, u: &u[..10], v: &v, y_stride: 64, uv_stride: 32 },
        YuvFrame { y: &y, u: &u, v: &v, y_stride: 0, uv_stride: 32 },
        YuvFrame { y: &y, u: &u, v: &v, y_stride: 64, uv_stride: 1 },
        YuvFrame { y: &y, u: &u, v: &v, y_stride: usize::MAX, uv_stride: usize::MAX },
    ] {
        assert!(matches!(enc.encode(&f), Err(Error::Frame(_))));
    }
    enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: 64, uv_stride: 32 }).unwrap();
    let n = enc.flush().unwrap().len() + enc.flush().unwrap().len();
    assert_eq!(n, 1);
    assert!(enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: 64, uv_stride: 32 }).is_err(), "no frames after flush");
}

/// The OBU splitter and sequence-header reader survive truncation and bit flips.
#[test]
fn obu_parsing_fuzz() {
    let mut cfg = EncoderConfig::new(32, 32, 25, 1);
    cfg.rate = RateControl::Quantizer(100);
    let (enc, packets, _) = encode_all(&cfg, 2);
    let seq = enc.sequence_header_obu().unwrap().to_vec();
    let mut inputs = vec![seq.clone(), packets[0].data.clone()];
    inputs.extend((0..seq.len()).map(|n| seq[..n].to_vec()));
    let mut state = 0x1234_5678u32;
    for _ in 0..2000 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let mut d = packets[0].data.clone();
        let len = d.len();
        d[state as usize % len] ^= 1 << (state % 8);
        d.truncate(len - (state as usize >> 8) % len.min(16));
        inputs.push(d);
    }
    for d in inputs {
        let r = std::panic::catch_unwind(|| {
            let _ = obus(&d);
            let _ = sequence_info(&d);
            let _ = strip_obus(&d);
            let _ = is_shown_key_frame(&d);
        });
        assert!(r.is_ok());
    }
    assert!(sequence_info(&[]).is_err());
    assert!(obus(&[0x12, 0x80]).is_err(), "unterminated leb128");
    assert!(obus(&[0x0A, 0x05, 0x00]).is_err(), "size past the end");
}

/// A panic inside rav1e becomes an error (and the encoder refuses further work).
#[test]
fn rav1e_panics_become_errors() {
    let r: Result<()> = guarded("probe", || panic!("boom"));
    assert_eq!(r, Err(Error::Encoder("probe: internal error (boom)".into())));
    let mut enc = Encoder::new(&EncoderConfig::new(32, 32, 25, 1)).unwrap();
    enc.broken = true;
    let (y, u, v) = (vec![0u8; 32 * 32], vec![0u8; 16 * 16], vec![0u8; 16 * 16]);
    assert!(enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: 32, uv_stride: 16 }).is_err());
    assert!(enc.flush().is_err());
}
