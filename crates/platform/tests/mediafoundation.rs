//! Media Foundation hardware decoding (Windows only): its pictures must be exactly our software
//! decoder's, in the same order, through seeks and a mid-stream failure, and damaged input must
//! never crash or hang. The stream is made by our own H.264 encoder, so the test needs no external
//! tool; it skips (with a note) on a machine whose GPU or session cannot decode H.264 in hardware
//! (a CI runner without a video adapter).
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use filmcraft_codecs::VideoDecoder;
use filmcraft_codecs::hw::NalStreamInfo;
use filmcraft_h264enc::{Encoder, EncoderConfig, PacketFormat, Profile, YuvFrame};
use filmcraft_isobmff::SampleEntry;
use filmcraft_platform::HybridDecoder;
use filmcraft_platform::mediafoundation::MfDecoder;

/// A High-profile stream with B-frames (reordering), several GOPs (seeks) and a 360-line picture
/// (coded 368 lines: cropping), as length-prefixed samples in decode order.
fn stream(width: u32, height: u32, frames: usize) -> Stream {
    let mut cfg = EncoderConfig::new(width, height, 24, 1);
    cfg.profile = Profile::High;
    cfg.keyint = 12;
    cfg.bframes = 2;
    cfg.scenecut = false;
    cfg.threads = 2;
    cfg.format = PacketFormat::LengthPrefixed;
    let mut enc = Encoder::new(cfg).expect("encoder");
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut packets = Vec::new();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for n in 0..frames {
        // moving gradients with a little noise, so there is motion and residual to decode
        let mut y = vec![0u8; w * h];
        for (i, p) in y.iter_mut().enumerate() {
            let (px, py) = (i % w, i / w);
            *p = (((px + n * 3) * 2 + py + (xorshift(&mut seed) % 5) as usize) % 220 + 16) as u8;
        }
        let u: Vec<u8> = (0..cw * ch).map(|i| ((i % cw + n * 2) % 200 + 28) as u8).collect();
        let v: Vec<u8> = (0..cw * ch).map(|i| ((i / cw + n) % 200 + 28) as u8).collect();
        packets.extend(enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: w, uv_stride: cw }, n as i64).expect("encode"));
    }
    packets.extend(enc.flush());
    let avcc = enc.avcc();
    let entry = SampleEntry::avc(filmcraft_isobmff::AvcConfig::parse(&avcc).expect("avcC"), width as u16, height as u16);
    Stream { entry, sync: packets.iter().map(|p| p.keyframe).collect(), samples: packets.into_iter().map(|p| (p.data, p.pts)).collect() }
}

fn info(s: &Stream) -> NalStreamInfo {
    NalStreamInfo::from_entry(&s.entry).expect("a NAL stream").expect("stream info")
}

/// The hardware decoder for the stream, or None (with a note) when this machine has none.
fn hardware(s: &Stream) -> Option<MfDecoder> {
    match MfDecoder::new(info(s)) {
        Ok(d) => Some(d),
        Err(why) => {
            eprintln!("no Media Foundation hardware decoder here ({why}): skipped");
            None
        }
    }
}

fn software(s: &Stream) -> Box<dyn VideoDecoder> {
    filmcraft_codecs::software_video_decoder(&s.entry).expect("software decoder")
}

#[test]
fn hardware_pictures_are_the_software_decoders() {
    for (w, h) in [(640, 360), (1280, 720), (322, 242)] {
        let s = stream(w, h, 40);
        assert!(s.samples.len() == 40 && s.sync.iter().filter(|k| **k).count() >= 3, "{w}x{h}: 40 pictures in several GOPs");
        let Some(mut hw) = hardware(&s) else { return };
        assert_eq!(hw.name(), "Media Foundation H.264");
        let reference = decode_all(software(&s).as_mut(), &s.samples);
        assert_eq!(reference.len(), 40);
        let out = decode_all(&mut hw, &s.samples);
        assert_same(&format!("{w}x{h}"), &out, &reference);

        // the same decoder again after a seek to each later key picture
        for k in (0..s.samples.len()).filter(|&i| s.sync[i]).skip(1) {
            let tail = &s.samples[k..];
            let expect = decode_all(software(&s).as_mut(), tail);
            hw.reset();
            assert_same(&format!("{w}x{h} after a seek to sample {k}"), &decode_all(&mut hw, tail), &expect);
        }
    }
}

#[test]
fn the_factory_decodes_in_hardware_and_follows_the_setting() {
    let s = stream(640, 360, 24);
    if hardware(&s).is_none() {
        return;
    }
    assert!(filmcraft_platform::hardware_decoder_for(&s.entry));
    let mut d = filmcraft_platform::mediafoundation_factory(&s.entry).expect("taken").expect("decoder");
    assert_eq!(d.name(), "Media Foundation H.264");
    assert_same("through the factory", &decode_all(d.as_mut(), &s.samples), &decode_all(software(&s).as_mut(), &s.samples));
}

#[test]
fn a_failure_mid_stream_continues_in_software_with_the_same_pictures() {
    let s = stream(640, 360, 36);
    let reference = decode_all(software(&s).as_mut(), &s.samples);
    for fail_after in [0u64, 1, 5, 13, 20, 35] {
        let Some(mut hw) = hardware(&s) else { return };
        hw.fail_after(fail_after);
        let mut d = HybridDecoder::new(Box::new(hw), s.entry.clone(), info(&s));
        let out = decode_all(&mut d, &s.samples);
        assert!(!d.is_hardware(), "fell back after {fail_after} samples");
        assert_same(&format!("failing after {fail_after} samples"), &out, &reference);
    }
}

