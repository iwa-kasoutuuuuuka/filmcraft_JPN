//! Script to sequence (`anime.*`): the assembly, the checks and the one-step run of the novel /
//! game to anime pipeline (`docs/script.md`).
//!
//! | command | what it does |
//! |---|---|
//! | `anime.build` | Lays a script out as a new sequence: backgrounds, the speaker's sprite with mouth and blink pictures, voices, sound effects, music, subtitles. One undo step. |
//! | `anime.check` | Looks for what is wrong with a sequence: gaps, overlapping voices, long silences, subtitles that are too long or too fast, offline media, a voice far louder or quieter than the rest. |
//! | `anime.run` | Validate, pick voices, speak the missing lines, build, check and optionally export, in one call. Every step is repeatable: spoken lines are cached, so a second run only redoes what changed. |
//!
//! The layout is computed first as a [`Plan`] (pure: times in ticks on the frame grid, no
//! session), then placed in a single edit. Time is exact: line lengths are the audio files'
//! lengths, pauses are converted once, every boundary is snapped to a frame so neighbouring clips
//! abut.
//!
//! Tracks: V1 background, V2 the speaking character's sprite, V3 its open-mouth picture while the
//! voice is loud, V4 its blink picture; A1 voices, A2 music, A3 sound effects; one subtitle track.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use filmcraft_audio_dsp::lipsync as lip;
use filmcraft_audio_dsp::vowels;
use filmcraft_edit as edit;
use filmcraft_project::keyframe::{Keyframe, ParamValue};
use filmcraft_project::script::{LineKind, Script, ScriptLine, Severity};
use filmcraft_project::{ItemId, ItemKind, Project, TrackItem, TrackKind};
use filmcraft_time::{FrameRate, Tick, TimeRange};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, str_p, u64_p};
use crate::script_tools::{Loaded, has_recording, line_speech, lip_options, load, mouth_of, out_dir, read_manifest};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

/// Most clips one build places (a hostile script cannot make the sequence unbounded).
const MAX_CLIPS: usize = 400_000;
/// Longest sequence a build makes, seconds (100 hours).
const MAX_SECONDS: f64 = 360_000.0;
/// Longest audio file used, seconds: a file whose header claims more is treated as unusable.
const MAX_AUDIO_SECONDS: f64 = lip::MAX_SECONDS;
/// Most repeats of music over a stretch (the number of clips, not a time).
const MAX_MUSIC_CLIPS: usize = 20_000;

// ---------------------------------------------------------------------------------------------
// Options

#[derive(Clone, Debug)]
pub struct Opts {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Silence after a spoken line, seconds (a line's own `pauseAfter` wins).
    pub pause: f64,
    /// A beat of background at the start of each scene, seconds.
    pub scene_gap: f64,
    /// A direction with no sound effect lasts this long, seconds.
    pub direction: f64,
    /// Speaking rate used for a line that has no audio yet, characters per second.
    pub cps: f64,
    /// The background of each scene pushes in by this many percent over the scene.
    pub push: f64,
    /// Scale of sprites in percent (None: as imported).
    pub sprite_scale: Option<f64>,
    pub bgm_gain_db: f64,
    pub sfx_gain_db: f64,
    /// Longest subtitle, characters.
    pub caption_chars: usize,
    pub blink: bool,
    /// Backgrounds are scaled to fill the frame (a 1280x720 picture in a 1080p sequence).
    pub fit_background: bool,
    /// Voices are brought to the same loudness (within +-12 dB).
    pub level_voices: bool,
    /// The loudness voices are brought to, LUFS (None: the median voice).
    pub voice_lufs: Option<f64>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 24.0,
            pause: 0.35,
            scene_gap: 0.6,
            direction: 1.0,
            cps: 7.0,
            push: 5.0,
            sprite_scale: None,
            bgm_gain_db: -18.0,
            sfx_gain_db: 0.0,
            caption_chars: 36,
            blink: true,
            fit_background: true,
            level_voices: true,
            voice_lufs: None,
        }
    }
}

fn num(p: &Value, k: &str, lo: f64, hi: f64, cmd: &str) -> Result<Option<f64>> {
    match f64_p(p, k) {
        None => Ok(None),
        Some(v) if v.is_finite() && (lo..=hi).contains(&v) => Ok(Some(v)),
        Some(v) => Err(bad(cmd, format!("`{k}` must be {lo}..{hi}, got {v}"))),
    }
}

impl Opts {
    fn from_params(p: &Value, cmd: &str) -> Result<Opts> {
        let mut o = Opts::default();
        if let Some(v) = u64_p(p, "width") {
            o.width = u32::try_from(v).map_err(|_| bad(cmd, "`width` is too large"))?;
        }
        if let Some(v) = u64_p(p, "height") {
            o.height = u32::try_from(v).map_err(|_| bad(cmd, "`height` is too large"))?;
        }
        o.fps = num(p, "fps", 1.0, 240.0, cmd)?.unwrap_or(o.fps);
        o.pause = num(p, "pause", 0.0, 10.0, cmd)?.unwrap_or(o.pause);
        o.scene_gap = num(p, "sceneGap", 0.0, 30.0, cmd)?.unwrap_or(o.scene_gap);
        o.direction = num(p, "directionSeconds", 0.1, 60.0, cmd)?.unwrap_or(o.direction);
        o.cps = num(p, "charsPerSecond", 1.0, 30.0, cmd)?.unwrap_or(o.cps);
        o.push = num(p, "push", 0.0, 50.0, cmd)?.unwrap_or(o.push);
        o.sprite_scale = num(p, "spriteScale", 1.0, 1000.0, cmd)?.or(o.sprite_scale);
        o.bgm_gain_db = num(p, "bgmGainDb", -60.0, 12.0, cmd)?.unwrap_or(o.bgm_gain_db);
        o.sfx_gain_db = num(p, "sfxGainDb", -60.0, 12.0, cmd)?.unwrap_or(o.sfx_gain_db);
        if let Some(v) = u64_p(p, "captionChars") {
            o.caption_chars = usize::try_from(v).unwrap_or(36).clamp(8, 400);
        }
        if let Some(b) = bool_p(p, "blink") {
            o.blink = b;
        }
        if let Some(b) = bool_p(p, "fitBackground") {
            o.fit_background = b;
        }
        if let Some(b) = bool_p(p, "levelVoices") {
            o.level_voices = b;
        }
        o.voice_lufs = num(p, "voiceLufs", -60.0, 0.0, cmd)?.or(o.voice_lufs);
        if bool_p(p, "kenBurns") == Some(false) {
            o.push = 0.0;
        }
        let st = filmcraft_project::SequenceSettings { width: o.width, height: o.height, frame_rate: FrameRate::from_f64(o.fps), ..Default::default() };
        st.check().map_err(|e| bad(cmd, e))?;
        Ok(o)
    }
}

