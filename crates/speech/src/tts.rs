//! Text-to-speech through a local **AivisSpeech** or **VOICEVOX** app.
//!
//! Both apps run their own engine as a separate program that answers the same HTTP API on the
//! local machine (VOICEVOX ENGINE's documented API; AivisSpeech Engine is compatible with it):
//!
//! | request | answer |
//! |---|---|
//! | `GET /version` | the engine version (a JSON string) |
//! | `GET /speakers` | `[{name, speaker_uuid, styles: [{name, id, type?}], version}]`; a style `id` is the "speaker" number below |
//! | `GET /speaker_info?speaker_uuid=…` | `{policy, portrait, style_infos}`: `policy` is the voice's terms (Markdown) |
//! | `GET /aivm_models` (AivisSpeech) | the installed voice models with their manifest (name, licence, speakers) |
//! | `POST /audio_query?text=…&speaker=<id>` | an AudioQuery (accent phrases with mora timings, `speedScale`, `pitchScale`, …) |
//! | `POST /synthesis?speaker=<id>` + AudioQuery | `audio/wav` |
//!
//! FilmCraft only talks to the engine over HTTP: it never links, bundles or starts it (the engines
//! are separately licensed programs). Every answer is treated as hostile: bodies are capped
//! ([`MAX_JSON_BYTES`], [`MAX_WAV_BYTES`]), every call has a timeout, numbers are range-checked and
//! a WAV is parsed before it is used.
//!
//! The HTTP client exists with the `tts` feature on native targets; elsewhere (wasm, or a build
//! without the feature) every request fails with [`TtsError::Unavailable`].

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Largest JSON answer read (speaker lists with many models, AudioQueries of long texts).
pub const MAX_JSON_BYTES: u64 = 10 << 20;
/// Largest WAV read from `/synthesis`.
pub const MAX_WAV_BYTES: u64 = 200 << 20;
/// Longest text sent in one request (characters).
pub const MAX_TEXT_CHARS: usize = 2000;
/// Most speakers / styles read from `/speakers`.
const MAX_SPEAKERS: usize = 4096;
const MAX_STYLES: usize = 256;
/// Longest terms text kept (characters).
const MAX_POLICY_CHARS: usize = 200_000;

/// A text-to-speech engine FilmCraft can talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TtsEngine {
    #[serde(rename = "aivisspeech")]
    AivisSpeech,
    #[serde(rename = "voicevox")]
    Voicevox,
}

impl TtsEngine {
    pub const ALL: [TtsEngine; 2] = [TtsEngine::AivisSpeech, TtsEngine::Voicevox];

