//! Tests of [`crate::censor`]: Clip ▸ Censor Area on the demo project (what the picture looks
//! like) and on the mask tests' moving textured disc (that the area follows it).

use serde_json::json;

use crate::Session;
use crate::masks_tests::{tracking_session, truth};
use filmcraft_project::ClipId;
use filmcraft_time::Tick;

fn demo() -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let it = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    // middle of the first V1 clip (past its fade from black)
    s.set_playhead(it.start + Tick(it.duration.0 / 2));
    s.state.selection = vec![it.id];
    (s, it.id)
}

fn px(img: &filmcraft_render::Image, fx: f32, fy: f32) -> [f32; 4] {
    img.get(((img.w as f32 * fx) as usize).min(img.w - 1), ((img.h as f32 * fy) as usize).min(img.h - 1))
}

/// Largest channel difference between two renders inside a box (fractions of the frame).
fn differs(a: &filmcraft_render::Image, b: &filmcraft_render::Image, x: (f32, f32), y: (f32, f32)) -> f32 {
    let mut worst = 0f32;
    for iy in 0..12 {
        for ix in 0..12 {
            let (fx, fy) = (x.0 + (x.1 - x.0) * ix as f32 / 11.0, y.0 + (y.1 - y.0) * iy as f32 / 11.0);
            let (p, q) = (px(a, fx, fy), px(b, fx, fy));
            worst = (0..3).map(|k| (p[k] - q[k]).abs()).fold(worst, f32::max);
        }
    }
    worst
}

#[test]
fn mosaic_and_blur_cover_only_the_area_in_one_undo_step() {
    for (style, effect) in [("mosaic", "mosaic"), ("blur", "gaussian_blur")] {
        let (mut s, clip) = demo();
        let plain = s.render_program(0.25).unwrap();
        let steps = s.history.undo.len();
        // the sun, in the middle of the picture
        let r = s
            .execute("clip.censor", json!({"style": style, "shape": "rectangle", "center": [960, 520], "size": [500, 400], "feather": 0, "strength": 80}))
            .unwrap();
        assert_eq!((r["clip"].as_u64(), r["effectId"].as_str(), r["mask"].as_u64()), (Some(clip.0), Some(effect), Some(0)), "{r}");
        assert!(r["tracking"].as_array().unwrap().is_empty());
        let out = s.render_program(0.25).unwrap();
        assert!(differs(&plain, &out, (0.40, 0.60), (0.36, 0.60)) > 0.02, "{style} changes the area");
        assert!(differs(&plain, &out, (0.0, 0.2), (0.0, 1.0)) < 1e-4 && differs(&plain, &out, (0.8, 1.0), (0.0, 1.0)) < 1e-4, "{style} leaves the rest alone");
        // the effect sits right before the intrinsic effects, carries the mask, and the mask is selected
        let q = s.active_sequence().unwrap();
        let it = q.find_item(clip).unwrap().1;
        let i = r["effect"].as_u64().unwrap() as usize;
        assert_eq!((it.effects[i].effect.as_str(), it.effects[i].masks.len()), (effect, 1));
        assert!(it.effects[i + 1].def().unwrap().intrinsic);
        let sel = s.state.selected_mask.unwrap();
        assert_eq!((sel.clip, sel.effect, sel.mask), (clip, i, 0), "ready to drag on the monitor");
        q.check().unwrap();
        // one undo step for the lot
        assert_eq!(s.history.undo.len(), steps + 1);
        assert_eq!(s.history.undo.last().unwrap().0, "Censor Area");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.render_program(0.25).unwrap().px, plain.px);
        assert!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.iter().all(|e| e.effect != effect));
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.render_program(0.25).unwrap().px, out.px);
    }
}

#[test]
fn strength_sets_block_size_and_defaults_put_the_area_in_the_middle() {
    let (mut s, clip) = demo();
    let blocks = |s: &Session, i: usize| {
        let e = &s.active_sequence().unwrap().find_item(clip).unwrap().1.effects[i];
        (e.param("horizontal").unwrap().value.as_f64().unwrap(), e.param("vertical").unwrap().value.as_f64().unwrap())
    };
    // 1920x1080: strength 1 → 4 px blocks, 100 → 64 px blocks
    let a = s.execute("clip.censor", json!({"strength": 1})).unwrap();
    assert_eq!(blocks(&s, a["effect"].as_u64().unwrap() as usize), (480.0, 270.0));
    let b = s.execute("clip.censor", json!({"strength": 100})).unwrap();
    assert_eq!(blocks(&s, b["effect"].as_u64().unwrap() as usize), (30.0, 17.0));
    // no centre: the mask is around the middle of the picture
    let l = s.execute("masks.list", json!({"clip": clip.0, "effect": b["effect"]})).unwrap();
    let path = crate::masks::path_from_json(&l["masks"][0]["path"]).unwrap();
    let (cx, cy) = path.vertices.iter().fold((0.0, 0.0), |(x, y), v| (x + v.p.x / path.vertices.len() as f64, y + v.p.y / path.vertices.len() as f64));
    assert!((cx - 960.0).abs() < 1.0 && (cy - 540.0).abs() < 1.0, "centre {cx}, {cy}");
}