// ---------------------------------------------------------------------------------------------
// The plan

/// An audio file and its length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Audio {
    pub path: PathBuf,
    pub duration: Tick,
}

/// The pictures of a character in one expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteRef {
    pub closed: PathBuf,
    pub open: Option<PathBuf>,
    /// Open-mouth pictures per vowel (`a`, `i`, `u`, `e`, `o`).
    pub vowels: BTreeMap<char, PathBuf>,
    pub blink: Option<PathBuf>,
}

/// One line on the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub line: String,
    pub scene: usize,
    pub start: Tick,
    pub end: Tick,
    pub voice: Option<Audio>,
    pub sfx: Option<Audio>,
    /// Subtitles (absolute start, end, text).
    pub captions: Vec<(Tick, Tick, String)>,
    pub speaker: Option<String>,
    pub sprite: Option<SpriteRef>,
    /// Stretches with the mouth open (absolute), on the frame grid, apart.
    pub mouth: Vec<(Tick, Tick)>,
    /// The same stretches cut by vowel (absolute, on the frame grid, apart); empty when the
    /// sprite has no vowel pictures. A `None` shape is the generic open mouth.
    pub shapes: Vec<(Tick, Tick, Option<char>)>,
    /// The line has no audio yet; its length is an estimate.
    pub estimated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenePlan {
    pub id: String,
    pub start: Tick,
    pub end: Tick,
    pub background: Option<PathBuf>,
}

/// A character on screen for a stretch (consecutive lines in the same picture are one run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteRun {
    pub speaker: String,
    pub scene: usize,
    pub start: Tick,
    pub end: Tick,
    pub sprite: SpriteRef,
    pub blinks: Vec<(Tick, Tick)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BgmRun {
    pub audio: Audio,
    pub start: Tick,
    pub end: Tick,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub scenes: Vec<ScenePlan>,
    pub cues: Vec<Cue>,
    pub runs: Vec<SpriteRun>,
    pub bgm: Vec<BgmRun>,
    pub end: Tick,
    pub warnings: Vec<String>,
}

/// What the plan needs to know about files. The session answers from disk; tests fake it.
pub trait PlanEnv {
    fn resolve(&self, rel: &str) -> PathBuf;
    /// The audio of a spoken line: its own recording, else the synthesized file.
    fn voice(&mut self, line: &ScriptLine) -> Option<Audio>;
    /// A sound effect or music file.
    fn audio_file(&mut self, path: &Path) -> Option<Audio>;
    fn image(&mut self, path: &Path) -> bool;
    /// Stretches (start, end seconds in the file) where the mouth is open.
    fn mouth(&mut self, audio: &Audio) -> Vec<(f64, f64)>;
}

/// `a + b` that stops at the largest time instead of overflowing.
fn add(a: Tick, b: Tick) -> Tick {
    Tick(a.0.saturating_add(b.0))
}

fn secs(x: f64) -> Tick {
    Tick::from_seconds_f64(if x.is_finite() { x.max(0.0) } else { 0.0 })
}

/// A small deterministic random sequence (blinks must not change between builds).
fn jitter(seed: &mut u64) -> f64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 33) as f64) / (1u64 << 31) as f64
}

fn split_keep(s: &str, seps: &[char]) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        cur.push(c);
        if seps.contains(&c) {
            v.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        v.push(cur);
    }
    v
}

fn hard_split(s: &str, max: usize) -> Vec<String> {
    s.chars().collect::<Vec<_>>().chunks(max.max(1)).map(|c| c.iter().collect()).collect()
}

