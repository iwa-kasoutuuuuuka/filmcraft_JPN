//! Text-to-speech: the [`Synthesizer`] trait, a [`FixedSynthesizer`] for tests and agents, and
//! [`ExternalSynthesizer`], which runs `tools/qwen3-tts/filmcraft_tts.py` (Qwen3-TTS, Python) as
//! an external process. FilmCraft links no Python: the script is found at run time, spoken lines
//! are written as WAV files, and the script reports progress and results on stdout as one JSON
//! object per line (the protocol is documented at the top of the script).
//!
//! Every number from the script is treated as hostile: lines and texts are capped, sample rates
//! and durations are range-checked, and a process that stops talking or exits without its final
//! `{"done": true}` line is an error with the tail of its stderr.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ProgressFn, SpeechError};
use filmcraft_time::Tick;

/// Most lines one synthesis takes.
pub const MAX_LINES: usize = 2_000;
/// Longest text of one line, in characters.
pub const MAX_TEXT_CHARS: usize = 4_000;
/// Longest spoken line the host accepts, seconds (a text of `MAX_TEXT_CHARS` at a slow pace).
const MAX_LINE_SECONDS: f64 = 20.0 * 60.0;
/// Most stderr kept for an error message.
const STDERR_TAIL: usize = 2_000;

/// The voice a line is spoken in: a reference recording to clone (a few seconds of clean speech)
/// and, for the best match, its transcript. Neither: the model's default voice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_audio: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_text: Option<String>,
}

/// One line to speak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The caller's id, returned with the result (a script line id, a caption id…).
    pub id: String,
    pub text: String,
    pub voice: Voice,
    /// Model language name (`"Japanese"`, `"English"`, `"Auto"`); None: the synthesizer's default.
    pub language: Option<String>,
    /// The WAV file to write.
    pub out: PathBuf,
}

/// One spoken line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Spoken {
    pub id: String,
    pub path: PathBuf,
    pub sample_rate: u32,
    pub duration: Tick,
}

/// What a synthesis produced: the lines spoken and the ones that failed (id, reason).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub spoken: Vec<Spoken>,
    pub failed: Vec<(String, String)>,
}

/// A speech synthesiser.
pub trait Synthesizer: Send + Sync {
    /// Stable id (`"qwen3-tts"`), stored with what it made.
    fn id(&self) -> String;
    /// Speak `lines`, writing each to its `out` path. A line that fails is reported in
    /// [`Outcome::failed`]; the whole call fails only when nothing could run.
    fn synthesize(&self, lines: &[Line], progress: ProgressFn) -> Result<Outcome, SpeechError>;
}

/// Check the lines a caller passed (count, text length, paths).
pub fn check_lines(lines: &[Line]) -> Result<(), SpeechError> {
    if lines.is_empty() {
        return Err(SpeechError::Io("nothing to synthesize".into()));
    }
    if lines.len() > MAX_LINES {
        return Err(SpeechError::Io(format!("more than {MAX_LINES} lines")));
    }
    for l in lines {
        if l.text.trim().is_empty() {
            return Err(SpeechError::Io(format!("line {}: empty text", l.id)));
        }
        if l.text.chars().count() > MAX_TEXT_CHARS {
            return Err(SpeechError::Io(format!("line {}: text longer than {MAX_TEXT_CHARS} characters", l.id)));
        }
        if l.out.as_os_str().is_empty() {
            return Err(SpeechError::Io(format!("line {}: no output path", l.id)));
        }
    }
    Ok(())
}

/// Write mono 16-bit PCM WAV `samples` (−1..1) at `rate` Hz.
pub fn write_wav16(path: &Path, samples: &[f32], rate: u32) -> Result<(), SpeechError> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).map_err(|_| SpeechError::Io("WAV too long".into()))?;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&rate.saturating_mul(2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        b.extend_from_slice(&v.to_le_bytes());
    }
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, b)?;
    Ok(())
}

/// A synthesizer that writes a tone whose length grows with the text (tests, demos, agents that
/// want the timing without a model). A text containing `FAIL` fails, so error paths can be tested.
pub struct FixedSynthesizer {
    /// Seconds per character of text (default 0.15).
    pub seconds_per_char: f64,
    pub sample_rate: u32,
}

impl Default for FixedSynthesizer {
    fn default() -> Self {
        Self { seconds_per_char: 0.15, sample_rate: 24_000 }
    }
}

