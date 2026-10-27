//! Tests of [`crate::silence`]: Sequence ▸ Remove Silence on generated speech-like audio (bursts
//! of tone separated by silences of different lengths).

use std::sync::Arc;

use filmcraft_time::{FrameRate, Tick, TimeRange};
use serde_json::json;

use super::*;
use crate::silence::find_silences;

const SR: u32 = 48_000;

/// Stereo audio that is a 440 Hz tone inside `sound` (seconds) and digital silence elsewhere.
fn bursts(total: f64, sound: &[(f64, f64)]) -> Vec<f32> {
    let n = (total * SR as f64) as usize;
    (0..n)
        .flat_map(|i| {
            let t = i as f64 / SR as f64;
            let on = sound.iter().any(|(a, b)| t >= *a && t < *b);
            let v = if on { (0.4 * (t * 440.0 * std::f64::consts::TAU).sin()) as f32 } else { 0.0 };
            [v, v]
        })
        .collect()
}

/// A session whose sequence holds `audio` on A1 from 0 s.
fn session(audio: &[f32]) -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "talk", "audio": 2, "video": 1})).unwrap();
    let bytes: Arc<[u8]> = crate::previews::write_wav_f32(audio, SR).into();
    let item = crate::commands::import_bytes(&mut s, "/talk.wav", bytes, None).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    s
}

fn secs(t: Tick) -> f64 {
    t.seconds()
}

/// 6 s: sound 0–1, silence 1–2.5, sound 2.5–3.5, a short breath 3.5–3.8, sound 3.8–5, silence 5–6.
const TALK: [(f64, f64); 3] = [(0.0, 1.0), (2.5, 3.5), (3.8, 5.0)];

#[test]
fn removes_long_silences_and_keeps_short_ones() {
    let mut s = session(&bursts(6.0, &TALK));
    let frame = s.sequence_rate().frame_duration();
    assert!((secs(s.active_sequence().unwrap().duration()) - 6.0).abs() < 0.05);

    // detect: reports, changes nothing
    let d = s.execute("sequence.removeSilence", json!({"action": "detect"})).unwrap();
    assert_eq!((d["found"].as_u64(), d["removed"].as_u64()), (Some(2), Some(0)), "{d}");
    assert!(s.history.undo.iter().all(|u| u.0 != "Remove Silence"));
    let ranges: Vec<(f64, f64)> =
        d["ranges"].as_array().unwrap().iter().map(|r| (secs(Tick(r["start"].as_i64().unwrap())), secs(Tick(r["end"].as_i64().unwrap())))).collect();
    // 0.1 s of each silence stays next to the sound; the cut points are on frames, inside the silence
    let f = secs(frame);
    assert!(ranges[0].0 >= 1.1 - 1e-9 && ranges[0].0 < 1.1 + f + 0.011, "{ranges:?}");
    assert!(ranges[0].1 <= 2.4 + 1e-9 && ranges[0].1 > 2.4 - f - 0.011, "{ranges:?}");
    // the silence at the end goes up to the end of the sequence
    assert!(ranges[1].0 >= 5.1 - 1e-9 && ranges[1].0 < 5.1 + f + 0.011 && ranges[1].1 > 6.0 - f - 0.011, "{ranges:?}");
    for (a, b) in &ranges {
        for t in [a, b] {
            let ticks = Tick::from_seconds_f64(*t);
            assert!((ticks.0 - s.sequence_rate().snap(ticks).0).abs() <= 1, "cut at {t} s is on a frame");
        }
    }

    // ripple: both are removed in one undo step
    let before = s.active_sequence().unwrap().duration();
    let r = s.execute("sequence.removeSilence", json!({})).unwrap();
    assert_eq!(r["removed"], 2, "{r}");
    let after = s.active_sequence().unwrap().duration();
    let gone = ranges.iter().map(|(a, b)| b - a).sum::<f64>();
    assert!((secs(before - after) - gone).abs() < 1e-6 && (r["seconds"].as_f64().unwrap() - gone).abs() < 1e-6, "{r}");
    assert!(gone > 2.0 && gone < 2.3, "about 1.3 s + 0.9 s: {gone}");
    s.active_sequence().unwrap().check().unwrap();
    assert_eq!(s.history.undo.last().unwrap().0, "Remove Silence");
    // what is left has no silence that long: the kept 0.2 s and the 0.3 s breath are too short
    let again = s.execute("sequence.removeSilence", json!({"action": "detect"})).unwrap();
    assert_eq!(again["found"], 0, "{again}");
    // nothing more to remove is not an error, and adds no undo step
    let n = s.history.undo.len();
    assert_eq!(s.execute("sequence.removeSilence", json!({})).unwrap()["removed"], 0);
    assert_eq!(s.history.undo.len(), n);

    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().duration(), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().duration(), after);
}

