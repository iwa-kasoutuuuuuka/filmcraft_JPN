//! Tests of [`crate::vertical`]: Sequence ▸ Make Vertical Version on the demo project.

use serde_json::json;

use crate::Session;
use crate::vertical::target_size;
use filmcraft_project::ItemId;
use filmcraft_time::Tick;

fn demo() -> (Session, ItemId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let seq = s.state.active_sequence.unwrap();
    // inside the first clip, past its fade from black
    s.set_playhead(Tick::from_seconds_f64(2.0));
    (s, seq)
}

/// Mean brightness of a box of a render (fractions of the frame).
fn mean(img: &filmcraft_render::Image, x: (f32, f32), y: (f32, f32)) -> f32 {
    let (mut acc, mut n) = (0f32, 0f32);
    for iy in 0..16 {
        for ix in 0..16 {
            let fx = ((x.0 + (x.1 - x.0) * ix as f32 / 15.0) * img.w as f32) as usize;
            let fy = ((y.0 + (y.1 - y.0) * iy as f32 / 15.0) * img.h as f32) as usize;
            let p = img.get(fx.min(img.w - 1), fy.min(img.h - 1));
            acc += p[0] + p[1] + p[2];
            n += 3.0;
        }
    }
    acc / n
}

#[test]
fn sizes_keep_the_shorter_side() {
    assert_eq!(target_size(1080, (9, 16)), (1080, 1920));
    assert_eq!(target_size(1080, (1, 1)), (1080, 1080));
    assert_eq!(target_size(1080, (4, 5)), (1080, 1350));
    assert_eq!(target_size(2160, (9, 16)), (2160, 3840));
    assert_eq!(target_size(721, (9, 16)), (720, 1280), "even sizes");
    // odd inputs give a usable size, never a panic
    assert_eq!(target_size(0, (9, 16)), (2, 2));
    assert_eq!(target_size(u32::MAX, (9, 16)), (16_384, 16_384));
    assert_eq!(target_size(1080, (0, 0)), (1080, 1080));
}

#[test]
fn every_mode_makes_a_filled_vertical_sequence_that_nests_the_original() {
    for (mode, layers) in [("fill", 1usize), ("follow", 1), ("blur", 2)] {
        let (mut s, src) = demo();
        let original = (*s.project.sequence(src).unwrap()).clone();
        let wide = s.render_program(0.25).unwrap();
        let steps = s.history.undo.len();
        let r = s.execute("sequence.makeVertical", json!({"mode": mode})).unwrap();
        let seq = ItemId(r["sequence"].as_u64().unwrap());
        assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["source"].as_u64()), (Some(1080), Some(1920), Some(src.0)), "{mode}: {r}");
        assert_eq!(r["name"], "Main Edit (Vertical 9x16)");
        // the new sequence is open, shaped 9:16, as long as the original, with its audio
        assert_eq!(s.state.active_sequence, Some(seq));
        let q = s.active_sequence().unwrap();
        assert_eq!((q.settings.width, q.settings.height), (1080, 1920));
        assert_eq!((q.settings.frame_rate, q.settings.sample_rate), (original.settings.frame_rate, original.settings.sample_rate));
        assert_eq!(q.duration(), original.duration(), "{mode}");
        let clips: Vec<_> = q.video_tracks.iter().flat_map(|t| t.items.iter()).collect();
        assert_eq!(clips.len(), layers, "{mode}");
        assert!(clips.iter().all(|c| c.item == src), "{mode}: the clips are the original sequence, nested");
        assert_eq!(q.audio_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 1, "{mode}: one audio clip");
        q.check().unwrap();
        // the original is untouched
        assert_eq!(*s.project.sequence(src).unwrap(), original, "{mode}");
        // the frame is filled top to bottom with picture, not letterboxed
        s.set_playhead(Tick::from_seconds_f64(2.0));
        let tall = s.render_program(0.25).unwrap();
        assert_eq!((tall.w, tall.h), (270, 480));
        let (top, middle, bottom) = (mean(&tall, (0.2, 0.8), (0.02, 0.10)), mean(&tall, (0.2, 0.8), (0.45, 0.55)), mean(&tall, (0.2, 0.8), (0.90, 0.98)));
        assert!(top > 0.02 && middle > 0.02 && bottom > 0.005, "{mode}: top {top}, middle {middle}, bottom {bottom}");
        assert!(mean(&wide, (0.3, 0.7), (0.3, 0.7)) > 0.02, "the original has a picture there");
        // one undo step; undoing closes and removes the new sequence
        assert_eq!(s.history.undo.len(), steps + 1, "{mode}");
        assert_eq!(s.history.undo.last().unwrap().0, "Make Vertical Version");
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.project.sequence(seq).is_none(), "{mode}: undo removes the sequence");
        assert_eq!(*s.project.sequence(src).unwrap(), original);
    }
}

