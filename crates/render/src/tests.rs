use super::*;
use filmcraft_media::generators::GeneratorSource;
use filmcraft_media::{DemoScene, Generator, MediaSource};
use filmcraft_project::{Label, MediaClip, MediaRef, SequenceSettings, TrackKind};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, TimeRange};

fn setup() -> (Project, ItemId, ItemId, ItemId, SourceMap) {
    let mut p = Project::new("r");
    let mut map = SourceMap::default();
    let mut add = |p: &mut Project, g: GeneratorSource| {
        let info = g.info().clone();
        let generator = g.generator.clone();
        let id = p.add_item(
            &info.name.clone(),
            Label::Iris,
            ItemKind::Media(MediaClip {
                media: MediaRef::Generator(generator),
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
        map.0.insert(id, Arc::new(g) as SharedSource);
        id
    };
    let red =
        add(&mut p, GeneratorSource::new(Generator::ColorMatte { color: [1.0, 0.0, 0.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND)));
    let ocean = add(&mut p, GeneratorSource::demo(DemoScene::OceanSunset));
    let seq = p.new_sequence("s", SequenceSettings { width: 320, height: 180, frame_rate: FrameRate::FPS_24, ..Default::default() }, 2, 2, None);
    (p, red, ocean, seq, map)
}

fn place(p: &mut Project, seq: ItemId, track: usize, item: ItemId, start_f: i64, dur_f: i64) -> filmcraft_project::ClipId {
    let r = FrameRate::FPS_24;
    let ti = p.make_track_item(item, TrackKind::Video, r.tick_of(start_f), TimeRange::new(Tick::ZERO, r.tick_of(dur_f)), r).unwrap();
    let id = ti.id;
    let mut ti = ti;
    ti.scale_to_frame = true;
    p.sequence_mut(seq).unwrap().video_tracks[track].items.push(ti);
    p.sequence_mut(seq).unwrap().video_tracks[track].sort();
    id
}

#[test]
fn composites_tracks_and_opacity() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 0, 48);
    let img = render_sequence(&p, seq, Tick(1000), RenderOptions::default(), &map);
    assert_eq!((img.w, img.h), (320, 180));
    let c = img.get(160, 90);
    assert!((c[0] - 1.0).abs() < 1e-4 && c[1] < 1e-4, "red on top: {c:?}");
    // 50% opacity shows the bottom layer through
    let s = p.sequence_mut(seq).unwrap();
    let (_, it) = s.find_item_mut(top).unwrap();
    it.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(50.0);
    let img = render_sequence(&p, seq, Tick(1000), RenderOptions::default(), &map);
    let c = img.get(160, 20);
    assert!(c[0] > 0.5 && c[0] < 1.0 && c[2] > 0.0, "{c:?}");
}

#[test]
fn half_resolution_matches_full_downsampled() {
    let (mut p, _red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let full = render_sequence(&p, seq, Tick(1000), RenderOptions::default(), &map);
    let half = render_sequence(&p, seq, Tick(1000), RenderOptions { scale: 0.5, ..Default::default() }, &map);
    assert_eq!((half.w, half.h), (160, 90));
    let a = full.get(200, 60);
    let b = half.get(100, 30);
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() < 0.06, "{a:?} vs {b:?}");
    }
}

#[test]
fn motion_scale_and_position() {
    let (mut p, red, _o, seq, map) = setup();
    let id = place(&mut p, seq, 0, red, 0, 48);
    let s = p.sequence_mut(seq).unwrap();
    let (_, it) = s.find_item_mut(id).unwrap();
    let m = it.effect_mut("motion").unwrap();
    m.params.get_mut("scale").unwrap().value = ParamValue::Float(50.0);
    m.params.get_mut("position").unwrap().value = ParamValue::Vec2(filmcraft_geom::Vec2::new(80.0, 45.0));
    let img = render_sequence(&p, seq, Tick(0), RenderOptions::default(), &map);
    assert!(img.get(80, 45)[3] > 0.99);
    assert!(img.get(10, 10)[3] > 0.99, "top-left quadrant covered");
    assert_eq!(img.get(200, 120)[3], 0.0, "rest transparent");
}

#[test]
fn cross_dissolve_midpoint() {
    let (mut p, red, _o, seq, map) = setup();
    let a = place(&mut p, seq, 0, red, 0, 24);
    let blue_src = GeneratorSource::new(Generator::ColorMatte { color: [0.0, 0.0, 1.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND));
    let info = blue_src.info().clone();
    let blue = p.add_item(
        "blue",
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(blue_src.generator.clone()),
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
    let mut map = map;
    map.0.insert(blue, Arc::new(blue_src));
    let b = place(&mut p, seq, 0, blue, 24, 24);
    let r = FrameRate::FPS_24;
    let tr = filmcraft_project::Transition {
        id: filmcraft_project::TransitionId(999),
        effect: filmcraft_project::find_effect("cross_dissolve").unwrap().instance(),
        start: r.tick_of(18),
        duration: r.tick_of(12),
        from: Some(a),
        to: Some(b),
        align: Default::default(),
        reverse: false,
    };
    p.sequence_mut(seq).unwrap().video_tracks[0].transitions.push(tr);
    let img = render_sequence(&p, seq, r.tick_of(24), RenderOptions::default(), &map);
    let c = img.get(10, 10);
    assert!((c[0] - 0.5).abs() < 0.05 && (c[2] - 0.5).abs() < 0.05, "{c:?}");
    // Iris Round at 50 %: B opens in the centre; reversed, A closes into the centre instead.
    for reverse in [false, true] {
        let trs = &mut p.sequence_mut(seq).unwrap().video_tracks[0].transitions;
        trs[0].effect = filmcraft_project::find_effect("iris_round").unwrap().instance();
        trs[0].reverse = reverse;
        let opts = RenderOptions { scale: 0.25, ..RenderOptions::default() };
        let img = render_sequence(&p, seq, r.tick_of(24), opts, &map);
        let (mid, corner) = (img.get(img.w / 2, img.h / 2), img.get(2, 2));
        let (centre_is_b, corner_is_b) = (mid[2] > 0.9 && mid[0] < 0.1, corner[2] > 0.9 && corner[0] < 0.1);
        assert_eq!((centre_is_b, corner_is_b), (!reverse, reverse), "reverse={reverse}: {mid:?} {corner:?}");
    }
}

#[test]
fn audio_mix_bars_tone() {
    let mut p = Project::new("a");
    let g = GeneratorSource::new(Generator::BarsAndTone, 64, 36, FrameRate::FPS_24, Tick(5 * TICKS_PER_SECOND));
    let info = g.info().clone();
    let id = p.add_item(
        "bars",
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(Generator::BarsAndTone),
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
    let mut map = SourceMap::default();
    map.0.insert(id, Arc::new(g));
    let seq = p.new_sequence("s", SequenceSettings::default(), 1, 1, None);
    let r = FrameRate::FPS_23_976;
    let mut ti = p.make_track_item(id, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, Tick(TICKS_PER_SECOND)), r).unwrap();
    ti.effect_mut("volume").unwrap().params.get_mut("level").unwrap().value = ParamValue::Float(-6.0);
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(ti);
    let s = p.sequence(seq).unwrap();
    let buf = audio::mix_sequence(&p, s, 0, 4800, &map);
    let peak = buf.peaks()[0];
    let expect = 10f32.powf(-26.0 / 20.0);
    assert!((peak - expect).abs() < 0.01, "{peak} vs {expect}");
}

#[test]
fn plan_matches_reference_renderer() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 10, 20);
    {
        let s = p.sequence_mut(seq).unwrap();
        let (_, it) = s.find_item_mut(top).unwrap();
        let m = it.effect_mut("motion").unwrap();
        m.params.get_mut("scale").unwrap().value = ParamValue::Float(40.0);
        m.params.get_mut("rotation").unwrap().value = ParamValue::Float(15.0);
        it.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().value = ParamValue::Float(70.0);
    }
    let r = FrameRate::FPS_24;
    for f in [0, 12, 20, 40] {
        let opts = RenderOptions { scale: 0.5, ..Default::default() };
        let reference = render_sequence(&p, seq, r.tick_of(f), opts, &map);
        let plan = plan::plan_frame(&p, seq, r.tick_of(f), opts, &map);
        assert!(matches!(plan, plan::FramePlan::Layers { .. }), "simple frame should be GPU-drawable");
        let got = plan::execute_cpu(&plan);
        assert_eq!((got.w, got.h), (reference.w, reference.h));
        let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(max < 0.02, "frame {f}: max diff {max}");
    }
}

/// Masked effects and Opacity masks of a media clip stay GPU layers: the plan carries the masks
/// flattened for the working image, and running it gives the reference renderer's picture.
#[test]
fn plan_keeps_masked_effects_as_gpu_layers() {
    use filmcraft_geom::Vec2;
    use filmcraft_project::{Mask, MaskMode, MaskPath};
    let (mut p, red, ocean, seq, map) = setup();
    let bottom = place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 0, 48);
    let (sw, sh) = {
        let s = p.sequence(seq).unwrap();
        (s.settings.width as f64, s.settings.height as f64)
    };
    let mut blur = filmcraft_project::find_effect("gaussian_blur").unwrap().instance();
    blur.params.get_mut("blurriness").unwrap().value = ParamValue::Float(30.0);
    let mut spot = Mask::new("spot", MaskPath::ellipse(Vec2::new(sw * 0.4, sh * 0.5), Vec2::new(sw * 0.25, sh * 0.3)));
    spot.feather.value = ParamValue::Float(12.0);
    blur.masks.push(spot.clone());
    let mut hole = Mask::new("hole", MaskPath::rect(sw * 0.3, sh * 0.3, sw * 0.5, sh * 0.6));
    hole.mode = MaskMode::Subtract;
    blur.masks.push(hole);
    {
        let s = p.sequence_mut(seq).unwrap();
        let (_, it) = s.find_item_mut(bottom).unwrap();
        it.effects.push(blur);
        let (_, it) = s.find_item_mut(top).unwrap();
        it.effect_mut("opacity").unwrap().masks.push(spot);
        it.effect_mut("motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(60.0);
    }
    let r = FrameRate::FPS_24;
    for scale in [1.0, 0.5] {
        let opts = RenderOptions { scale, ..Default::default() };
        let reference = render_sequence(&p, seq, r.tick_of(12), opts, &map);
        let plan = plan::plan_frame(&p, seq, r.tick_of(12), opts, &map);
        let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("masked clips should be GPU-drawable") };
        let fx: Vec<&plan::LayerFx> = layers.iter().filter_map(|l| l.fx.as_deref()).collect();
        assert_eq!(fx.len(), 2, "both clips run the effect stage");
        assert!(fx[0].masks.len() == 1 && fx[0].masks[0].len() == 2 && fx[0].opacity_masks.is_empty());
        assert!(fx[1].ops.is_empty() && fx[1].opacity_masks.len() == 1);
        let got = plan::execute_cpu(&plan);
        assert_eq!((got.w, got.h), (reference.w, reference.h));
        let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(max < 0.02, "scale {scale}: max diff {max}");
    }
    // a mask with more points than the GPU evaluates per pixel: that clip is a CPU image again
    {
        let s = p.sequence_mut(seq).unwrap();
        let (_, it) = s.find_item_mut(top).unwrap();
        let pts: Vec<Vec2> = (0..plan::MAX_GPU_MASK_POINTS + 8)
            .map(|i| Vec2::new(sw * 0.5 + (i as f64 * 0.37).cos() * sw * 0.3, sh * 0.5 + (i as f64 * 0.37).sin() * sh * 0.3))
            .collect();
        it.effect_mut("opacity").unwrap().masks.push(Mask::new("many", MaskPath::polygon(&pts)));
    }
    let opts = RenderOptions::default();
    let plan = plan::plan_frame(&p, seq, r.tick_of(12), opts, &map);
    let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("layers") };
    assert_eq!(layers.iter().filter(|l| l.fx.is_some()).count(), 1, "the clip with too many mask points is rendered on the CPU");
    let (got, reference) = (plan::execute_cpu(&plan), render_sequence(&p, seq, r.tick_of(12), opts, &map));
    let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    assert!(max < 0.02, "max diff {max}");
}

