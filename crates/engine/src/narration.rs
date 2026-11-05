//! Narration: text-to-speech through a local **AivisSpeech** or **VOICEVOX** app
//! (Graphics and Titles ▸ Generate Narration…).
//!
//! | command | does |
//! |---|---|
//! | `tts.engines` | which engines answer at their addresses (Settings ▸ Narration), with versions |
//! | `tts.voices {engine?, refresh?}` | the voices (speaker / style / style id / uuid), cached for 30 s |
//! | `tts.voiceTerms {engine?, speaker_uuid? \| voice?}` | a voice's terms (the engine's policy text), credit and model licence |
//! | `tts.preview {engine?, text, voice?, speed?…}` | synthesize one line to a temporary WAV (nothing changes in the project) |
//! | `tts.synthesize {engine?, lines \| script \| text, …}` | synthesize, import and place every line on an audio track, with captions; one undo step |
//!
//! FilmCraft talks to the engine over HTTP only ([`filmcraft_speech::tts`]); the engine is a
//! separate app the user installs and starts. Each line's WAV is written to the project's
//! `Narration` folder (`<project dir>/Narration/<name>_<n>_<speaker>-<style>.wav`; Settings ▸
//! Narration ▸ Folder, or `dir`, override it; an unsaved project uses `Narration` in the data
//! directory). Existing files are never overwritten: a free name is chosen.
//!
//! Every voice has terms: VOICEVOX characters require the credit `VOICEVOX:<name>`, AivisSpeech
//! models carry their own licence. `tts.synthesize` returns the distinct `credits` of the voices
//! it used and `creditText` to paste into the video description; `tts.voiceTerms` returns the full
//! terms. See `docs/narration.md` and `docs/voice-terms.md`.
//!
//! The placed narration is mixed in (Settings ▸ Narration, or `autoLevel` / `duck`): its clips
//! get the Dialogue audio type and a loudness match to the dialogue target, and the Music /
//! Ambience / untyped clips on other audio tracks under it are ducked ([`auto_mix`]), all in the
//! same undo step. The result's `mix` says what was done.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::{AudioType, CaptionFormat, CaptionTrack, ClipId, ItemId, TrackId, TrackKind};
use filmcraft_speech::tts::{self, AivmModel, Client, Prosody, Speaker, TtsEngine, TtsError, WavInfo};
use filmcraft_time::{Tick, TimeRange};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

/// Most lines one `tts.synthesize` reads.
pub const MAX_LINES: usize = 500;
/// How long a voice list is reused before the engine is asked again.
const VOICE_CACHE_SECONDS: u64 = 30;
/// Undo label of `tts.synthesize`.
pub const UNDO_LABEL: &str = "Synthesize Narration";
/// Settings ▸ Narration ▸ Duck other audio by (the Essential Sound ducking default).
pub const DEFAULT_DUCK_DB: f64 = -15.0;

// ------------------------------------------------------------------------------------- prefs

/// Settings ▸ Narration (persisted in the preferences as `narration`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NarrationPrefs {
    /// Default engine: `aivisspeech` | `voicevox`.
    pub engine: String,
    /// Where the AivisSpeech app's engine answers.
    pub aivisspeech_url: String,
    /// Where the VOICEVOX app's engine answers.
    pub voicevox_url: String,
    /// Default voice: `Speaker/Style`, a speaker name or a style id ("" = the engine's first voice).
    pub default_voice: String,
    /// Folder for the narration WAVs ("" = `Narration` next to the project).
    pub folder: String,
    /// Silence after each line (seconds).
    pub pause_seconds: f64,
    /// Make captions from the lines.
    pub captions: bool,
    /// Longer lines are split into several captions at punctuation (0 = never).
    pub caption_max_chars: u32,
    /// Give the narration clips the Dialogue audio type and match their loudness to the
    /// dialogue target (Settings ▸ Audio; `essentialSound.autoMatch`).
    pub auto_level: bool,
    /// Duck the Music / Ambience (and untyped) clips on other audio tracks under the narration
    /// (`essentialSound.generateDucking`).
    pub duck: bool,
    /// How far ducked clips go down under the narration (dB, −40…0).
    pub duck_reduce_db: f64,
}

impl Default for NarrationPrefs {
    fn default() -> Self {
        Self {
            engine: TtsEngine::AivisSpeech.id().into(),
            aivisspeech_url: TtsEngine::AivisSpeech.default_url().into(),
            voicevox_url: TtsEngine::Voicevox.default_url().into(),
            default_voice: String::new(),
            folder: String::new(),
            pause_seconds: 0.3,
            captions: true,
            caption_max_chars: 40,
            auto_level: true,
            duck: true,
            duck_reduce_db: DEFAULT_DUCK_DB,
        }
    }
}

impl NarrationPrefs {
    pub fn url(&self, e: TtsEngine) -> &str {
        match e {
            TtsEngine::AivisSpeech => &self.aivisspeech_url,
            TtsEngine::Voicevox => &self.voicevox_url,
        }
    }
    pub fn default_engine(&self) -> TtsEngine {
        TtsEngine::from_id(&self.engine).unwrap_or(TtsEngine::AivisSpeech)
    }
}

// ------------------------------------------------------------------------------------- state

/// Session state: cached voice lists and a synthesis finished in the background (the UI renders
/// off the UI thread, then runs `tts.synthesize {prepared: token}`).
#[derive(Default)]
pub struct NarrationState {
    voices: HashMap<TtsEngine, (web_time::Instant, String, Vec<Speaker>)>,
    pub prepared: Option<Prepared>,
}

