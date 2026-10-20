//! AV1 export: MP4 round trip through our demuxer + AV1 decoder against the CPU render, ffprobe
//! as an external oracle, hostile settings.

use super::*;
use filmcraft_media::generators::{DemoScene, GeneratorSource};
use filmcraft_media::{FrameRequest, Generator, MediaSource};
use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use filmcraft_render::SourceMap;
use filmcraft_time::TICKS_PER_SECOND;

/// A moving demo scene (`w`×`h`, 24 fps, 1 s) plus a tone.
fn moving_project(w: u32, h: u32) -> (Arc<Project>, ItemId, SourceMap) {
    let mut p = Project::new("av1");
    let r = FrameRate::FPS_24;
    let g = GeneratorSource::new(Generator::Demo(DemoScene::Plasma), w, h, r, Tick(2 * TICKS_PER_SECOND));
    let tone = GeneratorSource::new(Generator::Tone { hz: 440.0, db: -6.0 }, w, h, r, Tick(2 * TICKS_PER_SECOND));
    let add = |p: &mut Project, g: &GeneratorSource| {
        let info = g.info().clone();
        p.add_item(
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
        )
    };
    let vid = add(&mut p, &g);
    let t = add(&mut p, &tone);
    let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: r, ..Default::default() }, 1, 1, None);
    let v = p.make_track_item(vid, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    let a = p.make_track_item(t, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(a);
    let mut m = SourceMap::default();
    m.0.insert(vid, Arc::new(g));
    m.0.insert(t, Arc::new(tone));
    (Arc::new(p), seq, m)
}

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("fc-export-av1-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

/// PSNR (dB) of the RGB channels of two RGBA8 pictures.
fn psnr_rgb(a: &[u8], b: &[u8]) -> f64 {
    let (mut se, mut n) = (0f64, 0usize);
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        for c in 0..3 {
            se += (pa[c] as f64 - pb[c] as f64).powi(2);
            n += 1;
        }
    }
    let mse = se / n.max(1) as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// Export a moving scene to AV1 MP4, read it back with our MP4 demuxer and AV1 decoder: 24
/// frames, `av01`/`av1C`, keyframes flagged, AAC audio, and every checked frame within PSNR of
/// the CPU render of the same sequence time.
#[test]
fn av1_mp4_roundtrip_matches_cpu_render() {
    let (w, h) = (320u32, 180u32);
    let (p, seq, m) = moving_project(w, h);
    let path = tmp("av1.mp4");
    let s = ExportSettings { format: Format::Av1, path: path.clone(), bitrate_kbps: 3_000, keyframe_distance: Some(12), ..Default::default() };
    assert_eq!(s.extension(), "mp4");
    assert_eq!(s.audio_codec(), AudioCodec::Aac);
    let prog = Progress::default();
    let t0 = std::time::Instant::now();
    let r = export(&p, seq, &s, &m, &prog).unwrap();
    eprintln!("AV1 {w}x{h}: {} frames in {:.2}s ({:.1} fps), {} bytes", r.frames, t0.elapsed().as_secs_f64(), r.render_fps, r.bytes);
    assert_eq!(r.frames, 24);
    assert!(prog.finished.load(Ordering::Relaxed));

    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let mp4 = filmcraft_isobmff::open(bytes.clone()).unwrap();
    assert!(mp4.compatible_brands.iter().any(|b| &b.0 == b"av01"), "av01 compatible brand");
    // the sample entry carries the sequence header OBU (filled in at finish)
    let vt = &mp4.tracks[mp4.track_of_kind(filmcraft_isobmff::TrackKind::Video).unwrap()];
    let entry = &vt.entries[0];
    assert_eq!(&entry.format.0, b"av01");
    let filmcraft_isobmff::CodecConfig::Av1(cfg) = &entry.codec else { panic!("av1C: {:?}", entry.codec) };
    assert_eq!(cfg.seq_profile, 0);
    assert!(cfg.chroma_subsampling_x && cfg.chroma_subsampling_y && !cfg.high_bitdepth);
    assert!(!cfg.config_obus.is_empty() && (cfg.config_obus[0] >> 3) & 0xF == 1, "sequence header OBU in configOBUs");
    assert_eq!(vt.samples.len(), 24);
    let sync = vt.sync_samples();
    assert!(sync.contains(&0) && sync.contains(&12), "keyframes at 0 and 12: {sync:?}");

    let src = filmcraft_codecs::open_bytes("av1.mp4", bytes).unwrap();
    let info = src.info();
    let vi = info.video.as_ref().unwrap();
    assert_eq!((vi.width, vi.height), (w, h));
    assert!(vi.codec.contains("AV1"), "{}", vi.codec);
    assert!((info.duration.seconds() - 1.0).abs() < 0.05, "{}", info.duration.seconds());
    let a = src.audio(0, 24_000, 48_000).unwrap();
    assert!((a.peaks()[0] - 0.5).abs() < 0.05, "AAC tone at -6 dB");

    let rate = FrameRate::FPS_24;
    let mut worst = f64::MAX;
    for f in [0i64, 1, 5, 11, 12, 13, 23] {
        let t = rate.tick_of(f);
        let reference = filmcraft_render::render_sequence(&p, seq, t, filmcraft_render::RenderOptions::default(), &m).over_black_rgba8();
        let got = src.video_frame(FrameRequest::full(t)).unwrap().to_rgba8();
        assert_eq!(got.len(), reference.len());
        let db = psnr_rgb(&reference, &got);
        worst = worst.min(db);
        assert!(db > 30.0, "frame {f}: PSNR {db:.1} dB against the CPU render");
    }
    eprintln!("AV1 round trip: worst PSNR {worst:.1} dB");

    // ffprobe (external oracle) reads the same stream
    if let Some(ffprobe) = filmcraft_testkit::ffprobe_or_skip("AV1 export") {
        let out = std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,profile,width,height,pix_fmt,nb_read_frames,color_primaries",
                "-of",
                "default=nw=1",
                &path,
            ])
            .output()
            .unwrap();
        let txt = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        for want in ["codec_name=av1", "width=320", "height=180", "pix_fmt=yuv420p", "nb_read_frames=24", "color_primaries=bt709"] {
            assert!(txt.contains(want), "ffprobe: missing {want} in\n{txt}");
        }
        assert!(String::from_utf8_lossy(&out.stderr).trim().is_empty(), "ffprobe decode errors: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// Hostile AV1 settings fail with an error (or are clamped), never panic.
#[test]
fn av1_hostile_settings() {
    let (p, seq, m) = moving_project(64, 36);
    let run = |s: ExportSettings| {
        let s = ExportSettings { format: Format::Av1, include_audio: false, range: Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(3))), ..s };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export(&p, seq, &s, &m, &Progress::default()))).expect("no panic")
    };
    // below the AV1 encoder's minimum size: an error naming the limit
    let e = run(ExportSettings { path: tmp("tiny.mp4"), frame_size: Some((8, 8)), ..Default::default() }).unwrap_err();
    assert!(e.to_string().contains("16"), "{e}");
    // absurd frame rate
    assert!(run(ExportSettings { path: tmp("fps.mp4"), frame_rate: Some(FrameRate::new(1_000_000, 1)), ..Default::default() }).is_err());
    // extreme but representable numbers are clamped and still export
    for (i, s) in [
        ExportSettings { bitrate_kbps: u32::MAX, keyframe_distance: Some(u32::MAX), ..Default::default() },
        ExportSettings { bitrate_kbps: 0, max_bitrate_kbps: Some(0), adaptive_bitrate: Some(f32::NAN), ..Default::default() },
        ExportSettings { pixel_aspect: Some((u32::MAX, 0)), bitrate_mode: BitrateMode::Vbr2Pass, quality: 255, ..Default::default() },
        ExportSettings { bitrate_mode: BitrateMode::Cbr, adaptive_bitrate: Some(f32::INFINITY), ..Default::default() },
    ]
    .into_iter()
    .enumerate()
    {
        let r = run(ExportSettings { path: tmp(&format!("clamp{i}.mp4")), ..s }).unwrap_or_else(|e| panic!("case {i}: {e}"));
        assert_eq!(r.frames, 3, "case {i}");
    }
    // the encoder itself rejects frames that do not match the stream
    let s = ExportSettings { format: Format::Av1, bitrate_kbps: 500, ..Default::default() };
    let mut enc = av1_out::av1_factory(Format::Av1, 64, 36, FrameRate::FPS_24, &s).unwrap().unwrap();
    let rgba = vec![0u8; 10];
    assert!(enc.encode(&EncoderFrame { width: 64, height: 36, rgba: &rgba, hdr: None, index: 0, prepared: None }).is_err());
    assert!(enc.encode(&EncoderFrame { width: u32::MAX, height: u32::MAX, rgba: &rgba, hdr: None, index: 0, prepared: None }).is_err());
    assert!(av1_out::av1_factory(Format::Av1, 64, 36, FrameRate::new(0, 0), &s).unwrap().is_err());
    assert!(av1_out::av1_factory(Format::Av1, 64, 36, FrameRate::new(-24, 1), &s).unwrap().is_err());
    assert!(av1_out::av1_factory(Format::H264, 64, 36, FrameRate::FPS_24, &s).is_none());
}

