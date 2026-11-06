//! Text-to-speech commands (`speech.*`): spoken lines as WAV files and media items.
//!
//! | command | what it does |
//! |---|---|
//! | `speech.synthesize` | Speak `lines` (or one `text`) into WAV files in `out` (default `<data dir>/tts`) and import them (`import: false` keeps them as files only). One undo step. |
//! | `speech.synthesizer` | What would speak: the installed synthesizer, else the Qwen3-TTS helper script and Python found (`python`, `script`), with the script's `--probe` report (torch, CUDA, qwen-tts); with a local `engine`, whether it answers. |
//!
//! Synthesis goes through a [`Synthesizer`], picked by `engine` ([`Route`]):
//!
//! - `qwen3` (the default): [`Session::synthesizer`] when a host or a test installed one, else
//!   [`ExternalSynthesizer`] running `tools/qwen3-tts/filmcraft_tts.py` in a Python interpreter
//!   (FilmCraft links no Python; without it the command fails with the path it looked at). A
//!   line's voice is a reference recording (`refAudio`) and its transcript (`refText`), or a preset
//!   `speaker` with an `instruct` direction.
//! - `aivisspeech` / `voicevox`: [`EngineSynthesizer`], the local app's engine over HTTP at `url`
//!   (else Settings ▸ Narration's address, as `tts.synthesize` uses). A line's voice is `speaker`
//!   = `"Speaker/Style"` (or a style id); none: Settings ▸ Narration ▸ Default voice, else the
//!   engine's first voice. Imported items get the voice's credit like `tts.synthesize`'s
//!   ([`crate::narration_credits`]), so `tts.credits` and the credit roll cover them.
//!
//! See `docs/text-to-speech.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use filmcraft_speech::EngineSynthesizer;
use filmcraft_speech::synth::{ExternalSynthesizer, Line, Synthesizer, Voice};
use filmcraft_speech::tts::{Client, Prosody, TtsEngine};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, params: &'static str, enabled: Enabled, run: Run, journal: bool) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal }
}

/// Where spoken lines go by default: `<data dir>/tts` (next to the speech models).
pub fn tts_dir(s: &Session) -> Option<PathBuf> {
    crate::transcript::models_dir(s).and_then(|m| m.parent().map(|d| d.join("tts")))
}

/// A file name for a line id: its characters that are safe in a file name, else the index.
pub(crate) fn file_stem(id: &str, index: usize) -> String {
    let safe: String = id.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.')).take(64).collect();
    if safe.is_empty() || safe.chars().all(|c| c == '.') { format!("line-{index:04}") } else { safe }
}

/// The synthesizer to use: the installed one, else the external script from the params.
pub(crate) fn synthesizer(s: &Session, p: &Value) -> Arc<dyn Synthesizer> {
    if let Some(t) = &s.synthesizer {
        return t.clone();
    }
    let mut e = ExternalSynthesizer::discover(str_p(p, "python"), str_p(p, "script"));
    e.model = str_p(p, "model").map(str::to_string);
    e.device = str_p(p, "device").map(str::to_string);
    e.language = str_p(p, "language").map(str::to_string);
    Arc::new(e)
}

/// Which text-to-speech route speaks: Qwen3-TTS (voice cloning and preset speakers, through the
/// external script) or a local AivisSpeech / VOICEVOX app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Route {
    Qwen3,
    Local(TtsEngine),
}

impl Route {
    /// What speaks when nothing says otherwise (scripts written before local engines were a
    /// route keep speaking as they did).
    pub const DEFAULT: Route = Route::Qwen3;

    /// `qwen3` (also `qwen`, `qwen3-tts`), `aivisspeech` (`aivis`), `voicevox`, in any case;
    /// None for anything else.
    pub fn from_id(s: &str) -> Option<Route> {
        let k: String = s.trim().chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect::<String>().to_ascii_lowercase();
        match k.as_str() {
            "qwen3" | "qwen" | "qwen3tts" => Some(Route::Qwen3),
            _ => TtsEngine::from_id(s).map(Route::Local),
        }
    }

    /// The parameter value: `qwen3` | `aivisspeech` | `voicevox`.
    pub fn id(self) -> &'static str {
        match self {
            Route::Qwen3 => "qwen3",
            Route::Local(e) => e.id(),
        }
    }
}

/// The call's `engine` (missing or empty: [`Route::DEFAULT`]; unknown: an error).
pub(crate) fn route_p(p: &Value, cmd: &str) -> Result<Route> {
    match str_p(p, "engine").filter(|e| !e.trim().is_empty()) {
        None => Ok(Route::DEFAULT),
        Some(e) => Route::from_id(e).ok_or_else(|| bad(cmd, format!("unknown engine `{e}` (qwen3, aivisspeech or voicevox)"))),
    }
}