/// Break a subtitle into pieces of at most `max` characters: at sentence ends, else at commas,
/// else anywhere; short sentences share a piece.
pub fn split_caption(text: &str, max: usize) -> Vec<String> {
    let max = max.clamp(8, 400);
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= max {
        return vec![text.to_string()];
    }
    let mut units: Vec<String> = Vec::new();
    for sentence in split_keep(text, &['。', '！', '？', '!', '?', '\n']) {
        if sentence.chars().count() <= max {
            units.push(sentence);
            continue;
        }
        for phrase in split_keep(&sentence, &['、', '，', ',', ' ']) {
            if phrase.chars().count() <= max { units.push(phrase) } else { units.extend(hard_split(&phrase, max)) }
        }
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for u in units {
        if cur.chars().count() + u.chars().count() <= max {
            cur.push_str(&u);
        } else {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur = u;
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The pieces back into one subtitle (a space only between two words of Latin letters).
fn join_pieces(pieces: &[String]) -> String {
    let mut out = String::new();
    for p in pieces {
        if out.chars().last().is_some_and(|c| c.is_ascii_alphanumeric()) && p.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) {
            out.push(' ');
        }
        out.push_str(p);
    }
    out
}

/// Spread `pieces` over `[start, end)` in proportion to their length, on the frame grid. Falls back
/// to one subtitle when the time is too short for every piece to last a frame.
pub(crate) fn spread(pieces: &[String], start: Tick, end: Tick, rate: FrameRate) -> Vec<(Tick, Tick, String)> {
    let one = rate.frame_duration();
    let total: i64 = pieces.iter().map(|p| p.chars().count() as i64).sum::<i64>().max(1);
    let span = (end - start).0.max(0);
    if pieces.len() > 1 && span < one.0.saturating_mul(pieces.len() as i64) {
        return vec![(start, end.max(start + one), join_pieces(pieces))];
    }
    let mut out = Vec::new();
    let mut at = start;
    let mut done: i64 = 0;
    for (i, p) in pieces.iter().enumerate() {
        done += p.chars().count() as i64;
        let to = if i + 1 == pieces.len() { end } else { rate.snap_nearest(start + Tick((span as i128 * done as i128 / total as i128) as i64)) };
        let remaining = (pieces.len() - 1 - i) as i64;
        let to = to.max(at + one).min(Tick(end.0.saturating_sub(one.0.saturating_mul(remaining))));
        let to = to.max(at + one);
        out.push((at, to, p.clone()));
        at = to;
    }
    out
}

/// Lay the script out in time. Pure: the environment answers questions about files, `rate` is the
/// sequence's frame rate.
pub fn plan(script: &Script, rate: FrameRate, o: &Opts, env: &mut dyn PlanEnv) -> Plan {
    let one = rate.frame_duration();
    let snap = |t: Tick| rate.snap_nearest(t);
    let mut pl = Plan::default();
    let mut t = Tick::ZERO;
    let mut clips = 0usize;
    for (si, scene) in script.scenes_in_route().into_iter().enumerate() {
        let start = t;
        t = snap(add(t, secs(o.scene_gap)));
        let background = scene.background.as_deref().map(|b| env.resolve(b)).and_then(|p| {
            if env.image(&p) {
                Some(p)
            } else {
                pl.warnings.push(format!("scene {}: background {} is missing", scene.id, p.display()));
                None
            }
        });
        let bgm = scene.bgm.as_deref().map(|b| env.resolve(b)).and_then(|p| {
            let a = env.audio_file(&p);
            if a.is_none() {
                pl.warnings.push(format!("scene {}: music {} is missing, has no audio or is longer than an hour", scene.id, p.display()));
            }
            a
        });
        for line in &scene.lines {
            clips += 1;
            if t.seconds() > MAX_SECONDS {
                pl.warnings.push(format!("the script is too long: stopped at {} hours", (MAX_SECONDS / 3600.0) as u64));
                clips = MAX_CLIPS + 1;
                break;
            }
            if clips > MAX_CLIPS {
                pl.warnings.push(format!("the script is too long: stopped after {MAX_CLIPS} lines"));
                break;
            }
            let cue_start = t;
            let spoken = line.is_spoken();
            let voice = if spoken { env.voice(line) } else { None };
            let sfx = line.sfx.as_deref().and_then(|f| {
                let p = env.resolve(f);
                let a = env.audio_file(&p);
                if a.is_none() {
                    pl.warnings.push(format!("line {}: sound effect {} is missing, has no audio or is longer than an hour", line.id, p.display()));
                }
                a
            });
            let mut estimated = false;
            let speech = if spoken {
                match &voice {
                    Some(a) => a.duration,
                    None => {
                        estimated = true;
                        pl.warnings.push(format!("line {}: no audio yet (its length is estimated)", line.id));
                        let chars = crate::script_tools::line_speech(line).chars().count() as f64;
                        secs(chars / o.cps).max(secs(0.8))
                    }
                }
            } else {
                sfx.as_ref().map(|a| a.duration).unwrap_or_else(|| secs(o.direction))
            };
            let pause = match line.pause_after {
                Some(p) => secs(p),
                None if spoken => secs(o.pause),
                None => Tick::ZERO,
            };
            let end = snap(add(add(cue_start, speech), pause)).max(add(cue_start, one));
            // the sprite
            let sprite = if line.kind == LineKind::Direction {
                None
            } else {
                script.expression_of(line).and_then(|(name, sp)| {
                    let closed = env.resolve(&sp.image);
                    if !env.image(&closed) {
                        pl.warnings.push(format!("line {}: picture {} is missing", line.id, closed.display()));
                        return None;
                    }
                    let open = sp.open_image.as_deref().map(|f| env.resolve(f)).filter(|p| {
                        let ok = env.image(p);
                        if !ok {
                            pl.warnings.push(format!("line {}: open-mouth picture {} is missing", line.id, p.display()));
                        }
                        ok
                    });
                    let blink = line
                        .speaker
                        .as_deref()
                        .and_then(|s| script.characters.get(s))
                        .and_then(|c| c.expressions.get(&format!("{name}_blink")).or_else(|| c.expressions.get("blink")))
                        .map(|b| env.resolve(&b.image))
                        .filter(|p| {
                            let ok = env.image(p);
                            if !ok {
                                pl.warnings.push(format!("line {}: blink picture {} is missing", line.id, p.display()));
                            }
                            ok
                        });
                    let mut vowels = BTreeMap::new();
                    for (k, f) in &sp.vowel_images {
                        let mut it = k.trim().chars();
                        let (Some(v), None) = (it.next().map(|c| c.to_ascii_lowercase()), it.next()) else {
                            pl.warnings.push(format!("line {}: vowel \"{k}\" is not one of a, i, u, e, o", line.id));
                            continue;
                        };
                        let path = env.resolve(f);
                        if !"aiueo".contains(v) {
                            pl.warnings.push(format!("line {}: vowel \"{k}\" is not one of a, i, u, e, o", line.id));
                        } else if env.image(&path) {
                            vowels.insert(v, path);
                        } else {
                            pl.warnings.push(format!("line {}: mouth picture {} is missing", line.id, path.display()));
                        }
                    }
                    Some(SpriteRef { closed, open, vowels, blink })
                })
            };
            // the mouth
            let mut mouth = Vec::new();
            let mut shapes = Vec::new();
            if let (Some(v), Some(sp)) = (&voice, &sprite)
                && (sp.open.is_some() || !sp.vowels.is_empty())
            {
                let limit = snap(cue_start + v.duration).min(end);
                let open_spans = env.mouth(v);
                let mut prev = cue_start;
                for (a, b) in &open_spans {
                    let (a, b) = (snap(cue_start + secs(*a)).max(prev), snap(cue_start + secs(*b)).min(limit));
                    if b - a >= one {
                        mouth.push((a, b));
                        prev = b;
                    }
                }
                if !sp.vowels.is_empty() {
                    // the reading says which vowel, the open stretches say when
                    let beats = vowels::moras(&line_speech(line));
                    prev = cue_start;
                    for seg in vowels::fit(&open_spans, &beats) {
                        let (a, b) = (snap(cue_start + secs(seg.start)).max(prev), snap(cue_start + secs(seg.end)).min(limit));
                        if b - a >= one {
                            shapes.push((a, b, seg.vowel));
                            prev = b;
                        }
                    }
                }
            }
            // the subtitles
            let mut captions = Vec::new();
            if spoken {
                let plain = filmcraft_text::ruby::parse(&line.text).plain;
                let pieces = split_caption(&plain, o.caption_chars);
                let tail = secs(0.2).min(pause);
                captions = spread(&pieces, cue_start, snap(cue_start + speech + tail).min(end).max(cue_start + one), rate);
            }
            pl.cues.push(Cue {
                line: line.id.clone(),
                scene: si,
                start: cue_start,
                end,
                voice,
                sfx,
                captions,
                speaker: line.speaker.clone(),
                sprite,
                mouth,
                shapes,
                estimated,
            });
            t = end;
        }
        // a scene lasts at least a frame, or its background would sit on the next one's
        if t < add(start, one) {
            t = add(start, one);
        }
        if let Some(a) = bgm {
            match pl.bgm.last_mut() {
                Some(last) if last.audio.path == a.path && last.end == start => last.end = t,
                _ => pl.bgm.push(BgmRun { audio: a, start, end: t }),
            }
        }
        pl.scenes.push(ScenePlan { id: scene.id.clone(), start, end: t, background });
        if clips > MAX_CLIPS {
            break;
        }
    }
    pl.end = t;
    // characters on screen: consecutive lines in the same picture are one clip
    for c in &pl.cues {
        let (Some(sp), Some(who)) = (&c.sprite, &c.speaker) else { continue };
        match pl.runs.last_mut() {
            Some(r) if &r.speaker == who && r.scene == c.scene && r.sprite.closed == sp.closed && r.end == c.start => r.end = c.end,
            _ => pl.runs.push(SpriteRun { speaker: who.clone(), scene: c.scene, start: c.start, end: c.end, sprite: sp.clone(), blinks: Vec::new() }),
        }
    }
    if o.blink {
        let blink_len = secs(0.12).max(Tick(one.0.saturating_mul(3)));
        for r in &mut pl.runs {
            let Some(_) = r.sprite.blink else { continue };
            let mut seed = (r.start.0 as u64) ^ 0x9e37_79b9_7f4a_7c15;
            let mut at = r.start + secs(1.2 + jitter(&mut seed));
            while at + blink_len + secs(0.2) < r.end {
                let a = snap(at);
                r.blinks.push((a, a + blink_len));
                at = a + blink_len + secs(2.6 + 1.8 * jitter(&mut seed));
            }
        }
    }
    pl
}

// ---------------------------------------------------------------------------------------------
// The session's environment

struct SessionEnv<'a> {
    s: &'a Session,
    l: &'a Loaded,
    out: PathBuf,
    manifest: crate::script_tools::Manifest,
    lip: lip::Options,
    lengths: HashMap<PathBuf, Option<Tick>>,
    mouths: HashMap<PathBuf, Vec<(f64, f64)>>,
}

impl PlanEnv for SessionEnv<'_> {
    fn resolve(&self, rel: &str) -> PathBuf {
        self.l.resolve(rel)
    }
    fn voice(&mut self, line: &ScriptLine) -> Option<Audio> {
        let path = if has_recording(self.l, line) {
            line.voice_file.as_deref().map(|f| self.l.resolve(f))
        } else {
            self.manifest.lines.get(&line.id).map(|e| self.out.join(&e.file)).filter(|p| p.is_file())
        }?;
        self.audio_file(&path)
    }
    fn audio_file(&mut self, path: &Path) -> Option<Audio> {
        let s = self.s;
        let d = *self.lengths.entry(path.to_path_buf()).or_insert_with(|| {
            let src = s.media.open_file(&path.to_string_lossy(), &*s.services).ok()?;
            (src.info().has_audio() && src.info().duration > Tick::ZERO && src.info().duration.seconds() <= MAX_AUDIO_SECONDS).then_some(src.info().duration)
        });
        d.map(|duration| Audio { path: path.to_path_buf(), duration })
    }
    fn image(&mut self, path: &Path) -> bool {
        path.is_file()
    }
    fn mouth(&mut self, audio: &Audio) -> Vec<(f64, f64)> {
        if let Some(m) = self.mouths.get(&audio.path) {
            return m.clone();
        }
        let m: Vec<(f64, f64)> = mouth_of(self.s, &audio.path, &self.lip)
            .map(|(_, segs)| segs.iter().filter(|g| g.level >= 1).map(|g| (g.start, g.end)).collect())
            .unwrap_or_default();
        self.mouths.insert(audio.path.clone(), m.clone());
        m
    }
}

fn session_env<'a>(s: &'a Session, l: &'a Loaded, out: PathBuf) -> SessionEnv<'a> {
    let manifest = read_manifest(&out);
    SessionEnv { s, l, out, manifest, lip: lip::Options::default(), lengths: HashMap::new(), mouths: HashMap::new() }
}

// ---------------------------------------------------------------------------------------------
// anime.build

/// A track item for `item` placed at `start` for `dur` from the start of its media.
fn make_clip(p: &mut Project, item: ItemId, kind: TrackKind, start: Tick, dur: Tick, rate: FrameRate, frame: (u32, u32), scaling: &str) -> Option<TrackItem> {
    let src_size = match &p.item(item)?.kind {
        ItemKind::Media(m) => m.info.video.as_ref().map(|v| (v.width, v.height)),
        _ => None,
    }
    .unwrap_or((1920, 1080));
    let mut ti = p.make_track_item(item, kind, start, TimeRange::new(Tick::ZERO, dur), rate)?;
    if kind == TrackKind::Video {
        for e in &mut ti.effects {
            filmcraft_project::resolve_auto_points(e, frame, src_size);
        }
        crate::settings::apply_media_scaling(&mut ti, scaling, frame, src_size);
    }
    Some(ti)
}

fn motion_scale(ti: &mut TrackItem) -> Option<&mut filmcraft_project::Param> {
    ti.effects.iter_mut().find(|e| e.effect == "motion").and_then(|e| e.params.get_mut("scale"))
}

fn set_scale(ti: &mut TrackItem, percent: f64) {
    if let Some(p) = motion_scale(ti) {
        p.value = ParamValue::Float(percent);
    }
}

/// The background pushes in slowly: Scale from its value to `push` percent more.
fn push_in(ti: &mut TrackItem, push: f64, one: Tick) {
    let dur = ti.duration;
    if push <= 0.0 || dur <= Tick(one.0.saturating_mul(2)) {
        return;
    }
    let Some(p) = motion_scale(ti) else { return };
    let base = p.value.as_f64().unwrap_or(100.0);
    p.keyframes = vec![Keyframe::new(Tick::ZERO, ParamValue::Float(base)), Keyframe::new(dur - one, ParamValue::Float(base * (1.0 + push / 100.0)))];
}

/// Every file the plan puts on the timeline, once each, in order of first use.
fn plan_files(pl: &Plan) -> Vec<PathBuf> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut v: Vec<PathBuf> = Vec::new();
    let mut add_file = |p: &Path| {
        if seen.insert(p.to_path_buf()) {
            v.push(p.to_path_buf());
        }
    };
    for s in &pl.scenes {
        if let Some(b) = &s.background {
            add_file(b);
        }
    }
    for c in &pl.cues {
        if let Some(a) = &c.voice {
            add_file(&a.path);
        }
        if let Some(a) = &c.sfx {
            add_file(&a.path);
        }
    }
    for r in &pl.runs {
        add_file(&r.sprite.closed);
        if let Some(o) = &r.sprite.open {
            add_file(o);
        }
        for v in r.sprite.vowels.values() {
            add_file(v);
        }
        if let Some(b) = &r.sprite.blink {
            add_file(b);
        }
    }
    for b in &pl.bgm {
        add_file(&b.audio.path);
    }
    v
}

