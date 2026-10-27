//! Round trips through symphonia, the decoder FilmCraft imports FLAC with (always available, unlike
//! the ffmpeg oracle): every sample back exactly, and the STREAMINFO MD5 verified by the decoder.

use filmcraft_flac::{Encoder, EncoderConfig, STREAMINFO_OFFSET};
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

fn encode(planar: &[Vec<i32>], cfg: EncoderConfig, chunk: usize) -> Vec<u8> {
    let mut enc = Encoder::new(cfg).unwrap();
    let mut file = enc.header();
    let n = planar.first().map_or(0, Vec::len);
    let mut at = 0;
    while at < n {
        let end = (at + chunk).min(n);
        let parts: Vec<&[i32]> = planar.iter().map(|c| &c[at..end]).collect();
        file.extend(enc.encode(&parts).unwrap());
        at = end;
    }
    file.extend(enc.finish());
    let o = STREAMINFO_OFFSET as usize;
    file[o..o + 34].copy_from_slice(&enc.streaminfo());
    file
}

/// Decode with symphonia (MD5 verification on): planar samples at their native size.
fn decode(file: &[u8], bps: u8) -> Vec<Vec<i32>> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(file.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).unwrap();
    let mut format = probed.format;
    let track = format.default_track().unwrap().clone();
    let mut dec = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions { verify: true }).unwrap();
    let mut out: Vec<Vec<i32>> = Vec::new();
    while let Ok(pkt) = format.next_packet() {
        let buf = dec.decode(&pkt).unwrap();
        let AudioBufferRef::S32(buf) = buf else { panic!("FLAC decodes to i32") };
        let planes = buf.planes();
        if out.is_empty() {
            out = vec![Vec::new(); planes.planes().len()];
        }
        for (o, p) in out.iter_mut().zip(planes.planes()) {
            // symphonia scales samples to the full 32-bit range
            o.extend(p.iter().map(|&v| v >> (32 - u32::from(bps))));
        }
    }
    let fin = dec.finalize();
    assert_eq!(fin.verify_ok, Some(true), "STREAMINFO MD5 must match the decoded audio");
    out
}

fn sine(n: usize, bps: u8, freq: f64, amp: f64) -> Vec<i32> {
    let max = f64::from((1i32 << (bps - 1)) - 1);
    (0..n).map(|i| ((2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin() * amp * max).round() as i32).collect()
}

fn noise(n: usize, bps: u8, mut seed: u64) -> Vec<i32> {
    let half = 1i64 << (bps - 1);
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as i64 % (2 * half) - half) as i32
        })
        .collect()
}

fn check(name: &str, planar: Vec<Vec<i32>>, cfg: EncoderConfig) -> usize {
    let bps = cfg.bits_per_sample;
    let file = encode(&planar, cfg, 3001);
    let back = decode(&file, bps);
    assert_eq!(back.len(), planar.len(), "{name}: channels");
    for (c, (a, b)) in planar.iter().zip(&back).enumerate() {
        assert_eq!(a.len(), b.len(), "{name}: channel {c} length");
        assert!(a == b, "{name}: channel {c} differs");
    }
    file.len()
}

#[test]
fn signals_round_trip_bit_exactly() {
    let n = 48_000 + 1234; // the last block is short
    check("silence", vec![vec![0; n], vec![0; n]], EncoderConfig::new(48_000, 2, 16));
    let s = sine(n, 16, 440.0, 0.8);
    let pcm = n * 2 * 2;
    let size = check("sine", vec![s.clone(), sine(n, 16, 660.0, 0.5)], EncoderConfig::new(48_000, 2, 16));
    assert!(size * 100 < pcm * 60, "a sine compresses below 60% of PCM: {size} of {pcm}");
    check("noise16", vec![noise(n, 16, 3), noise(n, 16, 5)], EncoderConfig::new(48_000, 2, 16));
    check("identical", vec![s.clone(), s], EncoderConfig::new(48_000, 2, 16));
    check("noise24", vec![noise(n, 24, 7), sine(n, 24, 1000.0, 0.99)], EncoderConfig::new(96_000, 2, 24));
    check("mono", vec![sine(n, 24, 100.0, 0.3)], EncoderConfig::new(44_100, 1, 24));
    let six: Vec<Vec<i32>> = (0..6).map(|c| if c == 3 { vec![0; 5000] } else { sine(5000, 16, 200.0 * f64::from(c + 1), 0.4) }).collect();
    check("5.1", six, EncoderConfig::new(48_000, 6, 16));
    check("odd-rate", vec![noise(777, 16, 9)], EncoderConfig::new(37_800, 1, 16));
}

#[test]
fn every_level_round_trips() {
    for level in 0..=8 {
        let cfg = EncoderConfig { level, ..EncoderConfig::new(48_000, 2, 16) };
        check(&format!("level{level}"), vec![sine(10_000, 16, 300.0, 0.7), noise(10_000, 16, 11)], cfg);
    }
}