/// The synthesizer of `route`. A local engine is reached at the call's `url` when the call's own
/// `engine` is that engine, else at Settings ▸ Narration's address for it; the call's `speed`,
/// `pitch`, `intonation`, `volume`… apply to its lines.
pub(crate) fn synthesizer_for(s: &Session, p: &Value, route: Route, cmd: &str) -> Result<Arc<dyn Synthesizer>> {
    match route {
        Route::Qwen3 => Ok(synthesizer(s, p)),
        Route::Local(e) => {
            let own = route_p(p, cmd).ok() == Some(route);
            let url = str_p(p, "url").filter(|_| own).unwrap_or(s.prefs.narration.url(e));
            let client = Client::new(e, url).map_err(|err| bad(cmd, err.to_string()))?;
            let mut syn = EngineSynthesizer::new(client, s.prefs.narration.default_voice.clone());
            syn.prosody = Prosody::from_params(p, &Prosody::default());
            Ok(Arc::new(syn))
        }
    }
}

fn voice_of(p: &Value, fallback: &Voice) -> Voice {
    Voice {
        ref_audio: str_p(p, "refAudio").map(PathBuf::from).or_else(|| fallback.ref_audio.clone()),
        ref_text: str_p(p, "refText").map(str::to_string).or_else(|| fallback.ref_text.clone()),
        // `voice` is what `tts.synthesize` calls it
        speaker: str_p(p, "speaker").or_else(|| str_p(p, "voice")).map(str::to_string).or_else(|| fallback.speaker.clone()),
        instruct: str_p(p, "instruct").map(str::to_string).or_else(|| fallback.instruct.clone()),
    }
}

/// The lines a call asks for, with their output paths.
fn lines(p: &Value, out: &Path) -> Result<Vec<Line>> {
    let default_voice = voice_of(p, &Voice::default());
    let language = str_p(p, "language").map(str::to_string);
    let mut v = Vec::new();
    match p.get("lines").and_then(Value::as_array) {
        Some(a) => {
            if a.len() > filmcraft_speech::synth::MAX_LINES {
                return Err(bad("speech.synthesize", format!("more than {} lines", filmcraft_speech::synth::MAX_LINES)));
            }
            for (i, l) in a.iter().enumerate() {
                let text = str_p(l, "text").ok_or_else(|| bad("speech.synthesize", format!("lines[{i}]: `text` is required")))?;
                let id = match l.get("id") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Number(n)) => n.to_string(),
                    _ => (i + 1).to_string(),
                };
                v.push(Line {
                    out: out.join(format!("{}.wav", file_stem(&id, i + 1))),
                    id,
                    text: text.to_string(),
                    voice: voice_of(l, &default_voice),
                    language: str_p(l, "language").map(str::to_string).or_else(|| language.clone()),
                });
            }
        }
        None => {
            let text = str_p(p, "text").ok_or_else(|| bad("speech.synthesize", "`lines` or `text` is required"))?;
            let id = str_p(p, "id").unwrap_or("1").to_string();
            v.push(Line { out: out.join(format!("{}.wav", file_stem(&id, 1))), id, text: text.to_string(), voice: default_voice, language });
        }
    }
    let mut seen = std::collections::HashSet::new();
    for l in &v {
        if !seen.insert(l.out.clone()) {
            return Err(bad("speech.synthesize", format!("two lines would write {} (give them different ids)", l.out.display())));
        }
    }
    Ok(v)
}