fn build(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "anime.build";
    let o = Opts::from_params(p, cmd)?;
    let l = load(s, p, cmd)?;
    let problems = l.script.validate();
    if let Some(e) = problems.iter().find(|x| x.severity == Severity::Error) {
        return Err(bad(
            cmd,
            format!("the script has errors (run script.validate): {}{}", e.line.as_deref().map(|i| format!("line {i}: ")).unwrap_or_default(), e.message),
        ));
    }
    let out = out_dir(s, &l, p)?;
    let rate = FrameRate::from_f64(o.fps);
    if rate.num <= 0 || rate.frame_duration() <= Tick::ZERO {
        return Err(bad(cmd, format!("fps must be a positive frame rate, got {}", o.fps)));
    }
    // plan first: nothing is created when the script is empty
    let mut env = session_env(s, &l, out.clone());
    env.lip = lip_options(p);
    let pl = plan(&l.script, rate, &o, &mut env);
    drop(env);
    if pl.cues.is_empty() {
        return Err(bad(cmd, "the script has no lines to place"));
    }

    // One undo step, whatever the history holds: the limit is lifted while the steps pile up (a
    // step is dropped from the front when it is exceeded, which would shift the ones to fold),
    // then they fold into one. A failure puts the project back as it was, not step by step.
    let before = s.project.clone();
    let state_before = s.state.clone();
    let limit = s.history.limit;
    let n0 = s.history.undo.len();
    s.history.limit = limit.saturating_add(64);
    let result = place_all(s, p, &l, &o, &pl, rate);
    s.history.limit = limit;
    if result.is_err() {
        s.project = before;
        s.state = state_before;
        s.history.undo.truncate(n0);
        s.history.redo.clear();
        s.bump();
    } else {
        crate::clip_ops::collapse_history(s, n0, "Build Anime Sequence");
        while s.history.undo.len() > limit {
            s.history.undo.remove(0);
        }
    }
    result
}