/// A project with a 4 s bars-and-tone clip (1 kHz at −20 dBFS) on A1, with extra audio effects.
fn tone_with(effects: &[(&str, &[(&str, f64)])]) -> (Project, ItemId, SourceMap) {
    let mut p = Project::new("fx");
    let g = GeneratorSource::new(Generator::BarsAndTone, 64, 36, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND));
    let info = g.info().clone();
    let id = p.add_item(
        "bars",
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(Generator::BarsAndTone),
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
    let mut map = SourceMap::default();
    map.0.insert(id, Arc::new(g));
    let seq = p.new_sequence("s", SequenceSettings::default(), 1, 1, None);
    let r = FrameRate::FPS_23_976;
    let mut ti = p.make_track_item(id, TrackKind::Audio, Tick::ZERO, TimeRange::new(Tick::ZERO, Tick(4 * TICKS_PER_SECOND)), r).unwrap();
    for (fx, params) in effects {
        let mut e = filmcraft_project::find_effect(fx).expect("effect").instance();
        for (k, v) in *params {
            e.param_mut(k).expect("param").value = ParamValue::Float(*v);
        }
        ti.effects.push(e);
    }
    p.sequence_mut(seq).unwrap().audio_tracks[0].items.push(ti);
    (p, seq, map)
}