impl Synthesizer for FixedSynthesizer {
    fn id(&self) -> String {
        "fixed".into()
    }
    fn synthesize(&self, lines: &[Line], progress: ProgressFn) -> Result<Outcome, SpeechError> {
        check_lines(lines)?;
        let rate = self.sample_rate.clamp(8_000, 192_000);
        let mut out = Outcome::default();
        for (i, l) in lines.iter().enumerate() {
            if !progress(i as f32 / lines.len() as f32, "Synthesising") {
                return Err(SpeechError::Cancelled);
            }
            if l.text.contains("FAIL") {
                out.failed.push((l.id.clone(), "the fixed synthesizer fails texts containing FAIL".into()));
                continue;
            }
            let secs = (l.text.chars().count() as f64 * self.seconds_per_char).clamp(0.2, MAX_LINE_SECONDS);
            let n = (secs * rate as f64) as usize;
            let samples: Vec<f32> = (0..n).map(|k| (k as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3).collect();
            match write_wav16(&l.out, &samples, rate) {
                Ok(()) => out.spoken.push(Spoken { id: l.id.clone(), path: l.out.clone(), sample_rate: rate, duration: Tick::from_seconds_f64(secs) }),
                Err(e) => out.failed.push((l.id.clone(), e.to_string())),
            }
        }
        progress(1.0, "Done");
        Ok(out)
    }
}

/// One line of the script's stdout.
#[derive(Debug, PartialEq)]
pub enum Event {
    Progress(f32, String),
    Spoken { id: String, out: PathBuf, sample_rate: u32, seconds: f64 },
    Failed { id: String, error: String },
    Done { ok: u64, failed: u64 },
}

/// Parse one stdout line of the script (None: not an event; such lines are ignored).
pub fn parse_event(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let o = v.as_object()?;
    if o.get("done").and_then(Value::as_bool) == Some(true) {
        return Some(Event::Done { ok: o.get("ok").and_then(Value::as_u64).unwrap_or(0), failed: o.get("failed").and_then(Value::as_u64).unwrap_or(0) });
    }
    if let Some(p) = o.get("progress").and_then(Value::as_f64) {
        let status = o.get("status").and_then(Value::as_str).unwrap_or("").to_string();
        return Some(Event::Progress(p.clamp(0.0, 1.0) as f32, status));
    }
    let id = o.get("id")?;
    let id = match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if let Some(e) = o.get("error").and_then(Value::as_str) {
        return Some(Event::Failed { id, error: e.chars().take(STDERR_TAIL).collect() });
    }
    if o.get("ok").and_then(Value::as_bool) == Some(true) {
        let out = PathBuf::from(o.get("out").and_then(Value::as_str)?);
        let sample_rate = u32::try_from(o.get("sampleRate").and_then(Value::as_u64)?).ok()?;
        let seconds = o.get("seconds").and_then(Value::as_f64)?;
        if !(8_000..=192_000).contains(&sample_rate) || !seconds.is_finite() || !(0.0..=MAX_LINE_SECONDS).contains(&seconds) {
            return Some(Event::Failed { id, error: format!("implausible result (sample rate {sample_rate}, {seconds} s)") });
        }
        return Some(Event::Spoken { id, out, sample_rate, seconds });
    }
    None
}

/// Runs the Qwen3-TTS helper script in a Python interpreter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSynthesizer {
    /// The interpreter (`python`, or a venv's `python.exe`).
    pub python: PathBuf,
    /// `filmcraft_tts.py`.
    pub script: PathBuf,
    /// Model id or directory passed as `--model` (None: the script's default).
    pub model: Option<String>,
    /// `--device` (None: the script picks CUDA when available).
    pub device: Option<String>,
    /// `--language` for lines without one (None: the script's default, Japanese).
    pub language: Option<String>,
}

/// The script's path relative to a FilmCraft checkout or install.
pub const SCRIPT_RELATIVE: &str = "tools/qwen3-tts/filmcraft_tts.py";

