use super::*;
use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{FrameRequest, Generator, MediaSource};
use filmcraft_project::{ItemKind, Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use filmcraft_render::SourceMap;
use filmcraft_time::TICKS_PER_SECOND;

pub(crate) fn project() -> (Arc<Project>, ItemId, SourceMap) {
    let mut p = Project::new("x");
    let g = GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
    let tone = GeneratorSource::new(Generator::Tone { hz: 440.0, db: -6.0 }, 320, 180, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
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
    let red = add(&mut p, &g);
    let t = add(&mut p, &tone);
    let seq = p.new_sequence("s", SequenceSettings { width: 320, height: 180, frame_rate: FrameRate::FPS_24, ..Default::default() }, 1, 1, None);
    let r = FrameRate::FPS_24;
    let v = p.make_track_item(red, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    let a = p.make_track_item(t, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(v);
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(a);
    let mut m = SourceMap::default();
    m.0.insert(red, Arc::new(g));
    m.0.insert(t, Arc::new(tone));
    (Arc::new(p), seq, m)
}

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("fc-export-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

#[test]
fn mjpeg_mov_roundtrip() {
    let (p, seq, m) = project();
    let path = tmp("out.mov");
    let prog = Progress::default();
    let r = export(&p, seq, &ExportSettings { format: Format::Mjpeg, path: path.clone(), ..Default::default() }, &m, &prog).unwrap();
    assert_eq!(r.frames, 24);
    assert!(prog.finished.load(Ordering::Relaxed));
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = filmcraft_codecs::open_bytes("out.mov", bytes).unwrap();
    let info = src.info();
    assert_eq!(info.video.as_ref().unwrap().width, 320);
    assert!((info.duration.seconds() - 1.0).abs() < 0.05, "{}", info.duration.seconds());
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 2))).unwrap().to_rgba8();
    assert!(f[0] > 230 && f[1] < 30, "{:?}", &f[..4]);
    let a = src.audio(0, 24_000, 48_000).unwrap();
    let pk = a.peaks()[0];
    assert!((pk - 0.5).abs() < 0.05, "tone at -6 dB: {pk}");
    if let Some(ffprobe) = filmcraft_testkit::ffprobe_or_skip("export mjpeg frame count") {
        let out = std::process::Command::new(ffprobe)
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0", &path])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "24");
    }
}

#[test]
fn wav_png_gif() {
    let (p, seq, m) = project();
    for (fmt, name) in [(Format::Wav, "a.wav"), (Format::Gif, "a.gif"), (Format::PngSequence, "seq.png")] {
        let path = tmp(name);
        let prog = Progress::default();
        let mut s = ExportSettings { format: fmt, path: path.clone(), scale: 0.5, ..Default::default() };
        s.range = Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(6)));
        let r = export(&p, seq, &s, &m, &prog).unwrap();
        assert!(r.bytes > 0, "{fmt:?}");
    }
    assert!(std::path::Path::new(&tmp("seq005.png")).exists());
}

/// A damaged project with a zero frame rate must export (at the default rate), not panic
/// computing the GIF frame delay.
#[test]
fn gif_with_zero_sequence_rate() {
    let (p, seq, m) = project();
    let mut p = (*p).clone();
    p.sequence_mut(seq).unwrap().settings.frame_rate = FrameRate { num: 0, den: 0 };
    let p = Arc::new(p);
    let mut s = ExportSettings { format: Format::Gif, path: tmp("zero_rate.gif"), scale: 0.25, ..Default::default() };
    s.range = Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(2)));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export(&p, seq, &s, &m, &Progress::default())));
    assert!(r.is_ok(), "export panicked");
}