    /// Parameter / preference id: `aivisspeech` | `voicevox`.
    pub fn id(self) -> &'static str {
        match self {
            TtsEngine::AivisSpeech => "aivisspeech",
            TtsEngine::Voicevox => "voicevox",
        }
    }
    /// Product name.
    pub fn name(self) -> &'static str {
        match self {
            TtsEngine::AivisSpeech => "AivisSpeech",
            TtsEngine::Voicevox => "VOICEVOX",
        }
    }
    /// Where the app's engine listens by default.
    pub fn default_url(self) -> &'static str {
        match self {
            TtsEngine::AivisSpeech => "http://127.0.0.1:10101",
            TtsEngine::Voicevox => "http://127.0.0.1:50021",
        }
    }
    /// The app's home page (download and install instructions).
    pub fn home_url(self) -> &'static str {
        match self {
            TtsEngine::AivisSpeech => "https://aivis-project.com/",
            TtsEngine::Voicevox => "https://voicevox.hiroshiba.jp/",
        }
    }
    /// The app's general terms of use. Each voice (VOICEVOX character, AivisSpeech model) has its
    /// own terms on top: [`Client::speaker_info`]'s policy and the model licence.
    pub fn terms_url(self) -> &'static str {
        match self {
            TtsEngine::AivisSpeech => "https://aivis-project.com/",
            TtsEngine::Voicevox => "https://voicevox.hiroshiba.jp/term/",
        }
    }
    /// `aivisspeech`, `AivisSpeech`, `aivis`, `voicevox`, `VOICEVOX`…
    pub fn from_id(s: &str) -> Option<TtsEngine> {
        let k: String = s.trim().chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect::<String>().to_ascii_lowercase();
        match k.as_str() {
            "aivisspeech" | "aivis" => Some(TtsEngine::AivisSpeech),
            "voicevox" => Some(TtsEngine::Voicevox),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TtsError {
    /// Nothing answers at the engine's address.
    #[error("{engine} is not running at {url} — start the {engine} app")]
    NotRunning { engine: &'static str, url: String },
    /// The engine answered with an HTTP error.
    #[error("{engine} at {url} refused the request (HTTP {status}): {msg}")]
    Status { engine: &'static str, url: String, status: u16, msg: String },
    /// The answer could not be used (too large, not JSON, not a WAV…).
    #[error("{engine} sent an unusable answer: {msg}")]
    BadResponse { engine: &'static str, msg: String },
    /// This build cannot make HTTP requests (web, or built without the `tts` feature).
    #[error("narration is not available in this build ({0})")]
    Unavailable(String),
    /// A bad parameter (base URL, text…).
    #[error("{0}")]
    Invalid(String),
}

/// One voice style of a speaker; `id` is what `/audio_query` and `/synthesis` call `speaker`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub name: String,
    pub id: i64,
    /// `talk` (default), `singing_teacher`, `frame_decode`, `sing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// A speaker (character / model voice) and its styles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
    pub uuid: String,
    pub styles: Vec<Style>,
    #[serde(default)]
    pub version: String,
}

/// What `/speaker_info` says about a speaker (the portrait is not kept).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakerInfo {
    /// The voice's terms (Markdown, as the engine ships them).
    pub policy: String,
    pub has_portrait: bool,
}

/// An installed AivisSpeech voice model (`/aivm_models`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AivmModel {
    pub uuid: String,
    pub name: String,
    /// The model's licence text or name, when the manifest has one.
    pub license: Option<String>,
    /// Speaker uuids the model provides.
    pub speakers: Vec<String>,
}

impl AivmModel {
    /// The model's page on AivisHub (where its licence is shown).
    pub fn hub_url(&self) -> String {
        format!("https://hub.aivis-project.com/aivm-models/{}", self.uuid)
    }
}

/// Prosody overrides applied to an AudioQuery (None = the engine's value).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prosody {
    /// `speedScale`, 0.5–2.
    pub speed: Option<f64>,
    /// `pitchScale`, −0.15–0.15 (AivisSpeech: changing it can lower quality).
    pub pitch: Option<f64>,
    /// `intonationScale`, 0–2 (AivisSpeech: the strength of the style's emotion).
    pub intonation: Option<f64>,
    /// `volumeScale`, 0–2.
    pub volume: Option<f64>,
    /// `tempoDynamicsScale`, 0–2 (AivisSpeech only).
    pub tempo_dynamics: Option<f64>,
    /// `prePhonemeLength` / `postPhonemeLength` seconds, 0–1.5.
    pub pre_phoneme: Option<f64>,
    pub post_phoneme: Option<f64>,
}

impl Prosody {
    /// Overrides from command parameters (`speed`, `pitch`, `intonation`, `volume`,
    /// `tempoDynamics`, `prePhoneme`, `postPhoneme`); missing keys fall back to `base`.
    pub fn from_params(p: &Value, base: &Prosody) -> Prosody {
        let f = |k: &str, b: Option<f64>| p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).or(b);
        Prosody {
            speed: f("speed", base.speed),
            pitch: f("pitch", base.pitch),
            intonation: f("intonation", base.intonation),
            volume: f("volume", base.volume),
            tempo_dynamics: f("tempoDynamics", base.tempo_dynamics),
            pre_phoneme: f("prePhoneme", base.pre_phoneme),
            post_phoneme: f("postPhoneme", base.post_phoneme),
        }
    }
}