fn mix(p: &Project, seq: ItemId, map: &SourceMap, start: i64, n: usize) -> Vec<f32> {
    audio::mix_sequence(p, p.sequence(seq).unwrap(), start, n, map).channels[0].clone()
}

#[test]
fn audio_fx_limiter_holds_ceiling() {
    // +30 dB boost via Amplify (tone at +10 dBFS), then a −6 dB hard limiter.
    let (p, seq, map) = tone_with(&[("amplify", &[("gain", 30.0)]), ("hard_limiter", &[("max", -6.0)])]);
    let out = mix(&p, seq, &map, 24_000, 48_000);
    let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
    let ceiling = 10f32.powf(-6.0 / 20.0);
    assert!(peak <= ceiling * 1.01 && peak > ceiling * 0.7, "peak {peak} vs ceiling {ceiling}");
}

#[test]
fn audio_fx_chain_is_continuous_and_random_access_matches() {
    let (p, seq, map) = tone_with(&[("delay", &[("delay", 0.05), ("feedback", 50.0), ("mix", 50.0)])]);
    let start = 48_000;
    // one long call (fresh chain with pre-roll)
    let whole = mix(&p, seq, &map, start, 9_600);
    // the same range in consecutive blocks from a different position (continues a cached chain)
    let (p2, seq2, map2) = tone_with(&[("delay", &[("delay", 0.05), ("feedback", 50.0), ("mix", 50.0)])]);
    let mut parts = Vec::new();
    let mut s = start - 4_800;
    let _ = mix(&p2, seq2, &map2, s, 4_800);
    s += 4_800;
    while s < start + 9_600 {
        parts.extend(mix(&p2, seq2, &map2, s, 1_200));
        s += 1_200;
    }
    let max = whole.iter().zip(&parts).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    assert!(max < 1e-4, "sequential blocks differ from one call by {max}");
    // the delay changes the signal (wet echoes mixed in)
    let dry = mix(&tone_with(&[]).0, tone_with(&[]).1, &tone_with(&[]).2, start, 9_600);
    let diff = whole.iter().zip(&dry).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    assert!(diff > 1e-3, "delay had no effect");
}