/// Audio rendered ahead of `tts.synthesize` (see [`render`]).
pub struct Prepared {
    pub token: u64,
    pub rendered: Rendered,
}

/// A token for [`Prepared`].
pub fn next_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

// ------------------------------------------------------------------------------------- voices

/// A resolved voice.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Voice {
    pub engine: TtsEngine,
    pub speaker: String,
    pub style: String,
    /// The style id (`speaker` in the engine API).
    pub id: i64,
    pub uuid: String,
}

impl Voice {
    /// `Speaker/Style`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.speaker, self.style)
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && !matches!(c, '・' | '_' | '-')).flat_map(char::to_lowercase).collect()
}

fn voice_of(engine: TtsEngine, sp: &Speaker, st: &tts::Style) -> Voice {
    Voice { engine, speaker: sp.name.clone(), style: st.name.clone(), id: st.id, uuid: sp.uuid.clone() }
}

/// Find a speaker by name: exact, then ignoring case / spaces, then the only one containing it.
fn find_speaker<'a>(speakers: &'a [Speaker], name: &str) -> Option<&'a Speaker> {
    let n = norm(name);
    if n.is_empty() {
        return None;
    }
    speakers.iter().find(|s| s.name == name).or_else(|| speakers.iter().find(|s| norm(&s.name) == n)).or_else(|| {
        let mut it = speakers.iter().filter(|s| norm(&s.name).contains(&n));
        match (it.next(), it.next()) {
            (Some(s), None) => Some(s),
            _ => None,
        }
    })
}

/// A voice reference: a style id (number or digits), `"Speaker/Style"`, `"Speaker"` (its first
/// style), or `{id}` / `{speaker, style}`.
pub fn resolve_voice(engine: TtsEngine, speakers: &[Speaker], v: &Value) -> std::result::Result<Voice, String> {
    let by_id = |id: i64| {
        speakers
            .iter()
            .find_map(|sp| sp.styles.iter().find(|st| st.id == id).map(|st| voice_of(engine, sp, st)))
            .ok_or_else(|| format!("{} has no voice with style id {id}", engine.name()))
    };
    let by_names = |speaker: &str, style: Option<&str>| -> std::result::Result<Voice, String> {
        let sp = find_speaker(speakers, speaker).ok_or_else(|| format!("{} has no voice named `{speaker}` (tts.voices lists them)", engine.name()))?;
        let st = match style.map(str::trim).filter(|s| !s.is_empty()) {
            None => sp.styles.first(),
            Some(st) => sp.styles.iter().find(|x| x.name == st).or_else(|| sp.styles.iter().find(|x| norm(&x.name) == norm(st))),
        };
        let st = st.ok_or_else(|| format!("{}'s voice `{}` has no style `{}`", engine.name(), sp.name, style.unwrap_or_default()))?;
        Ok(voice_of(engine, sp, st))
    };
    match v {
        Value::Null => speakers
            .iter()
            .find_map(|sp| sp.styles.first().map(|st| voice_of(engine, sp, st)))
            .ok_or_else(|| format!("{} has no voices installed", engine.name())),
        Value::Number(n) => n.as_i64().ok_or_else(|| format!("bad style id {n}")).and_then(by_id),
        Value::String(s) => {
            let s = s.trim();
            if let Ok(id) = s.parse::<i64>() {
                return by_id(id);
            }
            match s.split_once(['/', '／']) {
                Some((a, b)) => by_names(a.trim(), Some(b)),
                None => by_names(s, None),
            }
        }
        Value::Object(m) => {
            if let Some(id) = m.get("id").or(m.get("styleId")).and_then(Value::as_i64) {
                return by_id(id);
            }
            if let Some(id) = m.get("speaker").and_then(Value::as_i64) {
                return by_id(id);
            }
            let sp = m.get("speaker").or(m.get("name")).and_then(Value::as_str).ok_or("a voice object needs `id` or `speaker`")?;
            by_names(sp, m.get("style").and_then(Value::as_str))
        }
        other => Err(format!("bad voice {other}")),
    }
}

/// `話者: セリフ` / `話者「セリフ」` lines of a script: (speaker, text). Lines without a speaker
/// have `None`; empty lines and `#` comments are skipped.
pub fn parse_script(script: &str) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    for line in script.lines().take(MAX_LINES * 4) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let colon = line.char_indices().find(|(_, c)| matches!(c, ':' | '：'));
        if let Some((i, c)) = colon {
            let (name, rest) = (line[..i].trim(), line[i + c.len_utf8()..].trim());
            if !name.is_empty() && name.chars().count() <= 40 && !rest.is_empty() && !name.chars().all(|c| c.is_ascii_digit()) {
                out.push((Some(name.to_string()), unquote(rest).to_string()));
                continue;
            }
        }
        if let Some(i) = line.find('「')
            && line.ends_with('」')
            && i > 0
        {
            let name = line[..i].trim();
            let text = &line[i..];
            if name.chars().count() <= 40 {
                out.push((Some(name.to_string()), unquote(text).to_string()));
                continue;
            }
        }
        out.push((None, line.to_string()));
    }
    out
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('「').and_then(|x| x.strip_suffix('」')).map(str::trim).unwrap_or(s)
}

// ------------------------------------------------------------------------------------- jobs