/// Set the overrides of `p` on `query` (clamped to the engines' ranges; NaN ignored).
pub fn apply_prosody(query: &mut Value, p: &Prosody, engine: TtsEngine) {
    let Some(m) = query.as_object_mut() else { return };
    let mut set = |k: &str, v: Option<f64>, lo: f64, hi: f64| {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            m.insert(k.to_string(), json!(v.clamp(lo, hi)));
        }
    };
    set("speedScale", p.speed, 0.5, 2.0);
    set("pitchScale", p.pitch, -0.15, 0.15);
    set("intonationScale", p.intonation, 0.0, 2.0);
    set("volumeScale", p.volume, 0.0, 2.0);
    set("prePhonemeLength", p.pre_phoneme, 0.0, 1.5);
    set("postPhonemeLength", p.post_phoneme, 0.0, 1.5);
    if engine == TtsEngine::AivisSpeech {
        set("tempoDynamicsScale", p.tempo_dynamics, 0.0, 2.0);
    }
}

// ------------------------------------------------------------------------------------- WAV

/// The format and length of a PCM / float WAV.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    /// Sample frames in the data chunk.
    pub frames: u64,
}

impl WavInfo {
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.sample_rate.max(1))
    }
    /// The exact length in ticks.
    pub fn duration(&self) -> filmcraft_time::Tick {
        filmcraft_time::Tick::from_units(i64::try_from(self.frames).unwrap_or(i64::MAX / 2), i64::from(self.sample_rate.max(1)))
    }
}

/// Parse the header of a RIFF/WAVE file (PCM or float). None for anything else, an empty or
/// impossible format, or no audio.
pub fn wav_info(b: &[u8]) -> Option<WavInfo> {
    if b.get(0..4)? != b"RIFF" || b.get(8..12)? != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| b.get(i..i.checked_add(2)?).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |i: usize| b.get(i..i.checked_add(4)?).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    // a WAV has a handful of chunks; a corrupt one cannot make this loop long
    for _ in 0..1024 {
        let id = b.get(pos..pos.checked_add(4)?)?;
        let len = u32_at(pos + 4)? as usize;
        let body = pos.checked_add(8)?;
        if id == b"fmt " {
            let tag = u16_at(body)?;
            let channels = u16_at(body + 2)?;
            let rate = u32_at(body + 4)?;
            let bits = u16_at(body + 14)?;
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = fmt?;
            // 1 = PCM, 3 = float, 0xFFFE = extensible
            if !matches!(tag, 1 | 3 | 0xFFFE) || channels == 0 || channels > 64 || rate == 0 || rate > 768_000 || !matches!(bits, 8 | 16 | 24 | 32 | 64) {
                return None;
            }
            // streaming writers leave the size at 0 / 0xFFFFFFFF: count what is there
            let avail = b.len().saturating_sub(body);
            let len = if len == 0 || len > avail { avail } else { len };
            let frame = usize::from(channels).checked_mul(usize::from(bits / 8))?;
            let frames = (len / frame.max(1)) as u64;
            if frames == 0 {
                return None;
            }
            return Some(WavInfo { sample_rate: rate, channels, bits, frames });
        }
        pos = body.checked_add(len)?.checked_add(len & 1)?;
    }
    None
}

// ------------------------------------------------------------------------------------- timing

/// Seconds into the synthesized audio of each pause (`pause_mora`: the engine's 、 and 。 breaks)
/// and the total length the AudioQuery predicts, from its mora timings. None when the engine does
/// not report timings (AivisSpeech's are all zero) or the query is not one.
pub fn pause_times(query: &Value) -> Option<(Vec<f64>, f64)> {
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0 && *x < 60.0).unwrap_or(0.0);
    let speed = query.get("speedScale").and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.1).unwrap_or(1.0);
    let mut t = num(query, "prePhonemeLength");
    let mut spoken = 0.0;
    let mut pauses = Vec::new();
    let phrases = query.get("accent_phrases")?.as_array()?;
    for ph in phrases.iter().take(10_000) {
        for m in ph.get("moras").and_then(Value::as_array).into_iter().flatten().take(10_000) {
            let d = (num(m, "consonant_length") + num(m, "vowel_length")) / speed;
            t += d;
            spoken += d;
        }
        if let Some(pm) = ph.get("pause_mora").filter(|v| v.is_object()) {
            let d = num(pm, "vowel_length") / speed;
            pauses.push(t + d / 2.0);
            t += d;
        }
    }
    if spoken <= 0.0 {
        return None;
    }
    Some((pauses, t + num(query, "postPhonemeLength")))
}