#[test]
fn prores_export_roundtrip() {
    let (p, seq, m) = project();
    let path = tmp("pr.mov");
    export(&p, seq, &ExportSettings { format: Format::ProRes, path: path.clone(), ..Default::default() }, &m, &Progress::default()).unwrap();
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = filmcraft_codecs::open_bytes("pr.mov", bytes).unwrap();
    assert!(src.info().video.as_ref().unwrap().codec.contains("ProRes 422 HQ"));
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 3))).unwrap().to_rgba8();
    assert!(f[0] > 240 && f[1] < 15 && f[2] < 15, "{:?}", &f[..4]);
}

/// FLAC export: the same samples as a WAV export of the range (both quantised to 24 bit), decoded
/// by the import path (symphonia), which also finds the stream by its `fLaC` signature under any
/// file name; and well below the size of the PCM.
#[test]
fn flac_export_matches_wav() {
    let (p, seq, m) = project();
    let decode = |path: &str, name: &str| {
        let bytes: Arc<[u8]> = std::fs::read(path).unwrap().into();
        let src = filmcraft_codecs::open_bytes(name, bytes).unwrap();
        let info = src.info().clone();
        (info, src.audio(0, 48_000, 48_000).unwrap())
    };
    for bits in [16u16, 24] {
        let mut s = ExportSettings { format: Format::Flac, path: tmp(&format!("a{bits}.flac")), ..Default::default() };
        s.audio.bits = bits;
        let flac = export(&p, seq, &s, &m, &Progress::default()).unwrap();
        let wav = export(&p, seq, &ExportSettings { format: Format::Wav, path: tmp(&format!("a{bits}.wav")), ..s.clone() }, &m, &Progress::default()).unwrap();
        assert!(flac.bytes * 100 < wav.bytes * 60, "{bits}-bit tone: {} vs {} bytes", flac.bytes, wav.bytes);
        let (fi, fa) = decode(&s.path, "a.flac");
        let (_, wa) = decode(&tmp(&format!("a{bits}.wav")), "a.wav");
        let ai = fi.audio.as_ref().unwrap();
        assert_eq!((ai.sample_rate, ai.channels, ai.bits_per_sample), (48_000, 2, Some(u32::from(bits))), "{bits}");
        assert_eq!(fa.channels.len(), wa.channels.len());
        for (c, (f, w)) in fa.channels.iter().zip(&wa.channels).enumerate() {
            let diff = f.iter().zip(w).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(diff < 1e-6, "{bits}-bit channel {c}: max difference {diff}");
        }
        let pk = fa.peaks()[0];
        assert!((pk - 0.5).abs() < 0.05, "tone at -6 dB: {pk}");
        // found by its signature whatever the file is called
        let (renamed, _) = decode(&s.path, "clip.bin");
        assert_eq!(renamed.audio.as_ref().map(|a| a.channels), Some(2));
        assert_eq!(renamed.container, "FLAC");
        if let Some(ffmpeg) = filmcraft_testkit::ffmpeg_or_skip("flac export decode") {
            let out = std::process::Command::new(ffmpeg).args(["-v", "error", "-i", &s.path, "-f", "null", "-"]).output().unwrap();
            assert!(out.status.success() && out.stderr.is_empty(), "ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    // presets and naming
    assert_eq!(Format::from_name("FLAC"), Some(Format::Flac));
    assert_eq!(Format::Flac.extension(), "flac");
    assert!(Format::ALL.contains(&Format::Flac));
    let s = ExportSettings { format: Format::Flac, ..Default::default() };
    assert!(!s.has_video());
    assert_eq!(s.audio_codec(), crate::settings::AudioCodec::Flac);
    assert!(s.summary(1920, 1080, FrameRate::FPS_24, 48_000, Tick(TICKS_PER_SECOND)).audio.starts_with("FLAC"));
}

#[test]
fn prores_profiles_by_name() {
    use filmcraft_prores::Profile;
    for (name, p) in [
        ("proxy", Profile::Proxy),
        ("lt", Profile::Lt),
        ("standard", Profile::Standard),
        ("", Profile::Hq),
        ("hq", Profile::Hq),
        ("4444", Profile::P4444),
        ("4444xq", Profile::P4444Xq),
        ("4444 XQ", Profile::P4444Xq),
        ("bogus", Profile::Hq),
    ] {
        assert_eq!(prores_profile(name), p, "{name}");
    }
    let s = |profile: &str, format: Format, alpha: bool| ExportSettings { format, prores_profile: profile.into(), alpha, ..Default::default() };
    assert!(s("4444", Format::ProRes, true).keeps_alpha());
    assert!(s("4444xq", Format::ProRes, true).keeps_alpha());
    assert!(!s("4444", Format::ProRes, false).keeps_alpha());
    assert!(!s("hq", Format::ProRes, true).keeps_alpha(), "4:2:2 has no alpha");
    assert!(!s("4444", Format::MxfOp1a, true).keeps_alpha(), "MXF ProRes stays opaque");
    assert!(!s("4444", Format::H264, true).keeps_alpha());
}

/// A 320×180 sequence whose only picture is a 160×90 matte of `color` in the middle: the rest of
/// the frame is empty (transparent).
fn small_matte_project(color: [f32; 4]) -> (Arc<Project>, ItemId, SourceMap) {
    let (p, seq, mut m) = project();
    let mut p = (*p).clone();
    let small = GeneratorSource::new(Generator::ColorMatte { color }, 160, 90, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
    let item = p.sequence(seq).unwrap().video_tracks[0].items[0].item;
    if let ItemKind::Media(mc) = &mut p.item_mut(item).unwrap().kind {
        mc.media = MediaRef::Generator(small.generator.clone());
        mc.info = small.info().clone();
    }
    m.0.insert(item, Arc::new(small));
    (Arc::new(p), seq, m)
}

/// ProRes 4444 / 4444 XQ: 4:4:4 frames under `ap4h` / `ap4x`; with Include Alpha Channel the
/// sequence's straight alpha is coded (0 where nothing is drawn), without it frames are opaque.
#[test]
fn prores_4444_export_keeps_alpha() {
    let (p, seq, m) = small_matte_project([1.0, 1.0, 1.0, 1.0]);
    for (profile, label, tag) in [("4444", "ProRes 4444", "ap4h"), ("4444xq", "ProRes 4444 XQ", "ap4x")] {
        for alpha in [true, false] {
            let path = tmp(&format!("pr_{profile}_{alpha}.mov"));
            let s = ExportSettings { format: Format::ProRes, path: path.clone(), prores_profile: profile.into(), alpha, ..Default::default() };
            export(&p, seq, &s, &m, &Progress::default()).unwrap();
            let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
            let src = filmcraft_codecs::open_bytes("pr.mov", bytes).unwrap();
            let codec = src.info().video.as_ref().unwrap().codec.clone();
            assert!(codec.contains(label), "{profile}: {codec}");
            let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 3))).unwrap();
            assert!(f.format_label().contains("4:4:4"), "{profile}: {}", f.format_label());
            let px = f.to_rgba8();
            let at = |x: usize, y: usize| &px[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4];
            let (corner, centre) = (at(5, 5), at(160, 90));
            assert!(centre[0] > 245 && centre[1] > 245 && centre[2] > 245 && centre[3] == 255, "{profile}: centre {centre:?}");
            if alpha {
                assert_eq!(corner[3], 0, "{profile}: empty area is transparent: {corner:?}");
            } else {
                assert!(corner[3] == 255 && corner[0] < 10, "{profile}: flattened over black: {corner:?}");
            }
            if let Some(ffprobe) = filmcraft_testkit::ffprobe_or_skip("prores 4444 pix_fmt") {
                let out = std::process::Command::new(ffprobe)
                    .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_tag_string,pix_fmt", "-of", "csv=p=0", &path])
                    .output()
                    .unwrap();
                let o = String::from_utf8_lossy(&out.stdout).trim().to_string();
                assert!(o.contains(tag), "{profile}: {o}");
                let want = if alpha { "yuva444p12" } else { "yuv444p12" };
                assert!(o.contains(want), "{profile}: {o}");
            }
        }
    }

    // a half-transparent matte keeps its partial alpha
    let (p, seq, m) = small_matte_project([1.0, 0.0, 0.0, 0.5]);
    let path = tmp("pr_4444_half.mov");
    let s = ExportSettings { format: Format::ProRes, path: path.clone(), prores_profile: "4444".into(), alpha: true, ..Default::default() };
    export(&p, seq, &s, &m, &Progress::default()).unwrap();
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = filmcraft_codecs::open_bytes("pr.mov", bytes).unwrap();
    let px = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 3))).unwrap().to_rgba8();
    let c = &px[(90 * 320 + 160) * 4..(90 * 320 + 160) * 4 + 4];
    assert!((c[3] as i32 - 128).abs() <= 3 && c[0] > 240 && c[1] < 15, "straight half alpha: {c:?}");
}