/// One line to synthesize.
#[derive(Clone, Debug)]
pub struct LineReq {
    pub text: String,
    /// Voice reference ([`resolve_voice`]); Null = the default voice.
    pub voice: Value,
    pub prosody: Prosody,
    /// Silence after the line (seconds).
    pub pause_after: f64,
}

/// Everything [`render`] needs; no session (it can run on a worker thread).
#[derive(Clone, Debug)]
pub struct Job {
    pub client: Client,
    pub lines: Vec<LineReq>,
    /// The voice list when it is cached (else [`render`] asks the engine).
    pub speakers: Option<Vec<Speaker>>,
    /// Settings ▸ Narration ▸ Default voice.
    pub default_voice: String,
}

pub struct RenderedLine {
    pub text: String,
    pub voice: Voice,
    pub wav: Vec<u8>,
    pub info: WavInfo,
    pub query: Value,
    pub pause_after: f64,
}

/// The result of [`render`].
pub struct Rendered {
    pub engine: TtsEngine,
    pub url: String,
    pub lines: Vec<RenderedLine>,
    pub speakers: Vec<Speaker>,
    pub models: Vec<AivmModel>,
}

fn engine_p(s: &Session, p: &Value, cmd: &str) -> Result<TtsEngine> {
    match str_p(p, "engine") {
        Some(e) => TtsEngine::from_id(e).ok_or_else(|| bad(cmd, format!("unknown engine `{e}` (aivisspeech or voicevox)"))),
        None => Ok(s.prefs.narration.default_engine()),
    }
}

fn client_p(s: &Session, p: &Value, cmd: &str) -> Result<Client> {
    let e = engine_p(s, p, cmd)?;
    let url = str_p(p, "url").unwrap_or(s.prefs.narration.url(e));
    Client::new(e, url).map_err(|err| bad(cmd, err.to_string()))
}

fn tts_err(e: TtsError) -> EngineError {
    EngineError::Other(e.to_string())
}

fn cached(s: &Session, c: &Client) -> Option<Vec<Speaker>> {
    let (at, url, v) = s.narration.voices.get(&c.engine)?;
    (url == c.base_url() && at.elapsed().as_secs() < VOICE_CACHE_SECONDS).then(|| v.clone())
}

/// The engine's voices (cached for 30 s; `refresh` asks again).
pub fn voices(s: &mut Session, c: &Client, refresh: bool) -> Result<Vec<Speaker>> {
    if !refresh && let Some(v) = cached(s, c) {
        return Ok(v);
    }
    let v = c.speakers().map_err(tts_err)?;
    s.narration.voices.insert(c.engine, (web_time::Instant::now(), c.base_url().to_string(), v.clone()));
    Ok(v)
}

/// Build the job of a `tts.synthesize` / `tts.preview` call from its parameters and the
/// preferences.
pub fn prepare(s: &Session, p: &Value, cmd: &str) -> Result<Job> {
    let client = client_p(s, p, cmd)?;
    let prefs = &s.prefs.narration;
    let base = Prosody::from_params(p, &Prosody::default());
    let pause_default = f64_p(p, "pause").or(f64_p(p, "pauseAfter")).unwrap_or(prefs.pause_seconds);
    let pause = |v: Option<f64>| {
        let x = v.unwrap_or(pause_default);
        if x.is_finite() { x.clamp(0.0, 30.0) } else { 0.0 }
    };
    let default_voice = p.get("voice").cloned().unwrap_or(Value::Null);
    let cast = p.get("cast").and_then(Value::as_object);
    let mut lines = Vec::new();
    let text_of = |t: &str| -> Result<String> {
        let t = t.trim();
        if t.is_empty() {
            return Err(bad(cmd, "a line has no text"));
        }
        if t.chars().count() > tts::MAX_TEXT_CHARS {
            return Err(bad(cmd, format!("a line can have at most {} characters", tts::MAX_TEXT_CHARS)));
        }
        Ok(t.to_string())
    };
    if let Some(arr) = p.get("lines").and_then(Value::as_array) {
        if arr.len() > MAX_LINES {
            return Err(bad(cmd, format!("at most {MAX_LINES} lines at a time")));
        }
        for l in arr {
            let (text, voice, prosody, after) = match l {
                Value::String(t) => (text_of(t)?, default_voice.clone(), base.clone(), None),
                Value::Object(_) => {
                    let t = str_p(l, "text").ok_or_else(|| bad(cmd, "every line needs `text`"))?;
                    let voice = l.get("voice").or(l.get("speaker")).filter(|v| !v.is_null()).cloned().unwrap_or_else(|| default_voice.clone());
                    let voice = match (&voice, cast) {
                        (Value::String(n), Some(c)) => c.get(n).cloned().unwrap_or(voice),
                        _ => voice,
                    };
                    (text_of(t)?, voice, Prosody::from_params(l, &base), f64_p(l, "pauseAfter"))
                }
                _ => return Err(bad(cmd, "`lines` holds objects {text, voice?, speed?, …} or strings")),
            };
            lines.push(LineReq { text, voice, prosody, pause_after: pause(after) });
        }
    } else if let Some(script) = str_p(p, "script") {
        for (who, text) in parse_script(script) {
            let voice = match &who {
                Some(n) => cast.and_then(|c| c.get(n)).cloned().unwrap_or_else(|| Value::String(n.clone())),
                None => default_voice.clone(),
            };
            lines.push(LineReq { text: text_of(&text)?, voice, prosody: base.clone(), pause_after: pause(None) });
            if lines.len() > MAX_LINES {
                return Err(bad(cmd, format!("at most {MAX_LINES} lines at a time")));
            }
        }
    } else if let Some(t) = str_p(p, "text") {
        lines.push(LineReq { text: text_of(t)?, voice: default_voice.clone(), prosody: base.clone(), pause_after: pause(None) });
    }
    if lines.is_empty() {
        return Err(bad(cmd, "need `lines` ([{text, voice?}]), `script` (\"Speaker: line\" per line) or `text`"));
    }
    Ok(Job { speakers: cached(s, &client), client, lines, default_voice: prefs.default_voice.clone() })
}