fn place_all(s: &mut Session, p: &Value, l: &Loaded, o: &Opts, pl: &Plan, rate: FrameRate) -> Result<Value> {
    let cmd = "anime.build";
    // the pictures and sounds, imported once each into a bin of their own
    let title = if l.script.title.trim().is_empty() { l.stem_for_names() } else { l.script.title.clone() };
    let mut warnings: Vec<String> = pl.warnings.clone();
    // everything is opened first, then added in a single edit (one history step; a step per file
    // would also copy the whole project once per file)
    let mut prepared = Vec::new();
    for f in plan_files(pl) {
        let ps = f.to_string_lossy().to_string();
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| ps.clone());
        let identity = match crate::relink::identity_of(&*s.services, &ps) {
            Ok(i) => i,
            Err(e) => {
                warnings.push(format!("{ps}: {e}"));
                continue;
            }
        };
        match s.media.open_file(&ps, &*s.services) {
            Ok(src) => {
                let (label, clip) = crate::commands::imported_clip(s, &ps, &src, identity);
                prepared.push((f, name, label, clip, src));
            }
            Err(e) => warnings.push(format!("{ps}: {e}")),
        }
    }
    // lines spoken by a local engine carry their voice's credit (script.synthesize's manifest):
    // their items remember it as tts.synthesize's do, so tts.credits and the credit roll see them
    let credits: HashMap<PathBuf, Value> = match out_dir(s, l, p) {
        Ok(out) => read_manifest(&out).lines.values().filter_map(|e| e.credit.clone().map(|c| (out.join(&e.file), c))).collect(),
        Err(_) => HashMap::new(),
    };
    let ids = s.edit(&format!("Import {title}"), |pr, _| {
        let bin = pr.add_bin(&title, None);
        let mut ids = Vec::with_capacity(prepared.len());
        for (f, name, label, clip, _) in &prepared {
            let id = pr.add_item(name, *label, ItemKind::Media(clip.clone()), Some(bin));
            if let Some(c) = credits.get(f) {
                crate::narration_credits::mark_item(pr, id, c);
            }
            ids.push(id);
        }
        Ok(ids)
    })?;
    let mut items: HashMap<PathBuf, ItemId> = HashMap::new();
    for ((f, _, _, _, src), id) in prepared.into_iter().zip(ids) {
        s.media.insert_file(id, &f.to_string_lossy(), src);
        items.insert(f, id);
    }
    let name = unique_sequence_name(s, str_p(p, "name").filter(|n| !n.trim().is_empty()).map(str::to_string).unwrap_or_else(|| title.clone()));
    let seq = s.execute("file.newSequence", json!({"name": name, "width": o.width, "height": o.height, "fps": o.fps, "video": 4, "audio": 3}))?["sequence"]
        .as_u64()
        .map(ItemId)
        .ok_or_else(|| bad(cmd, "the sequence was not created"))?;
    let one = rate.frame_duration();
    let frame = (o.width, o.height);
    let scaling = s.prefs.media.default_media_scaling.clone();
    let mut counts = [0usize; 7];
    s.edit(&format!("Build {name}"), |pr, _| {
        let Some((v_ids, a_ids)) =
            pr.sequence(seq).map(|q| (q.video_tracks.iter().map(|t| t.id).collect::<Vec<_>>(), q.audio_tracks.iter().map(|t| t.id).collect::<Vec<_>>()))
        else {
            return Err(EngineError::NoSequence);
        };
        let mut video: Vec<Vec<TrackItem>> = vec![Vec::new(); 4];
        let mut audio: Vec<Vec<TrackItem>> = vec![Vec::new(); 3];
        let find = |m: &HashMap<PathBuf, ItemId>, path: &Path| m.get(path).copied();
        // V1: one background per scene
        for sc in &pl.scenes {
            let Some(item) = sc.background.as_deref().and_then(|b| find(&items, b)) else { continue };
            if let Some(mut ti) = make_clip(pr, item, TrackKind::Video, sc.start, sc.end - sc.start, rate, frame, &scaling) {
                if o.fit_background {
                    ti.scale_to_frame = true;
                }
                push_in(&mut ti, o.push, one);
                video[0].push(ti);
                counts[0] += 1;
            }
        }
        // V2-V4: the speaker, its open mouth, its blink
        for r in &pl.runs {
            let place = |pr: &mut Project, path: &Path, start: Tick, end: Tick| -> Option<TrackItem> {
                let item = find(&items, path)?;
                let mut ti = make_clip(pr, item, TrackKind::Video, start, end - start, rate, frame, &scaling)?;
                if let Some(sc) = o.sprite_scale {
                    set_scale(&mut ti, sc);
                }
                Some(ti)
            };
            if let Some(ti) = place(pr, &r.sprite.closed, r.start, r.end) {
                video[1].push(ti);
                counts[1] += 1;
            }
            if let Some(blink) = &r.sprite.blink {
                for (a, b) in &r.blinks {
                    if let Some(ti) = place(pr, blink, *a, *b) {
                        video[3].push(ti);
                        counts[3] += 1;
                    }
                }
            }
        }
        for c in &pl.cues {
            if let Some(sp) = &c.sprite {
                // per-vowel pictures where the sprite has them, else the one open-mouth picture
                let pieces: Vec<(Tick, Tick, Option<&PathBuf>)> = if sp.vowels.is_empty() {
                    c.mouth.iter().map(|(a, b)| (*a, *b, sp.open.as_ref())).collect()
                } else {
                    c.shapes.iter().map(|(a, b, v)| (*a, *b, v.and_then(|v| sp.vowels.get(&v)).or(sp.open.as_ref()))).collect()
                };
                for (a, b, path) in pieces {
                    if let Some(path) = path
                        && let Some(item) = find(&items, path)
                        && let Some(mut ti) = make_clip(pr, item, TrackKind::Video, a, b - a, rate, frame, &scaling)
                    {
                        if let Some(sc) = o.sprite_scale {
                            set_scale(&mut ti, sc);
                        }
                        video[2].push(ti);
                        counts[2] += 1;
                    }
                }
            }
            // A1 voices, A3 sound effects
            if let Some(v) = &c.voice
                && let Some(item) = find(&items, &v.path)
                && let Some(ti) = make_clip(pr, item, TrackKind::Audio, c.start, v.duration.min(c.end - c.start), rate, frame, &scaling)
            {
                audio[0].push(ti);
                counts[4] += 1;
            }
            if let Some(f) = &c.sfx
                && let Some(item) = find(&items, &f.path)
                && let dur = f.duration.min(pl.end - c.start)
                && dur >= one
                && let Some(mut ti) = make_clip(pr, item, TrackKind::Audio, c.start, dur, rate, frame, &scaling)
            {
                ti.gain_db = o.sfx_gain_db;
                // one sound effect at a time: a long one ends where the next begins
                if let Some(prev) = audio[2].last_mut()
                    && prev.end() > ti.start
                {
                    prev.duration = (ti.start - prev.start).max(one);
                }
                audio[2].push(ti);
                counts[6] += 1;
            }
        }
        // A2: music, repeated to fill its scenes. The length of one repeat is snapped to the
        // frame grid first: the track item snaps its own length, and stepping by the unsnapped one
        // would put the next repeat inside the last.
        for b in &pl.bgm {
            let Some(item) = find(&items, &b.audio.path) else { continue };
            if b.audio.duration < one {
                warnings.push(format!("music {} is shorter than a frame", b.audio.path.display()));
                continue;
            }
            let len = rate.snap_nearest(b.audio.duration).max(one);
            let mut at = b.start;
            let mut n = 0usize;
            while at < b.end {
                if n >= MAX_MUSIC_CLIPS {
                    warnings.push(format!("music {} stops after {MAX_MUSIC_CLIPS} repeats; the rest has none", b.audio.path.display()));
                    break;
                }
                let dur = len.min(b.end - at);
                if dur < one {
                    break;
                }
                if let Some(mut ti) = make_clip(pr, item, TrackKind::Audio, at, dur, rate, frame, &scaling) {
                    ti.gain_db = o.bgm_gain_db;
                    audio[1].push(ti);
                    counts[5] += 1;
                }
                at += dur;
                n += 1;
            }
        }
        let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        for (id, items) in v_ids.iter().zip(video) {
            if let Some(t) = q.video_tracks.iter_mut().find(|t| t.id == *id) {
                t.items = items;
            }
        }
        for (id, items) in a_ids.iter().zip(audio) {
            if let Some(t) = q.audio_tracks.iter_mut().find(|t| t.id == *id) {
                t.items = items;
            }
        }
        for (id, label) in v_ids.iter().zip(["Background", "Character", "Mouth", "Blink"]) {
            if let Some(t) = q.video_tracks.iter_mut().find(|t| t.id == *id) {
                t.name = label.into();
            }
        }
        for (id, label) in a_ids.iter().zip(["Voice", "Music", "Effects"]) {
            if let Some(t) = q.audio_tracks.iter_mut().find(|t| t.id == *id) {
                t.name = label.into();
            }
        }
        Ok(())
    })?;
    // level the voices: the same recording quality is not the same loudness (a cloned voice and
    // a preset one, a whisper and a shout)
    let mut leveled = 0usize;
    if o.level_voices {
        let measured: Vec<(filmcraft_project::ClipId, f64)> = s
            .project
            .sequence(seq)
            .and_then(|q| q.audio_tracks.first())
            .map(|t| t.items.iter().filter(|it| it.duration >= secs(0.4)).filter_map(|it| clip_loudness(s, it).map(|l| (it.id, l))).collect())
            .unwrap_or_default();
        let target = o.voice_lufs.or_else(|| median(&mut measured.iter().map(|m| m.1).collect::<Vec<f64>>()));
        if let (Some(target), true) = (target, measured.len() >= 2) {
            let gains: HashMap<filmcraft_project::ClipId, f64> =
                measured.iter().filter_map(|(id, l)| Some((*id, (target - l).clamp(-12.0, 12.0))).filter(|(_, g)| g.abs() >= 0.5)).collect();
            if !gains.is_empty() {
                leveled = gains.len();
                s.edit("Level Voices", |pr, _| {
                    let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
                    if let Some(t) = q.audio_tracks.first_mut() {
                        for it in &mut t.items {
                            if let Some(g) = gains.get(&it.id) {
                                it.gain_db = *g;
                            }
                        }
                    }
                    Ok(())
                })?;
            }
        }
    }
    // the subtitles
    let total_captions: usize = pl.cues.iter().map(|c| c.captions.len()).sum();
    let mut captions = 0usize;
    if total_captions > 0 {
        s.execute("captions.newTrack", json!({"format": "Subtitle", "name": "字幕", "language": "ja"}))?;
        let tid = s.active_sequence().and_then(|q| q.caption_tracks.last().map(|t| t.id)).ok_or_else(|| bad(cmd, "no subtitle track"))?;
        captions = s.edit_sequence("Subtitles", |q, ctx, _| {
            let mut n = 0usize;
            for c in &pl.cues {
                for (a, b, text) in &c.captions {
                    if edit::captions::add_caption(q, tid, *a, *b - *a, text, ctx).is_ok() {
                        n += 1;
                    }
                }
            }
            Ok(n)
        })?;
    }
    let [backgrounds, characters, mouths, blinks, voices, music, effects] = counts;
    Ok(json!({
        "sequence": seq.0,
        "name": name,
        "seconds": pl.end.seconds(),
        "scenes": pl.scenes.len(),
        "lines": pl.cues.len(),
        "clips": {"backgrounds": backgrounds, "characters": characters, "mouths": mouths, "blinks": blinks, "voices": voices, "music": music, "effects": effects, "subtitles": captions},
        "leveledVoices": leveled,
        "withoutAudio": pl.cues.iter().filter(|c| c.estimated).map(|c| c.line.clone()).collect::<Vec<_>>(),
        "warnings": warnings,
    }))
}

