//! Clip ▸ Multi-Camera ▸ Auto Switch by Speaker: cut a multi-camera clip to whoever is talking.
//!
//! | Id | Menu | |
//! |---|---|---|
//! | `multicam.autoSwitch` | Clip ▸ Multi-Camera ▸ Auto Switch by Speaker… | each camera's own audio decides which angle the clip shows |
//!
//! `multicam.autoSwitch {clip?, minShotSeconds=2, marginDb=6, silenceDb=-45, wideCamera?,
//! bothDb=3, videoOnly?, action="apply"|"detect"}` works on the multi-camera clip at the playhead
//! (or `clip`) and the pieces of the same clip right next to it on its track (an earlier switch
//! leaves several). For every shown camera that has audio tracks of its own in the multi-camera
//! source, the level of that audio alone is measured in 50 ms windows (the mixer graph, so clip
//! and track volume count) and smoothed over 0.3 s. The loudest camera is shown, with rules that
//! keep the cut calm:
//!
//! - a new camera must be `marginDb` louder than the one showing (when that one is talking);
//! - it must stay the loudest for 0.4 s before the cut, and the cut lands where it took over;
//! - no shot is shorter than `minShotSeconds`;
//! - with `wideCamera` (1-based, shown order; 0 = none), two people within `bothDb` of each other, or
//!   nobody above `silenceDb`, show that camera; without it the current shot holds. The wide
//!   camera's own audio does not vote (its microphone hears everybody).
//!
//! Cuts land on frames of the sequence. Applying is one undo step ("Auto Switch by Speaker") and
//! switches the linked audio too when audio follows video (and `videoOnly` is not set), like the
//! keyboard cuts. No transcript and no speech model are needed.
//!
//! The analysis (mixing each camera's audio) runs as a background job so the UI keeps going:
//! `jobs.list` shows "Camera 2 of 3: 40%", `jobs.cancel` stops it without changing anything, and
//! when it finishes the cuts are applied from [`poll`]. If the clip was edited meanwhile (moved,
//! trimmed, deleted), nothing is applied and a message says why. `wait: true` runs it in place and
//! answers with the cuts (scripts, tests); `action: "detect"` does too and changes nothing.

use filmcraft_edit as edit;
use filmcraft_edit::multicam::Cut;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use filmcraft_project::{ItemId, Project, Sequence, TrackId, TrackItem};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, clip_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "multicam.autoSwitch";
/// Level windows per second (50 ms each).
const WINDOWS_PER_SECOND: i64 = 20;
/// Windows of the power moving average (0.3 s).
const SMOOTH_WINDOWS: usize = 6;
/// Windows a new speaker must stay the loudest before the cut (0.4 s).
const HOLD_WINDOWS: usize = 8;
/// Longest stretch analysed (6 hours).
const MAX_SECONDS: i64 = 6 * 3600;
/// Level of digital silence in dB (power floor).
const FLOOR_DB: f32 = -120.0;

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Auto Switch by Speaker…",
        menu: &["Clip", "Multi-Camera"],
        shortcut: None,
        params: r#"{"clip":id?,"time":ticks?,"minShotSeconds":0..600=2,"marginDb":0..40=6,"silenceDb":-96..0=-45,"wideCamera":0..16 (0 = none)?,"bothDb":0..20=3,"videoOnly":bool?,"action":"apply|detect"="apply","wait":bool=false}"#,
        enabled: has_multicam_clip,
        run: auto_switch,
        journal: true,
    }]
}

fn has_multicam_clip(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    let q = s.active_sequence().ok_or("no sequence is open")?;
    if q.video_tracks.iter().flat_map(|t| t.items.iter()).any(|i| i.multicam.is_some()) { Ok(()) } else { Err("the sequence has no multi-camera clip".into()) }
}

/// How calm the cutting is, in level windows and dB (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchRules {
    /// Shortest shot, in windows.
    pub min_shot: usize,
    /// Windows a new speaker must stay the loudest before the cut.
    pub hold: usize,
    /// How much louder than the camera showing a new one must be.
    pub margin_db: f32,
    /// Below this nobody is talking.
    pub silence_db: f32,
    /// Two cameras this close count as both talking (wide shot, when there is one).
    pub both_db: f32,
}

