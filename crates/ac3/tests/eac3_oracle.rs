//! E-AC-3 against ffmpeg: ffmpeg's E-AC-3 encoder writes raw syncframes from generated tones and
//! noise; ffmpeg's decode of the same file is the reference. AC-3 output is not bit-exact across
//! decoders (zero-bit mantissas are filled with decoder-specific dither, §7.3.4; float vs fixed
//! arithmetic), so the criterion is SNR against ffmpeg's decode.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Spec {
    name: &'static str,
    input: &'static str,
    rate: u32,
    channels: u32,
    enc: &'static [&'static str],
    /// Minimum SNR (dB) against ffmpeg.
    min_snr: f64,
}

const SPECS: &[Spec] = &[
    Spec {
        name: "eac3_stereo_192k",
        input: "aevalsrc=exprs=0.3*sin(2*PI*441*t)+0.1*sin(2*PI*3100*t)|0.25*sin(2*PI*660*t)+0.1*sin(2*PI*(500+4000*t)*t):s=48000",
        rate: 48_000,
        channels: 2,
        enc: &["-b:a", "192k"],
        min_snr: 60.0,
    },
    // low bitrate stereo: coupling and rematrixing
    Spec {
        name: "eac3_stereo_96k_coupling",
        input: "aevalsrc=exprs=0.3*sin(2*PI*441*t)+0.05*sin(2*PI*9100*t)|0.3*sin(2*PI*441*t)+0.05*sin(2*PI*11000*t):s=44100",
        rate: 44_100,
        channels: 2,
        enc: &["-b:a", "96k", "-channel_coupling", "1"],
        min_snr: 40.0,
    },
    Spec { name: "eac3_mono_32k", input: "sine=frequency=1000:sample_rate=32000", rate: 32_000, channels: 1, enc: &["-b:a", "64k"], min_snr: 60.0 },
    // 5.1 with LFE at the usual streaming / broadcast rates, with downmix and informational metadata
    Spec {
        name: "eac3_51_384k",
        input: "aevalsrc=exprs=0.3*sin(2*PI*300*t)|0.3*sin(2*PI*400*t)|0.3*sin(2*PI*500*t)|0.3*sin(2*PI*60*t)|0.2*sin(2*PI*700*t)|0.2*sin(2*PI*800*t):s=48000:c=5.1",
        rate: 48_000,
        channels: 6,
        enc: &[
            "-b:a",
            "384k",
            "-dmix_mode",
            "loro",
            "-ltrt_cmixlev",
            "0.707",
            "-loro_surmixlev",
            "0.5",
            "-dsurex_mode",
            "on",
            "-room_type",
            "large",
            "-mixing_level",
            "100",
        ],
        min_snr: 55.0,
    },
    Spec {
        name: "eac3_51_640k",
        input: "aevalsrc=exprs=0.3*sin(2*PI*300*t)+0.05*sin(2*PI*7000*t)|0.3*sin(2*PI*400*t)|0.3*sin(2*PI*500*t)|0.3*sin(2*PI*60*t)|0.2*sin(2*PI*700*t)+0.05*sin(2*PI*12000*t)|0.2*sin(2*PI*800*t):s=48000:c=5.1",
        rate: 48_000,
        channels: 6,
        enc: &["-b:a", "640k", "-per_frame_metadata", "1"],
        min_snr: 55.0,
    },
    // 2/1 without LFE (acmod 4), stereo-coupled surround
    Spec {
        name: "eac3_21_128k",
        input: "aevalsrc=exprs=0.3*sin(2*PI*330*t)|0.3*sin(2*PI*440*t)|0.2*sin(2*PI*550*t):s=48000:c=2.1",
        rate: 48_000,
        channels: 3,
        enc: &["-b:a", "160k"],
        min_snr: 50.0,
    },
    // noise (wideband, many zero-bit mantissas): the SNR is limited by dither
    Spec {
        name: "eac3_noise_stereo",
        input: "anoisesrc=color=pink:sample_rate=48000:amplitude=0.3:seed=3,pan=stereo|c0=c0|c1=-0.5*c0",
        rate: 48_000,
        channels: 2,
        enc: &["-b:a", "128k"],
        min_snr: 18.0,
    },
];

fn make(ff: &Path, s: &Spec) -> Option<PathBuf> {
    let out = filmcraft_testkit::fixtures_dir("eac3").join(format!("{}.eac3", s.name));
    filmcraft_testkit::fixtures::generate(&out, |tmp| {
        let st = Command::new(ff)
            .args(["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", s.input, "-t", "1"])
            .args(["-c:a", "eac3"])
            .args(s.enc)
            .args(["-f", "eac3"])
            .arg(tmp)
            .stdin(Stdio::null())
            .status();
        matches!(st, Ok(x) if x.success())
    })
}