/// Characters a caption may break after.
fn is_break(c: char) -> bool {
    matches!(c, '、' | '。' | '，' | '．' | ',' | '！' | '？' | '!' | '?' | '…' | '‥')
}

/// Split one narration line into captions of at most `max_chars` characters (0 = never split),
/// breaking after punctuation. Returns (text, start seconds, end seconds) covering
/// 0..`duration`. Break times come from the AudioQuery's pauses when they match the punctuation,
/// else from the characters' share of the line.
pub fn split_caption(text: &str, query: Option<&Value>, duration: f64, max_chars: usize) -> Vec<(String, f64, f64)> {
    let duration = if duration.is_finite() { duration.max(0.0) } else { 0.0 };
    let whole = || vec![(text.trim().to_string(), 0.0, duration)];
    if max_chars == 0 || text.chars().count() <= max_chars {
        return whole();
    }
    // pieces end after a run of punctuation
    let mut pieces: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if is_break(c) && !chars.peek().is_some_and(|n| is_break(*n)) {
            pieces.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        pieces.push(cur);
    } else if let Some(last) = pieces.last_mut() {
        last.push_str(&cur);
    }
    if pieces.len() < 2 {
        return whole();
    }
    // the time each piece ends
    let counts: Vec<usize> = pieces.iter().map(|p| p.chars().filter(|c| !c.is_whitespace()).count().max(1)).collect();
    let total: usize = counts.iter().sum();
    let mut ends: Vec<f64> = Vec::with_capacity(pieces.len());
    let timed = query.and_then(pause_times).filter(|(p, _)| p.len() + 1 == pieces.len());
    match timed {
        Some((pauses, predicted)) if predicted > 0.0 => {
            let k = duration / predicted;
            ends.extend(pauses.iter().map(|t| (t * k).clamp(0.0, duration)));
        }
        _ => {
            let mut acc = 0usize;
            for c in counts.iter().take(pieces.len() - 1) {
                acc += c;
                ends.push(duration * acc as f64 / total.max(1) as f64);
            }
        }
    }
    ends.push(duration);
    // join pieces while they fit
    let mut out: Vec<(String, f64, f64)> = Vec::new();
    let mut start = 0.0;
    let mut acc = String::new();
    for (i, p) in pieces.iter().enumerate() {
        let end = ends.get(i).copied().unwrap_or(duration).max(start);
        if !acc.is_empty() && acc.chars().count() + p.chars().count() > max_chars {
            let prev_end = ends.get(i.saturating_sub(1)).copied().unwrap_or(start).max(start);
            out.push((acc.trim().to_string(), start, prev_end));
            start = prev_end;
            acc.clear();
        }
        acc.push_str(p);
        if i + 1 == pieces.len() {
            out.push((acc.trim().to_string(), start, end.max(start)));
        }
    }
    out.retain(|(t, _, _)| !t.is_empty());
    if out.is_empty() { whole() } else { out }
}

// ------------------------------------------------------------------------------------- client

/// A client for one engine at one base URL.
#[derive(Clone, Debug)]
pub struct Client {
    pub engine: TtsEngine,
    base: String,
}

/// How long a quick question (`/version`) may take.
const QUICK: Duration = Duration::from_secs(3);
/// Lists and AudioQueries.
const NORMAL: Duration = Duration::from_secs(30);
/// Synthesis of a long line on a slow CPU.
const SLOW: Duration = Duration::from_secs(300);

impl Client {
    /// `base_url` like `http://127.0.0.1:50021` (a trailing `/` is dropped). Only plain `http://`
    /// is accepted: the engines serve their API on the local machine.
    pub fn new(engine: TtsEngine, base_url: &str) -> Result<Client, TtsError> {
        let base = base_url.trim().trim_end_matches('/');
        let base = if base.is_empty() { engine.default_url() } else { base };
        let ok = base.len() <= 2048
            && base.strip_prefix("http://").is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
            && !base.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#'));
        if !ok {
            return Err(TtsError::Invalid(format!("{} address must look like {} (got `{base_url}`)", engine.name(), engine.default_url())));
        }
        Ok(Client { engine, base: base.to_string() })
    }
    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn not_running(&self) -> TtsError {
        TtsError::NotRunning { engine: self.engine.name(), url: self.base.clone() }
    }