#[test]
fn options_change_what_counts_as_silence() {
    let mut s = session(&bursts(6.0, &TALK));
    let found = |s: &mut Session, p: serde_json::Value| s.execute("sequence.removeSilence", p).unwrap()["found"].as_u64().unwrap();
    // a shorter minimum also takes the 0.3 s breath
    assert_eq!(found(&mut s, json!({"action": "detect", "minSeconds": 0.25, "keepSeconds": 0.05})), 3);
    // a longer one leaves the 1 s silence at the end alone
    assert_eq!(found(&mut s, json!({"action": "detect", "minSeconds": 1.2})), 1);
    // keeping more than half of a silence leaves nothing of it to remove
    assert_eq!(found(&mut s, json!({"action": "detect", "keepSeconds": 0.8})), 1, "only the tail, which keeps one side");
    // the tone peaks at -8 dB: a threshold above it makes everything silent
    let all = s.execute("sequence.removeSilence", json!({"action": "detect", "thresholdDb": -3.0, "keepSeconds": 0.0})).unwrap();
    assert_eq!(all["found"], 1);
    assert!((all["silentSeconds"].as_f64().unwrap() - 6.0).abs() < 0.05, "{all}");
}

#[test]
fn hostile_parameters_and_empty_sequences() {
    let mut s = session(&bursts(2.0, &[(0.0, 2.0)]));
    for p in [
        json!({"thresholdDb": 6.0}),
        json!({"thresholdDb": -500.0}),
        json!({"thresholdDb": f64::MAX}),
        json!({"minSeconds": 0.0}),
        json!({"minSeconds": -1.0}),
        json!({"minSeconds": 1e30}),
        json!({"keepSeconds": -0.1}),
        json!({"keepSeconds": 1e30}),
        json!({"action": "explode"}),
    ] {
        assert!(s.execute("sequence.removeSilence", p.clone()).is_err(), "{p}");
    }
    // no silence at all
    assert_eq!(s.execute("sequence.removeSilence", json!({})).unwrap()["removed"], 0);
    // a sequence without audio clips: disabled
    s.execute("file.newSequence", json!({"name": "Empty"})).unwrap();
    assert!(!s.is_enabled("sequence.removeSilence"));
    assert!(s.execute("sequence.removeSilence", json!({})).is_err());
}

#[test]
fn find_silences_on_envelopes() {
    let rate = FrameRate::new(25, 1);
    let (min, keep) = (Tick::from_seconds_f64(0.5), Tick::from_seconds_f64(0.1));
    let find = |p: &[f32]| find_silences(p, 100, 0.01, min, keep, rate);
    let range = |a: f64, b: f64| TimeRange::from_bounds(Tick::from_seconds_f64(a), Tick::from_seconds_f64(b));
    // 1 s sound, 1 s silence, 1 s sound
    let mut e = vec![0.5f32; 300];
    e[100..200].fill(0.0);
    assert_eq!(find(&e), vec![range(1.12, 1.88)], "0.1 s kept each side, snapped inward to 25 fps frames");
    // silence at the start and at the end reaches the edges
    let mut e = vec![0.0f32; 300];
    e[100..200].fill(0.5);
    assert_eq!(find(&e), vec![range(0.0, 0.88), range(2.12, 3.0)]);
    // all silent, all sound, empty
    assert_eq!(find(&[0.0; 200]), vec![range(0.0, 2.0)]);
    assert!(find(&[0.5; 200]).is_empty() && find(&[]).is_empty());
    // shorter than the minimum; NaN counts as sound
    let mut e = vec![0.5f32; 300];
    e[100..140].fill(0.0);
    assert!(find(&e).is_empty());
    let mut e = vec![0.0f32; 300];
    e[150] = f32::NAN;
    assert_eq!(find(&e).len(), 2, "split in two by the NaN window");
    // odd inputs never panic
    assert!(find_silences(&[0.0; 50], 0, 0.01, Tick::ZERO, Tick(i64::MAX / 4), rate).len() <= 1);
    assert!(find_silences(&[0.0; 50], i64::MAX, f32::INFINITY, Tick(-5), Tick(-5), rate).len() <= 1);
}