fn frames(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut p = 0;
    while let Ok(h) = filmcraft_ac3::parse_header(&data[p..]) {
        if p + h.frame_bytes > data.len() {
            break;
        }
        out.push(&data[p..p + h.frame_bytes]);
        p += h.frame_bytes;
    }
    out
}

#[test]
fn ffmpeg_encoded_eac3_streams_decode() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let mut failed = Vec::new();
    for s in SPECS {
        let f = make(&ff, s).unwrap_or_else(|| panic!("could not generate {}", s.name));
        let data = std::fs::read(&f).unwrap();
        let mut dec = filmcraft_ac3::Decoder::new();
        let mut ours: Vec<Vec<f32>> = vec![Vec::new(); s.channels as usize];
        for fr in frames(&data) {
            let d = dec.decode(fr).unwrap();
            assert_eq!((d.header.sample_rate, d.channels.len() as u32), (s.rate, s.channels), "{}", s.name);
            for (o, c) in ours.iter_mut().zip(d.channels) {
                o.extend(c);
            }
        }
        let o = Command::new(&ff).args(["-v", "error", "-i"]).arg(&f).args(["-f", "f32le", "-"]).output().unwrap();
        let want: Vec<f32> = o.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let ch = s.channels as usize;
        let n = want.len() / ch;
        assert_eq!(n, ours[0].len(), "{}: sample count", s.name);
        let mut worst = f64::INFINITY;
        for c in 0..ch {
            let (mut sig, mut err) = (0f64, 0f64);
            for i in 0..n {
                let (a, b) = (ours[c][i] as f64, want[i * ch + c] as f64);
                sig += b * b;
                err += (a - b) * (a - b);
            }
            let snr = 10.0 * (sig / err.max(1e-30)).log10();
            worst = worst.min(snr);
        }
        // the same stream decoded with another dither sequence: how far dither alone moves the
        // output (the difference to ffmpeg must be of that order)
        let mut dec2 = filmcraft_ac3::Decoder::new();
        dec2.set_dither_seed(0xDEAD_BEEF);
        let other: Vec<f32> = frames(&data).iter().flat_map(|fr| dec2.decode(fr).unwrap().channels.swap_remove(0)).collect();
        let (mut sig, mut err) = (0f64, 0f64);
        for i in 0..n {
            sig += (ours[0][i] as f64).powi(2);
            err += (ours[0][i] as f64 - other[i] as f64).powi(2);
        }
        let dither_snr = 10.0 * (sig / err.max(1e-30)).log10();
        println!(
            "{}: {} Hz, {} ch, {} frames, worst channel SNR vs ffmpeg {worst:.1} dB (two dither seeds: {dither_snr:.1} dB)",
            s.name,
            s.rate,
            ch,
            frames(&data).len()
        );
        if worst < s.min_snr {
            failed.push(format!("{}: SNR {worst:.1} dB < {}", s.name, s.min_snr));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

/// Valid E-AC-3 streams with bits flipped or cut short: decoding returns `Ok` or `Err`, never
/// panics (flipped flags reach spectral extension, AHT, enhanced coupling, block switching and
/// the other paths ffmpeg's encoder never writes).
#[test]
fn corrupted_eac3_streams_never_panic() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let mut x = 0x5EED_E4C3u32;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let (mut ok, mut err) = (0, 0);
    for s in SPECS {
        let f = make(&ff, s).unwrap_or_else(|| panic!("could not generate {}", s.name));
        let data = std::fs::read(&f).unwrap();
        let mut dec = filmcraft_ac3::Decoder::new();
        for fr in frames(&data).into_iter().take(12) {
            for trial in 0..60 {
                let mut m = fr.to_vec();
                // flips in the first 40 bytes (bsi, audfrm, first block) or anywhere
                for _ in 0..1 + trial % 4 {
                    let at = if rnd() & 1 == 0 { 2 + rnd() as usize % 38 } else { rnd() as usize % m.len() };
                    m[at] ^= 1 << (rnd() % 8);
                }
                if trial % 7 == 0 {
                    m.truncate(rnd() as usize % m.len());
                }
                match dec.decode(&m) {
                    Ok(_) => ok += 1,
                    Err(_) => err += 1,
                }
            }
        }
    }
    println!("corrupted frames: {ok} decoded, {err} rejected");
}

#[test]
#[ignore]
fn generate_fixtures() {
    let Some(ff) = filmcraft_testkit::ffmpeg_or_skip("eac3 fixtures") else { return };
    for s in SPECS {
        let out = filmcraft_testkit::fixtures_dir("eac3").join(format!("{}.eac3", s.name));
        filmcraft_testkit::fixtures::generate_and_report(&format!("eac3/{}", s.name), &[out], || make(&ff, s));
    }
}