impl ExternalSynthesizer {
    /// Find the interpreter and the script: the arguments, then `FILMCRAFT_TTS_PYTHON` /
    /// `FILMCRAFT_TTS_SCRIPT`, then `tools/qwen3-tts/` next to the executable, in the current
    /// directory and in the source tree; the interpreter defaults to `python` on the PATH.
    /// Nothing is run; [`ExternalSynthesizer::probe`] checks the install.
    pub fn discover(python: Option<&str>, script: Option<&str>) -> Self {
        let python =
            python.map(PathBuf::from).or_else(|| std::env::var_os("FILMCRAFT_TTS_PYTHON").map(PathBuf::from)).unwrap_or_else(|| PathBuf::from("python"));
        let script = script
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("FILMCRAFT_TTS_SCRIPT").map(PathBuf::from))
            .or_else(|| {
                let mut roots: Vec<PathBuf> = Vec::new();
                if let Ok(exe) = std::env::current_exe() {
                    // next to the executable, and above a `target/<profile>/` build directory
                    let mut d = exe.parent().map(Path::to_path_buf);
                    for _ in 0..4 {
                        if let Some(p) = d {
                            roots.push(p.clone());
                            d = p.parent().map(Path::to_path_buf);
                        } else {
                            break;
                        }
                    }
                }
                if let Ok(cwd) = std::env::current_dir() {
                    roots.push(cwd);
                }
                roots.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
                roots.into_iter().map(|r| r.join(SCRIPT_RELATIVE)).find(|p| p.is_file())
            })
            .unwrap_or_else(|| PathBuf::from(SCRIPT_RELATIVE));
        Self { python, script, model: None, device: None, language: None }
    }

    /// Whether the script file exists (the interpreter is only known once it runs).
    pub fn script_found(&self) -> bool {
        self.script.is_file()
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.python);
        c.arg("-I").arg(&self.script);
        c.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
        c
    }

    /// Run `--probe`: what is installed (`ok`, `torch`, `cuda`, `gpu`, `qwenTts`, `error`), as
    /// the script reports it, with `python` and `script` added. Loads no model.
    pub fn probe(&self) -> Result<Value, SpeechError> {
        if !self.script_found() {
            return Err(SpeechError::Unavailable(format!("the TTS script was not found at {}", self.script.display())));
        }
        let out = self
            .command()
            .arg("--probe")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| SpeechError::Unavailable(format!("cannot run {}: {e}", self.python.display())))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut v = text.lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).unwrap_or_else(|| {
            let err: String = String::from_utf8_lossy(&out.stderr).chars().take(STDERR_TAIL).collect();
            json!({"ok": false, "error": if err.is_empty() { format!("{} printed nothing", self.script.display()) } else { err }})
        });
        if let Some(o) = v.as_object_mut() {
            o.insert("python".into(), json!(self.python.to_string_lossy()));
            o.insert("script".into(), json!(self.script.to_string_lossy()));
        }
        Ok(v)
    }

    fn write_jobs(&self, lines: &[Line]) -> Result<PathBuf, SpeechError> {
        let dir = lines.first().and_then(|l| l.out.parent()).filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!(
            "filmcraft-tts-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
        ));
        let mut f = std::fs::File::create(&path)?;
        for l in lines {
            let mut j = json!({"id": l.id, "text": l.text, "out": l.out.to_string_lossy()});
            if let Some(a) = &l.voice.ref_audio {
                j["refAudio"] = json!(a.to_string_lossy());
            }
            if let Some(t) = &l.voice.ref_text {
                j["refText"] = json!(t);
            }
            if let Some(lang) = &l.language {
                j["language"] = json!(lang);
            }
            writeln!(f, "{j}")?;
        }
        Ok(path)
    }
}

impl Synthesizer for ExternalSynthesizer {
    fn id(&self) -> String {
        "qwen3-tts".into()
    }

    fn synthesize(&self, lines: &[Line], progress: ProgressFn) -> Result<Outcome, SpeechError> {
        check_lines(lines)?;
        if !self.script_found() {
            return Err(SpeechError::Unavailable(format!("the TTS script was not found at {} (see tools/qwen3-tts/README.md)", self.script.display())));
        }
        let jobs = self.write_jobs(lines)?;
        let result = self.run(&jobs, lines, progress);
        let _ = std::fs::remove_file(&jobs);
        result
    }
}