/// MXF OP1a with ProRes 4444 codes 4:4:4 (opaque) under profile 5.
#[test]
fn mxf_prores_4444_exports() {
    let (p, seq, m) = project();
    let path = tmp("pr4444.mxf");
    let mut s = ExportSettings { format: Format::MxfOp1a, path: path.clone(), prores_profile: "4444".into(), ..Default::default() };
    s.mxf_video_codec = crate::settings::MxfVideoCodec::ProRes;
    s.range = Some(TimeRange::new(Tick::ZERO, FrameRate::FPS_24.tick_of(4)));
    export(&p, seq, &s, &m, &Progress::default()).unwrap();
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = filmcraft_codecs::open_bytes("pr.mxf", bytes).unwrap();
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 24))).unwrap();
    assert!(f.format_label().contains("4:4:4"), "{}", f.format_label());
    let px = f.to_rgba8();
    assert!(px[0] > 240 && px[1] < 15 && px[3] == 255, "{:?}", &px[..4]);
}

#[test]
fn dnxhr_export_roundtrip() {
    let (p, seq, m) = project();
    for (profile, label) in [("hq", "DNxHR HQ"), ("hqx", "DNxHR HQX"), ("sq", "DNxHR SQ")] {
        let path = tmp(&format!("dnx_{profile}.mov"));
        let s = ExportSettings { format: Format::DnxHr, path: path.clone(), dnx_profile: profile.into(), ..Default::default() };
        export(&p, seq, &s, &m, &Progress::default()).unwrap();
        let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
        let src = filmcraft_codecs::open_bytes("dnx.mov", bytes).unwrap();
        let info = src.info().video.clone().unwrap();
        assert_eq!(info.codec, label);
        let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 3))).unwrap();
        assert!(f.format_label().contains("4:2:2"), "{}", f.format_label());
        let px = f.to_rgba8();
        assert!(px[0] > 240 && px[1] < 15 && px[2] < 15, "{profile}: {:?}", &px[..4]);
    }
}

