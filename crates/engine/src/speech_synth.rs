//! Text-to-speech commands (`speech.*`): spoken lines as WAV files and media items.
//!
//! | command | what it does |
//! |---|---|
//! | `speech.synthesize` | Speak `lines` (or one `text`) into WAV files in `out` (default `<data dir>/tts`) and import them (`import: false` keeps them as files only). One undo step. |
//! | `speech.synthesizer` | What would speak: the installed synthesizer, else the Qwen3-TTS helper script and Python found (`python`, `script`), with the script's `--probe` report (torch, CUDA, qwen-tts). |
//!
//! Synthesis goes through a [`Synthesizer`]: [`Session::synthesizer`] when a host or a test
//! installed one, else [`ExternalSynthesizer`] running `tools/qwen3-tts/filmcraft_tts.py` in a
//! Python interpreter (FilmCraft links no Python; without it the command fails with the path it
//! looked at). A line's voice is a reference recording (`refAudio`) and its transcript (`refText`);
//! see `docs/text-to-speech.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use filmcraft_speech::synth::{ExternalSynthesizer, Line, Synthesizer, Voice};

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

fn voice_of(p: &Value, fallback: &Voice) -> Voice {
    Voice {
        ref_audio: str_p(p, "refAudio").map(PathBuf::from).or_else(|| fallback.ref_audio.clone()),
        ref_text: str_p(p, "refText").map(str::to_string).or_else(|| fallback.ref_text.clone()),
        speaker: str_p(p, "speaker").map(str::to_string).or_else(|| fallback.speaker.clone()),
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
    let lines = lines(p, &out)?;
    let synth = synthesizer(s, p);
    let outcome = synth.synthesize(&lines, &mut |_, _| true).map_err(|e| EngineError::Other(e.to_string()))?;
    let import = bool_p(p, "import").unwrap_or(true);
    let bin = u64_p(p, "bin");
    let n0 = s.history.undo.len();
    let mut spoken = Vec::new();
    let mut failed: Vec<Value> = outcome.failed.iter().map(|(id, e)| json!({"id": id, "error": e})).collect();
    for sp in &outcome.spoken {
        let mut r = json!({"id": sp.id, "path": sp.path.to_string_lossy(), "seconds": sp.duration.seconds(), "sampleRate": sp.sample_rate});
        if import {
            let mut q = json!({"paths": [sp.path.to_string_lossy()]});
            if let Some(b) = bin {
                q["bin"] = json!(b);
            }
            match s.execute("file.import", q) {
                Ok(v) => match v["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64) {
                    Some(item) => r["item"] = json!(item),
                    None => failed.push(json!({"id": sp.id, "error": format!("spoken to {} but not imported", sp.path.display())})),
                },
                Err(e) => failed.push(json!({"id": sp.id, "error": format!("spoken to {} but not imported: {e}", sp.path.display())})),
            }
        }
        spoken.push(r);
    }
    crate::clip_ops::collapse_history(s, n0, "Synthesize Speech");
    Ok(json!({"synthesizer": synth.id(), "out": out.to_string_lossy(), "lines": spoken, "failed": failed}))
}

fn synthesizer_info(s: &mut Session, p: &Value) -> Result<Value> {
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
            r#"{"lines":[{"id":str?,"text":str,"refAudio":str?,"refText":str?,"language":str?}]?,"text":str?,"id":str?,"refAudio":str?,"refText":str?,"language":str?,"out":str?,"import":bool?,"bin":binId?,"model":str?,"device":str?,"python":str?,"script":str?}"#,
            always,
            synthesize,
            true,
        ),
        spec("speech.synthesizer", "Speech Synthesizer", r#"{"python":str?,"script":str?}"#, always, synthesizer_info, false),
    ]
}