fn synthesize(s: &mut Session, p: &Value) -> Result<Value> {
    let out = match str_p(p, "out") {
        Some(o) if !o.trim().is_empty() => PathBuf::from(o),
        _ => tts_dir(s).ok_or_else(|| EngineError::Other("no data directory for spoken lines; pass `out`".into()))?,
    };
    const CMD: &str = "speech.synthesize";
    let route = route_p(p, CMD)?;
    let lines = lines(p, &out)?;
    let synth = synthesizer_for(s, p, route, CMD)?;
    let outcome = synth.synthesize(&lines, &mut |_, _| true).map_err(|e| EngineError::Other(e.to_string()))?;
    let import = bool_p(p, "import").unwrap_or(true);
    let bin = u64_p(p, "bin");
    let n0 = s.history.undo.len();
    let mut spoken = Vec::new();
    let mut failed: Vec<Value> = outcome.failed.iter().map(|(id, e)| json!({"id": id, "error": e})).collect();
    let mut marks: Vec<(filmcraft_project::ItemId, Value)> = Vec::new();
    for sp in &outcome.spoken {
        let mut r = json!({"id": sp.id, "path": sp.path.to_string_lossy(), "seconds": sp.duration.seconds(), "sampleRate": sp.sample_rate});
        if let Some(c) = &sp.credit {
            r["credit"] = c.clone();
        }
        if import {
            let mut q = json!({"paths": [sp.path.to_string_lossy()]});
            if let Some(b) = bin {
                q["bin"] = json!(b);
            }
            match s.execute("file.import", q) {
                Ok(v) => match v["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64) {
                    Some(item) => {
                        r["item"] = json!(item);
                        if let Some(c) = &sp.credit {
                            marks.push((filmcraft_project::ItemId(item), c.clone()));
                        }
                    }
                    None => failed.push(json!({"id": sp.id, "error": format!("spoken to {} but not imported", sp.path.display())})),
                },
                Err(e) => failed.push(json!({"id": sp.id, "error": format!("spoken to {} but not imported: {e}", sp.path.display())})),
            }
        }
        spoken.push(r);
    }
    // each item remembers its voice's credit, as tts.synthesize's do
    if !marks.is_empty() {
        s.edit("Synthesize Speech", |pr, _| {
            for (item, c) in &marks {
                crate::narration_credits::mark_item(pr, *item, c);
            }
            Ok(())
        })?;
    }
    crate::clip_ops::collapse_history(s, n0, "Synthesize Speech");
    let credits = crate::narration_credits::distinct(outcome.spoken.iter().filter_map(|sp| sp.credit.as_ref()));
    Ok(json!({
        "engine": route.id(),
        "synthesizer": synth.id(),
        "out": out.to_string_lossy(),
        "lines": spoken,
        "failed": failed,
        "creditText": crate::narration_credits::credit_text(&credits),
        "credits": credits,
    }))
}

fn synthesizer_info(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "speech.synthesizer";
    if let Route::Local(e) = route_p(p, CMD)? {
        let url = str_p(p, "url").unwrap_or(s.prefs.narration.url(e));
        let client = Client::new(e, url).map_err(|err| bad(CMD, err.to_string()))?;
        let mut v = json!({"engine": e.id(), "id": e.id(), "url": client.base_url(), "out": tts_dir(s).map(|d| d.to_string_lossy().to_string())});
        match client.version() {
            Ok(ver) => {
                v["ok"] = json!(true);
                v["version"] = json!(ver);
            }
            Err(err) => {
                v["ok"] = json!(false);
                v["error"] = json!(err.to_string());
            }
        }
        return Ok(v);
    }
    if let Some(t) = &s.synthesizer {
        return Ok(json!({"installed": true, "id": t.id(), "ok": true}));
    }
    let e = ExternalSynthesizer::discover(str_p(p, "python"), str_p(p, "script"));
    let mut v = json!({
        "installed": false,
        "id": e.id(),
        "python": e.python.to_string_lossy(),
        "script": e.script.to_string_lossy(),
        "scriptFound": e.script_found(),
        "out": tts_dir(s).map(|d| d.to_string_lossy().to_string()),
    });
    let probe = if e.script_found() {
        e.probe().unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    } else {
        json!({"ok": false, "error": format!("the TTS script was not found at {} (see tools/qwen3-tts/README.md)", e.script.display())})
    };
    v["ok"] = probe["ok"].clone();
    v["probe"] = probe;
    Ok(v)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "speech.synthesize",
            "Synthesize Speech",
            r#"{"engine":"qwen3|aivisspeech|voicevox"="qwen3","url":str?,"lines":[{"id":str?,"text":str,"refAudio":str?,"refText":str?,"speaker":"Speaker/Style"|str?,"instruct":str?,"language":str?}]?,"text":str?,"id":str?,"refAudio":str?,"refText":str?,"speaker":"Speaker/Style"|str?,"instruct":str?,"language":str?,"speed":f64?,"pitch":f64?,"intonation":f64?,"volume":f64?,"out":str?,"import":bool?,"bin":binId?,"model":str?,"device":str?,"python":str?,"script":str?}"#,
            always,
            synthesize,
            true,
        ),
        spec(
            "speech.synthesizer",
            "Speech Synthesizer",
            r#"{"engine":"qwen3|aivisspeech|voicevox"="qwen3","url":str?,"python":str?,"script":str?}"#,
            always,
            synthesizer_info,
            false,
        ),
    ]
}