    fn bad(&self, msg: impl Into<String>) -> TtsError {
        TtsError::BadResponse { engine: self.engine.name(), msg: msg.into() }
    }

    fn call(&self, req: http::Req) -> Result<Vec<u8>, TtsError> {
        match http::send(&req) {
            Ok((status, body)) if (200..300).contains(&status) => Ok(body),
            Ok((status, body)) => {
                let text = String::from_utf8_lossy(&body[..body.len().min(2000)]).into_owned();
                // FastAPI: {"detail": "..."} or {"detail": [{"msg": ...}]}
                let msg = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("detail").map(|d| d.as_str().map(str::to_string).unwrap_or_else(|| d.to_string())))
                    .unwrap_or(text);
                Err(TtsError::Status { engine: self.engine.name(), url: self.base.clone(), status, msg: msg.chars().take(500).collect() })
            }
            Err(http::Fail::Connect) => Err(self.not_running()),
            Err(http::Fail::TooLarge) => Err(self.bad(format!("the answer is larger than {} MB", req.max >> 20))),
            Err(http::Fail::Unavailable(why)) => Err(TtsError::Unavailable(why)),
            Err(http::Fail::Other(e)) => Err(self.bad(e)),
        }
    }

    fn json(&self, req: http::Req) -> Result<Value, TtsError> {
        let b = self.call(req)?;
        serde_json::from_slice(&b).map_err(|e| self.bad(format!("not JSON ({e})")))
    }

    fn get(&self, path: &str, query: Vec<(&'static str, String)>, timeout: Duration) -> http::Req {
        http::Req { url: self.url(path), query, body: None, timeout, max: MAX_JSON_BYTES }
    }

    /// `GET /version`: the engine's version.
    pub fn version(&self) -> Result<String, TtsError> {
        let b = self.call(self.get("/version", vec![], QUICK))?;
        let v = match serde_json::from_slice::<Value>(&b) {
            Ok(Value::String(s)) => s,
            Ok(other) => other.to_string(),
            Err(_) => String::from_utf8_lossy(&b).trim().to_string(),
        };
        Ok(v.chars().take(64).collect())
    }

    /// `GET /speakers`: every speaker with its styles (singing-only styles are left out).
    pub fn speakers(&self) -> Result<Vec<Speaker>, TtsError> {
        let v = self.json(self.get("/speakers", vec![], NORMAL))?;
        parse_speakers(&v).ok_or_else(|| self.bad("the speaker list is not a list of speakers"))
    }

    /// `GET /speaker_info?speaker_uuid=…`: the voice's terms.
    pub fn speaker_info(&self, uuid: &str) -> Result<SpeakerInfo, TtsError> {
        if uuid.is_empty() || uuid.len() > 128 {
            return Err(TtsError::Invalid("need a speaker uuid".into()));
        }
        let v = self.json(self.get("/speaker_info", vec![("speaker_uuid", uuid.to_string())], NORMAL))?;
        if !v.is_object() {
            return Err(self.bad("speaker_info is not an object"));
        }
        Ok(SpeakerInfo {
            policy: v.get("policy").and_then(Value::as_str).unwrap_or_default().chars().take(MAX_POLICY_CHARS).collect(),
            has_portrait: v.get("portrait").and_then(Value::as_str).is_some_and(|p| !p.is_empty()),
        })
    }

    /// `GET /aivm_models` (AivisSpeech): installed models with licence and speakers. Empty for
    /// VOICEVOX, or an engine without the endpoint.
    pub fn aivm_models(&self) -> Result<Vec<AivmModel>, TtsError> {
        if self.engine != TtsEngine::AivisSpeech {
            return Ok(Vec::new());
        }
        match self.json(self.get("/aivm_models", vec![], NORMAL)) {
            Ok(v) => Ok(parse_aivm_models(&v)),
            Err(TtsError::Status { status: 404 | 405 | 501, .. }) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// `POST /audio_query?text=…&speaker=<style>`.
    pub fn audio_query(&self, text: &str, style: i64) -> Result<Value, TtsError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(TtsError::Invalid("the text is empty".into()));
        }
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(TtsError::Invalid(format!("a line can have at most {MAX_TEXT_CHARS} characters")));
        }
        let req = http::Req {
            url: self.url("/audio_query"),
            query: vec![("text", text.to_string()), ("speaker", style.to_string())],
            body: Some(Vec::new()),
            timeout: NORMAL,
            max: MAX_JSON_BYTES,
        };
        let v = self.json(req)?;
        if !v.get("accent_phrases").is_some_and(Value::is_array) {
            return Err(self.bad("the audio query has no accent_phrases"));
        }
        Ok(v)
    }

    /// `POST /synthesis?speaker=<style>` with `query`: the WAV and its format.
    pub fn synthesis(&self, query: &Value, style: i64) -> Result<(Vec<u8>, WavInfo), TtsError> {
        let body = serde_json::to_vec(query).map_err(|e| TtsError::Invalid(e.to_string()))?;
        let req = http::Req { url: self.url("/synthesis"), query: vec![("speaker", style.to_string())], body: Some(body), timeout: SLOW, max: MAX_WAV_BYTES };
        let wav = self.call(req)?;
        let info = wav_info(&wav).ok_or_else(|| self.bad("the synthesized audio is not a WAV file"))?;
        Ok((wav, info))
    }
}

