//! Script commands (`script.*`, `lipsync.analyze`): the automatable steps of the novel / game
//! to anime pipeline (`docs/script.md`). They read a standalone script file
//! ([`filmcraft_project::script`]) and change nothing in the project, so they are queries: results
//! are files beside the script's other outputs, which makes every step repeatable and resumable.
//!
//! | command | what it does |
//! |---|---|
//! | `script.validate` | Check the script (ids, speakers, route, prerequisites, story time) and count what there is to do: lines, recorded, to synthesize. |
//! | `script.pickVoices` | Choose each character's reference recording from the lines that have a recording of the game's own (clean length, calm, text known), optionally writing it into the script. |
//! | `script.synthesize` | Speak every line that has no recording, in its character's voice, into `<out>/<id>.wav`. Lines whose text, voice and model did not change since the last run are not spoken again. |
//! | `script.lipsync` | Mouth shapes for every line that has audio, written to `<out>/lipsync.json`. |
//! | `lipsync.analyze` | Mouth shapes for one audio file. |

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_audio_dsp::lipsync as lip;
use filmcraft_project::script::{LineKind, Problem, Scene, Script, ScriptLine, Severity, VoiceRef};
use filmcraft_speech::synth::{Line, Voice};
use filmcraft_time::Tick;

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, str_p};
use crate::speech_synth::{file_stem, synthesizer, tts_dir};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

/// A recording shorter or longer than this is no use as a voice reference, seconds.
const REF_MIN: f64 = 2.5;
const REF_MAX: f64 = 12.0;
/// The length a reference is best at, seconds.
const REF_BEST: f64 = 6.0;

/// The spoken form of a text: ruby markup replaced by its readings (`｜漢字《かんじ》` → かんじ).
pub fn spoken_text(raw: &str) -> String {
    let parsed = filmcraft_text::ruby::parse(raw);
    let mut rubies: Vec<_> = parsed.rubies.iter().collect();
    rubies.sort_by_key(|r| r.base.start);
    let mut out = String::with_capacity(parsed.plain.len());
    let mut at = 0;
    for r in rubies {
        let (Some(before), Some(_)) = (parsed.plain.get(at..r.base.start), parsed.plain.get(r.base.clone())) else { continue };
        out.push_str(before);
        out.push_str(&r.text);
        at = r.base.end;
    }
    out.push_str(parsed.plain.get(at..).unwrap_or_default());
    out.trim().to_string()
}

/// What is said by a line, ready for a synthesizer: its `reading` if it has one, else its text.
fn line_speech(l: &ScriptLine) -> String {
    spoken_text(l.reading.as_deref().filter(|r| !r.trim().is_empty()).unwrap_or(&l.text))
}

struct Loaded {
    path: PathBuf,
    dir: PathBuf,
    script: Script,
    /// The file as JSON, so that rewriting it keeps fields other tools put there.
    raw: Value,
}

impl Loaded {
    /// A path from the script, relative to the script's folder.
    fn resolve(&self, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() { p.to_path_buf() } else { self.dir.join(p) }
    }
    /// The folder name of this script's outputs: its file name without `.fcscript.json`, and a
    /// short hash of its folder, so `season1/ep1.fcscript.json` and `season2/ep1.fcscript.json`
    /// (and `ep1.v2.json`) do not share audio.
    fn stem(&self) -> String {
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let base = name.strip_suffix(".fcscript.json").or_else(|| name.strip_suffix(".json")).unwrap_or(&name);
        let base = if base.trim().is_empty() { "script" } else { base };
        let dir = std::fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone());
        let short: String = line_hash(&[&dir.to_string_lossy()]).chars().take(8).collect();
        format!("{}-{short}", file_stem(base, 0))
    }
}

fn load(s: &Session, p: &Value, cmd: &str) -> Result<Loaded> {
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "`path` (the script file) is required"))?;
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    let script = Script::from_json(text).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let raw: Value = serde_json::from_str(text).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let path = PathBuf::from(path);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    Ok(Loaded { path, dir, script, raw })
}

fn out_dir(s: &Session, l: &Loaded, p: &Value) -> Result<PathBuf> {
    match str_p(p, "out") {
        Some(o) if !o.trim().is_empty() => Ok(PathBuf::from(o)),
        _ => tts_dir(s).map(|d| d.join(l.stem())).ok_or_else(|| EngineError::Other("no data directory for spoken lines; pass `out`".into())),
    }
}

fn problems_json(v: &[Problem]) -> Vec<Value> {
    v.iter().map(|p| serde_json::to_value(p).unwrap_or(Value::Null)).collect()
}