#[test]
fn offline_slate_is_deterministic_and_marked() {
    use crate::offline::{FIELD, OfflineReason, slate_rgba8};
    let a = slate_rgba8(640, 360, "A001_C002.mov", OfflineReason::Missing);
    assert_eq!(a, slate_rgba8(640, 360, "A001_C002.mov", OfflineReason::Missing));
    assert_ne!(a, slate_rgba8(640, 360, "A001_C002.mov", OfflineReason::MadeOffline), "the headline names the reason");
    // corners are the red field / stripes, opaque
    let px = |x: usize, y: usize| &a[(y * 640 + x) * 4..(y * 640 + x) * 4 + 4];
    assert_eq!(px(0, 359)[3], 255);
    assert!(px(0, 359)[0] >= FIELD[0] && px(0, 359)[1] <= 0x20, "{:?}", px(0, 359));
    // tiny sizes work (thumbnails)
    assert_eq!(slate_rgba8(8, 4, "x", OfflineReason::Unreadable).len(), 8 * 4 * 4);
    if let Some(p) = std::env::var_os("FILMCRAFT_SLATE_PNG") {
        ::image::save_buffer(p, &a, 640, 360, ::image::ExtendedColorType::Rgba8).unwrap();
    }
}

/// A 320×180 4:2:0 source with smooth gradients (what decoders hand the renderer).
struct YuvSource(GeneratorSource);