/// Synthesize every line of `job` (network only; no session, safe on a worker thread).
pub fn render(job: &Job) -> std::result::Result<Rendered, TtsError> {
    let c = &job.client;
    let speakers = match &job.speakers {
        Some(v) => v.clone(),
        None => c.speakers()?,
    };
    let models = c.aivm_models().unwrap_or_default();
    let default = if job.default_voice.trim().is_empty() {
        Value::Null
    } else {
        // a default voice of the other engine (or one since removed) falls back to the first voice
        let v = Value::String(job.default_voice.clone());
        if resolve_voice(c.engine, &speakers, &v).is_ok() { v } else { Value::Null }
    };
    let mut lines = Vec::with_capacity(job.lines.len());
    for l in &job.lines {
        let vref = if l.voice.is_null() { &default } else { &l.voice };
        let voice = resolve_voice(c.engine, &speakers, vref).map_err(TtsError::Invalid)?;
        let mut query = c.audio_query(&l.text, voice.id)?;
        tts::apply_prosody(&mut query, &l.prosody, c.engine);
        let (wav, info) = c.synthesis(&query, voice.id)?;
        lines.push(RenderedLine { text: l.text.clone(), voice, wav, info, query, pause_after: l.pause_after });
    }
    Ok(Rendered { engine: c.engine, url: c.base_url().to_string(), lines, speakers, models })
}

// ------------------------------------------------------------------------------------- credits

/// One credit line per distinct voice, with where its terms are.
pub fn credits(r: &Rendered) -> Vec<Value> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    for l in &r.lines {
        let v = &l.voice;
        let key = (v.uuid.clone(), if v.uuid.is_empty() { v.speaker.clone() } else { String::new() });
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(credit_json(r.engine, v, &r.models));
    }
    out
}

fn credit_json(engine: TtsEngine, v: &Voice, models: &[AivmModel]) -> Value {
    match engine {
        TtsEngine::Voicevox => json!({
            "engine": engine.id(),
            "speaker": v.speaker,
            "uuid": v.uuid,
            "credit": format!("VOICEVOX:{}", v.speaker),
            "required": true,
            "termsUrl": engine.terms_url(),
            "note": "VOICEVOX requires this credit wherever the audio is published (video, description). Each character has its own terms: run tts.voiceTerms or see the character's page.",
            "termsCommand": {"command": "tts.voiceTerms", "params": {"engine": engine.id(), "speaker_uuid": v.uuid}},
        }),
        TtsEngine::AivisSpeech => {
            let model = models.iter().find(|m| !v.uuid.is_empty() && m.speakers.contains(&v.uuid));
            let license = model.and_then(|m| m.license.clone());
            let note = match &license {
                Some(l) => {
                    format!("Model licence: {}. Check what it requires (credit, commercial use) before publishing.", l.lines().next().unwrap_or_default())
                }
                None => "Check the model's licence before publishing (tts.voiceTerms, or the model's page on AivisHub).".to_string(),
            };
            json!({
                "engine": engine.id(),
                "speaker": v.speaker,
                "uuid": v.uuid,
                "credit": format!("AivisSpeech:{}", v.speaker),
                "required": false,
                "checkLicense": true,
                "model": model.map(|m| m.name.clone()),
                "modelUuid": model.map(|m| m.uuid.clone()),
                "license": license,
                "modelUrl": model.map(AivmModel::hub_url),
                "termsUrl": engine.terms_url(),
                "note": note,
                "termsCommand": {"command": "tts.voiceTerms", "params": {"engine": engine.id(), "speaker_uuid": v.uuid}},
            })
        }
    }
}

// ------------------------------------------------------------------------------------- placing

/// Characters that cannot be in a file name.
fn file_safe(s: &str) -> String {
    let t: String =
        s.chars().map(|c| if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).take(60).collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() { "voice".into() } else { t }
}

/// Folder the narration WAVs go to: `dir`, Settings ▸ Narration ▸ Folder, `Narration` next to
/// the project, else `Narration` in the data directory.
pub fn narration_dir(s: &Session, p: &Value) -> String {
    if let Some(d) = str_p(p, "dir").filter(|d| !d.trim().is_empty()) {
        return d.to_string();
    }
    if !s.prefs.narration.folder.trim().is_empty() {
        return s.prefs.narration.folder.clone();
    }
    if let Some(d) = s.path.as_deref().and_then(|x| std::path::Path::new(x).parent()).filter(|d| !d.as_os_str().is_empty()) {
        return d.join("Narration").to_string_lossy().into_owned();
    }
    let base = s
        .prefs_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.to_path_buf())
        .or_else(|| s.data_dir.clone())
        .unwrap_or_else(|| crate::temp_dir().join("FilmCraft"));
    base.join("Narration").to_string_lossy().into_owned()
}

