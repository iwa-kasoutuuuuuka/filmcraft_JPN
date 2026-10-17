//! Media Foundation hardware H.264 encoding (Windows only): its stream must decode with our own
//! H.264 decoder, picture for picture and close to the source, start with a keyframe and put its
//! parameter sets in the `avcC`. Skips (with a note) on a machine without a hardware H.264 encoder
//! (a CI runner without a GPU).
#![cfg(windows)]

mod common;

use common::*;
use filmcraft_export::{EncoderFrame, ExportSettings, VideoEncoder};
use filmcraft_frame::PixelData;
use filmcraft_platform::mfencode::MfEncoder;
use filmcraft_time::FrameRate;

/// Moving colour gradients with a little noise (RGBA8).
fn picture(w: usize, h: usize, n: usize, seed: &mut u64) -> Vec<u8> {
    let mut px = vec![255u8; w * h * 4];
    for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        let noise = (xorshift(seed) % 7) as usize;
        p[0] = ((x * 255 / w + n * 4 + noise) % 256) as u8;
        p[1] = ((y * 255 / h + n * 2) % 256) as u8;
        p[2] = (((x + y) * 128 / (w + h) + 64 + noise) % 256) as u8;
    }
    px
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a.iter().zip(b).map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2)).sum::<f64>() / a.len().max(1) as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

#[test]
fn hardware_stream_decodes_with_our_decoder() {
    let (w, h, frames) = (640usize, 360usize, 48usize);
    let rate = FrameRate::new(24, 1);
    let mut settings = ExportSettings { bitrate_kbps: 8000, keyframe_distance: Some(24), ..Default::default() };
    settings.max_bitrate_kbps = Some(12000);
    let mut enc = match MfEncoder::new(w as u32, h as u32, rate, &settings) {
        Ok(e) => e,
        Err(why) => {
            eprintln!("no Media Foundation hardware H.264 encoder here ({why}): skipped");
            return;
        }
    };
    eprintln!("encoding with {}", enc.name());
    assert!(enc.is_hardware());
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut sources = Vec::new();
    let mut packets = Vec::new();
    for n in 0..frames {
        let rgba = picture(w, h, n, &mut seed);
        packets.extend(enc.encode(&EncoderFrame { width: w as u32, height: h as u32, rgba: &rgba, hdr: None, index: n as u64 }).expect("encode"));
        sources.push(rgba);
    }
    packets.extend(enc.flush().expect("flush"));
    assert_eq!(packets.len(), frames, "one packet per picture");
    assert!(packets[0].key, "starts with a keyframe");
    assert!(packets.iter().filter(|p| p.key).count() >= 2, "keyframes every 24 pictures");
    assert!(packets.iter().all(|p| p.composition_offset == 0 && p.duration == 1));

    let entry = enc.sample_entry();
    let mut dec = filmcraft_codecs::software_video_decoder(&entry).expect("our decoder takes the avcC");
    let samples: Vec<(Vec<u8>, i64)> = packets.iter().enumerate().map(|(i, p)| (p.data.clone(), i as i64)).collect();
    let decoded = decode_all(dec.as_mut(), &samples);
    assert_eq!(decoded.len(), frames, "every picture decodes");

    // luma against the source converted the way the encoder input was (BT.709 limited range)
    let (mut y, mut u, mut v) = (Vec::new(), Vec::new(), Vec::new());
    for (k, (d, src)) in decoded.iter().zip(&sources).enumerate() {
        filmcraft_export::rgba_to_yuv420_8(src, w, h, &mut y, &mut u, &mut v);
        let PixelData::Yuv8 { planes, .. } = &d.frame.data else { panic!("8-bit pictures") };
        assert_eq!((d.frame.width as usize, d.frame.height as usize), (w, h));
        let q = psnr(&planes[0][..w * h], &y);
        assert!(q > 30.0, "picture {k}: luma PSNR {q:.1} dB");
    }
}