impl MediaSource for YuvSource {
    fn info(&self) -> &filmcraft_media::MediaInfo {
        self.0.info()
    }
    fn video_frame(&self, _req: FrameRequest) -> filmcraft_media::Result<Arc<filmcraft_frame::VideoFrame>> {
        let (w, h) = (320usize, 180usize);
        let y = (0..w * h).map(|i| (16 + (i % w) * 200 / w + (i / w) * 20 / h) as u8).collect();
        let u = (0..w * h / 4).map(|i| (90 + (i % (w / 2)) * 60 / (w / 2)) as u8).collect();
        let v = (0..w * h / 4).map(|i| (100 + (i / (w / 2)) * 50 / (h / 2)) as u8).collect();
        Ok(Arc::new(filmcraft_frame::VideoFrame {
            width: w as u32,
            height: h as u32,
            data: filmcraft_frame::PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: filmcraft_frame::Chroma::C420, alpha: None },
            color: filmcraft_color::ColorInfo::REC709,
            par: (1, 1),
            pts: Tick::ZERO,
        }))
    }
    fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> filmcraft_media::Result<filmcraft_frame::AudioBuffer> {
        self.0.audio(start, frames, sample_rate)
    }
}

#[test]
fn draft_reduced_resolution_plans_carry_decimated_yuv() {
    let (mut p, red, _ocean, seq, mut map) = setup();
    let matte = GeneratorSource::new(Generator::ColorMatte { color: [0.0, 0.0, 0.0, 1.0] }, 320, 180, FrameRate::FPS_24, Tick(10 * TICKS_PER_SECOND));
    map.0.insert(red, Arc::new(YuvSource(matte)) as SharedSource);
    place(&mut p, seq, 0, red, 0, 48);
    let t = FrameRate::FPS_24.tick_of(3);
    for draft in [false, true] {
        for (scale, small_w) in [(1.0f32, 320u32), (0.5, 160), (0.25, 80)] {
            let opts = RenderOptions { scale, ..Default::default() };
            let plan = filmcraft_media::cancel::with_draft(draft, || plan::plan_frame(&p, seq, t, opts, &map));
            let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("GPU-drawable") };
            // exact playback hands over the decoded picture; draft playback planes at the drawn size
            assert_eq!(layers[0].frame.width, if draft { small_w } else { 320 }, "scale {scale}, draft {draft}");
            assert!(matches!(layers[0].frame.data, filmcraft_frame::PixelData::Yuv8 { .. }));
            let got = plan::execute_cpu(&plan);
            let reference = render_sequence(&p, seq, t, opts, &map);
            assert_eq!((got.w, got.h), (reference.w, reference.h));
            let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            // the Y'CbCr-code mean differs from a linear-light mean by a little on gradients
            assert!(max < if draft { 0.03 } else { 1e-5 }, "scale {scale}, draft {draft}: max diff {max}");
        }
    }
}