fn unique_sequence_name(s: &Session, base: String) -> String {
    let taken = |n: &str| s.project.sequences().any(|it| it.name == n);
    if !taken(&base) {
        return base;
    }
    (2..1000).map(|i| format!("{base} ({i})")).find(|n| !taken(n)).unwrap_or(base)
}

// ---------------------------------------------------------------------------------------------
// anime.check

#[derive(Clone, Debug, serde::Serialize)]
struct Problem {
    severity: Severity,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<f64>,
    message: String,
}

fn problem(severity: Severity, kind: &'static str, at: Option<Tick>, message: String) -> Problem {
    Problem { severity, kind, seconds: at.map(Tick::seconds), message }
}

fn check(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "anime.check";
    let max_silence = num(p, "maxSilenceSeconds", 0.5, 600.0, cmd)?.unwrap_or(5.0);
    let max_cps = num(p, "maxCharsPerSecond", 2.0, 100.0, cmd)?.unwrap_or(20.0);
    let max_chars = u64_p(p, "captionChars").map(|v| usize::try_from(v).unwrap_or(36).clamp(8, 400)).unwrap_or(36);
    let tolerance = num(p, "loudnessToleranceLu", 1.0, 30.0, cmd)?.unwrap_or(6.0);
    let seq_item = u64_p(p, "sequence").map(ItemId).or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
    let q = s.project.sequence(seq_item).ok_or_else(|| bad(cmd, "that is not a sequence"))?.clone();
    let one = q.settings.frame_rate.frame_duration();
    let mut out: Vec<Problem> = Vec::new();

    // offline media
    for t in q.all_tracks() {
        for it in &t.items {
            if s.project.item(it.item).and_then(|i| i.as_media()).is_some_and(|m| m.offline) {
                out.push(problem(Severity::Error, "offline", Some(it.start), format!("{} is offline", it.name)));
            }
        }
    }
    // the picture never goes black between the first and the last background
    if let Some(v1) = q.video_tracks.first() {
        let mut at = Tick::ZERO;
        let end = v1.items.iter().map(|i| i.end()).max().unwrap_or_default();
        for it in &v1.items {
            if it.start - at >= one {
                out.push(problem(Severity::Error, "gap", Some(at), format!("nothing on the background track for {:.2} s", (it.start - at).seconds())));
            }
            at = at.max(it.end());
        }
        if v1.items.is_empty() {
            out.push(problem(Severity::Warning, "empty", None, "the background track is empty".into()));
        }
        let last_anywhere = q
            .video_tracks
            .iter()
            .flat_map(|t| t.items.iter().map(|i| i.end()))
            .chain(q.audio_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.end())))
            .max()
            .unwrap_or_default();
        if last_anywhere - end >= one && !v1.items.is_empty() {
            out.push(problem(
                Severity::Error,
                "gap",
                Some(end),
                format!("the background ends {:.2} s before the rest of the sequence", (last_anywhere - end).seconds()),
            ));
        }
    }
    // voices: no two at once, no long silence
    let mut loud: Vec<(f64, Tick, Tick, String)> = Vec::new();
    if let Some(a1) = q.audio_tracks.first() {
        let mut at: Option<Tick> = None;
        for it in &a1.items {
            if let Some(prev_end) = at {
                if prev_end > it.start {
                    out.push(problem(
                        Severity::Error,
                        "overlap",
                        Some(it.start),
                        format!("{} starts {:.2} s before the previous voice ends", it.name, (prev_end - it.start).seconds()),
                    ));
                } else if (it.start - prev_end).seconds() > max_silence {
                    out.push(problem(Severity::Warning, "silence", Some(prev_end), format!("{:.1} s without a voice", (it.start - prev_end).seconds())));
                }
            }
            at = Some(at.map_or(it.end(), |a| a.max(it.end())));
            if it.duration >= secs(0.4)
                && let Some(l) = clip_loudness(s, it)
            {
                loud.push((l, it.start, it.duration, it.name.clone()));
            }
        }
    }
    let mut median_voice = None;
    if loud.len() >= 3 {
        let mut v: Vec<f64> = loud.iter().map(|x| x.0).collect();
        let m = median(&mut v).unwrap_or(-23.0);
        median_voice = Some(m);
        for (l, start, _, name) in &loud {
            if (l - m).abs() > tolerance {
                out.push(problem(Severity::Warning, "loudness", Some(*start), format!("{name} is {:+.1} LU from the median voice ({m:.1} LUFS)", l - m)));
            }
        }
    }
    // subtitles
    let mut captions = 0usize;
    for t in &q.caption_tracks {
        for c in &t.captions {
            captions += 1;
            let chars = c.text.chars().filter(|ch| !ch.is_whitespace()).count();
            let secs_ = c.duration.seconds().max(1e-6);
            if chars > max_chars * 2 {
                out.push(problem(Severity::Warning, "caption-long", Some(c.start), format!("{chars} characters in one subtitle (at most {})", max_chars * 2)));
            }
            if chars as f64 / secs_ > max_cps {
                out.push(problem(
                    Severity::Warning,
                    "caption-fast",
                    Some(c.start),
                    format!("{:.0} characters per second (at most {max_cps:.0})", chars as f64 / secs_),
                ));
            }
            if c.duration < secs(0.4) && chars > 1 {
                out.push(problem(Severity::Warning, "caption-short", Some(c.start), format!("shown for only {secs_:.2} s")));
            }
        }
    }
    let voices = q.audio_tracks.first().map_or(0, |t| t.items.len());
    if voices == 0 {
        out.push(problem(Severity::Warning, "no-voice", None, "the voice track is empty".into()));
    }
    // against the script: lines that have no audio
    let mut missing_audio = 0usize;
    if str_p(p, "path").is_some() {
        let l = load(s, p, cmd)?;
        let dir = out_dir(s, &l, p)?;
        let manifest = read_manifest(&dir);
        for (_, line) in l.script.lines_in_route() {
            if line.is_spoken() && !has_recording(&l, line) && !manifest.lines.get(&line.id).is_some_and(|e| dir.join(&e.file).is_file()) {
                missing_audio += 1;
                out.push(problem(Severity::Warning, "no-audio", None, format!("line {} has no audio", line.id)));
            }
        }
    }
    let errors = out.iter().filter(|x| x.severity == Severity::Error).count();
    Ok(json!({
        "ok": errors == 0,
        "errors": errors,
        "warnings": out.len() - errors,
        "problems": out.iter().map(|x| serde_json::to_value(x).unwrap_or(Value::Null)).collect::<Vec<_>>(),
        "stats": {
            "seconds": q.audio_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.end())).chain(q.video_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.end()))).max().unwrap_or_default().seconds(),
            "voices": voices,
            "subtitles": captions,
            "medianVoiceLufs": median_voice,
            "linesWithoutAudio": missing_audio,
        },
    }))
}