/// Burn-in: an H.264 export of a dark-blue matte with a caption, decoded by ffmpeg (external test
/// oracle only), has bright caption pixels in the lower part of the frame, and none without
/// `burn_captions` or above the caption.
#[test]
fn caption_burn_in_h264_ffmpeg_oracle() {
    let Some(ffmpeg) = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"].into_iter().find(|p| std::path::Path::new(p).exists()) else {
        eprintln!("ffmpeg not found; skipping oracle test");
        return;
    };
    let mut p = Project::new("cap");
    let (w, h) = (640u32, 360u32);
    let g = GeneratorSource::new(Generator::ColorMatte { color: [0.0, 0.0, 0.2, 1.0] }, w, h, FrameRate::FPS_24, Tick(2 * TICKS_PER_SECOND));
    let info = g.info().clone();
    let matte = p.add_item(
        "matte",
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
    let r = FrameRate::FPS_24;
    let seq = p.new_sequence("s", SequenceSettings { width: w, height: h, frame_rate: r, ..Default::default() }, 1, 0, None);
    let v = p.make_track_item(matte, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, r.tick_of(24)), r).unwrap();
    let tid = filmcraft_project::TrackId(p.alloc_id());
    let cid = filmcraft_project::ClipId(p.alloc_id());
    let mut ct = filmcraft_project::CaptionTrack::new(tid, "Subtitle".into(), filmcraft_project::CaptionFormat::Subtitle);
    ct.style.background = false;
    ct.captions.push(filmcraft_project::Caption {
        id: cid,
        start: Tick::ZERO,
        duration: r.tick_of(24),
        text: "BURNED IN".into(),
        speaker: None,
        cue_id: None,
        settings: String::new(),
    });
    let q = p.sequence_mut(seq).unwrap();
    q.video_tracks[0].items.push(v);
    q.caption_tracks.push(ct);
    let mut m = SourceMap::default();
    m.0.insert(matte, Arc::new(g));
    let p = Arc::new(p);
    let decode = |burn: bool| -> Vec<u8> {
        let path = tmp(if burn { "burn.mp4" } else { "noburn.mp4" });
        let s = ExportSettings { format: Format::H264, path: path.clone(), include_audio: false, burn_captions: burn, ..Default::default() };
        export(&p, seq, &s, &m, &Progress::default()).unwrap();
        let out = std::process::Command::new(ffmpeg)
            .args(["-v", "error", "-i", &path, "-vf", "select=eq(n\\,12)", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout.len(), (w * h * 3) as usize);
        out.stdout
    };
    let bright = |rgb: &[u8], y0: u32, y1: u32, x0: u32, x1: u32| -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| ((y * w + x) * 3) as usize)).filter(|&i| rgb[i] > 200 && rgb[i + 1] > 200 && rgb[i + 2] > 200).count()
    };
    let with = decode(true);
    let without = decode(false);
    let lower = bright(&with, h * 2 / 3, h, 0, w);
    assert!(lower > 300, "white caption text in the lower third: {lower}");
    assert_eq!(bright(&with, 0, h / 2, 0, w), 0, "nothing above the caption");
    assert_eq!(bright(&without, 0, h, 0, w), 0, "no burn-in unless asked");
    // centred: text on both sides of the centre line
    let (left, right) = (bright(&with, h * 2 / 3, h, 0, w / 2), bright(&with, h * 2 / 3, h, w / 2, w));
    assert!(left > 100 && right > 100, "{left} / {right}");
}