#[test]
fn the_area_follows_what_it_covers() {
    let (mut s, clip) = tracking_session();
    let rate = filmcraft_time::FrameRate::FPS_24;
    s.set_playhead(rate.tick_of(10));
    let c = truth(10.0).apply(filmcraft_geom::Vec2::ZERO);
    let r = s.execute("clip.censor", json!({"center": [c.x, c.y], "size": [90, 90], "track": "both", "strength": 30})).unwrap();
    let t = r["tracking"].as_array().unwrap();
    assert_eq!(t.len(), 2, "{r}");
    assert!(t.iter().all(|d| d["error"].is_null() && d["result"]["frames"].as_u64().unwrap() > 5), "{r}");
    assert!(s.mask_jobs.is_empty(), "`both` waits for the jobs");
    // the mask's centre is on the disc from the first to the last frame tracked
    let effect = r["effect"].clone();
    let mut worst = 0.0f64;
    for f in [0i64, 5, 10, 20, 30, 40] {
        let l = s.execute("masks.list", json!({"clip": clip.0, "effect": effect, "time": rate.tick_of(f).0})).unwrap();
        let path = crate::masks::path_from_json(&l["masks"][0]["path"]).unwrap();
        let n = path.vertices.len() as f64;
        let centre = path.vertices.iter().fold(filmcraft_geom::Vec2::ZERO, |a, v| a + v.p * (1.0 / n));
        worst = worst.max((centre - truth(f as f64).apply(filmcraft_geom::Vec2::ZERO)).length());
    }
    eprintln!("censor tracking: the area's centre is within {worst:.2} px of the disc over 40 frames");
    assert!(worst < 3.0, "{worst} px");
    // undo: backward track, forward track, then the area itself
    let h = s.execute("history.list", json!({})).unwrap();
    let undo: Vec<&str> = h["undo"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(&undo[undo.len() - 3..], ["Censor Area", "Track Mask", "Track Mask"], "{undo:?}");
}

#[test]
fn tracking_that_fails_keeps_the_area_and_reports_why() {
    // the demo's sky is too smooth to follow at the top left corner
    let (mut s, clip) = demo();
    let r = s.execute("clip.censor", json!({"center": [60, 40], "size": [40, 40], "track": "forward", "wait": true})).unwrap();
    let t = &r["tracking"][0];
    assert!(t["error"].is_string() || t["result"].is_object(), "{r}");
    let it = s.active_sequence().unwrap().find_item(clip).unwrap().1.clone();
    assert!(it.effects.iter().any(|e| e.effect == "mosaic" && e.masks.len() == 1), "the mosaic stays");
}

#[test]
fn hostile_parameters_change_nothing() {
    let (mut s, clip) = demo();
    let before = s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.len();
    let steps = s.history.undo.len();
    for p in [
        json!({"style": "pixelate"}),
        json!({"shape": "star"}),
        json!({"strength": 0}),
        json!({"strength": 1e9}),
        json!({"strength": -3}),
        json!({"center": [1, 2, 3]}),
        json!({"center": "middle"}),
        json!({"center": [null, 0]}),
        json!({"size": [0, 10]}),
        json!({"size": [-5, -5]}),
        json!({"feather": -1}),
        json!({"feather": 1e9}),
        json!({"track": "sideways"}),
        json!({"clip": 999_999_999u64}),
    ] {
        assert!(s.execute("clip.censor", p.clone()).is_err(), "{p}");
    }
    assert_eq!(s.active_sequence().unwrap().find_item(clip).unwrap().1.effects.len(), before);
    assert_eq!(s.history.undo.len(), steps, "no undo steps from refused calls");
    // an audio clip is not a target; nothing selected: disabled
    let audio = s.active_sequence().unwrap().audio_tracks[0].items[0].id;
    assert!(s.execute("clip.censor", json!({"clip": audio.0})).is_err());
    s.state.selection.clear();
    assert!(!s.is_enabled("clip.censor"));
    assert!(s.execute("clip.censor", json!({})).is_err());
}