/// A path in `dir` for `stem`.wav that is neither a file nor a project item yet.
fn free_path(s: &Session, dir: &str, stem: &str, names: &std::collections::HashSet<String>) -> Result<String> {
    for k in 1..10_000 {
        let file = if k == 1 { format!("{stem}.wav") } else { format!("{stem}_{k}.wav") };
        let path = std::path::Path::new(dir).join(&file).to_string_lossy().into_owned();
        if !names.contains(&file) && s.services.file_size(&path).is_err() {
            return Ok(path);
        }
    }
    Err(EngineError::Other(format!("no free file name for {stem} in {dir}")))
}

/// The first frame boundary at or after `t`.
fn frame_ceil(rate: filmcraft_time::FrameRate, t: Tick) -> Tick {
    let f = rate.snap(t);
    if f < t { f + rate.frame_duration() } else { f }
}

/// Write, import and place `r` (one undo step), with captions.
pub fn place(s: &mut Session, p: &Value, r: Rendered) -> Result<Value> {
    const CMD: &str = "tts.synthesize";
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let rate = s.sequence_rate();
    let at = time_p(s, p, "").unwrap_or(s.playhead()).max(Tick::ZERO);
    // layout: each line after the previous one and its pause, on frame boundaries
    let mut starts = Vec::with_capacity(r.lines.len());
    let mut t = at;
    for l in &r.lines {
        let st = frame_ceil(rate, t);
        starts.push(st);
        t = st + l.info.duration() + Tick::from_seconds_f64(l.pause_after);
    }
    let end = match (starts.last(), r.lines.last()) {
        (Some(st), Some(l)) => *st + l.info.duration(),
        _ => return Err(bad(CMD, "nothing to place")),
    };
    // the track: `track`, else the first unlocked audio track with room, else a new one
    let n0 = s.history.undo.len();
    let track = match crate::commands::dest_track_p(s, p, "track", false, CMD)? {
        Some(t) => t,
        None => {
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let free = seq.audio_tracks.iter().find(|tr| !tr.locked && !tr.items.iter().any(|it| it.start < end && it.end() > at)).map(|tr| tr.id);
            match free {
                Some(t) => t,
                None => {
                    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1}))?;
                    s.active_sequence().and_then(|q| q.audio_tracks.last()).map(|t| t.id).ok_or_else(|| bad(CMD, "could not add an audio track"))?
                }
            }
        }
    };
    if s.active_sequence().is_some_and(|q| q.audio_tracks.iter().any(|t| t.id == track && t.locked)) {
        return Err(bad(CMD, "the audio track is locked"));
    }
    // files
    let dir = narration_dir(s, p);
    if !cfg!(target_arch = "wasm32") {
        let _ = std::fs::create_dir_all(&dir);
    }
    let name = file_safe(str_p(p, "name").unwrap_or("Narration"));
    let mut names: std::collections::HashSet<String> = s.project.items.values().map(|i| i.name.clone()).collect();
    let mut files = Vec::with_capacity(r.lines.len());
    let mut items: Vec<ItemId> = Vec::with_capacity(r.lines.len());
    for (i, l) in r.lines.iter().enumerate() {
        let stem = format!("{name}_{:03}_{}", i + 1, file_safe(&format!("{}-{}", l.voice.speaker, l.voice.style)));
        let path = free_path(s, &dir, &stem, &names)?;
        s.services.write_file(&path, &l.wav).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        let item = crate::commands::import_bytes(s, &path, l.wav.clone().into(), None)?;
        if let Some(f) = std::path::Path::new(&path).file_name() {
            names.insert(f.to_string_lossy().into_owned());
        }
        files.push(path);
        items.push(item);
    }
    // captions
    let captions = bool_p(p, "captions").unwrap_or(s.prefs.narration.captions);
    let max_chars = u64_p(p, "captionMaxChars").map_or(s.prefs.narration.caption_max_chars as usize, |n| n.min(10_000) as usize);
    let caption_track = if !captions {
        None
    } else if let Some(v) = p.get("captionTrack").filter(|v| !v.is_null()) {
        Some(crate::captions::track_param(s, &json!({ "track": v })).ok_or_else(|| bad(CMD, format!("no caption track {v}")))?)
    } else {
        None
    };
    let japanese = r.lines.iter().any(|l| l.text.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c)));
    let font = s.default_text_font();
    let speakers: std::collections::BTreeSet<&str> = r.lines.iter().map(|l| l.voice.speaker.as_str()).collect();
    let dialogue = speakers.len() > 1;
    let plan: Vec<(ItemId, Tick, Tick, Vec<(String, Tick, Tick)>, String)> = r
        .lines
        .iter()
        .zip(&items)
        .zip(&starts)
        .map(|((l, item), st)| {
            let dur = l.info.duration();
            let caps = if captions {
                tts::split_caption(&l.text, Some(&l.query), l.info.seconds(), max_chars)
                    .into_iter()
                    .map(|(t, a, b)| (t, *st + Tick::from_seconds_f64(a), *st + Tick::from_seconds_f64(b)))
                    .collect()
            } else {
                Vec::new()
            };
            (*item, *st, dur, caps, l.voice.speaker.clone())
        })
        .collect();
    let (clips, ctrack, caption_ids) = s.edit(UNDO_LABEL, |pr, st| {
        let mut clips = Vec::new();
        let mut made = Vec::new();
        for (item, start, dur, _, _) in &plan {
            let mut it = pr
                .make_track_item(*item, TrackKind::Audio, *start, TimeRange::new(Tick::ZERO, *dur), rate)
                .ok_or(EngineError::Other("the narration audio could not be imported".into()))?;
            it.duration = *dur;
            clips.push(it.id);
            made.push(it);
        }
        let ctrack_new = (captions && caption_track.is_none()).then(|| TrackId(pr.alloc_id()));
        let mut next = pr.next_id;
        let seq = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        if !seq.audio_tracks.iter().any(|t| t.id == track) {
            return Err(bad(CMD, "the audio track was deleted"));
        }
        let durations = |_: ItemId| -> Option<Tick> { None };
        let mut ctx =
            filmcraft_edit::EditCtx { next_id: &mut next, media_duration: &durations, media_start: &|_| Tick::ZERO, min_duration: rate.frame_duration() };
        for it in made {
            filmcraft_edit::overwrite(seq, vec![(track, it)], &mut ctx)?;
        }
        let mut caption_ids = Vec::new();
        let ctrack = match (ctrack_new, caption_track) {
            (Some(id), _) => {
                let mut t = CaptionTrack::new(id, "Narration".into(), CaptionFormat::Subtitle);
                t.language = if japanese { "ja".into() } else { "en".into() };
                t.style.font = font.clone();
                seq.caption_tracks.insert(0, t);
                Some(id)
            }
            (None, t) => t,
        };
        if let Some(tid) = ctrack {
            let min = rate.frame_duration();
            for (_, _, _, caps, who) in &plan {
                for (text, a, b) in caps {
                    let a = rate.snap_nearest(*a);
                    let dur = (rate.snap_nearest(*b) - a).max(min);
                    let id = filmcraft_edit::captions::add_caption(seq, tid, a, dur, text, &mut ctx).map_err(|e| {
                        bad(CMD, format!("caption \"{text}\": {e} (the caption track already has captions there; pass captions:false or another captionTrack)"))
                    })?;
                    if dialogue && let Some(c) = seq.caption_tracks.iter_mut().find(|t| t.id == tid).and_then(|t| t.captions.iter_mut().find(|c| c.id == id)) {
                        c.speaker = Some(who.clone());
                    }
                    caption_ids.push(id);
                }
            }
        }
        seq.check().map_err(EngineError::Other)?;
        pr.next_id = next;
        st.selection = clips.clone();
        Ok((clips, ctrack, caption_ids))
    })?;
    // sit the narration in the mix (nested commands, folded into the same undo step below)
    let span = TimeRange::from_bounds(starts.first().copied().unwrap_or(at), end);
    let mix = auto_mix(s, p, &clips, track, span);
    crate::clip_ops::collapse_history(s, n0, UNDO_LABEL);
    // cache what the engine said
    s.narration.voices.insert(r.engine, (web_time::Instant::now(), r.url.clone(), r.speakers.clone()));
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let credits = credits(&r);
    let credit_text = credits.iter().filter_map(|c| c["credit"].as_str()).collect::<Vec<_>>().join("\n");
    let lines: Vec<Value> = r
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            json!({
                "text": l.text,
                "voice": l.voice,
                "clip": clips.get(i).map(|c| c.0),
                "item": items.get(i).map(|c| c.0),
                "file": files.get(i),
                "start": starts.get(i).map(|t| t.0),
                "duration": l.info.duration().0,
                "seconds": l.info.seconds(),
                "sampleRate": l.info.sample_rate,
            })
        })
        .collect();
    Ok(json!({
        "engine": r.engine.id(),
        "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>(),
        "items": items.iter().map(|c| c.0).collect::<Vec<_>>(),
        "files": files,
        "dir": dir,
        "track": crate::mixer::strip_label(seq, track),
        "trackId": track.0,
        "captionTrack": ctrack.map(|t| t.0),
        "captions": caption_ids.iter().map(|c| c.0).collect::<Vec<_>>(),
        "start": at.0,
        "end": end.0,
        "lines": lines,
        "credits": credits,
        "creditText": credit_text,
        "mix": mix,
    }))
}