#[test]
fn av1_format_names_and_presets() {
    for n in ["av1", "AV1", "av01", "aom-av1"] {
        assert_eq!(Format::from_name(n), Some(Format::Av1), "{n}");
    }
    assert_eq!(Format::from_name("mp4"), Some(Format::H264), "mp4 stays H.264");
    assert_eq!(Format::Av1.id(), "av1");
    assert_eq!(serde_json::to_string(&Format::Av1).unwrap(), "\"av1\"");
    assert!(Format::ALL.contains(&Format::Av1) && available(Format::Av1) && stepped(Format::Av1));
    let ms = presets::find_builtin("Match Source - AV1").expect("Match Source – AV1 preset");
    assert_eq!(ms.settings.format, Format::Av1);
    let r = ms.settings.resolve(1920, 1080, FrameRate::FPS_24, 48_000);
    assert_eq!(r.target_kbps, (1920.0f64 * 1080.0 * 24.0 * 0.1 / 1000.0).round() as u32);
    let sum = ms.settings.summary(1920, 1080, FrameRate::FPS_24, 48_000, Tick(TICKS_PER_SECOND));
    assert_eq!(sum.format, "AV1 (MP4)");
    assert!(sum.video.contains("AV1") && sum.audio.starts_with("AAC"), "{sum:?}");
    assert!(presets::find_builtin("AV1 1080p Full HD").is_some());
}