impl ExternalSynthesizer {
    fn run(&self, jobs: &Path, lines: &[Line], progress: ProgressFn) -> Result<Outcome, SpeechError> {
        let mut c = self.command();
        c.arg("--jobs").arg(jobs);
        if let Some(m) = &self.model {
            c.arg("--model").arg(m);
        }
        if let Some(d) = &self.device {
            c.arg("--device").arg(d);
        }
        if let Some(l) = &self.language {
            c.arg("--language").arg(l);
        }
        let mut child = c
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SpeechError::Unavailable(format!("cannot run {}: {e}", self.python.display())))?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err(SpeechError::Io("no stdout from the TTS script".into()));
        };
        // stderr is drained on its own thread so a chatty model cannot block the pipe
        let stderr = child.stderr.take();
        let err_thread = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(e) = stderr {
                let mut r = BufReader::new(e);
                let mut line = String::new();
                while matches!(r.read_line(&mut line), Ok(n) if n > 0) {
                    s.push_str(&line);
                    line.clear();
                    if s.len() > STDERR_TAIL * 4 {
                        s.drain(..s.len() - STDERR_TAIL * 2);
                    }
                }
            }
            s
        });
        let mut out = Outcome::default();
        let mut done = None;
        let mut cancelled = false;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            match parse_event(&line) {
                Some(Event::Progress(p, status)) => {
                    if !progress(p, &status) {
                        cancelled = true;
                        let _ = child.kill();
                        break;
                    }
                }
                Some(Event::Spoken { id, out: path, sample_rate, seconds }) => {
                    if lines.iter().any(|l| l.id == id) {
                        out.spoken.push(Spoken { id, path, sample_rate, duration: Tick::from_seconds_f64(seconds) });
                    }
                }
                Some(Event::Failed { id, error }) => out.failed.push((id, error)),
                Some(Event::Done { ok, failed }) => done = Some((ok, failed)),
                None => {}
            }
        }
        let status = child.wait();
        let stderr_tail: String = err_thread.join().unwrap_or_default().chars().rev().take(STDERR_TAIL).collect::<Vec<_>>().into_iter().rev().collect();
        if cancelled {
            return Err(SpeechError::Cancelled);
        }
        if done.is_none() {
            let why = match status {
                Ok(s) if !s.success() => format!("the TTS script exited with {s}"),
                Ok(_) => "the TTS script ended without its final line".into(),
                Err(e) => format!("the TTS script could not be waited for: {e}"),
            };
            let tail = stderr_tail.trim();
            return Err(SpeechError::Model(if tail.is_empty() { why } else { format!("{why}: {tail}") }));
        }
        if out.spoken.is_empty() && !out.failed.is_empty() {
            let (id, e) = &out.failed[0];
            return Err(SpeechError::Model(format!("no line was spoken; line {id}: {e}")));
        }
        progress(1.0, "Done");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, text: &str, out: &Path) -> Line {
        Line { id: id.into(), text: text.into(), voice: Voice::default(), language: None, out: out.to_path_buf() }
    }

    #[test]
    fn events_parse_and_reject_nonsense() {
        assert_eq!(parse_event(r#"{"progress": 0.5, "status": "x"}"#), Some(Event::Progress(0.5, "x".into())));
        assert_eq!(parse_event(r#"{"progress": 7}"#), Some(Event::Progress(1.0, String::new())));
        assert_eq!(
            parse_event(r#"{"id": "a", "out": "a.wav", "sampleRate": 24000, "seconds": 1.5, "ok": true}"#),
            Some(Event::Spoken { id: "a".into(), out: "a.wav".into(), sample_rate: 24000, seconds: 1.5 })
        );
        assert_eq!(parse_event(r#"{"id": 3, "error": "boom"}"#), Some(Event::Failed { id: "3".into(), error: "boom".into() }));
        assert_eq!(parse_event(r#"{"done": true, "ok": 2, "failed": 1}"#), Some(Event::Done { ok: 2, failed: 1 }));
        assert!(matches!(parse_event(r#"{"id": "a", "out": "a.wav", "sampleRate": 1, "seconds": 1.5, "ok": true}"#), Some(Event::Failed { .. })));
        assert!(matches!(parse_event(r#"{"id": "a", "out": "a.wav", "sampleRate": 24000, "seconds": 1e9, "ok": true}"#), Some(Event::Failed { .. })));
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"id": "a"}"#), None);
        assert_eq!(parse_event("[1,2]"), None);
    }

    #[test]
    fn fixed_synthesizer_writes_wavs_and_reports_failures() {
        let dir = std::env::temp_dir().join(format!("filmcraft-synth-{}", std::process::id()));
        let lines = vec![line("a", "こんにちは", &dir.join("a.wav")), line("b", "FAIL", &dir.join("b.wav"))];
        let mut seen = 0;
        let out = FixedSynthesizer::default()
            .synthesize(&lines, &mut |_, _| {
                seen += 1;
                true
            })
            .unwrap();
        assert_eq!(out.spoken.len(), 1);
        assert_eq!(out.spoken[0].id, "a");
        assert_eq!(out.spoken[0].duration, Tick::from_seconds_f64(0.75));
        assert_eq!(out.failed, vec![("b".to_string(), "the fixed synthesizer fails texts containing FAIL".to_string())]);
        let wav = std::fs::read(dir.join("a.wav")).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 2 * (0.75 * 24_000.0) as usize);
        assert!(seen >= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fixed_synthesizer_checks_input() {
        let d = std::env::temp_dir();
        let s = FixedSynthesizer::default();
        assert!(s.synthesize(&[], &mut |_, _| true).is_err());
        assert!(s.synthesize(&[line("a", "  ", &d.join("x.wav"))], &mut |_, _| true).is_err());
        assert!(s.synthesize(&[line("a", &"あ".repeat(MAX_TEXT_CHARS + 1), &d.join("x.wav"))], &mut |_, _| true).is_err());
        assert_eq!(s.synthesize(&[line("a", "x", &d.join("x.wav"))], &mut |_, _| false), Err(SpeechError::Cancelled));
    }

    #[test]
    fn discover_prefers_arguments_and_finds_the_source_tree_script() {
        let e = ExternalSynthesizer::discover(Some("C:/venv/python.exe"), Some("x.py"));
        assert_eq!(e.python, PathBuf::from("C:/venv/python.exe"));
        assert_eq!(e.script, PathBuf::from("x.py"));
        let e = ExternalSynthesizer::discover(None, None);
        assert!(e.script_found(), "{}", e.script.display());
    }

    /// A Python interpreter on the PATH, if any (the external tests need one and are skipped
    /// without it).
    fn python() -> Option<PathBuf> {
        for p in ["python", "python3"] {
            if Command::new(p).arg("--version").stdin(Stdio::null()).output().is_ok_and(|o| o.status.success()) {
                return Some(PathBuf::from(p));
            }
        }
        None
    }

    fn fake() -> Option<ExternalSynthesizer> {
        let python = python()?;
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/qwen3-tts/tests/fake_tts.py");
        Some(ExternalSynthesizer { python, script, model: None, device: None, language: Some("Japanese".into()) })
    }

    #[test]
    fn external_runs_the_fake_script() {
        let Some(e) = fake() else {
            eprintln!("no python: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("filmcraft-synth-ext-{}", std::process::id()));
        let mut lines = vec![line("a", "こんにちは", &dir.join("a.wav")), line("b", "FAIL", &dir.join("b.wav"))];
        lines[0].voice = Voice { ref_audio: None, ref_text: Some("x".into()) };
        let mut progress = Vec::new();
        let out = e
            .synthesize(&lines, &mut |p, s| {
                progress.push((p, s.to_string()));
                true
            })
            .unwrap();
        assert_eq!(out.spoken.len(), 1, "{out:?}");
        assert_eq!(out.spoken[0].id, "a");
        assert_eq!(out.spoken[0].sample_rate, 24_000);
        assert_eq!(out.spoken[0].duration, Tick::from_seconds_f64(0.3));
        assert!(dir.join("a.wav").is_file());
        assert_eq!(out.failed.len(), 1);
        assert!(out.failed[0].1.contains("FAIL"));
        assert!(progress.iter().any(|(_, s)| s.contains("Loading")), "{progress:?}");
        let probe = e.probe().unwrap();
        assert_eq!(probe["ok"], json!(true));
        assert_eq!(probe["fake"], json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_cancels_and_reports_a_crash() {
        let Some(e) = fake() else {
            eprintln!("no python: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("filmcraft-synth-cancel-{}", std::process::id()));
        let lines = vec![line("a", "x", &dir.join("a.wav"))];
        assert_eq!(e.synthesize(&lines, &mut |_, _| false), Err(SpeechError::Cancelled));
        let lines = vec![line("a", "CRASH", &dir.join("a.wav"))];
        let err = e.synthesize(&lines, &mut |_, _| true).unwrap_err();
        assert!(matches!(err, SpeechError::Model(ref m) if m.contains("exited") && m.contains("simulated crash")), "{err}");
        let missing = ExternalSynthesizer { script: dir.join("nope.py"), ..e };
        assert!(matches!(missing.synthesize(&lines, &mut |_, _| true), Err(SpeechError::Unavailable(_))));
        assert!(matches!(missing.probe(), Err(SpeechError::Unavailable(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