/// A dB parameter or preference clamped to `lo..=hi` (non-finite = `def`).
fn db_clamped(v: f64, lo: f64, hi: f64, def: f64) -> f64 {
    if v.is_finite() { v.clamp(lo, hi) } else { def }
}

/// Auto-level the placed narration and duck the other audio under it, with the Essential Sound
/// commands (`autoLevel` / `duck` / `duckReduceDb` / `dialogueLufs`, else Settings ▸ Narration):
///
/// 1. the narration clips get the Dialogue type (`essentialSound.setType`; the ducking listens
///    to it) and, with auto-level, their loudness is matched to the dialogue target
///    (`essentialSound.autoMatch`);
/// 2. with ducking, the enabled clips on other unlocked audio tracks that overlap the narration
///    and are Music or Ambience, or have no audio type (they become Music), get
///    `ducking.reduceDb` and Volume keyframes (`essentialSound.generateDucking`).
///
/// Never fails the placement: what could not be done is listed in `errors`.
fn auto_mix(s: &mut Session, p: &Value, clips: &[ClipId], track: TrackId, span: TimeRange) -> Value {
    let prefs = &s.prefs.narration;
    let level = bool_p(p, "autoLevel").unwrap_or(prefs.auto_level);
    let duck = bool_p(p, "duck").unwrap_or(prefs.duck);
    let reduce = db_clamped(f64_p(p, "duckReduceDb").unwrap_or(prefs.duck_reduce_db), -40.0, 0.0, DEFAULT_DUCK_DB);
    let dialogue_target = s.prefs.audio.loudness_target(AudioType::Dialogue);
    let target = f64_p(p, "dialogueLufs").map(|t| db_clamped(t, -60.0, 0.0, dialogue_target));
    let mut errors: Vec<Value> = Vec::new();
    let mut out = json!({"autoLevel": level, "duck": duck, "dialogue": [], "loudness": null, "ducked": [], "typedMusic": [], "reduceDb": null});
    if (!level && !duck) || clips.is_empty() {
        out["errors"] = json!(errors);
        return out;
    }
    let ids: Vec<u64> = clips.iter().map(|c| c.0).collect();
    // 1. Dialogue (also what the ducking listens to)
    match s.execute("essentialSound.setType", json!({"clips": ids, "type": "dialogue"})) {
        Ok(_) => out["dialogue"] = json!(ids),
        Err(e) => {
            errors.push(json!({"step": "dialogue", "error": e.to_string()}));
            out["errors"] = json!(errors);
            return out;
        }
    }
    if level {
        let mut mp = json!({"clips": ids});
        if let Some(t) = target {
            mp["target"] = json!(t);
        }
        match s.execute("essentialSound.autoMatch", mp) {
            Ok(r) => out["loudness"] = json!({"targetLufs": target.unwrap_or(dialogue_target), "clips": r["clips"]}),
            Err(e) => errors.push(json!({"step": "loudness", "error": e.to_string()})),
        }
    }
    // 2. ducking
    if duck {
        let mut music: Vec<u64> = Vec::new();
        let mut untyped: Vec<u64> = Vec::new();
        if let Some(seq) = s.active_sequence() {
            for t in seq.audio_tracks.iter().filter(|t| t.id != track && !t.locked) {
                for it in t.items.iter().filter(|i| i.enabled && !clips.contains(&i.id) && i.range().overlaps(&span)) {
                    match it.essential.as_ref().map(|e| e.kind) {
                        Some(AudioType::Music | AudioType::Ambience) => music.push(it.id.0),
                        None => untyped.push(it.id.0),
                        Some(_) => {}
                    }
                }
            }
        }
        if !untyped.is_empty() {
            match s.execute("essentialSound.setType", json!({"clips": untyped, "type": "music"})) {
                Ok(_) => {
                    out["typedMusic"] = json!(untyped);
                    music.extend(&untyped);
                }
                Err(e) => errors.push(json!({"step": "typeMusic", "error": e.to_string()})),
            }
        }
        if !music.is_empty() {
            out["reduceDb"] = json!(reduce);
            if let Err(e) = s.execute("essentialSound.set", json!({"clips": music, "key": "ducking.reduceDb", "value": reduce, "begin": true})) {
                errors.push(json!({"step": "reduceDb", "error": e.to_string()}));
            }
            match s.execute("essentialSound.generateDucking", json!({"clips": music})) {
                Ok(r) => {
                    let ducked: Vec<Value> = r["clips"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|c| json!({"clip": c["clip"], "keyframes": c["keyframes"].as_array().map_or(0, Vec::len), "regions": c["regions"]}))
                                .collect()
                        })
                        .unwrap_or_default();
                    out["ducked"] = json!(ducked);
                }
                Err(e) => errors.push(json!({"step": "ducking", "error": e.to_string()})),
            }
        }
    }
    out["errors"] = json!(errors);
    out
}

