//! E-AC-3 (Dolby Digital Plus) audio in MP4 (`ec-3` + `dec3`), Matroska (`A_EAC3`) and MPEG-2 TS
//! (stream type 0x87): ffmpeg writes the files, and its decode of the same file is the oracle.
//! E-AC-3 output is not bit-exact across decoders (zero-bit mantissas carry decoder-specific
//! dither, A/52 §7.3.4), so the criterion is the SNR against ffmpeg.

mod common;

use std::path::Path;

use common::*;

const SURROUND: &str =
    "aevalsrc=exprs=0.3*sin(2*PI*300*t)|0.3*sin(2*PI*400*t)|0.3*sin(2*PI*500*t)|0.3*sin(2*PI*60*t)|0.2*sin(2*PI*700*t)|0.2*sin(2*PI*800*t):s=48000:c=5.1";
const STEREO: &str = "aevalsrc=exprs=0.3*sin(2*PI*441*t)+0.1*sin(2*PI*3100*t)|0.25*sin(2*PI*660*t)+0.1*sin(2*PI*(500+4000*t)*t):s=48000";

/// Worst-channel SNR (dB) of our decode of `file` against ffmpeg's.
fn snr(ff: &Path, file: &Path, channels: u32) -> f64 {
    let name = file.file_name().unwrap().to_str().unwrap();
    let src = filmcraft_codecs::open_bytes(name, bytes(file)).unwrap();
    let a = src.info().audio.clone().unwrap();
    assert_eq!((a.codec.as_str(), a.channels, a.sample_rate), ("E-AC-3", channels, 48_000), "{name}");
    let want = ffmpeg_audio_f32(ff, file, &[]);
    let ch = channels as usize;
    let n = want.len() / ch;
    assert!(n >= 40_000, "{name}: {n} samples");
    // Matroska stores the encoder's 256-sample priming as a millisecond timestamp (-5 ms), so the
    // two decodes may be placed a few samples apart: align on the best lag (|lag| ≤ 512)
    const MAXLAG: i64 = 512;
    let buf = src.audio(-MAXLAG, n + 2 * MAXLAG as usize, 48_000).unwrap();
    let range = 2000..n - 2000;
    let lag = (0..=2 * MAXLAG)
        .map(|l| {
            let e: f64 = range.clone().map(|i| (buf.channels[0][i + l as usize] as f64 - want[i * ch] as f64).powi(2)).sum();
            (e, l)
        })
        .fold((f64::MAX, 0), |a, b| if b.0 < a.0 { b } else { a })
        .1;
    if lag != MAXLAG {
        println!("{name}: aligned at a lag of {} samples", lag - MAXLAG);
    }
    let mut worst = f64::INFINITY;
    for c in 0..ch {
        let (mut sig, mut err) = (0f64, 0f64);
        for i in range.clone() {
            let (x, y) = (buf.channels[c][i + lag as usize] as f64, want[i * ch + c] as f64);
            sig += y * y;
            err += (x - y) * (x - y);
        }
        worst = worst.min(10.0 * (sig / err.max(1e-30)).log10());
    }
    worst
}

#[test]
fn eac3_in_mp4_mkv_and_ts() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    for (name, input, rate, channels, fmt) in [
        ("eac3_51.mp4", SURROUND, "384k", 6, "mp4"),
        ("eac3_51.mkv", SURROUND, "448k", 6, "matroska"),
        ("eac3_51.ts", SURROUND, "640k", 6, "mpegts"),
        ("eac3_stereo.mp4", STEREO, "192k", 2, "mp4"),
        ("eac3_stereo.ts", STEREO, "128k", 2, "mpegts"),
    ] {
        let f = fixture(&ff, name, &["-f", "lavfi", "-i", input, "-t", "1", "-c:a", "eac3", "-b:a", rate, "-f", fmt])
            .unwrap_or_else(|| panic!("could not generate {name}"));
        let s = snr(&ff, &f, channels);
        println!("{name}: worst channel SNR vs ffmpeg {s:.1} dB");
        assert!(s >= 55.0, "{name}: SNR {s:.1} dB");
    }
}

/// A packet holding an independent syncframe followed by a dependent substream (the layout of
/// 7.1 E-AC-3 in MP4 / Matroska) decodes to the independent substream alone.
#[test]
fn dependent_substreams_in_a_packet_are_skipped() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let f = fixture(&ff, "eac3_51_raw.eac3", &["-f", "lavfi", "-i", SURROUND, "-t", "0.2", "-c:a", "eac3", "-b:a", "384k", "-f", "eac3"]).unwrap();
    let data = std::fs::read(&f).unwrap();
    let h = filmcraft_ac3::parse_header(&data).unwrap();
    let first = &data[..h.frame_bytes];
    // a dependent substream (strmtyp 1) of 64 words, 2/0, six blocks: skipped without decoding
    let mut dep = vec![0u8; 128];
    dep[..6].copy_from_slice(&[0x0B, 0x77, 0x40, 63, 0b0011_0100, 16 << 3]);
    let mut plain = filmcraft_codecs::audio::PacketDecoder::ac3().unwrap();
    let want = plain.decode(first, 0).unwrap();
    let mut with_dep = filmcraft_codecs::audio::PacketDecoder::ac3().unwrap();
    let packet = [first, &dep].concat();
    let got = with_dep.decode(&packet, 0).unwrap();
    assert_eq!((got.len(), got[0].len()), (6, 1536));
    assert_eq!(got, want);
    // a truncated packet is an error, not a panic
    let mut d = filmcraft_codecs::audio::PacketDecoder::ac3().unwrap();
    assert!(d.decode(&first[..100], 0).is_err());
}

/// 5.1 AAC comes out of our decoder in the WAV / SMPTE order (L R C LFE Ls Rs) the mixer and
/// the BS.775 fold-down assume, not AAC's own element order (C, L/R, surrounds, LFE): before,
/// the centre landed in the left channel. Each channel carries its own tone, so a swap shows as
/// a low SNR on the swapped channels.
#[test]
fn aac_51_channels_come_out_in_wav_order() {
    let ff = filmcraft_testkit::require_ffmpeg!();
    let f = fixture(&ff, "aac_51_order.mp4", &["-f", "lavfi", "-i", SURROUND, "-t", "1", "-c:a", "aac", "-b:a", "384k", "-f", "mp4"])
        .unwrap_or_else(|| panic!("could not generate aac_51_order.mp4"));
    let src = filmcraft_codecs::open_bytes("aac_51_order.mp4", bytes(&f)).unwrap();
    assert_eq!(src.info().audio.as_ref().map(|a| a.channels), Some(6));
    let want = ffmpeg_audio_f32(&ff, &f, &[]);
    let n = want.len() / 6;
    let buf = src.audio(0, n, 48_000).unwrap();
    for c in 0..6 {
        let (mut sig, mut err) = (0f64, 0f64);
        for i in 4000..n - 4000 {
            let (x, y) = (buf.channels[c][i] as f64, want[i * 6 + c] as f64);
            sig += y * y;
            err += (x - y) * (x - y);
        }
        let snr = 10.0 * (sig / err.max(1e-30)).log10();
        assert!(snr >= 20.0, "channel {c}: SNR {snr:.1} dB against ffmpeg (swapped channels?)");
    }
}