/// HDR sequences export PQ / HLG with colour signalling in the bitstream (VUI, ProRes header) and
/// the container (`colr`, `mdcv`, `clli`); our demuxer and ffprobe both read it back, and the
/// picture survives the round trip.
#[test]
fn hdr_exports_signal_pq_and_hlg() {
    use filmcraft_color::{ColorPipeline, Transfer, WorkingSpace};
    for (ws, fmt, ext, transfer, ff_transfer) in [
        (WorkingSpace::Rec2100Pq, Format::ProRes, "mov", Transfer::Pq, "smpte2084"),
        (WorkingSpace::Rec2100Pq, Format::H264, "mp4", Transfer::Pq, "smpte2084"),
        (WorkingSpace::Rec2100Hlg, Format::ProRes, "mov", Transfer::Hlg, "arib-std-b67"),
        (WorkingSpace::Rec2100Hlg, Format::H264, "mp4", Transfer::Hlg, "arib-std-b67"),
    ] {
        let (p, seq, m) = project();
        let mut p = (*p).clone();
        p.sequence_mut(seq).unwrap().settings.color = ColorPipeline { working: ws, ..ColorPipeline::REC709 };
        let p = Arc::new(p);
        let path = tmp(&format!("hdr-{}.{ext}", ws.id()));
        let s = ExportSettings { format: fmt, path: path.clone(), include_audio: false, ..Default::default() };
        export(&p, seq, &s, &m, &Progress::default()).unwrap();
        // our demuxer/decoder sees the signalling
        let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
        let src = filmcraft_codecs::open_bytes(&path, bytes).unwrap();
        let v = src.info().video.clone().unwrap();
        assert_eq!(v.color.transfer, transfer, "{path}");
        assert_eq!(v.color.primaries, filmcraft_color::Primaries::Bt2020, "{path}");
        assert_eq!(v.color.matrix, filmcraft_color::Matrix::Bt2020Ncl, "{path}");
        // PQ exports carry mastering metadata (1000 cd/m² peak; MaxCLL 0 = unknown), which sets
        // the tone-mapping peak when the file is used again
        if transfer == Transfer::Pq {
            let hdr = v.hdr.unwrap_or_else(|| panic!("{path}: no HDR metadata"));
            assert_eq!((hdr.mastering_max_nits, hdr.max_cll, hdr.peak_nits()), (Some(1000.0), None, Some(1000.0)), "{path}");
        }
        // 709 red matte → BT.2020 HDR → decoded back into Rec. 709 (tone mapped) stays red
        let f = src.video_frame(FrameRequest::full(Tick::ZERO)).unwrap();
        let back = filmcraft_render::colorman::decode(&Project::new("x"), ItemId(0), &f, 1, &ColorPipeline::REC709);
        let c = filmcraft_render::Image::unpremul(back.get(160, 90));
        assert!(c[0] > 0.6 && c[1] < 0.05 && c[2] < 0.05, "{path}: {c:?}");
        // ffprobe agrees
        let Some(ffprobe) = filmcraft_testkit::ffprobe_or_skip("hdr export signalling") else { continue };
        let out = std::process::Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=color_transfer,color_primaries,color_space:stream_side_data",
                "-of",
                "default=nw=1",
                &path,
            ])
            .output()
            .unwrap();
        let txt = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(txt.contains(&format!("color_transfer={ff_transfer}")), "{path}: {txt}");
        assert!(txt.contains("color_primaries=bt2020"), "{path}: {txt}");
        assert!(txt.contains("color_space=bt2020nc"), "{path}: {txt}");
        if transfer == Transfer::Pq {
            assert!(txt.contains("Mastering display metadata") && txt.contains("max_luminance=10000000/10000"), "{path}: {txt}");
            assert!(txt.contains("Content light level metadata"), "{path}: {txt}");
        }
        eprintln!("{path}:\n{txt}");
    }
}