fn has_recording(l: &Loaded, line: &ScriptLine) -> bool {
    line.voice_file.as_deref().is_some_and(|f| l.resolve(f).is_file())
}

// ---------------------------------------------------------------------------------------------

fn validate(s: &mut Session, p: &Value) -> Result<Value> {
    let l = load(s, p, "script.validate")?;
    let problems = l.script.validate();
    let errors = problems.iter().filter(|x| x.severity == Severity::Error).count();
    let warnings = problems.len() - errors;
    let (mut lines, mut spoken, mut recorded, mut missing_files) = (0usize, 0usize, 0usize, 0usize);
    for (_, line) in l.script.lines_in_route() {
        lines += 1;
        if !line.is_spoken() {
            continue;
        }
        spoken += 1;
        match line.voice_file.as_deref() {
            Some(f) if l.resolve(f).is_file() => recorded += 1,
            Some(_) => missing_files += 1,
            None => {}
        }
    }
    Ok(json!({
        "ok": errors == 0,
        "errors": errors,
        "warnings": warnings,
        "problems": problems_json(&problems),
        "stats": {
            "characters": l.script.characters.len(),
            "scenes": l.script.scenes_in_route().len(),
            "lines": lines,
            "spoken": spoken,
            "recorded": recorded,
            "recordingMissing": missing_files,
            "toSynthesize": spoken - recorded,
        },
    }))
}

// ---------------------------------------------------------------------------------------------

/// How good a recording is as a voice reference (smaller is better): near [`REF_BEST`] seconds, a
/// calm line (no shouting or trailing off), and a plausible speaking rate.
fn reference_score(seconds: f64, text: &str) -> Option<f64> {
    if !(REF_MIN..=REF_MAX).contains(&seconds) {
        return None;
    }
    let chars = text.chars().filter(|c| !c.is_whitespace()).count() as f64;
    let rate = chars / seconds;
    if !(2.0..=12.0).contains(&rate) {
        return None;
    }
    let loud = text.chars().filter(|c| matches!(c, '！' | '!' | '？' | '?' | '…' | '―' | '—' | '、')).count() as f64;
    Some((seconds - REF_BEST).abs() + loud * 1.5 + (rate - 6.0).abs() * 0.2)
}

#[derive(Serialize)]
struct Pick {
    character: String,
    line: String,
    ref_audio: String,
    ref_text: String,
    seconds: f64,
    candidates: usize,
}

fn pick_voices(s: &mut Session, p: &Value) -> Result<Value> {
    let l = load(s, p, "script.pickVoices")?;
    let overwrite = bool_p(p, "overwrite").unwrap_or(false);
    let write = bool_p(p, "write").unwrap_or(false);
    // best candidate per character: (score, line id, file as written, text, seconds)
    let mut best: BTreeMap<String, (f64, String, String, String, f64)> = BTreeMap::new();
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    let mut unreadable = Vec::new();
    for (_, line) in l.script.lines_in_route() {
        let (Some(sp), Some(file)) = (line.speaker.as_deref(), line.voice_file.as_deref()) else { continue };
        if !matches!(line.kind, LineKind::Dialogue | LineKind::Monologue) || !line.is_spoken() {
            continue;
        }
        let path = l.resolve(file);
        let Ok(src) = s.media.open_file(&path.to_string_lossy(), &*s.services) else {
            unreadable.push(json!({"line": line.id, "file": file}));
            continue;
        };
        if !src.info().has_audio() {
            continue;
        }
        let seconds = src.info().duration.seconds();
        let text = line_speech(line);
        let Some(score) = reference_score(seconds, &text) else { continue };
        *count.entry(sp.to_string()).or_default() += 1;
        if best.get(sp).is_none_or(|b| score < b.0) {
            best.insert(sp.to_string(), (score, line.id.clone(), file.to_string(), text, seconds));
        }
    }
    let mut picks = Vec::new();
    let mut skipped = Vec::new();
    let mut script = l.script.clone();
    for (id, c) in &mut script.characters {
        if c.voice.is_some() && !overwrite {
            skipped.push(json!({"character": id, "reason": "already has a voice (pass overwrite)"}));
            continue;
        }
        match best.get(id) {
            Some((_, line, file, text, seconds)) => {
                picks.push(Pick {
                    character: id.clone(),
                    line: line.clone(),
                    ref_audio: file.clone(),
                    ref_text: text.clone(),
                    seconds: *seconds,
                    candidates: count.get(id).copied().unwrap_or(0),
                });
                c.voice = Some(VoiceRef { ref_audio: file.clone(), ref_text: Some(text.clone()), ..Default::default() });
            }
            None => skipped.push(json!({"character": id, "reason": format!("no recording of {REF_MIN}-{REF_MAX} s with a known text")})),
        }
    }
    if write && !picks.is_empty() {
        // patch the file's own JSON, so fields this build does not know (notes, page numbers...)
        // survive; the file is re-formatted
        let mut raw = l.raw.clone();
        for pk in &picks {
            let Some(c) = raw.get_mut("characters").and_then(|c| c.get_mut(&pk.character)).and_then(Value::as_object_mut) else { continue };
            let v = c.entry("voice").or_insert_with(|| json!({}));
            if !v.is_object() {
                *v = json!({});
            }
            if let Some(o) = v.as_object_mut() {
                o.insert("refAudio".into(), json!(pk.ref_audio));
                o.insert("refText".into(), json!(pk.ref_text));
            }
        }
        let text = serde_json::to_string_pretty(&raw).map_err(|e| EngineError::Other(e.to_string()))?;
        s.services.write_file(&l.path.to_string_lossy(), text.as_bytes()).map_err(|e| EngineError::Other(format!("{}: {e}", l.path.display())))?;
    }
    Ok(json!({"picked": picks, "skipped": skipped, "unreadable": unreadable, "written": write && !picks.is_empty()}))
}