// ------------------------------------------------------------------------------------- commands

fn engines(s: &mut Session, _p: &Value) -> Result<Value> {
    let default = s.prefs.narration.default_engine();
    let list: Vec<Value> = TtsEngine::ALL
        .iter()
        .map(|&e| {
            let url = s.prefs.narration.url(e).to_string();
            let r = Client::new(e, &url).and_then(|c| c.version());
            json!({
                "id": e.id(),
                "name": e.name(),
                "url": url,
                "running": r.is_ok(),
                "version": r.as_ref().ok(),
                "error": r.as_ref().err().map(|e| e.to_string()),
                "default": e == default,
                "homeUrl": e.home_url(),
                "termsUrl": e.terms_url(),
            })
        })
        .collect();
    Ok(json!({"engines": list, "default": default.id()}))
}

fn voice_rows(e: TtsEngine, v: &[Speaker]) -> Vec<Value> {
    v.iter()
        .flat_map(|sp| {
            sp.styles.iter().map(move |st| json!({"engine": e.id(), "speaker": sp.name, "style": st.name, "id": st.id, "uuid": sp.uuid, "voice": format!("{}/{}", sp.name, st.name)}))
        })
        .collect()
}

fn list_voices(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.voices";
    let refresh = bool_p(p, "refresh").unwrap_or(false);
    let one = str_p(p, "engine").is_some() || str_p(p, "url").is_some();
    let engines: Vec<TtsEngine> = if one { vec![engine_p(s, p, CMD)?] } else { TtsEngine::ALL.to_vec() };
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for e in &engines {
        let c = if one { client_p(s, p, CMD)? } else { Client::new(*e, s.prefs.narration.url(*e)).map_err(tts_err)? };
        match voices(s, &c, refresh) {
            Ok(v) => rows.extend(voice_rows(*e, &v)),
            Err(err) if !one => errors.push(json!({"engine": e.id(), "error": err.to_string()})),
            Err(err) => return Err(err),
        }
    }
    if !one && rows.is_empty() {
        // nothing answers: the default engine's error
        let d = s.prefs.narration.default_engine();
        let c = Client::new(d, s.prefs.narration.url(d)).map_err(tts_err)?;
        return Err(voices(s, &c, true).err().unwrap_or_else(|| EngineError::Other(format!("{} has no voices", d.name()))));
    }
    Ok(json!({"voices": rows, "errors": errors}))
}