#[test]
fn blur_mode_shows_the_whole_picture_over_a_blurred_copy() {
    let (mut s, _) = demo();
    s.execute("sequence.makeVertical", json!({"mode": "blur"})).unwrap();
    s.set_playhead(Tick::from_seconds_f64(2.0));
    let q = s.active_sequence().unwrap();
    let back = &q.video_tracks[0].items[0];
    let front = &q.video_tracks[1].items[0];
    let scale = |it: &filmcraft_project::TrackItem| it.effects.iter().find(|e| e.effect == "motion").unwrap().param("scale").unwrap().value.as_f64().unwrap();
    // 1920x1080 in 1080x1920: all of it fits at 56.25 %, it fills at 177.8 %
    assert!((scale(front) - 56.25).abs() < 0.01 && (scale(back) - 177.78).abs() < 0.01, "{} / {}", scale(front), scale(back));
    let blur = back.effects.iter().find(|e| e.effect == "gaussian_blur").expect("the background is blurred");
    assert!(blur.param("blurriness").unwrap().value.as_f64().unwrap() > 50.0);
    assert!(front.effects.iter().all(|e| e.effect != "gaussian_blur"));
    // the sharp picture is a band across the middle: detail there, smooth above and below
    let img = s.render_program(0.5).unwrap();
    let detail = |y0: f32, y1: f32| {
        let (mut d, mut n) = (0f32, 0f32);
        for iy in ((y0 * img.h as f32) as usize)..((y1 * img.h as f32) as usize) {
            for ix in 8..img.w - 9 {
                d += (img.get(ix, iy)[0] - img.get(ix + 1, iy)[0]).abs();
                n += 1.0;
            }
        }
        d / n.max(1.0)
    };
    let (band, above) = (detail(0.42, 0.58), detail(0.08, 0.24));
    assert!(band > above * 1.5, "detail in the band {band} vs the blurred area {above}");
}

#[test]
fn aspects_names_and_edits_to_the_original() {
    let (mut s, src) = demo();
    let r = s.execute("sequence.makeVertical", json!({"aspect": "1:1", "mode": "fill", "name": "  Square cut  "})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["name"].as_str()), (Some(1080), Some(1080), Some("Square cut")));
    let square = ItemId(r["sequence"].as_u64().unwrap());
    // 4:5 of the original, not of the square one
    s.state.active_sequence = Some(src);
    let r = s.execute("sequence.makeVertical", json!({"aspect": "4:5", "mode": "fill"})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1080), Some(1350)));
    assert_eq!(r["name"], "Main Edit (Vertical 4x5)");
    // an edit to the original shows in the nested version
    s.state.active_sequence = Some(square);
    s.set_playhead(Tick::from_seconds_f64(2.0));
    let before = s.render_program(0.25).unwrap();
    s.state.active_sequence = Some(src);
    let first = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("effects.apply", json!({"clips": [first.0], "effect": "invert"})).unwrap();
    s.state.active_sequence = Some(square);
    s.set_playhead(Tick::from_seconds_f64(2.0));
    let after = s.render_program(0.25).unwrap();
    assert_ne!(before.px, after.px, "the vertical version follows the original");
}

#[test]
fn hostile_parameters_and_empty_sequences() {
    let (mut s, src) = demo();
    let sequences = |s: &Session| s.project.items.values().filter(|i| matches!(i.kind, filmcraft_project::ItemKind::Sequence(_))).count();
    let (n, steps) = (sequences(&s), s.history.undo.len());
    for p in [json!({"aspect": "16:9"}), json!({"aspect": 7}), json!({"aspect": ""}), json!({"mode": "stretch"}), json!({"mode": 1})] {
        assert!(s.execute("sequence.makeVertical", p.clone()).is_err(), "{p}");
    }
    assert_eq!((sequences(&s), s.history.undo.len()), (n, steps), "refused calls make nothing");
    assert_eq!(s.state.active_sequence, Some(src));
    // a very long name is cut, not refused
    let r = s.execute("sequence.makeVertical", json!({"name": "x".repeat(5000), "mode": "fill"})).unwrap();
    assert_eq!(r["name"].as_str().unwrap().len(), 200);
    // a sequence without video: disabled
    s.execute("file.newSequence", json!({"name": "Empty"})).unwrap();
    assert!(!s.is_enabled("sequence.makeVertical"));
    assert!(s.execute("sequence.makeVertical", json!({})).is_err());
}

/// Horizontal position (0..1) of the bright disc of the mask tests' moving picture in a render.
fn disc_x(img: &filmcraft_render::Image) -> f32 {
    let (mut sx, mut n) = (0f32, 0f32);
    for y in 0..img.h {
        for x in 0..img.w {
            let p = img.get(x, y);
            if p[0] > 0.16 {
                sx += x as f32;
                n += 1.0;
            }
        }
    }
    assert!(n > 20.0, "the disc is in the picture ({n} bright samples)");
    sx / n / img.w as f32
}

#[test]
fn follow_keeps_an_off_centre_subject_in_the_frame() {
    // a textured disc that moves from the left of the picture to the right over 48 frames
    let at = |mode: &str, frame: i64| {
        let (mut s, _) = crate::masks_tests::tracking_session();
        s.execute("sequence.makeVertical", json!({"mode": mode})).unwrap();
        s.set_playhead(filmcraft_time::FrameRate::FPS_24.tick_of(frame));
        disc_x(&s.render_program(1.0).unwrap())
    };
    // late in the clip the disc is right of the centre: a centre crop shows it near the right edge
    let (fill, follow) = (at("fill", 40), at("follow", 40));
    eprintln!("disc at frame 40: centre crop {fill:.3}, follow {follow:.3} (0.5 = middle of the frame)");
    assert!(fill > 0.62, "the centre crop has the disc off to the right: {fill}");
    assert!((follow - 0.5).abs() < (fill - 0.5).abs() - 0.05, "follow brings it towards the middle: {follow} vs {fill}");
}