// ---------------------------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    lines: BTreeMap<String, Entry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    hash: String,
    file: String,
    seconds: f64,
}

const MANIFEST: &str = "manifest.json";

/// Lines handed to the synthesizer at a time (one model load per chunk), and so the most a dead
/// run can lose. Below the synthesizer's own limit.
pub const SYNTH_CHUNK: usize = 400;

fn read_manifest(out: &Path) -> Manifest {
    std::fs::read_to_string(out.join(MANIFEST)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_manifest(out: &Path, m: &Manifest) -> Result<()> {
    std::fs::create_dir_all(out).map_err(|e| EngineError::Other(format!("{}: {e}", out.display())))?;
    let text = serde_json::to_string_pretty(m).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::write(out.join(MANIFEST), text).map_err(|e| EngineError::Other(format!("{}: {e}", out.join(MANIFEST).display())))
}

/// FNV-1a over the parts of a line that decide how it sounds (stable across runs and builds).
fn line_hash(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain(std::iter::once(0x1f)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

fn synthesize(s: &mut Session, p: &Value) -> Result<Value> {
    let l = load(s, p, "script.synthesize")?;
    let problems = l.script.validate();
    if let Some(e) = problems.iter().find(|x| x.severity == Severity::Error) {
        return Err(bad(
            "script.synthesize",
            format!("the script has errors (run script.validate): {}{}", e.line.as_deref().map(|i| format!("line {i}: ")).unwrap_or_default(), e.message),
        ));
    }
    let out = out_dir(s, &l, p)?;
    let force = bool_p(p, "force").unwrap_or(false);
    let all = bool_p(p, "all").unwrap_or(false);
    let only: Option<HashSet<String>> = p.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
    let language = str_p(p, "language").unwrap_or(&l.script.language).to_string();
    let model = str_p(p, "model").unwrap_or("").to_string();
    let synth = synthesizer(s, p);
    let synth_id = synth.id();
    let mut manifest = read_manifest(&out);
    manifest.version = 1;

    let mut todo: Vec<(Line, String)> = Vec::new();
    let mut cached = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut recorded = 0usize;
    for (_, line) in l.script.lines_in_route() {
        if !line.is_spoken() || only.as_ref().is_some_and(|o| !o.contains(&line.id)) {
            continue;
        }
        if !all && has_recording(&l, line) {
            recorded += 1;
            continue;
        }
        let voice = line.speaker.as_deref().and_then(|sp| l.script.characters.get(sp)).and_then(|c| c.voice.as_ref());
        let ref_audio = voice.filter(|v| !v.ref_audio.trim().is_empty()).map(|v| l.resolve(&v.ref_audio));
        // a re-recorded reference of the same length is another voice: size and modified time
        let ref_size = ref_audio
            .as_ref()
            .and_then(|a| std::fs::metadata(a).ok())
            .map(|m| {
                let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
                format!("{}@{mtime}", m.len())
            })
            .unwrap_or_default();
        let ref_text = voice.and_then(|v| v.ref_text.clone());
        let speaker = voice.and_then(|v| v.speaker.clone()).filter(|x| !x.trim().is_empty());
        // a line's own direction wins over the character's standing one
        let instruct = line.instruct.clone().or_else(|| voice.and_then(|v| v.instruct.clone())).filter(|x| !x.trim().is_empty());
        let text = line_speech(line);
        let hash = line_hash(&[
            &text,
            &language,
            &model,
            &synth_id,
            &ref_audio.as_ref().map(|a| a.to_string_lossy().to_string()).unwrap_or_default(),
            &ref_size,
            ref_text.as_deref().unwrap_or(""),
            speaker.as_deref().unwrap_or(""),
            instruct.as_deref().unwrap_or(""),
        ]);
        let out_path = out.join(format!("{}.wav", file_stem(&line.id, 0)));
        // Windows and macOS file systems ignore case: `A1` and `a1` are one file there
        if !seen_paths.insert(PathBuf::from(out_path.to_string_lossy().to_lowercase())) {
            return Err(bad(
                "script.synthesize",
                format!("line {} would write {} again (ids that differ only in unusual characters)", line.id, out_path.display()),
            ));
        }
        if !force
            && let Some(e) = manifest.lines.get(&line.id)
            && e.hash == hash
            && out.join(&e.file).is_file()
        {
            cached.push(json!({"id": line.id, "path": out.join(&e.file).to_string_lossy(), "seconds": e.seconds, "status": "cached"}));
            continue;
        }
        todo.push((
            Line { id: line.id.clone(), text, voice: Voice { ref_audio, ref_text, speaker, instruct }, language: Some(language.clone()), out: out_path },
            hash,
        ));
    }

    let mut spoken = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    // Chunks of SYNTH_CHUNK lines, the manifest saved after each: a run that dies (a GPU crash, a
    // kill) loses at most one chunk, and the next run goes on from the lines already recorded.
    let mut stopped: Option<EngineError> = None;
    for chunk in todo.chunks(SYNTH_CHUNK) {
        let lines: Vec<Line> = chunk.iter().map(|(l, _)| l.clone()).collect();
        let outcome = match synth.synthesize(&lines, &mut |_, _| true) {
            Ok(o) => o,
            Err(e) => {
                stopped = Some(EngineError::Other(format!("{e} ({} lines were spoken and recorded before this; run it again to go on)", spoken.len())));
                break;
            }
        };
        for sp in &outcome.spoken {
            let hash = chunk.iter().find(|(l, _)| l.id == sp.id).map(|(_, h)| h.clone()).unwrap_or_default();
            let file = sp.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            manifest.lines.insert(sp.id.clone(), Entry { hash, file, seconds: sp.duration.seconds() });
            spoken.push(json!({"id": sp.id, "path": sp.path.to_string_lossy(), "seconds": sp.duration.seconds(), "status": "synthesized"}));
        }
        failed.extend(outcome.failed.iter().map(|(id, e)| json!({"id": id, "error": e})));
        write_manifest(&out, &manifest)?;
    }
    if let Some(e) = stopped {
        return Err(e);
    }
    let mut lines_out = cached.clone();
    lines_out.extend(spoken.iter().cloned());
    Ok(json!({
        "synthesizer": synth_id,
        "out": out.to_string_lossy(),
        "manifest": out.join(MANIFEST).to_string_lossy(),
        "synthesized": spoken.len(),
        "cached": cached.len(),
        "recorded": recorded,
        "failed": failed,
        "lines": lines_out,
    }))
}

// ---------------------------------------------------------------------------------------------

fn lip_options(p: &Value) -> lip::Options {
    let mut o = lip::Options::default();
    if let Some(b) = bool_p(p, "three") {
        o.three_level = b;
    }
    if let Some(v) = f64_p(p, "openDb") {
        o.open_db = v as f32;
    }
    if let Some(v) = f64_p(p, "halfDb") {
        o.half_db = v as f32;
    }
    if let Some(v) = f64_p(p, "minSeconds") {
        o.min_s = v as f32;
    }
    o
}

/// Mouth segments of an audio file: `(start, end, level)` in seconds, and the duration.
fn mouth_of(s: &Session, path: &Path, o: &lip::Options) -> Result<(f64, Vec<lip::MouthSegment>)> {
    let src = s.media.open_file(&path.to_string_lossy(), &*s.services).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    if !src.info().has_audio() {
        return Err(EngineError::Other(format!("{} has no audio", path.display())));
    }
    let dur = src.info().duration;
    let rate = filmcraft_speech::SAMPLE_RATE;
    let frames = usize::try_from(dur.to_units_floor(rate as i64).max(0)).unwrap_or(0);
    if frames as f64 / rate as f64 > lip::MAX_SECONDS {
        return Err(EngineError::Other(format!("{} is longer than an hour", path.display())));
    }
    let buf = src.audio(0, frames, rate).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    let mono = filmcraft_speech::downmix(&buf.channels);
    Ok((dur.seconds(), lip::mouth_segments(&mono, rate, o)))
}

fn segments_json(v: &[lip::MouthSegment]) -> Vec<Value> {
    v.iter().map(|g| json!({"start": g.start, "end": g.end, "level": g.level, "startTick": Tick::from_seconds_f64(g.start), "endTick": Tick::from_seconds_f64(g.end)})).collect()
}

fn lipsync_analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("lipsync.analyze", "`path` (an audio file) is required"))?;
    let (seconds, segs) = mouth_of(s, Path::new(path), &lip_options(p))?;
    Ok(json!({"seconds": seconds, "segments": segments_json(&segs)}))
}

fn script_lipsync(s: &mut Session, p: &Value) -> Result<Value> {
    let l = load(s, p, "script.lipsync")?;
    let out = out_dir(s, &l, p)?;
    let manifest = read_manifest(&out);
    let o = lip_options(p);
    let mut lines = BTreeMap::new();
    let mut failed = Vec::new();
    let mut no_audio = 0usize;
    let scenes: Vec<&Scene> = l.script.scenes_in_route();
    for sc in scenes {
        for line in &sc.lines {
            if !line.is_spoken() {
                continue;
            }
            let audio = if has_recording(&l, line) {
                line.voice_file.as_deref().map(|f| l.resolve(f))
            } else {
                manifest.lines.get(&line.id).map(|e| out.join(&e.file)).filter(|f| f.is_file())
            };
            let Some(audio) = audio else {
                no_audio += 1;
                continue;
            };
            match mouth_of(s, &audio, &o) {
                Ok((seconds, segs)) => {
                    lines.insert(line.id.clone(), json!({"audio": audio.to_string_lossy(), "seconds": seconds, "segments": segments_json(&segs)}));
                }
                Err(e) => failed.push(json!({"id": line.id, "error": e.to_string()})),
            }
        }
    }
    std::fs::create_dir_all(&out).map_err(|e| EngineError::Other(format!("{}: {e}", out.display())))?;
    let file = out.join("lipsync.json");
    let text = serde_json::to_string_pretty(&json!({"version": 1, "three": o.three_level, "lines": lines})).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::write(&file, text).map_err(|e| EngineError::Other(format!("{}: {e}", file.display())))?;
    Ok(json!({"file": file.to_string_lossy(), "lines": lines.len(), "withoutAudio": no_audio, "failed": failed}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        query("script.validate", "Validate Script", r#"{"path":str}"#, validate),
        query("script.pickVoices", "Pick Voice References", r#"{"path":str,"write":bool?,"overwrite":bool?}"#, pick_voices),
        query(
            "script.synthesize",
            "Synthesize Script Lines",
            r#"{"path":str,"out":str?,"ids":[str]?,"force":bool?,"all":bool?,"language":str?,"model":str?,"device":str?,"python":str?,"script":str?}"#,
            synthesize,
        ),
        query("script.lipsync", "Lip Sync Script", r#"{"path":str,"out":str?,"three":bool?,"openDb":f?,"halfDb":f?,"minSeconds":f?}"#, script_lipsync),
        query("lipsync.analyze", "Analyze Lip Sync", r#"{"path":str,"three":bool?,"openDb":f?,"halfDb":f?,"minSeconds":f?}"#, lipsync_analyze),
    ]
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn ruby_is_spoken_as_its_reading() {
        assert_eq!(spoken_text("｜漢字《かんじ》を読む"), "かんじを読む");
        assert_eq!(spoken_text("東京《とうきょう》へ行く。"), "とうきょうへ行く。");
        assert_eq!(spoken_text("ふつうの文。"), "ふつうの文。");
        assert_eq!(spoken_text("  《》壊れた  "), "《》壊れた");
    }

    #[test]
    fn references_prefer_calm_lines_near_six_seconds() {
        let calm = reference_score(6.0, "今日はとても良い天気ですね。少し散歩に行きましょう。").unwrap();
        let shout = reference_score(6.0, "やめろ！！！？").unwrap_or(f64::MAX);
        assert!(calm < shout);
        assert!(reference_score(1.0, "はい。").is_none());
        assert!(reference_score(30.0, "長い").is_none());
        assert!(reference_score(6.0, "あ").is_none());
        assert!(reference_score(5.0, "今日はとても良い天気ですね。少し散歩に行きましょう。").unwrap() > calm);
    }

    #[test]
    fn hashes_differ_by_every_part() {
        let a = line_hash(&["a", "b"]);
        assert_eq!(a, line_hash(&["a", "b"]));
        assert_ne!(a, line_hash(&["ab", ""]));
        assert_ne!(a, line_hash(&["a", "c"]));
    }
}