/// Every GPU-capable effect's CPU twin (`FxOp::apply`, what the GPU pass is tested against) must be
/// what `effects::apply` actually renders, or GPU and CPU frames disagree (Ultra Key once went to the
/// GPU with a simplified twin while the CPU ran the full `vfx::color::ultra_key`).
#[test]
fn gpu_effect_twins_match_the_cpu_path() {
    let (w, h) = (23usize, 9usize);
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let f = x as f32 / (w - 1) as f32;
            let c = match x % 4 {
                0 => [f, 0.8 * (1.0 - f), 0.3],
                1 => [1.4 * f, 1.1 * f, 0.2],
                2 => [0.1, 0.75, 0.05],
                _ => [f, f, f],
            };
            let a = [1.0, 0.6, 0.0][y % 3];
            img.px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[c[0] * a, c[1] * a, c[2] * a, a]);
        }
    }
    let cx = crate::effects::FxCtx {
        t: Tick::ZERO,
        px_scale: 1.0,
        seconds: 0.0,
        timecode: "",
        clip_name: "",
        project: None,
        env: None,
        working: filmcraft_color::WorkingSpace::Rec709,
    };
    for id in crate::gpufx::GPU_EFFECTS {
        let e = filmcraft_project::find_effect(id).unwrap_or_else(|| panic!("{id}")).instance();
        let Some(op) = crate::gpufx::FxOp::eval(&e, &cx, w, h) else { continue };
        let mut twin = img.clone();
        op.apply(&mut twin);
        let mut cpu = img.clone();
        crate::effects::apply(&mut cpu, &e, &cx);
        let worst = twin.px.iter().zip(&cpu.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-5, "{id}: twin differs from effects::apply by {worst}");
    }
}

/// HDR and wide-gamut sequences are planned for the GPU too: BT.709 media is decoded there and
/// converted into the working gamut (`plan::input_matrix`), effects run in the working space, and
/// the plan converts for the monitor at the end. Run on the CPU (the GPU's reference), the plan
/// gives the frame `render_sequence` gives.
#[test]
fn hdr_and_wide_gamut_sequences_plan_for_the_gpu() {
    use filmcraft_color::{ColorPipeline, WorkingSpace};
    for color in [
        ColorPipeline { working: WorkingSpace::Rec2100Pq, ..ColorPipeline::REC709 },
        ColorPipeline { working: WorkingSpace::Rec2100Hlg, ..ColorPipeline::REC709 },
        ColorPipeline { wide_gamut: true, ..ColorPipeline::REC709 },
    ] {
        let (mut p, red, ocean, seq, map) = setup();
        place(&mut p, seq, 0, ocean, 0, 48);
        let top = place(&mut p, seq, 1, red, 0, 48);
        {
            let s = p.sequence_mut(seq).unwrap();
            s.settings.color = color;
            let it = s.video_tracks[1].items.iter_mut().find(|i| i.id == top).unwrap();
            let mut l = filmcraft_project::find_effect("lumetri").unwrap().instance();
            for (k, v) in [("exposure", 0.8), ("contrast", 30.0), ("saturation", 140.0)] {
                l.params.get_mut(k).unwrap().value = ParamValue::Float(v);
            }
            it.effects.push(l);
            if let Some(o) = it.effect_mut("opacity").and_then(|o| o.params.get_mut("opacity")) {
                o.value = ParamValue::Float(60.0);
            }
        }
        let t = Tick(1000);
        let plan = plan::plan_frame(&p, seq, t, RenderOptions::default(), &map);
        let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("{color:?}: GPU-drawable") };
        assert!(layers.iter().any(|l| l.fx.as_ref().is_some_and(|f| !f.ops.is_empty())), "{color:?}: Lumetri on the GPU");
        let cpu = render_sequence(&p, seq, t, RenderOptions::default(), &map);
        let via_plan = plan::execute_cpu(&plan);
        let worst = cpu.px.iter().zip(&via_plan.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(worst < 4e-3, "{color:?}: plan differs from render_sequence by {worst}");
    }
}