/// A hardware encoder that fails on its first picture (`hw`): only for exports whose path says so.
struct FailingHardware;

impl VideoEncoder for FailingHardware {
    fn sample_entry(&self) -> SampleEntry {
        SampleEntry::avc(filmcraft_isobmff::AvcConfig::new(Vec::new(), Vec::new(), 4), 320, 180)
    }
    fn timescale(&self) -> u32 {
        24
    }
    fn encode(&mut self, _: &EncoderFrame) -> Result<Vec<EncodedPacket>> {
        Err(ExportError::Encode("device lost".into()))
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        Ok(Vec::new())
    }
    fn is_hardware(&self) -> bool {
        true
    }
}

fn failing_hardware_factory(format: Format, _: u32, _: u32, _: FrameRate, s: &ExportSettings) -> Option<Result<Box<dyn VideoEncoder>>> {
    (format == Format::H264 && s.path.contains("failing-hw")).then(|| Ok(Box::new(FailingHardware) as Box<dyn VideoEncoder>))
}

/// A hardware encoder failing on the first frames: the export finishes with our encoder.
#[test]
fn failing_hardware_encoder_falls_back_to_ours() {
    register_encoder(failing_hardware_factory);
    let (p, seq, m) = project();
    let path = tmp("failing-hw.mp4");
    let before = hw::hw_encode_stats();
    let r = export(&p, seq, &ExportSettings { format: Format::H264, path: path.clone(), ..Default::default() }, &m, &Progress::default()).unwrap();
    assert_eq!(r.frames, 24);
    assert!(hw::hw_encode_stats().fallbacks > before.fallbacks);
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let src = filmcraft_codecs::open_bytes("failing-hw.mp4", bytes).unwrap();
    let f = src.video_frame(FrameRequest::full(Tick(TICKS_PER_SECOND / 2))).unwrap().to_rgba8();
    assert!(f[0] > 230 && f[1] < 30, "red, from our encoder: {:?}", &f[..4]);
}

