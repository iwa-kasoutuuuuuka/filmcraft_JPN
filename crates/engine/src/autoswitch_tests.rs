//! `multicam.autoSwitch` end to end: two people take turns talking, each on their own camera's
//! microphone (with the other one bleeding in), and a wide camera hears both.

use std::path::PathBuf;

use filmcraft_media::DemoScene;
use filmcraft_project::ItemId;
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::Session;
use crate::media_test_util::{make_movie_with_audio, tmp_dir};

const SR: usize = 48_000;
const FRAMES: i64 = 96; // 4 s at 24 fps

/// A voice-like signal: a 180 Hz buzz with a syllable rhythm, at `gain` while `talking(t)`.
fn voice(seconds: f64, talking: impl Fn(f64) -> bool, gain: f32, bleed: f32) -> Vec<f32> {
    let n = (seconds * SR as f64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let syllable = if (t * 4.0).fract() < 0.7 { 1.0 } else { 0.3 };
            let buzz = ((t * 180.0 * std::f64::consts::TAU).sin() + 0.5 * (t * 360.0 * std::f64::consts::TAU).sin()) as f32;
            buzz * syllable as f32 * if talking(t) { gain } else { bleed }
        })
        .collect()
}

/// Cameras A (talks 0–2 s), B (talks 2–4 s) and a wide camera C hearing both; a multi-camera
/// source of the three and an edit sequence holding one multi-camera clip on V1/A1.
fn session() -> (Session, ItemId) {
    let dir = tmp_dir("autoswitch");
    let files: Vec<PathBuf> = ["a.mov", "b.mov", "c.mov"].iter().map(|f| dir.join(f)).collect();
    let secs = 4.0;
    let a = voice(secs, |t| t < 2.0, 0.5, 0.04);
    let b = voice(secs, |t| t >= 2.0, 0.5, 0.04);
    let c = voice(secs, |_| true, 0.2, 0.2);
    for ((f, sc), audio) in files.iter().zip([DemoScene::OceanSunset, DemoScene::CityNight, DemoScene::Aurora]).zip([a, b, c]) {
        make_movie_with_audio(f, sc, 160, 90, FRAMES, &audio);
    }
    let mut s = Session::default();
    let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let r = s.execute("file.import", json!({"paths": paths})).unwrap();
    let items: Vec<u64> = r["items"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    s.execute("project.select", json!({"items": items})).unwrap();
    let r = s.execute("clip.createMulticam", json!({"items": items, "method": "in", "audio": "switch", "cameraNames": "track"})).unwrap();
    let src = ItemId(r["sequence"].as_u64().unwrap());
    let mut p = (*s.project).clone();
    let edit =
        p.new_sequence("Talk", filmcraft_project::SequenceSettings { width: 160, height: 90, frame_rate: FrameRate::FPS_24, ..Default::default() }, 2, 2, None);
    s.project = std::sync::Arc::new(p);
    s.state.active_sequence = Some(edit);
    s.execute("source.open", json!({"item": src.0})).unwrap();
    s.execute("source.overwrite", json!({})).unwrap();
    s.set_playhead(Tick(TICKS_PER_SECOND / 2));
    (s, edit)
}

/// (start frame, angle) of the clips on the first video / audio track.
fn angles(s: &Session, video: bool) -> Vec<(i64, u32)> {
    let q = s.active_sequence().unwrap();
    let rate = q.settings.frame_rate;
    let tracks = if video { &q.video_tracks } else { &q.audio_tracks };
    tracks[0].items.iter().map(|i| (rate.frame_at(i.start), i.multicam.map_or(99, |m| m.angle))).collect()
}

fn cut_seconds(r: &Value) -> Vec<(f64, u32)> {
    r["cuts"].as_array().unwrap().iter().map(|c| (c["seconds"].as_f64().unwrap(), c["angle"].as_u64().unwrap() as u32)).collect()
}

#[test]
fn cuts_to_whoever_talks_and_back_in_one_undo_step() {
    let (mut s, _) = session();
    assert_eq!(angles(&s, true).len(), 1, "one multi-camera clip to start with");
    let before = s.project.clone();

    // detect: the cut from A to B lands where B starts talking, nothing changes
    let r = s.execute("multicam.autoSwitch", json!({"action": "detect", "wideCamera": 3})).unwrap();
    let cuts = cut_seconds(&r);
    assert_eq!(cuts.len(), 2, "{r}");
    assert_eq!((cuts[0].0, cuts[0].1), (0.0, 0), "opens on A: {r}");
    assert_eq!(cuts[1].1, 1, "then B: {r}");
    assert!((cuts[1].0 - 2.0).abs() <= 0.2, "B starts talking at 2 s: {r}");
    assert!(std::sync::Arc::ptr_eq(&before, &s.project), "detect changes nothing");

    // audio follows video off (Premiere's default): only the picture is cut
    s.execute("multicam.autoSwitch", json!({"wideCamera": 3, "wait": true})).unwrap();
    assert_eq!(angles(&s, true).len(), 2);
    assert_eq!(angles(&s, false).len(), 1, "the audio is left alone");
    s.execute("edit.undo", json!({})).unwrap();

    // apply with audio following video: V1 and the linked A1 are cut and switched, in one undo step
    s.execute("multicam.audioFollowsVideo", json!({"enabled": true})).unwrap();
    let r = s.execute("multicam.autoSwitch", json!({"wideCamera": 3, "wait": true})).unwrap();
    assert_eq!(r["applied"], true);
    let v = angles(&s, true);
    assert_eq!(v.iter().map(|x| x.1).collect::<Vec<_>>(), vec![0, 1], "{v:?}");
    assert!((v[1].0 - 48).abs() <= 5, "the cut is near frame 48: {v:?}");
    let a = angles(&s, false);
    assert_eq!(a, v, "the audio follows the video");
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Auto Switch by Speaker"));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(angles(&s, true).len(), 1, "one undo restores the single clip");

    // a second run over the pieces the first one left is stable
    s.execute("multicam.autoSwitch", json!({"wideCamera": 3, "wait": true})).unwrap();
    let once = angles(&s, true);
    s.execute("multicam.autoSwitch", json!({"wideCamera": 3, "wait": true})).unwrap();
    assert_eq!(angles(&s, true), once, "running it again changes nothing");

    // video only: the audio keeps its angle
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("multicam.autoSwitch", json!({"wideCamera": 3, "videoOnly": true, "wait": true})).unwrap();
    assert_eq!(angles(&s, true).len(), 2);
    assert_eq!(angles(&s, false).len(), 1, "videoOnly leaves the audio alone");
}

#[test]
fn the_wide_camera_does_not_vote_and_long_minimum_shots_hold() {
    let (mut s, _) = session();
    // without a wide shot, C's microphone (which hears both) votes too, but A and B are louder
    let r = s.execute("multicam.autoSwitch", json!({"action": "detect"})).unwrap();
    let angles_only: Vec<u32> = cut_seconds(&r).iter().map(|c| c.1).collect();
    assert_eq!(angles_only, vec![0, 1], "{r}");
    // a minimum shot longer than the clip: one shot only
    let r = s.execute("multicam.autoSwitch", json!({"action": "detect", "minShotSeconds": 10, "wideCamera": 3})).unwrap();
    assert_eq!(cut_seconds(&r).len(), 1, "{r}");
}

#[test]
fn bad_parameters_are_refused_and_change_nothing() {
    let (mut s, _) = session();
    let before = s.project.clone();
    for p in [json!({"wideCamera": 9, "wait": true}), json!({"action": "explode"}), json!({"clip": 123_456}), json!({"time": i64::MAX})] {
        assert!(s.execute("multicam.autoSwitch", p.clone()).is_err(), "{p}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.project), "{p} changed the project");
    }
    // camera 0 means no wide shot
    assert!(s.execute("multicam.autoSwitch", json!({"wideCamera": 0, "action": "detect"})).is_ok());
    // out-of-range numbers are clamped, not trusted
    for p in [json!({"minShotSeconds": -5, "marginDb": 1e9, "silenceDb": "loud", "bothDb": -1, "wait": true}), json!({"minShotSeconds": 1e300, "wait": true})] {
        assert!(s.execute("multicam.autoSwitch", p.clone()).is_ok(), "{p}");
    }
}

/// Poll until the session has no running analysis (at most 60 s).
fn finish_jobs(s: &mut Session) {
    let t0 = std::time::Instant::now();
    while !s.autoswitch_jobs.is_empty() {
        assert!(t0.elapsed().as_secs() < 60, "the analysis did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_persistence();
    }
}

#[test]
fn runs_as_a_background_job_and_applies_in_one_undo_step() {
    let (mut s, _) = session();
    let undo = s.history.undo.len();
    let r = s.execute("multicam.autoSwitch", json!({"wideCamera": 3})).unwrap();
    let job = r["job"].as_u64().expect("a background job");
    assert_eq!(r["cameras"], 2, "{r}");
    assert!(s.jobs.iter().any(|j| j.id == job && j.label == "Auto Switch by Speaker"));
    // a second start on the same sequence while it runs is refused
    if !s.autoswitch_jobs.is_empty() {
        assert!(s.execute("multicam.autoSwitch", json!({"wideCamera": 3})).is_err());
    }
    finish_jobs(&mut s);
    assert_eq!(angles(&s, true).iter().map(|x| x.1).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Auto Switch by Speaker"));
    let j = s.jobs.iter().find(|j| j.id == job).unwrap();
    assert!(j.progress.finished.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(j.progress.done.load(std::sync::atomic::Ordering::Relaxed), j.progress.total.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn a_cancelled_job_changes_nothing() {
    let (mut s, _) = session();
    let (before, undo) = (s.project.clone(), s.history.undo.len());
    let r = s.execute("multicam.autoSwitch", json!({"wideCamera": 3})).unwrap();
    s.execute("jobs.cancel", json!({"job": r["job"]})).unwrap();
    finish_jobs(&mut s);
    assert!(std::sync::Arc::ptr_eq(&before, &s.project), "cancelled: nothing applied");
    assert_eq!(s.history.undo.len(), undo);
}

#[test]
fn an_edit_during_the_analysis_wins_over_stale_cuts() {
    let (mut s, _) = session();
    s.execute("multicam.autoSwitch", json!({"wideCamera": 3})).unwrap();
    // the user trims the clip while the analysis runs
    let mut p = (*s.project).clone();
    let seq = s.state.active_sequence.unwrap();
    let it = &mut p.sequence_mut(seq).unwrap().video_tracks[0].items[0];
    it.duration -= FrameRate::FPS_24.tick_of(12);
    s.project = std::sync::Arc::new(p);
    let trimmed = s.project.clone();
    finish_jobs(&mut s);
    assert!(std::sync::Arc::ptr_eq(&trimmed, &s.project), "stale cuts are not applied");
    assert_eq!(angles(&s, true).len(), 1);
}

#[test]
fn a_locked_track_is_refused_and_leaves_no_undo_step() {
    let (mut s, _) = session();
    let mut p = (*s.project).clone();
    let seq = s.state.active_sequence.unwrap();
    p.sequence_mut(seq).unwrap().video_tracks[0].locked = true;
    s.project = std::sync::Arc::new(p);
    let (before, undo) = (s.project.clone(), s.history.undo.len());
    assert!(s.execute("multicam.autoSwitch", json!({"wideCamera": 3, "wait": true})).is_err(), "the clip's track is locked");
    assert!(std::sync::Arc::ptr_eq(&before, &s.project));
    assert_eq!(s.history.undo.len(), undo, "no empty undo step");
}

#[test]
fn a_sequence_without_multicam_clips_disables_it() {
    let mut s = Session::default();
    let mut p = (*s.project).clone();
    let q = p.new_sequence("Empty", Default::default(), 1, 1, None);
    s.project = std::sync::Arc::new(p);
    s.state.active_sequence = Some(q);
    let c = crate::find_command("multicam.autoSwitch").unwrap();
    assert!((c.enabled)(&s).is_err());
    assert!(s.execute("multicam.autoSwitch", json!({"wait": true})).is_err());
}