/// The angle a window asks for: the loudest camera, the wide shot or the current one.
fn wanted(levels: &[Vec<f32>], angles: &[u32], w: usize, current: u32, wide: Option<u32>, r: &SwitchRules) -> u32 {
    let level = |k: usize| levels.get(k).and_then(|l| l.get(w)).copied().filter(|v| v.is_finite()).unwrap_or(FLOOR_DB);
    let mut best: Option<(usize, f32)> = None;
    let mut second = FLOOR_DB;
    for k in 0..angles.len().min(levels.len()) {
        let v = level(k);
        match best {
            Some((_, b)) if v <= b => second = second.max(v),
            Some((_, b)) => {
                second = second.max(b);
                best = Some((k, v));
            }
            None => best = Some((k, v)),
        }
    }
    let Some((top, top_db)) = best.filter(|(_, v)| *v >= r.silence_db) else { return wide.unwrap_or(current) };
    if let Some(wd) = wide
        && second >= r.silence_db
        && top_db - second < r.both_db
    {
        return wd;
    }
    // the camera showing keeps the shot while it talks, unless the new one is clearly louder
    if let Some(cur) = angles.iter().position(|a| *a == current) {
        let cur_db = level(cur);
        if cur_db >= r.silence_db && top_db - cur_db < r.margin_db {
            return current;
        }
    }
    angles.get(top).copied().unwrap_or(current)
}

/// The cuts for per-camera levels: `levels[k][w]` is the level (dB) of camera `angles[k]` in
/// window `w`. Returns `(window, angle)` in time order, the first at window 0 (the opening shot,
/// `start` when nothing asks for another one). Empty when there are no windows or cameras.
pub fn speaker_cuts(levels: &[Vec<f32>], angles: &[u32], wide: Option<u32>, start: u32, r: &SwitchRules) -> Vec<(usize, u32)> {
    let n = levels.iter().map(Vec::len).min().unwrap_or(0);
    if n == 0 || angles.is_empty() {
        return Vec::new();
    }
    let mut current = wanted(levels, angles, 0, start, wide, r);
    let mut cuts = vec![(0, current)];
    let (mut last, mut cand, mut since) = (0usize, current, 0usize);
    for w in 1..n {
        let d = wanted(levels, angles, w, current, wide, r);
        if d != cand {
            cand = d;
            since = w;
        }
        if cand != current && w + 1 - since >= r.hold.max(1) {
            let at = since.max(last.saturating_add(r.min_shot));
            if at <= w {
                cuts.push((at, cand));
                current = cand;
                last = at;
            }
        }
    }
    cuts
}

/// Window cuts as cuts on frames of `rate` from `start`, in order, before `end`. Switches that
/// snap onto the frame of an earlier one replace it (the later angle is what shows from there, as
/// in a recording pass); a cut left showing the angle already showing is dropped.
pub fn frame_cuts(window_cuts: &[(usize, u32)], start: Tick, end: Tick, rate: FrameRate) -> Vec<Cut> {
    let mut cuts: Vec<Cut> = Vec::new();
    for &(w, angle) in window_cuts {
        let at = if w == 0 { start } else { rate.snap(start + Tick::from_units(i64::try_from(w).unwrap_or(i64::MAX), WINDOWS_PER_SECOND)) };
        if at >= end {
            continue;
        }
        match cuts.last_mut() {
            Some(last) if at <= last.time => last.angle = angle,
            _ => cuts.push(Cut { time: at.max(start), angle }),
        }
        // a replaced cut may now repeat the angle before it
        let n = cuts.len();
        if n >= 2 && cuts.get(n - 2).map(|c| c.angle) == cuts.get(n - 1).map(|c| c.angle) {
            cuts.pop();
        }
    }
    cuts
}

/// The pieces of one multi-camera clip around `clip` on `track`: neighbours that touch and show
/// the same source. Sorted by time.
fn run_of(q: &Sequence, track: TrackId, clip: &TrackItem) -> Vec<TrackItem> {
    let Some(tr) = q.track(track) else { return vec![clip.clone()] };
    let mut items: Vec<&TrackItem> = tr.items.iter().collect();
    items.sort_by_key(|i| i.start);
    let Some(k) = items.iter().position(|i| i.id == clip.id) else { return vec![clip.clone()] };
    let same = |a: &TrackItem, b: &TrackItem| a.end() == b.start && a.item == b.item && a.multicam.is_some() && b.multicam.is_some();
    let mut lo = k;
    while lo > 0 && items.get(lo - 1).zip(items.get(lo)).is_some_and(|(a, b)| same(a, b)) {
        lo -= 1;
    }
    let mut hi = k;
    while items.get(hi).zip(items.get(hi + 1)).is_some_and(|(a, b)| same(a, b)) {
        hi += 1;
    }
    items.get(lo..=hi).map(|v| v.iter().map(|i| (*i).clone()).collect()).unwrap_or_else(|| vec![clip.clone()])
}