/// The `/speakers` list; None when it is not a list.
pub fn parse_speakers(v: &Value) -> Option<Vec<Speaker>> {
    let a = v.as_array()?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().chars().take(200).collect::<String>();
    let out = a
        .iter()
        .take(MAX_SPEAKERS)
        .filter_map(|sp| {
            let styles: Vec<Style> = sp
                .get("styles")?
                .as_array()?
                .iter()
                .take(MAX_STYLES)
                .filter_map(|st| {
                    let id = st.get("id")?.as_i64()?;
                    let kind = st.get("type").and_then(Value::as_str).map(str::to_string);
                    // singing voices cannot read text
                    if kind.as_deref().is_some_and(|k| k != "talk") {
                        return None;
                    }
                    Some(Style { name: s(st, "name"), id, kind })
                })
                .collect();
            let name = s(sp, "name");
            (!styles.is_empty() && !name.is_empty()).then(|| Speaker { name, uuid: s(sp, "speaker_uuid"), styles, version: s(sp, "version") })
        })
        .collect();
    Some(out)
}

/// The `/aivm_models` answer: an object keyed by model uuid (or a list) of `{manifest: {uuid,
/// name, license?, speakers: [{uuid}]}, speakers: [{speaker: {speaker_uuid}}]}`. Anything it does
/// not recognise is skipped.
pub fn parse_aivm_models(v: &Value) -> Vec<AivmModel> {
    let entries: Vec<(&str, &Value)> = match v {
        Value::Object(m) => m.iter().map(|(k, v)| (k.as_str(), v)).collect(),
        Value::Array(a) => a.iter().map(|v| ("", v)).collect(),
        _ => return Vec::new(),
    };
    let s = |v: Option<&Value>| v.and_then(Value::as_str).map(|x| x.chars().take(20_000).collect::<String>());
    entries
        .into_iter()
        .take(1024)
        .filter_map(|(key, e)| {
            let man = e.get("manifest").unwrap_or(e);
            let uuid = s(man.get("uuid")).or_else(|| s(e.get("uuid"))).unwrap_or_else(|| key.to_string());
            let name = s(man.get("name")).or_else(|| s(e.get("name"))).unwrap_or_default();
            let license = s(man.get("license")).or_else(|| s(e.get("license"))).filter(|l| !l.trim().is_empty());
            let mut speakers: Vec<String> = Vec::new();
            for sp in man.get("speakers").and_then(Value::as_array).into_iter().flatten().take(256) {
                if let Some(u) = s(sp.get("uuid")) {
                    speakers.push(u);
                }
            }
            for sp in e.get("speakers").and_then(Value::as_array).into_iter().flatten().take(256) {
                if let Some(u) = s(sp.get("speaker").and_then(|x| x.get("speaker_uuid")).or(sp.get("speaker_uuid")))
                    && !speakers.contains(&u)
                {
                    speakers.push(u);
                }
            }
            (!uuid.is_empty()).then_some(AivmModel { uuid, name, license, speakers })
        })
        .collect()
}