fn voice_terms(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.voiceTerms";
    let c = client_p(s, p, CMD)?;
    let speakers = voices(s, &c, false)?;
    let uuid = match str_p(p, "speaker_uuid").or(str_p(p, "uuid")) {
        Some(u) => u.to_string(),
        None => {
            let v = p.get("voice").cloned().ok_or_else(|| bad(CMD, "need `speaker_uuid` or `voice`"))?;
            resolve_voice(c.engine, &speakers, &v).map_err(|e| bad(CMD, e))?.uuid
        }
    };
    let sp = speakers.iter().find(|x| x.uuid == uuid);
    let info = c.speaker_info(&uuid).map_err(tts_err)?;
    let models = c.aivm_models().unwrap_or_default();
    let voice = Voice {
        engine: c.engine,
        speaker: sp.map(|x| x.name.clone()).unwrap_or_default(),
        style: sp.and_then(|x| x.styles.first()).map(|x| x.name.clone()).unwrap_or_default(),
        id: sp.and_then(|x| x.styles.first()).map_or(0, |x| x.id),
        uuid: uuid.clone(),
    };
    let credit = credit_json(c.engine, &voice, &models);
    Ok(json!({
        "engine": c.engine.id(),
        "speaker": voice.speaker,
        "uuid": uuid,
        "policy": info.policy,
        "credit": credit["credit"],
        "license": credit["license"],
        "model": credit["model"],
        "modelUrl": credit["modelUrl"],
        "termsUrl": c.engine.terms_url(),
        "note": credit["note"],
    }))
}

fn preview(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.preview";
    let mut job = prepare(s, p, CMD)?;
    job.lines.truncate(1);
    let r = render(&job).map_err(tts_err)?;
    s.narration.voices.insert(r.engine, (web_time::Instant::now(), r.url.clone(), r.speakers.clone()));
    let l = r.lines.first().ok_or_else(|| bad(CMD, "nothing to preview"))?;
    let dir = crate::temp_dir().join("FilmCraft Narration Preview");
    if !cfg!(target_arch = "wasm32") {
        let _ = std::fs::create_dir_all(&dir);
    }
    let names = Default::default();
    let path = free_path(s, &dir.to_string_lossy(), &format!("preview_{}", file_safe(&l.voice.label())), &names)?;
    s.services.write_file(&path, &l.wav).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"path": path, "seconds": l.info.seconds(), "voice": l.voice, "credit": credits(&r).first()}))
}

fn synthesize(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.synthesize";
    let token = u64_p(p, "prepared");
    let r = match (token, s.narration.prepared.take()) {
        (Some(t), Some(pr)) if pr.token == t => pr.rendered,
        (Some(_), other) => {
            s.narration.prepared = other;
            return Err(bad(CMD, "the prepared narration is gone; synthesize again"));
        }
        (None, other) => {
            s.narration.prepared = other;
            let job = prepare(s, p, CMD)?;
            render(&job).map_err(tts_err)?
        }
    };
    place(s, p, r)
}

fn spec(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    params: &'static str,
    enabled: fn(&Session) -> std::result::Result<(), String>,
    run: fn(&mut Session, &Value) -> Result<Value>,
    journal: bool,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: None, params, enabled, run, journal }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec("tts.engines", "Narration Engines", &[], "{}", always, engines, false),
        spec("tts.voices", "Narration Voices", &[], r#"{"engine":"aivisspeech|voicevox"?,"url":str?,"refresh":bool?}"#, always, list_voices, false),
        spec(
            "tts.voiceTerms",
            "Voice Terms",
            &[],
            r#"{"engine":"aivisspeech|voicevox"?,"speaker_uuid":str?,"voice":"Speaker/Style"|id?,"url":str?}"#,
            always,
            voice_terms,
            false,
        ),
        spec(
            "tts.preview",
            "Preview Narration Line",
            &[],
            r#"{"engine":"aivisspeech|voicevox"?,"text":str,"voice":"Speaker/Style"|id?,"speed":f64?,"pitch":f64?,"intonation":f64?,"volume":f64?,"tempoDynamics":f64?,"url":str?}"#,
            always,
            preview,
            false,
        ),
        spec(
            "tts.synthesize",
            "Generate Narration…",
            &["Graphics and Titles"],
            r#"{"engine":"aivisspeech|voicevox"?,"lines":[{"text":str,"voice":"Speaker/Style"|id|{speaker,style}?,"speed":f64?,"pitch":f64?,"intonation":f64?,"volume":f64?,"tempoDynamics":f64?,"pauseAfter":seconds?}]?,"script":"Speaker: line\n…"?,"cast":{"Speaker":"Voice/Style"|id}?,"text":str?,"voice":str|id?,"speed":f64?,"pause":seconds?,"track":"A2"|id?,"time":ticks?,"seconds":f64?,"captions":bool=true,"captionTrack":"C1"|id?,"captionMaxChars":n=40,"name":str="Narration","dir":str?,"url":str?,"autoLevel":bool=true,"dialogueLufs":f64?,"duck":bool=true,"duckReduceDb":f64=-15}"#,
            has_seq,
            synthesize,
            true,
        ),
    ]
}
