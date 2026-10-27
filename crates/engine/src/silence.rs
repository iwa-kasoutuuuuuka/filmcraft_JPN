//! Sequence ▸ Remove Silence (jump cuts).
//!
//! `sequence.removeSilence {thresholdDb=-40, minSeconds=0.5, keepSeconds=0.1, action="ripple"}`
//! finds the stretches of the active sequence where the mix stays below `thresholdDb` for at
//! least `minSeconds` and ripple-deletes them on every unlocked track in one undo step ("Remove
//! Silence"). `keepSeconds` of the silence is left next to the sound on each side, so words are
//! not clipped; silence at the very start or end of the sequence is removed up to the edge.
//! `action: "detect"` changes nothing and returns the ranges.
//!
//! The level is the peak of the sequence's mix (the mixer graph, as playback and export hear it:
//! muted tracks, fades and effects included) in 10 ms windows. Ranges are snapped inward to
//! frames, so every cut lands on a frame boundary and never eats into the sound. No transcript
//! and no speech model are needed (`transcript.removePauses` is the word-based counterpart).

use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, str_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "sequence.removeSilence";
/// Envelope windows per second (10 ms each).
const WINDOWS_PER_SECOND: i64 = 100;
/// Longest sequence the detector scans (24 hours of windows).
const MAX_WINDOWS: usize = 24 * 3600 * WINDOWS_PER_SECOND as usize;
/// Ranges listed in the reply (all of them are removed; the list is for display).
const MAX_LISTED: usize = 1000;

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Remove Silence…",
        menu: &["Sequence"],
        shortcut: None,
        params: r#"{"thresholdDb":-96..0=-40,"minSeconds":f=0.5,"keepSeconds":f=0.1,"action":"ripple|detect"="ripple"}"#,
        enabled: has_audio_clips,
        run: remove_silence,
        journal: true,
    }]
}

fn has_audio_clips(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.audio_tracks.iter().any(|t| !t.items.is_empty()) { Ok(()) } else { Err("the sequence has no audio clips".into()) }
}

/// The silent stretches of an envelope as the ranges to remove.
///
/// `peaks[i]` is the peak level (linear) of window `i`; windows are `1 / windows_per_second` long
/// and start at time 0. A stretch is silent while every window is below `threshold`; stretches
/// shorter than `min` are left alone. `keep` of each stretch stays next to the sound on both
/// sides, except at the two ends of the envelope, where there is no sound to protect. Ranges are
/// snapped inward to frames of `rate` and are in time order, apart from each other.
pub fn find_silences(peaks: &[f32], windows_per_second: i64, threshold: f32, min: Tick, keep: Tick, rate: FrameRate) -> Vec<TimeRange> {
    let wps = windows_per_second.max(1);
    let at = |i: usize| Tick::from_units(i as i64, wps);
    let frame = rate.frame_duration();
    let mut out = Vec::new();
    let mut i = 0;
    while i < peaks.len() {
        // NaN is not below anything: it counts as sound
        let silent = |p: &f32| *p < threshold;
        if !peaks.get(i).is_some_and(silent) {
            i += 1;
            continue;
        }
        let start = i;
        while peaks.get(i).is_some_and(silent) {
            i += 1;
        }
        let (a, b) = (at(start), at(i));
        if b - a < min {
            continue;
        }
        let s = if start == 0 { a } else { a + keep };
        let e = if i == peaks.len() { b } else { b - keep };
        let mut s2 = rate.snap(s);
        if s2 < s {
            s2 += frame;
        }
        let e2 = rate.snap(e);
        if e2 > s2 {
            out.push(TimeRange::from_bounds(s2, e2));
        }
    }
    out
}

/// Peak level of the active sequence's mix in windows of `1 / WINDOWS_PER_SECOND` seconds.
fn mix_envelope(s: &Session) -> Result<Vec<f32>> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.project.sequence(seq_id).ok_or(EngineError::NoSequence)?;
    let sr = i64::from(q.settings.sample_rate.max(1));
    let total = q.duration().to_units_floor(sr);
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let windows = usize::try_from(Tick::from_units(total, sr).to_units_floor(WINDOWS_PER_SECOND)).unwrap_or(0);
    if windows > MAX_WINDOWS {
        return Err(EngineError::Other(format!("the sequence is longer than {} hours", MAX_WINDOWS / WINDOWS_PER_SECOND as usize / 3600)));
    }
    let mut peaks = vec![0f32; windows];
    let mut pos = 0i64;
    while pos < total {
        let n = (total - pos).min(sr);
        let b = filmcraft_render::audio::mix_sequence(&s.project, q, pos, n as usize, &provider);
        for ch in &b.channels {
            for (k, v) in ch.iter().enumerate() {
                // window of sample `pos + k`
                let w = (pos + k as i64).saturating_mul(WINDOWS_PER_SECOND) / sr;
                if let Some(p) = usize::try_from(w).ok().and_then(|w| peaks.get_mut(w)) {
                    *p = p.max(v.abs());
                }
            }
        }
        pos += n;
    }
    Ok(peaks)
}

fn seconds(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

fn remove_silence(s: &mut Session, p: &Value) -> Result<Value> {
    let threshold_db = f64_p(p, "thresholdDb").unwrap_or(-40.0);
    if !(-96.0..=0.0).contains(&threshold_db) {
        return Err(bad(CMD, "`thresholdDb` must be between -96 and 0"));
    }
    let min = f64_p(p, "minSeconds").unwrap_or(0.5);
    if !(0.05..=600.0).contains(&min) {
        return Err(bad(CMD, "`minSeconds` must be between 0.05 and 600"));
    }
    let keep = f64_p(p, "keepSeconds").unwrap_or(0.1);
    if !(0.0..=60.0).contains(&keep) {
        return Err(bad(CMD, "`keepSeconds` must be between 0 and 60"));
    }
    let detect_only = match str_p(p, "action").unwrap_or("ripple") {
        "ripple" => false,
        "detect" => true,
        _ => return Err(bad(CMD, "`action` must be `ripple` or `detect`")),
    };
    let peaks = mix_envelope(s)?;
    let threshold = 10f64.powf(threshold_db / 20.0) as f32;
    let ranges = find_silences(&peaks, WINDOWS_PER_SECOND, threshold, Tick::from_seconds_f64(min), Tick::from_seconds_f64(keep), s.sequence_rate());
    let listed: Vec<Value> = ranges.iter().take(MAX_LISTED).map(|r| json!({"start": r.start.0, "end": r.end().0, "seconds": seconds(r.duration)})).collect();
    let found: Tick = ranges.iter().fold(Tick::ZERO, |a, r| a + r.duration);
    let n = ranges.len();
    if detect_only || n == 0 {
        return Ok(json!({"removed": 0, "found": n, "ticks": 0, "seconds": 0.0, "silentSeconds": seconds(found), "ranges": listed}));
    }
    let total = s.edit_sequence("Remove Silence", |q, ctx, _| Ok(filmcraft_edit::transcript::ripple_delete_ranges(q, ranges, ctx)))?;
    Ok(json!({"removed": n, "found": n, "ticks": total.0, "seconds": seconds(total), "silentSeconds": seconds(found), "ranges": listed}))
}