/// Clips with the GPU3 effects (Noise, Drop Shadow, warps, Generate) are planned as GPU effect
/// layers, at full and half resolution, and the plan run on the CPU gives `render_sequence`'s frame.
#[test]
fn gpu3_effects_plan_as_gpu_layers() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 0, 48);
    let fx = |id: &str, params: &[(&str, f64)]| {
        let mut e = filmcraft_project::find_effect(id).unwrap().instance();
        for (k, v) in params {
            e.params.get_mut(*k).unwrap().value = ParamValue::Float(*v);
        }
        e
    };
    {
        let s = p.sequence_mut(seq).unwrap();
        let (_, it) = s.find_item_mut(top).unwrap();
        it.effects.push(fx("noise", &[("amount", 25.0)]));
        it.effects.push(fx("twirl", &[("angle", 90.0)]));
        it.effects.push(fx("drop_shadow", &[("distance", 20.0), ("softness", 30.0)]));
        it.effects.push(fx("grid", &[("opacity", 40.0)]));
        it.effect_mut("motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(60.0);
    }
    let r = FrameRate::FPS_24;
    for scale in [1.0, 0.5] {
        let opts = RenderOptions { scale, ..Default::default() };
        let plan = plan::plan_frame(&p, seq, r.tick_of(12), opts, &map);
        let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("GPU3 effects should be GPU-drawable") };
        let fx: Vec<&plan::LayerFx> = layers.iter().filter_map(|l| l.fx.as_deref()).collect();
        assert_eq!(fx.len(), 1, "the clip runs the effect stage");
        assert_eq!(fx[0].ops.len(), 4);
        let got = plan::execute_cpu(&plan);
        let reference = render_sequence(&p, seq, r.tick_of(12), opts, &map);
        assert_eq!((got.w, got.h), (reference.w, reference.h));
        let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(max < 0.02, "scale {scale}: max diff {max}");
    }
}

/// Clips with GPU4 effects (glows, Turbulent Displace, Median, Channel Blur, Roughen Edges…) are
/// planned as GPU effect layers, at full and half resolution, and the plan run on the CPU gives
/// `render_sequence`'s frame.
#[test]
fn gpu4_effects_plan_as_gpu_layers() {
    let (mut p, red, ocean, seq, map) = setup();
    place(&mut p, seq, 0, ocean, 0, 48);
    let top = place(&mut p, seq, 1, red, 0, 48);
    let fx = |id: &str, params: &[(&str, f64)]| {
        let mut e = filmcraft_project::find_effect(id).unwrap().instance();
        for (k, v) in params {
            e.params.get_mut(*k).unwrap().value = ParamValue::Float(*v);
        }
        e
    };
    let ids = ["edge_glow", "turbulent_displace", "median", "channel_blur", "roughen_edges", "lens_flare", "reduce_interlace_flicker"];
    {
        let s = p.sequence_mut(seq).unwrap();
        let (_, it) = s.find_item_mut(top).unwrap();
        it.effects.push(fx("edge_glow", &[("radius", 12.0)]));
        it.effects.push(fx("turbulent_displace", &[("amount", 20.0)]));
        it.effects.push(fx("median", &[("radius", 2.0)]));
        it.effects.push(fx("channel_blur", &[("red", 8.0)]));
        it.effects.push(fx("roughen_edges", &[("border", 10.0)]));
        it.effects.push(fx("lens_flare", &[]));
        it.effects.push(fx("reduce_interlace_flicker", &[("softness", 2.0)]));
        it.effect_mut("motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(60.0);
    }
    let r = FrameRate::FPS_24;
    for scale in [1.0, 0.5] {
        let opts = RenderOptions { scale, ..Default::default() };
        let plan = plan::plan_frame(&p, seq, r.tick_of(12), opts, &map);
        let plan::FramePlan::Layers { layers, .. } = &plan else { panic!("GPU4 effects should be GPU-drawable") };
        let fx: Vec<&plan::LayerFx> = layers.iter().filter_map(|l| l.fx.as_deref()).collect();
        assert_eq!(fx.len(), 1, "the clip runs the effect stage");
        assert_eq!(fx[0].ops.len(), ids.len());
        let got = plan::execute_cpu(&plan);
        let reference = render_sequence(&p, seq, r.tick_of(12), opts, &map);
        assert_eq!((got.w, got.h), (reference.w, reference.h));
        let max = got.px.iter().zip(&reference.px).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(max < 0.02, "scale {scale}: max diff {max}");
    }
}