/// Smoothed level (dB) per window of a camera's own audio (`audio`, tracks of the source
/// sequence `src`), over source time `from` for `windows` windows. `step(windows_done)` is called
/// after each second of audio; it returns false to stop (None is returned then).
fn camera_levels(
    project: &Project,
    provider: &dyn filmcraft_render::SourceProvider,
    src: &Sequence,
    audio: &[TrackId],
    from: Tick,
    windows: usize,
    mut step: impl FnMut(u64) -> bool,
) -> Option<Vec<f32>> {
    // the source with only this camera's audio left in it
    let mut solo = src.clone();
    for t in &mut solo.video_tracks {
        t.items.clear();
    }
    for t in &mut solo.audio_tracks {
        if audio.contains(&t.id) {
            t.muted = false;
            t.solo = false;
        } else {
            t.items.clear();
        }
    }
    let sr = i64::from(src.settings.sample_rate.max(1));
    let first = from.to_units_floor(sr);
    let total = Tick::from_units(i64::try_from(windows).unwrap_or(0), WINDOWS_PER_SECOND).to_units_floor(sr);
    let mut power = vec![0f64; windows];
    let mut count = vec![0u32; windows];
    let mut pos = 0i64;
    while pos < total {
        let n = (total - pos).min(sr);
        let b = filmcraft_render::audio::mix_sequence(project, &solo, first.saturating_add(pos), usize::try_from(n).unwrap_or(0), provider);
        let chans = b.channels.len().max(1) as f64;
        for k in 0..usize::try_from(n).unwrap_or(0) {
            let sq: f64 = b.channels.iter().map(|c| c.get(k).map_or(0.0, |v| f64::from(*v) * f64::from(*v))).sum();
            let w = (pos + k as i64).saturating_mul(WINDOWS_PER_SECOND) / sr;
            if let Some(i) = usize::try_from(w).ok()
                && let (Some(p), Some(c)) = (power.get_mut(i), count.get_mut(i))
            {
                *p += sq / chans;
                *c += 1;
            }
        }
        pos += n;
        let done = u64::try_from(pos.saturating_mul(WINDOWS_PER_SECOND) / sr).unwrap_or(0).min(windows as u64);
        if !step(done) {
            return None;
        }
    }
    let mean: Vec<f64> = power.iter().zip(&count).map(|(p, c)| if *c == 0 { 0.0 } else { p / f64::from(*c) }).collect();
    // centred moving average of the power
    let half = SMOOTH_WINDOWS / 2;
    Some(
        (0..windows)
            .map(|i| {
                let (a, b) = (i.saturating_sub(half), (i + half + 1).min(windows));
                let slice = mean.get(a..b).unwrap_or(&[]);
                let p = slice.iter().sum::<f64>() / slice.len().max(1) as f64;
                ((10.0 * (p + 1e-12).log10()) as f32).max(FLOOR_DB)
            })
            .collect(),
    )
}

/// Everything the analysis needs, owned, so it can run on a worker thread.
struct Analysis {
    project: Arc<Project>,
    provider: crate::media_pool::PoolProvider,
    src: Sequence,
    /// (angle, its audio tracks) of the cameras that vote.
    voiced: Vec<(u32, Vec<TrackId>)>,
    from: Tick,
    windows: usize,
    rules: SwitchRules,
    wide: Option<u32>,
    opening: u32,
    start: Tick,
    end: Tick,
    rate: FrameRate,
}