// ------------------------------------------------------------------------------------- HTTP

mod http {
    use std::time::Duration;

    #[allow(dead_code)] // unread where the HTTP client is compiled out
    pub struct Req {
        pub url: String,
        pub query: Vec<(&'static str, String)>,
        /// Some = POST (JSON when not empty).
        pub body: Option<Vec<u8>>,
        pub timeout: Duration,
        pub max: u64,
    }

    #[allow(dead_code)] // which variants are built depends on the feature and target
    pub enum Fail {
        /// Nothing listens there (refused, unreachable, timed out connecting).
        Connect,
        TooLarge,
        Unavailable(String),
        Other(String),
    }

    #[cfg(all(feature = "tts", not(target_arch = "wasm32")))]
    pub fn send(r: &Req) -> Result<(u16, Vec<u8>), Fail> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // the engine is on this machine: never through a proxy from the environment
            .proxy(None)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(2)))
            .timeout_global(Some(r.timeout))
            .user_agent(concat!("FilmCraft/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let result = match &r.body {
            None => {
                let mut q = agent.get(&r.url);
                for (k, v) in &r.query {
                    q = q.query(*k, v);
                }
                q.call()
            }
            Some(body) => {
                let mut q = agent.post(&r.url);
                for (k, v) in &r.query {
                    q = q.query(*k, v);
                }
                if body.is_empty() { q.send_empty() } else { q.content_type("application/json").send(&body[..]) }
            }
        };
        let mut resp = result.map_err(map_err)?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(r.max).read_to_vec().map_err(map_err)?;
        Ok((status, body))
    }

    #[cfg(all(feature = "tts", not(target_arch = "wasm32")))]
    fn map_err(e: ureq::Error) -> Fail {
        use std::io::ErrorKind;
        match e {
            ureq::Error::BodyExceedsLimit(_) => Fail::TooLarge,
            ureq::Error::ConnectionFailed | ureq::Error::HostNotFound | ureq::Error::Timeout(ureq::Timeout::Connect | ureq::Timeout::Resolve) => Fail::Connect,
            ureq::Error::Io(ref io) if matches!(io.kind(), ErrorKind::ConnectionRefused | ErrorKind::NotConnected | ErrorKind::AddrNotAvailable) => {
                Fail::Connect
            }
            other => Fail::Other(other.to_string()),
        }
    }

    #[cfg(not(all(feature = "tts", not(target_arch = "wasm32"))))]
    pub fn send(_r: &Req) -> Result<(u16, Vec<u8>), Fail> {
        let why = if cfg!(target_arch = "wasm32") { "the web version cannot reach a local engine" } else { "built without the `tts` feature" };
        Err(Fail::Unavailable(why.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(frames: usize, rate: u32) -> Vec<u8> {
        let data = frames * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&((36 + data) as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data as u32).to_le_bytes());
        v.resize(44 + data, 0);
        v
    }

    #[test]
    fn wav_header_and_hostile_bytes() {
        let w = wav(24_000, 24_000);
        let i = wav_info(&w).unwrap();
        assert_eq!((i.sample_rate, i.channels, i.frames), (24_000, 1, 24_000));
        assert!((i.seconds() - 1.0).abs() < 1e-9);
        assert!(wav_info(b"not a wav").is_none());
        assert!(wav_info(b"").is_none());
        // every truncation and some corruptions: never a panic
        for n in 0..w.len().min(64) {
            let _ = wav_info(&w[..n]);
        }
        let mut c = w.clone();
        c[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(wav_info(&c).is_none());
        let mut z = w.clone();
        z[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(wav_info(&z).is_none(), "zero sample rate");
    }

    #[test]
    fn engine_ids_and_urls() {
        assert_eq!(TtsEngine::from_id("AivisSpeech"), Some(TtsEngine::AivisSpeech));
        assert_eq!(TtsEngine::from_id("voice-vox"), Some(TtsEngine::Voicevox));
        assert_eq!(TtsEngine::from_id("sapi"), None);
        assert!(Client::new(TtsEngine::Voicevox, "").unwrap().base_url().ends_with(":50021"));
        assert_eq!(Client::new(TtsEngine::Voicevox, "http://127.0.0.1:1/").unwrap().base_url(), "http://127.0.0.1:1");
        assert!(Client::new(TtsEngine::Voicevox, "https://example.com").is_err());
        assert!(Client::new(TtsEngine::Voicevox, "http://a b").is_err());
        assert!(Client::new(TtsEngine::Voicevox, "file:///etc").is_err());
    }

    #[test]
    fn prosody_is_clamped() {
        let mut q = json!({"speedScale": 1.0, "accent_phrases": []});
        let p = Prosody { speed: Some(9.0), pitch: Some(f64::NAN), volume: Some(-1.0), tempo_dynamics: Some(1.5), ..Default::default() };
        apply_prosody(&mut q, &p, TtsEngine::Voicevox);
        assert_eq!(q["speedScale"], 2.0);
        assert_eq!(q["volumeScale"], 0.0);
        assert!(q.get("pitchScale").is_none());
        assert!(q.get("tempoDynamicsScale").is_none(), "VOICEVOX has no tempo dynamics");
        apply_prosody(&mut q, &p, TtsEngine::AivisSpeech);
        assert_eq!(q["tempoDynamicsScale"], 1.5);
        apply_prosody(&mut Value::Null, &p, TtsEngine::AivisSpeech);
    }

    #[test]
    fn captions_split_at_pauses_or_by_characters() {
        let mora = |c: f64, v: f64| json!({"text": "ア", "consonant_length": c, "vowel_length": v, "pitch": 5.0});
        let q = json!({
            "speedScale": 1.0, "prePhonemeLength": 0.1, "postPhonemeLength": 0.1,
            "accent_phrases": [
                {"moras": [mora(0.05, 0.15), mora(0.0, 0.2)], "pause_mora": {"vowel_length": 0.4}},
                {"moras": [mora(0.1, 0.1), mora(0.1, 0.1), mora(0.0, 0.2), mora(0.1, 0.1)], "pause_mora": null}
            ]
        });
        let (pauses, total) = pause_times(&q).unwrap();
        assert_eq!(pauses.len(), 1);
        assert!((pauses[0] - 0.7).abs() < 1e-9, "{pauses:?}");
        assert!((total - 1.8).abs() < 1e-9);
        let text = "今日はいい天気ですね。散歩に行きましょう。";
        let c = split_caption(text, Some(&q), 3.6, 12);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(c[0].0, "今日はいい天気ですね。");
        assert!((c[0].2 - 1.4).abs() < 1e-9 && c[1].1 == c[0].2 && c[1].2 == 3.6, "{c:?}");
        // AivisSpeech reports no timings: by characters
        let c = split_caption(text, Some(&json!({"accent_phrases": []})), 2.0, 12);
        assert_eq!(c.len(), 2);
        assert!((c[0].2 - 2.0 * 11.0 / 21.0).abs() < 1e-9, "{c:?}");
        // short lines and max 0 stay whole; hostile durations do not panic
        assert_eq!(split_caption("短い", None, 1.0, 40).len(), 1);
        assert_eq!(split_caption(text, None, 1.0, 0).len(), 1);
        assert!(!split_caption("。。。、、", None, f64::NAN, 1).is_empty());
        let _ = pause_times(&json!({"accent_phrases": "x"}));
    }

    #[test]
    fn speaker_lists_from_hostile_json() {
        let v = json!([
            {"name": "ずんだもん", "speaker_uuid": "u1", "styles": [{"name": "ノーマル", "id": 3}, {"name": "歌", "id": 9, "type": "sing"}], "version": "0.1"},
            {"name": "", "styles": [{"id": 1}]},
            {"name": "no styles"},
            7
        ]);
        let s = parse_speakers(&v).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].styles.len(), 1, "singing styles are left out");
        assert!(parse_speakers(&json!({"x": 1})).is_none());
        let m = parse_aivm_models(&json!({"m1": {"manifest": {"uuid": "m1", "name": "Anneli", "license": "ACML 1.0", "speakers": [{"uuid": "s1"}]}}}));
        assert_eq!(m[0].license.as_deref(), Some("ACML 1.0"));
        assert_eq!(m[0].speakers, ["s1"]);
        assert!(parse_aivm_models(&json!(3)).is_empty());
    }
}