/// Run `f` on a thread; fail if it panics or takes longer than `limit`.
fn bounded(what: &str, limit: Duration, f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let _ = tx.send(r.is_ok());
    });
    match rx.recv_timeout(limit) {
        Ok(true) => {}
        Ok(false) => panic!("{what}: panicked"),
        Err(_) => panic!("{what}: hung"),
    }
}

#[test]
fn damaged_samples_and_parameter_sets_never_crash_or_hang() {
    let s = stream(640, 360, 40);
    if hardware(&s).is_none() {
        return;
    }
    // damaged samples (bit flips, truncation, corrupt length prefixes, empty) through the hybrid
    for seed in 1..=6u64 {
        let (samples, entry) = (s.samples.clone(), s.entry.clone());
        bounded(&format!("seed {seed}"), Duration::from_secs(60), move || {
            let mut seed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let Some(Ok(mut d)) = filmcraft_platform::mediafoundation_factory(&entry) else { return };
            for (i, (smp, pts)) in samples.iter().enumerate() {
                let mut m = smp.clone();
                match xorshift(&mut seed) % 5 {
                    0 => {
                        for _ in 0..1 + xorshift(&mut seed) % 8 {
                            let at = (xorshift(&mut seed) as usize) % m.len().max(1);
                            if let Some(b) = m.get_mut(at) {
                                *b ^= 1 << (xorshift(&mut seed) % 8);
                            }
                        }
                    }
                    1 => m.truncate((xorshift(&mut seed) as usize) % m.len().max(1)),
                    2 if m.len() > 4 => m[..4].copy_from_slice(&(xorshift(&mut seed) as u32).to_be_bytes()),
                    3 if i % 7 == 3 => m.clear(),
                    _ => {}
                }
                let _ = d.decode(&m, *pts);
                if i % 13 == 12 {
                    d.reset();
                }
            }
            let _ = d.flush();
        });
    }
    // damaged parameter sets: the decoder is declined, errors, or decodes something; never worse
    let filmcraft_isobmff::CodecConfig::Avc(a) = &s.entry.codec else { panic!("an avc1 entry") };
    let rec = a.to_bytes();
    let mut seed = 0x1234_5678u64;
    for round in 0..24usize {
        let mut r = rec.clone();
        if round % 3 == 2 {
            r.truncate(r.len() * (round + 1) / 30);
        } else {
            for _ in 0..1 + round % 4 {
                let at = 6 + (xorshift(&mut seed) as usize) % r.len().saturating_sub(6).max(1);
                if let Some(b) = r.get_mut(at) {
                    *b ^= 1 << (xorshift(&mut seed) % 8);
                }
            }
        }
        let samples: Vec<_> = s.samples.iter().take(12).cloned().collect();
        bounded(&format!("record round {round}"), Duration::from_secs(60), move || {
            let Ok(info) = NalStreamInfo::from_avcc(&r) else { return };
            let Ok(mut mf) = MfDecoder::new(info) else { return };
            for (smp, pts) in &samples {
                if mf.decode(smp, *pts).is_err() {
                    break;
                }
            }
            let _ = mf.flush();
        });
    }
}

/// Formats the hardware path does not take are declined up front, so the software decoder is used.
#[test]
fn unsupported_formats_are_declined() {
    let s = stream(640, 360, 2);
    let base = info(&s);
    for (what, change) in [
        ("4:2:2", (|i: &mut NalStreamInfo| i.chroma_format_idc = 2) as fn(&mut NalStreamInfo)),
        ("10-bit H.264", |i| (i.bit_depth_luma, i.bit_depth_chroma) = (10, 10)),
        ("field-coded", |i| i.interlaced = true),
        ("no picture", |i| i.crop = (0, 0, 0, 0)),
        ("a picture larger than the coded size", |i| i.crop = (0, 0, i.coded.0 + 16, i.coded.1)),
        ("a huge picture", |i| (i.coded, i.crop) = ((20000, 20000), (0, 0, 20000, 20000))),
    ] {
        let mut i = base.clone();
        change(&mut i);
        assert!(MfDecoder::new(i).is_err(), "{what} is declined");
    }
}

/// Throughput of the hardware and the software decoder on the same stream (run with
/// `--ignored --nocapture`; numbers depend on the machine and its load).
#[test]
#[ignore = "a measurement, not a check"]
fn throughput() {
    for (w, h) in [(1920, 1080), (3840, 2160)] {
        let s = stream(w, h, 48);
        let Some(mut hw) = hardware(&s) else { return };
        let mut sw = software(&s);
        for (name, d) in [("hardware", &mut hw as &mut dyn VideoDecoder), ("software", sw.as_mut())] {
            decode_all(d, &s.samples);
            d.reset();
            let (t, cpu) = (std::time::Instant::now(), cpu_time());
            let n = (0..4).map(|_| decode_all(d, &s.samples).len()).sum::<usize>();
            let (wall, cpu) = (t.elapsed().as_secs_f64(), cpu_time() - cpu);
            eprintln!("{w}x{h} {name}: {:.0} fps, {:.2} ms CPU per frame", n as f64 / wall, cpu * 1000.0 / n as f64);
        }
    }
}

/// Processor time used by this process so far, in seconds (user + kernel, every thread), asked of
/// the system; 0 when it cannot be read.
fn cpu_time() -> f64 {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("(Get-Process -Id {}).TotalProcessorTime.TotalSeconds", std::process::id())])
        .output();
    out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().replace(',', ".").parse().ok()).unwrap_or(0.0)
}