/// The NV12 converter of the hardware encoders gives exactly the planes of `rgba_to_yuv420_8`.
#[test]
fn rgba_to_nv12_matches_the_planar_conversion() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for (w, h) in [(64usize, 36usize), (33, 17), (1, 1), (2, 3)] {
        let rgba: Vec<u8> = (0..w * h * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect();
        let (mut y, mut u, mut v) = (Vec::new(), Vec::new(), Vec::new());
        rgba_to_yuv420_8(&rgba, w, h, &mut y, &mut u, &mut v);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let (mut ny, mut nuv) = (vec![0u8; w * h], vec![0u8; cw * 2 * ch]);
        rgba_to_nv12(&rgba, w, h, &mut ny, &mut nuv);
        assert_eq!(ny, y, "{w}x{h} luma");
        let inter: Vec<u8> = u.iter().zip(&v).flat_map(|(a, b)| [*a, *b]).collect();
        assert_eq!(nuv, inter, "{w}x{h} chroma");
        // short buffers are left alone, not indexed out of bounds
        rgba_to_nv12(&rgba[..4], w, h, &mut ny, &mut nuv);
    }
}

/// P010 for the hardware HDR encoders: the 8-bit conversion's arithmetic at 10 bits (values in the
/// top ten bits of little-endian 16-bit samples), limited range and clamped.
#[test]
fn rgbf_to_p010_is_the_10_bit_conversion() {
    let (w, h) = (6usize, 4usize);
    let rgb: Vec<f32> = (0..w * h * 3).map(|i| [0.0, 0.25, 0.5, 1.0, 1.3, -0.2][i % 6]).collect();
    let (kr, kb) = (0.2627, 0.0593);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (mut y, mut uv) = (vec![0u8; w * h * 2], vec![0u8; cw * ch * 4]);
    rgbf_to_p010(&rgb, w, h, kr, kb, &mut y, &mut uv);
    let (mut y8, mut u8_, mut v8) = (Vec::new(), Vec::new(), Vec::new());
    rgbf_to_yuv420_8(&rgb, w, h, kr, kb, &mut y8, &mut u8_, &mut v8);
    let sample = |b: &[u8], i: usize| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
    for i in 0..w * h {
        let s = sample(&y, i);
        assert_eq!(s & 0x3f, 0, "the low six bits are zero");
        let ten = s >> 6;
        assert!((4..=1019).contains(&ten));
        // the same luma as the 8-bit conversion, at four times the resolution
        assert!((ten as f32 / 4.0 - y8[i] as f32).abs() <= 1.0, "{i}: {ten} vs {}", y8[i]);
    }
    for k in 0..cw * ch {
        let (u, v) = (sample(&uv, k * 2) >> 6, sample(&uv, k * 2 + 1) >> 6);
        assert!((u as f32 / 4.0 - u8_[k] as f32).abs() <= 1.0 && (v as f32 / 4.0 - v8[k] as f32).abs() <= 1.0, "{k}");
    }
    // short buffers are left alone
    rgbf_to_p010(&rgb[..3], w, h, kr, kb, &mut y, &mut uv);
}