impl Analysis {
    /// The cuts, or None when `progress` asks to stop. Reports progress in windows (all cameras).
    fn run(&self, progress: &filmcraft_export::Progress) -> Option<Vec<Cut>> {
        let n = self.voiced.len();
        let mut levels = Vec::with_capacity(n);
        for (k, (_, audio)) in self.voiced.iter().enumerate() {
            let base = (k * self.windows) as u64;
            let l = camera_levels(&self.project, &self.provider, &self.src, audio, self.from, self.windows, |done| {
                progress.done.store(base + done, Ordering::Relaxed);
                let pct = done * 100 / (self.windows as u64).max(1);
                *lock(&progress.status) = format!("Camera {} of {n}: {pct}%", k + 1);
                !progress.cancel.load(Ordering::Relaxed)
            })?;
            levels.push(l);
        }
        let angles: Vec<u32> = self.voiced.iter().map(|(a, _)| *a).collect();
        Some(frame_cuts(&speaker_cuts(&levels, &angles, self.wide, self.opening, &self.rules), self.start, self.end, self.rate))
    }
}

/// Where the cuts go, and what the clip looked like when the analysis started.
#[derive(Clone)]
struct Target {
    seq: ItemId,
    track: TrackId,
    /// Tracks the cuts switch (the video track, and the linked audio when it follows).
    tracks: Vec<TrackId>,
    source: ItemId,
    start: Tick,
    end: Tick,
    source_in: Tick,
}