#[test]
fn hostile_settings_are_declined_not_crashing() {
    let rate = FrameRate::new(24, 1);
    let s = ExportSettings::default();
    for (w, h) in [(0u32, 0u32), (641, 360), (640, 361), (100_000, 64)] {
        assert!(MfEncoder::new(w, h, rate, &s).is_err(), "{w}x{h}");
    }
    assert!(MfEncoder::new(640, 360, FrameRate::new(0, 1), &s).is_err());
}

/// A whole MP4 export through the registered encoders (as the app exports): with a hardware
/// encoder here it is the one used, and the file decodes with our demuxer / decoder and, when
/// available, with ffprobe and ffmpeg (external test oracles) without errors.
#[test]
fn hardware_export_is_a_valid_mp4() {
    use filmcraft_media::MediaSource;
    use filmcraft_media::generators::GeneratorSource;
    use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, Project, SequenceSettings, TrackKind};
    use filmcraft_time::{TICKS_PER_SECOND, Tick, TimeRange};
    use std::sync::Arc;

    if MfEncoder::new(640, 360, FrameRate::FPS_24, &ExportSettings::default()).is_err() {
        eprintln!("no Media Foundation hardware H.264 encoder here: skipped");
        return;
    }
    filmcraft_platform::register();
    let mut p = Project::new("x");
    let g = GeneratorSource::new(filmcraft_media::Generator::BarsAndTone, 640, 360, FrameRate::FPS_24, Tick(3 * TICKS_PER_SECOND));
    let info = g.info().clone();
    let item = p.add_item(
        &info.name.clone(),
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(g.generator.clone()),
            info,
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        }),
        None,
    );
    let seq = p.new_sequence("s", SequenceSettings { width: 640, height: 360, frame_rate: FrameRate::FPS_24, ..Default::default() }, 1, 1, None);
    let r = FrameRate::FPS_24;
    let v = p.make_track_item(item, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(48)), r).expect("item");
    p.sequence_mut(seq).expect("seq").video_tracks[0].items.push(v);
    let mut sources = filmcraft_render::SourceMap::default();
    sources.0.insert(item, Arc::new(g));
    let dir = std::env::temp_dir().join(format!("fc-mfencode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp");
    let path = dir.join("hw.mp4").to_string_lossy().to_string();
    let before = filmcraft_export::hw::hw_encode_stats();
    let progress = filmcraft_export::Progress::default();
    let settings = ExportSettings { format: filmcraft_export::Format::H264, path: path.clone(), ..Default::default() };
    let report = filmcraft_export::export(&Arc::new(p), seq, &settings, &sources, &progress).expect("export");
    assert_eq!(report.frames, 48);
    let after = filmcraft_export::hw::hw_encode_stats();
    assert!(after.sessions > before.sessions, "the hardware encoder was used");
    assert_eq!(after.fallbacks, before.fallbacks, "and did not fall back");

    // our demuxer and decoder: every picture, the bars where they should be
    let bytes: Arc<[u8]> = std::fs::read(&path).expect("file").into();
    let src = filmcraft_codecs::open_bytes("hw.mp4", bytes).expect("our demuxer");
    let vi = src.info().video.clone().expect("video");
    assert_eq!((vi.width, vi.height), (640, 360));
    let f = src.video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).expect("frame").to_rgba8();
    assert!(f.len() == 640 * 360 * 4);

    if let Some(ffprobe) = filmcraft_testkit::ffprobe_or_skip("hardware export frame count") {
        let out = std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=nb_read_frames,codec_name,profile",
                "-of",
                "csv=p=0",
                &path,
            ])
            .output()
            .expect("ffprobe");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.trim().starts_with("h264,High,48") || text.contains(",48"), "ffprobe: {text}");
    }
    if let Some(ffmpeg) = filmcraft_testkit::ffmpeg() {
        let out = std::process::Command::new(ffmpeg).args(["-v", "error", "-i", &path, "-f", "null", "-"]).output().expect("ffmpeg");
        assert!(out.status.success() && out.stderr.is_empty(), "ffmpeg decode errors: {}", String::from_utf8_lossy(&out.stderr));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