/// The middle of the values (the mean of the two middle ones for an even count).
fn median(v: &mut [f64]) -> Option<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n == 0 {
        return None;
    }
    Some(if n % 2 == 1 { *v.get(n / 2)? } else { (*v.get(n / 2 - 1)? + *v.get(n / 2)?) / 2.0 })
}

/// Integrated loudness (LUFS) of a clip as heard (its source range, with its own gain), or None
/// when it cannot be read.
fn clip_loudness(s: &Session, it: &TrackItem) -> Option<f64> {
    const RATE: u32 = 48_000;
    let src = s.source(it.item)?;
    if !src.info().has_audio() {
        return None;
    }
    let start = it.source_in.to_units_floor(RATE as i64);
    let len = usize::try_from(it.duration.to_units_floor(RATE as i64)).ok().filter(|n| *n > 0 && *n <= RATE as usize * 600)?;
    let buf = src.audio(start, len, RATE).ok()?;
    let chans: Vec<&[f32]> = buf.channels.iter().map(Vec::as_slice).collect();
    if chans.is_empty() {
        return None;
    }
    let mut m = filmcraft_audio_dsp::loudness::LoudnessMeter::new(RATE as f64, chans.len());
    m.process(&chans);
    let l = m.integrated() + it.gain_db;
    l.is_finite().then_some(l)
}