/// A running analysis; applied by [`poll`] when it finishes.
pub struct PendingSwitch {
    job: u64,
    target: Target,
    cuts: Arc<Mutex<Option<Vec<Cut>>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Apply finished analyses (one undo step each) and drop finished / cancelled ones. Called once
/// per UI frame from [`Session::poll_persistence`].
pub fn poll(s: &mut Session) {
    let mut i = 0;
    while i < s.autoswitch_jobs.len() {
        let Some(pj) = s.autoswitch_jobs.get(i) else { break };
        let job = s.jobs.iter().find(|j| j.id == pj.job);
        let finished = job.is_none_or(|j| j.progress.finished.load(Ordering::Relaxed));
        // cancelled before the cuts were applied: nothing changes
        let cancelled = job.is_some_and(|j| j.progress.cancel.load(Ordering::Relaxed));
        if !finished {
            i += 1;
            continue;
        }
        let pj = s.autoswitch_jobs.remove(i);
        let Some(cuts) = lock(&pj.cuts).take().filter(|_| !cancelled) else { continue };
        match apply(s, &pj.target, &cuts) {
            Ok(_) => {
                let n = cuts.len().saturating_sub(1);
                s.toast(format!("Auto Switch by Speaker: {n} switch{}", if n == 1 { "" } else { "es" }));
            }
            Err(e) => s.error_toast(CMD, format!("Auto Switch by Speaker: {e}")),
        }
    }
}

/// The run of the multi-camera clip on the target track now: (start, end, source In of its first
/// piece), or None when it is gone.
fn run_now(s: &Session, t: &Target) -> Option<(Tick, Tick, Tick)> {
    let q = s.project.sequence(t.seq)?;
    let tr = q.track(t.track)?;
    let clip = tr.items.iter().find(|i| i.item == t.source && i.multicam.is_some() && i.start <= t.start && t.start < i.end())?;
    let run = run_of(q, t.track, clip);
    Some((run.first()?.start, run.last()?.end(), run.first()?.source_in))
}

/// Switch the clip by `cuts` in one undo step, unless it changed since the analysis started.
fn apply(s: &mut Session, t: &Target, cuts: &[Cut]) -> Result<usize> {
    if run_now(s, t) != Some((t.start, t.end, t.source_in)) {
        return Err(EngineError::Other("the multi-camera clip changed during the analysis; nothing was switched".into()));
    }
    if s.project.sequence(t.seq).and_then(|q| q.track(t.track)).is_some_and(|tr| tr.locked) {
        return Err(bad(CMD, "the multi-camera clip's track is locked"));
    }
    let (seq, tracks, source, end) = (t.seq, t.tracks.clone(), t.source, t.end);
    let media = s.media.clone();
    s.edit("Auto Switch by Speaker", |pr, _| {
        crate::multicam::seq_edit(pr, seq, &media, |q, ctx| Ok(edit::multicam::record(q, &tracks, source, cuts, end, ctx)))
    })
}

fn seconds(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

/// A finite parameter in `range`, `default` when absent or not a number.
fn num(p: &Value, k: &str, default: f64, range: std::ops::RangeInclusive<f64>) -> f64 {
    f64_p(p, k).filter(|v| v.is_finite()).unwrap_or(default).clamp(*range.start(), *range.end())
}

fn auto_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let detect = match str_p(p, "action").unwrap_or("apply") {
        "apply" => false,
        "detect" => true,
        a => return Err(bad(CMD, format!("unknown action `{a}` (apply or detect)"))),
    };
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    // the multi-camera clip: `clip`, or the one at the playhead (`time`)
    let (track, clip) = match clip_p(p, "clip") {
        Some(id) => q
            .video_tracks
            .iter()
            .find_map(|t| t.items.iter().find(|i| i.id == id).map(|i| (t.id, i.clone())))
            .filter(|(_, i)| i.multicam.is_some())
            .ok_or_else(|| bad(CMD, "`clip` is not a multi-camera clip on a video track"))?,
        None => {
            let t = time_p(s, p, "").unwrap_or(s.playhead());
            crate::multicam::multicam_clip_at(s, t).ok_or_else(|| bad(CMD, "no multi-camera clip at the playhead"))?
        }
    };
    // a locked track would be skipped by the edit: refuse instead of an undo step that does nothing
    if q.track(track).is_some_and(|t| t.locked) {
        return Err(bad(CMD, "the multi-camera clip's track is locked"));
    }
    let run = run_of(q, track, &clip);
    let (Some(first), Some(last)) = (run.first(), run.last()) else { return Err(bad(CMD, "no multi-camera clip")) };
    // the analysis maps timeline time to source time linearly
    let linear = run.iter().all(|i| i.speed == 1.0 && !i.reverse && i.frame_hold.is_none())
        && run.windows(2).all(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.source_out() == b.source_in));
    if !linear {
        return Err(bad(CMD, "the multi-camera clip has a speed change, reverse, frame hold or a jump in its source; switch it by hand"));
    }
    let (start, end, source) = (first.start, last.end(), first.item);
    let length = end - start;
    if length <= Tick::ZERO {
        return Err(bad(CMD, "the multi-camera clip is empty"));
    }
    if length > Tick::from_units(MAX_SECONDS, 1) {
        return Err(bad(CMD, format!("the multi-camera clip is longer than {} hours", MAX_SECONDS / 3600)));
    }
    let src = s.project.sequence(source).ok_or_else(|| bad(CMD, "the multi-camera source sequence is missing"))?;
    let cams = src.cameras();
    let shown = cams.shown_angles();
    let wide = match u64_p(p, "wideCamera") {
        // 0: no wide shot (the dialog's "none")
        None | Some(0) => None,
        Some(n) => {
            let a = usize::try_from(n).ok().and_then(|n| n.checked_sub(1)).and_then(|i| shown.get(i)).copied();
            Some(a.ok_or_else(|| bad(CMD, format!("no camera {n} (cameras 1–{} are shown)", shown.len())))? as u32)
        }
    };
    // cameras with audio of their own take part, except the wide shot (its microphone hears
    // everybody); cameras without audio can still be the wide shot
    let voiced: Vec<(u32, Vec<TrackId>)> = shown
        .iter()
        .filter(|a| wide != Some(**a as u32))
        .filter_map(|a| cams.cameras.get(*a).filter(|c| !c.audio_tracks.is_empty()).map(|c| (*a as u32, c.audio_tracks.clone())))
        .collect();
    if voiced.len() < 2 {
        return Err(bad(CMD, "needs at least two shown cameras (besides the wide shot) with audio of their own"));
    }
    let windows = usize::try_from(length.to_units_floor(WINDOWS_PER_SECOND)).unwrap_or(0).max(1);
    let rules = SwitchRules {
        min_shot: (num(p, "minShotSeconds", 2.0, 0.0..=600.0) * WINDOWS_PER_SECOND as f64).round() as usize,
        hold: HOLD_WINDOWS,
        margin_db: num(p, "marginDb", 6.0, 0.0..=40.0) as f32,
        silence_db: num(p, "silenceDb", -45.0, -96.0..=0.0) as f32,
        both_db: num(p, "bothDb", 3.0, 0.0..=20.0) as f32,
    };
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    // the clip's video track, and the linked audio when audio follows video
    let mut tracks = vec![track];
    if s.state.multicam_audio_follows_video && !bool_p(p, "videoOnly").unwrap_or(false) {
        let partners: Vec<_> = run.iter().flat_map(|i| crate::multicam::audio_partners(q, i)).collect();
        tracks.extend(q.audio_tracks.iter().filter(|t| t.items.iter().any(|i| partners.contains(&i.id))).map(|t| t.id));
    }
    let target = Target { seq: seq_id, track, tracks, source, start, end, source_in: first.source_in };
    let analysis = Analysis {
        project: s.project.clone(),
        provider: s.media.full_res_provider(s.project.clone(), s.services.clone()),
        src: src.clone(),
        voiced,
        from: first.source_in,
        windows,
        rules,
        wide,
        opening: clip.multicam.map(|m| m.angle).unwrap_or(0),
        start,
        end,
        rate,
    };
    let cameras = analysis.voiced.len();
    let listed = |cuts: &[Cut]| -> Vec<Value> {
        let camera_of = |a: u32| shown.iter().position(|x| *x as u32 == a).map(|i| i + 1);
        cuts.iter().map(|c| json!({"time": c.time.0, "seconds": seconds(c.time), "angle": c.angle, "camera": camera_of(c.angle)})).collect()
    };
    // detect and `wait` answer with the cuts; otherwise the analysis runs as a background job
    if detect || bool_p(p, "wait").unwrap_or(false) || cfg!(target_arch = "wasm32") {
        let cuts = analysis.run(&filmcraft_export::Progress::default()).unwrap_or_default();
        let switches = cuts.len().saturating_sub(1);
        if detect {
            return Ok(json!({"applied": false, "cuts": listed(&cuts), "switches": switches}));
        }
        let points = apply(s, &target, &cuts)?;
        return Ok(json!({"applied": true, "cuts": listed(&cuts), "switches": switches, "editPoints": points}));
    }
    if s.autoswitch_jobs.iter().any(|j| j.target.seq == seq_id) {
        return Err(EngineError::Other("Auto Switch by Speaker is already running on this sequence".into()));
    }
    let id = s.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let job = crate::Job { id, label: "Auto Switch by Speaker".into(), progress: Default::default(), result: Default::default() };
    job.progress.total.store((cameras * windows) as u64, Ordering::Relaxed);
    let cuts: Arc<Mutex<Option<Vec<Cut>>>> = Arc::default();
    let (prog, res, out) = (job.progress.clone(), job.result.clone(), cuts.clone());
    let work = move || {
        let t0 = web_time::Instant::now();
        let found = analysis.run(&prog);
        let secs = t0.elapsed().as_secs_f64();
        *lock(&prog.status) = match &found {
            None => "Stopped: nothing was changed".into(),
            Some(c) => format!("Found {} switch(es) ({secs:.1}s)", c.len().saturating_sub(1)),
        };
        let r = match found {
            None => Err("stopped".to_string()),
            Some(c) => {
                *lock(&out) = Some(c);
                let windows = prog.total.load(Ordering::Relaxed);
                Ok(filmcraft_export::Report {
                    path: String::new(),
                    frames: windows,
                    seconds: secs,
                    bytes: 0,
                    render_fps: windows as f64 / secs.max(1e-6),
                    extra_files: Vec::new(),
                })
            }
        };
        *lock(&res) = Some(r);
        prog.finished.store(true, Ordering::Relaxed);
    };
    let work = crate::export_tools::guard_job(job.progress.clone(), job.result.clone(), work);
    s.jobs.push(job);
    s.autoswitch_jobs.push(PendingSwitch { job: id, target, cuts });
    if let Err(e) = std::thread::Builder::new().name("filmcraft-autoswitch".into()).spawn(work) {
        s.autoswitch_jobs.retain(|j| j.job != id);
        s.jobs.retain(|j| j.id != id);
        return Err(EngineError::Other(e.to_string()));
    }
    Ok(json!({"job": id, "cameras": cameras, "seconds": seconds(length)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: SwitchRules = SwitchRules { min_shot: 4, hold: 2, margin_db: 6.0, silence_db: -45.0, both_db: 3.0 };

    /// `segments` of (windows, level per camera).
    fn levels(cams: usize, segments: &[(usize, &[f32])]) -> Vec<Vec<f32>> {
        let mut v = vec![Vec::new(); cams];
        for (n, l) in segments {
            for (k, cam) in v.iter_mut().enumerate() {
                cam.extend(std::iter::repeat_n(l.get(k).copied().unwrap_or(FLOOR_DB), *n));
            }
        }
        v
    }

    #[test]
    fn the_loudest_camera_is_shown_after_the_hold() {
        let l = levels(2, &[(10, &[-20.0, -60.0]), (10, &[-60.0, -20.0])]);
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 0, &R), vec![(0, 0), (10, 1)]);
        // camera numbers are the angles given
        assert_eq!(speaker_cuts(&l, &[3, 5], None, 0, &R), vec![(0, 3), (10, 5)]);
    }

    #[test]
    fn short_shots_wait_for_the_minimum() {
        // B talks from window 2: the first shot lasts min_shot windows
        let l = levels(2, &[(2, &[-20.0, -60.0]), (10, &[-60.0, -20.0])]);
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 0, &R), vec![(0, 0), (4, 1)]);
        // a blip shorter than the hold is ignored
        let l = levels(2, &[(6, &[-20.0, -60.0]), (1, &[-60.0, -20.0]), (6, &[-20.0, -60.0])]);
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 0, &R), vec![(0, 0)]);
    }

    #[test]
    fn the_current_speaker_holds_within_the_margin() {
        // B only 3 dB louder while A still talks: no cut
        let l = levels(2, &[(6, &[-20.0, -60.0]), (10, &[-23.0, -20.0])]);
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 0, &R), vec![(0, 0)]);
        // A stops: B takes over
        let l = levels(2, &[(6, &[-20.0, -60.0]), (10, &[-70.0, -20.0])]);
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 0, &R), vec![(0, 0), (6, 1)]);
    }

    #[test]
    fn silence_and_crosstalk_go_wide_when_there_is_a_wide_shot() {
        let l = levels(2, &[(6, &[-20.0, -60.0]), (8, &[-70.0, -70.0]), (8, &[-21.0, -20.0])]);
        // no wide shot: silence holds the current camera, two voices at once too
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 0, &R), vec![(0, 0)]);
        // wide shot (angle 2, no audio of its own)
        assert_eq!(speaker_cuts(&l, &[0, 1], Some(2), 0, &R), vec![(0, 0), (6, 2)]);
        // everybody silent from the start: open on the wide shot
        let l = levels(2, &[(8, &[-80.0, -80.0])]);
        assert_eq!(speaker_cuts(&l, &[0, 1], Some(2), 1, &R), vec![(0, 2)]);
        assert_eq!(speaker_cuts(&l, &[0, 1], None, 1, &R), vec![(0, 1)]);
    }

    #[test]
    fn cuts_on_one_frame_keep_the_later_angle() {
        // 5 fps: 200 ms frames, 50 ms windows; a switch at window 1 (50 ms) lands on frame 0
        let rate = FrameRate::new(5, 1);
        let end = Tick::from_units(10, 1);
        let c = frame_cuts(&[(0, 0), (1, 1)], Tick::ZERO, end, rate);
        assert_eq!(c, vec![Cut { time: Tick::ZERO, angle: 1 }], "the later switch wins, as in a recording pass");
        // two switches snapped onto frame 1: the last one is shown from there
        let c = frame_cuts(&[(0, 0), (4, 1), (5, 2)], Tick::ZERO, end, rate);
        assert_eq!(c.iter().map(|c| (c.time, c.angle)).collect::<Vec<_>>(), vec![(Tick::ZERO, 0), (rate.tick_of(1), 2)]);
        // nothing at or after the end
        assert_eq!(frame_cuts(&[(0, 0), (400, 1)], Tick::ZERO, end, rate).len(), 1);
    }

    #[test]
    fn hostile_levels_never_panic() {
        assert!(speaker_cuts(&[], &[0, 1], None, 0, &R).is_empty());
        assert!(speaker_cuts(&[vec![-20.0; 4]], &[], None, 0, &R).is_empty());
        let l = vec![vec![f32::NAN, f32::INFINITY, -20.0], vec![f32::NEG_INFINITY, f32::NAN]];
        let c = speaker_cuts(&l, &[0, 1], None, 7, &R);
        assert_eq!(c.first().map(|c| c.0), Some(0));
        let huge = SwitchRules { min_shot: usize::MAX, hold: usize::MAX, ..R };
        assert_eq!(speaker_cuts(&levels(2, &[(5, &[-20.0, -60.0]), (5, &[-60.0, -20.0])]), &[0, 1], None, 0, &huge), vec![(0, 0)]);
    }
}