// ---------------------------------------------------------------------------------------------
// anime.run

fn run_all(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "anime.run";
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "`path` (the script file) is required"))?.to_string();
    let mut steps: Vec<Value> = Vec::new();
    // the sub-commands take the same path/out/model/... parameters as this one
    let base = {
        let mut b = p.clone();
        if let Some(o) = b.as_object_mut() {
            for k in ["export", "preset", "pickVoices", "synthesize", "check"] {
                o.remove(k);
            }
        }
        b
    };
    let stop = |steps: &mut Vec<Value>, name: &str, e: String| -> Result<Value> {
        steps.push(json!({"step": name, "ok": false, "error": e}));
        Ok(json!({"ok": false, "failedStep": name, "steps": steps}))
    };

    let v = match s.execute("script.validate", json!({"path": path})) {
        Ok(v) => v,
        Err(e) => return stop(&mut steps, "validate", e.to_string()),
    };
    steps.push(json!({"step": "validate", "ok": v["ok"], "errors": v["errors"], "warnings": v["warnings"], "stats": v["stats"]}));
    if v["ok"] != json!(true) {
        return Ok(json!({"ok": false, "failedStep": "validate", "steps": steps, "problems": v["problems"]}));
    }
    if bool_p(p, "pickVoices").unwrap_or(false) {
        match s.execute("script.pickVoices", json!({"path": path, "write": true})) {
            Ok(r) => steps.push(json!({"step": "pickVoices", "ok": true, "picked": r["picked"].as_array().map_or(0, Vec::len), "skipped": r["skipped"].as_array().map_or(0, Vec::len)})),
            Err(e) => return stop(&mut steps, "pickVoices", e.to_string()),
        }
    }
    if bool_p(p, "synthesize").unwrap_or(true) {
        let mut q = base.clone();
        if let Some(o) = q.as_object_mut() {
            for k in ["name", "width", "height", "fps"] {
                o.remove(k);
            }
        }
        match s.execute("script.synthesize", q) {
            Ok(r) => steps.push(json!({"step": "synthesize", "ok": true, "synthesized": r["synthesized"], "cached": r["cached"], "recorded": r["recorded"], "failed": r["failed"]})),
            Err(e) => return stop(&mut steps, "synthesize", e.to_string()),
        }
    }
    let built = match s.execute("anime.build", base.clone()) {
        Ok(v) => v,
        Err(e) => return stop(&mut steps, "build", e.to_string()),
    };
    steps.push(json!({"step": "build", "ok": true, "sequence": built["sequence"], "seconds": built["seconds"], "lines": built["lines"], "clips": built["clips"], "withoutAudio": built["withoutAudio"], "warnings": built["warnings"]}));
    let mut ok = true;
    if bool_p(p, "check").unwrap_or(true) {
        let mut q = json!({"sequence": built["sequence"], "path": path});
        for k in ["out", "captionChars", "maxSilenceSeconds", "maxCharsPerSecond", "loudnessToleranceLu"] {
            if let Some(v) = p.get(k) {
                q[k] = v.clone();
            }
        }
        match s.execute("anime.check", q) {
            Ok(r) => {
                ok = r["ok"] == json!(true);
                steps.push(
                    json!({"step": "check", "ok": r["ok"], "errors": r["errors"], "warnings": r["warnings"], "problems": r["problems"], "stats": r["stats"]}),
                );
            }
            Err(e) => return stop(&mut steps, "check", e.to_string()),
        }
    }
    if let Some(dest) = str_p(p, "export").filter(|d| !d.trim().is_empty()) {
        let mut q = json!({"sequence": built["sequence"], "path": dest, "start": true, "wait": true});
        if let Some(pre) = str_p(p, "preset") {
            q["preset"] = json!(pre);
        }
        match s.execute("export.queue.add", q) {
            Ok(r) => {
                // the queue says Ok even when the encode failed: look at this export's own status
                let id = r["added"].as_array().and_then(|a| a.first()).and_then(Value::as_u64);
                let item = r["items"].as_array().and_then(|a| a.iter().find(|i| i["id"].as_u64() == id));
                let status = item.and_then(|i| i["status"].as_str()).unwrap_or("unknown");
                if status != "done" {
                    let why = item.and_then(|i| i["error"].as_str()).unwrap_or("");
                    return stop(&mut steps, "export", format!("the export ended as `{status}`{}{why}", if why.is_empty() { "" } else { ": " }));
                }
                steps.push(json!({"step": "export", "ok": true, "path": item.map(|i| i["path"].clone()), "format": item.map(|i| i["format"].clone())}));
            }
            Err(e) => return stop(&mut steps, "export", e.to_string()),
        }
    }
    Ok(json!({"ok": ok, "sequence": built["sequence"], "steps": steps}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id: &'static str, label: &'static str, params: &'static str, run: Run, journal: bool| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal,
    };
    vec![
        spec(
            "anime.build",
            "Build Anime Sequence",
            r#"{"path":str,"out":str?,"name":str?,"width":u32=1920,"height":u32=1080,"fps":f64=24,"pause":f64=0.35,"sceneGap":f64=0.6,"directionSeconds":f64=1,"charsPerSecond":f64=7,"push":f64=5,"kenBurns":bool?,"spriteScale":f64?,"bgmGainDb":f64=-18,"sfxGainDb":f64=0,"captionChars":n=36,"blink":bool=true,"fitBackground":bool=true,"levelVoices":bool=true,"voiceLufs":f?,"three":bool?,"openDb":f?,"halfDb":f?,"minSeconds":f?}"#,
            build,
            true,
        ),
        spec(
            "anime.check",
            "Check Anime Sequence",
            r#"{"sequence":id?,"path":str?,"out":str?,"maxSilenceSeconds":f64=5,"maxCharsPerSecond":f64=20,"captionChars":n=36,"loudnessToleranceLu":f64=6}"#,
            check,
            false,
        ),
        spec(
            "anime.run",
            "Run Anime Pipeline",
            r#"{"path":str,"engine":"qwen3|aivisspeech|voicevox"="qwen3","url":str?,"pickVoices":bool=false,"synthesize":bool=true,"check":bool=true,"export":str?,"preset":str?,"out":str?,"name":str?,"language":str?,"python":str?,"script":str?,"device":str?,"…build options":any?}"#,
            run_all,
            true,
        ),
    ]
}
