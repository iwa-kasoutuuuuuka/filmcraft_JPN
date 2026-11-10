use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use filmcraft_speech::FixedSynthesizer;
use filmcraft_speech::synth::write_wav16;

const RATE: u32 = 16_000;

/// `(seconds, amplitude)` pieces of a 220 Hz tone.
fn tone(pieces: &[(f64, f32)]) -> Vec<f32> {
    let mut v = Vec::new();
    for (secs, amp) in pieces {
        let t0 = v.len();
        v.extend((0..(secs * RATE as f64) as usize).map(|k| amp * ((t0 + k) as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin()));
    }
    v
}

fn temp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("filmcraft-script-{name}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A script with two recordings of marina (one good reference), two lines to speak (one with
/// ruby) and a direction, in a temp folder. Returns (session, script path, folder).
fn setup(name: &str) -> (Session, PathBuf, PathBuf) {
    let dir = temp(name);
    write_wav16(&dir.join("m1.wav"), &tone(&[(1.0, 0.0), (2.0, 0.5), (1.0, 0.0), (2.0, 0.5)]), RATE).unwrap();
    write_wav16(&dir.join("m2.wav"), &tone(&[(3.0, 0.5)]), RATE).unwrap();
    let script = json!({
        "title": "test",
        "characters": {"marina": {"name": "まりな"}, "kojiro": {"name": "小次郎"}},
        "scenes": [{"id": "a", "lines": [
            {"id": "a1", "speaker": "marina", "text": "待って、まだ話は終わってない。", "voiceFile": "m1.wav"},
            {"id": "a2", "speaker": "marina", "text": "そんなはずはない。", "voiceFile": "m2.wav"},
            {"id": "a3", "speaker": "marina", "text": "どうして？"},
            {"id": "a4", "speaker": "kojiro", "text": "｜漢字《かんじ》だ。"},
            {"id": "a5", "kind": "direction", "sfx": "door"}
        ]}]
    });
    let path = dir.join("story.fcscript.json");
    std::fs::write(&path, serde_json::to_string_pretty(&script).unwrap()).unwrap();
    let s = Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), ..Default::default() };
    (s, path, dir)
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn validate_counts_what_there_is_to_do() {
    let (mut s, path, dir) = setup("validate");
    let r = run(&mut s, "script.validate", json!({"path": path}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["stats"]["lines"], json!(5));
    assert_eq!(r["stats"]["spoken"], json!(4));
    assert_eq!(r["stats"]["recorded"], json!(2));
    assert_eq!(r["stats"]["toSynthesize"], json!(2));
    // a recording that is not there is reported, and is still to be synthesized
    std::fs::remove_file(dir.join("m2.wav")).unwrap();
    let r = run(&mut s, "script.validate", json!({"path": path}));
    assert_eq!(r["stats"]["recordingMissing"], json!(1));
    assert_eq!(r["stats"]["toSynthesize"], json!(3));
    // errors make synthesis refuse
    let text = std::fs::read_to_string(&path).unwrap().replace("\"kojiro\",", "\"ghost\",");
    std::fs::write(&path, text).unwrap();
    let r = run(&mut s, "script.validate", json!({"path": path}));
    assert_eq!(r["ok"], json!(false));
    assert!(r["problems"].as_array().unwrap().iter().any(|p| p["severity"] == "error" && p["message"].as_str().unwrap().contains("ghost")), "{r}");
    let e = s.execute("script.synthesize", json!({"path": path, "out": dir.join("out")})).unwrap_err().to_string();
    assert!(e.contains("ghost"), "{e}");
    assert!(s.execute("script.validate", json!({"path": dir.join("nope.json")})).is_err());
    std::fs::write(&path, "[1,2]").unwrap();
    assert!(s.execute("script.validate", json!({"path": path})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_best_recording_becomes_the_voice_reference() {
    let (mut s, path, dir) = setup("pick");
    let r = run(&mut s, "script.pickVoices", json!({"path": path}));
    assert_eq!(r["picked"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["picked"][0]["character"], json!("marina"));
    assert_eq!(r["picked"][0]["line"], json!("a1"));
    assert_eq!(r["picked"][0]["candidates"], json!(2));
    assert_eq!(r["skipped"][0]["character"], json!("kojiro"), "{r}");
    assert_eq!(r["written"], json!(false));
    assert!(!std::fs::read_to_string(&path).unwrap().contains("refAudio"));
    let r = run(&mut s, "script.pickVoices", json!({"path": path, "write": true}));
    assert_eq!(r["written"], json!(true));
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written["characters"]["marina"]["voice"]["refAudio"], json!("m1.wav"));
    assert_eq!(written["characters"]["marina"]["voice"]["refText"], json!("待って、まだ話は終わってない。"));
    // a voice that is there is kept unless asked
    let r = run(&mut s, "script.pickVoices", json!({"path": path, "write": true}));
    assert!(r["picked"].as_array().unwrap().is_empty());
    let marina = r["skipped"].as_array().unwrap().iter().find(|x| x["character"] == "marina").unwrap();
    assert!(marina["reason"].as_str().unwrap().contains("overwrite"), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_lines_without_a_recording_are_spoken_and_only_once() {
    let (mut s, path, dir) = setup("synth");
    let out = dir.join("tts");
    let p = |extra: Value| {
        let mut v = json!({"path": path, "out": out});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    let r = run(&mut s, "script.synthesize", p(json!({})));
    assert_eq!(r["synthesized"], json!(2), "{r}");
    assert_eq!(r["recorded"], json!(2));
    assert_eq!(r["cached"], json!(0));
    assert_eq!(ids(&r["lines"]), vec!["a3", "a4"]);
    // ruby is spoken as its reading: かんじだ。 is 5 characters at 0.15 s
    let a4 = r["lines"].as_array().unwrap().iter().find(|l| l["id"] == "a4").unwrap();
    assert!((a4["seconds"].as_f64().unwrap() - 0.75).abs() < 1e-6, "{a4}");
    assert!(out.join("a3.wav").is_file() && out.join("a4.wav").is_file() && out.join("manifest.json").is_file());
    // again: nothing to do
    let r = run(&mut s, "script.synthesize", p(json!({})));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(0), json!(2)), "{r}");
    // one line changes: only it is spoken again
    let text = std::fs::read_to_string(&path).unwrap().replace("どうして？", "どうしてなの？");
    std::fs::write(&path, text).unwrap();
    let r = run(&mut s, "script.synthesize", p(json!({})));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(1), json!(1)), "{r}");
    assert_eq!(ids(&r["lines"]), vec!["a4", "a3"]);
    // a deleted file is made again; force redoes everything; `all` includes recorded lines
    std::fs::remove_file(out.join("a4.wav")).unwrap();
    let r = run(&mut s, "script.synthesize", p(json!({})));
    assert_eq!(r["synthesized"], json!(1));
    let r = run(&mut s, "script.synthesize", p(json!({"force": true})));
    assert_eq!(r["synthesized"], json!(2));
    let r = run(&mut s, "script.synthesize", p(json!({"all": true, "force": true})));
    assert_eq!(r["synthesized"], json!(4));
    let r = run(&mut s, "script.synthesize", p(json!({"ids": ["a3"], "force": true})));
    assert_eq!(ids(&r["lines"]), vec!["a3"]);
    // a different language is a different voice: nothing cached
    let r = run(&mut s, "script.synthesize", p(json!({"language": "English"})));
    assert_eq!(r["synthesized"], json!(2));
    // a corrupt manifest is just no cache
    std::fs::write(out.join("manifest.json"), "{{{").unwrap();
    let r = run(&mut s, "script.synthesize", p(json!({"language": "English"})));
    assert_eq!(r["synthesized"], json!(2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_voice_reference_is_spoken_again() {
    let (mut s, path, dir) = setup("voicechange");
    let out = dir.join("tts");
    run(&mut s, "script.pickVoices", json!({"path": path, "write": true}));
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!(r["synthesized"], json!(2));
    // marina's reference file changes (a different take): her line is redone, kojiro's is not
    write_wav16(&dir.join("m1.wav"), &tone(&[(1.0, 0.0), (2.5, 0.5), (1.0, 0.0), (2.0, 0.5)]), RATE).unwrap();
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(1), json!(1)), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mouths_follow_the_recording_and_the_spoken_lines() {
    let (mut s, path, dir) = setup("lipsync");
    let out = dir.join("tts");
    let r = run(&mut s, "lipsync.analyze", json!({"path": dir.join("m1.wav")}));
    let segs = r["segments"].as_array().unwrap();
    assert_eq!(segs.iter().map(|g| g["level"].as_u64().unwrap()).collect::<Vec<_>>(), vec![0, 1, 0, 1], "{r}");
    assert!((segs[1]["start"].as_f64().unwrap() - 1.0).abs() < 0.04 && (segs[1]["end"].as_f64().unwrap() - 3.0).abs() < 0.04);
    assert!(segs[1]["startTick"].is_number());
    assert!(s.execute("lipsync.analyze", json!({"path": dir.join("nope.wav")})).is_err());
    assert!(s.execute("lipsync.analyze", json!({})).is_err());
    // the whole script: recorded lines from their files, spoken lines from the manifest
    run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    let r = run(&mut s, "script.lipsync", json!({"path": path, "out": out}));
    assert_eq!(r["lines"], json!(4), "{r}");
    assert_eq!(r["withoutAudio"], json!(0));
    let file: Value = serde_json::from_str(&std::fs::read_to_string(Path::new(r["file"].as_str().unwrap())).unwrap()).unwrap();
    assert_eq!(file["lines"]["a1"]["segments"].as_array().unwrap().len(), 4);
    assert_eq!(file["lines"]["a3"]["segments"][0]["level"], json!(1));
    // before any synthesis the spoken lines have no audio yet
    let (mut s2, path2, dir2) = setup("lipsync2");
    let r = run(&mut s2, "script.lipsync", json!({"path": path2, "out": dir2.join("o")}));
    assert_eq!((r["lines"].clone(), r["withoutAudio"].clone()), (json!(2), json!(2)));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

/// Speaks like the fixed synthesizer (and shares its cache id) but fails any chunk that holds the
/// word BOOM, as a dying process would.
struct Flaky(FixedSynthesizer);

impl filmcraft_speech::Synthesizer for Flaky {
    fn id(&self) -> String {
        "fixed".into()
    }
    fn synthesize(
        &self,
        lines: &[filmcraft_speech::SynthLine],
        p: filmcraft_speech::ProgressFn,
    ) -> Result<filmcraft_speech::SynthOutcome, filmcraft_speech::SpeechError> {
        if lines.iter().any(|l| l.text.contains("BOOM")) {
            return Err(filmcraft_speech::SpeechError::Model("simulated crash".into()));
        }
        self.0.synthesize(lines, p)
    }
}

fn many_lines(dir: &Path, n: usize, boom_at: Option<usize>) -> PathBuf {
    let lines: Vec<Value> = (0..n)
        .map(|i| json!({"id": format!("l{i:04}"), "kind": "narration", "text": if Some(i) == boom_at { "BOOM".to_string() } else { format!("行{i}") }}))
        .collect();
    let path = dir.join("many.fcscript.json");
    std::fs::write(&path, serde_json::to_string(&json!({"scenes": [{"id": "s", "lines": lines}]})).unwrap()).unwrap();
    path
}

#[test]
fn a_long_script_is_spoken_in_chunks_and_a_dead_run_loses_at_most_one() {
    let dir = temp("chunks");
    let out = dir.join("tts");
    let n = crate::script_tools::SYNTH_CHUNK * 2 + 50;
    let path = many_lines(&dir, n, Some(crate::script_tools::SYNTH_CHUNK + 100));
    // the second chunk dies: the first is recorded, the command says so
    let mut s = Session { synthesizer: Some(Arc::new(Flaky(FixedSynthesizer::default()))), ..Default::default() };
    let e = s.execute("script.synthesize", json!({"path": path, "out": out})).unwrap_err().to_string();
    assert!(e.contains("simulated crash") && e.contains("400 lines were spoken and recorded"), "{e}");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["lines"].as_object().unwrap().len(), crate::script_tools::SYNTH_CHUNK);
    // run again with a synthesizer that works: only the rest is spoken, more lines than the
    // synthesizer accepts in one call
    let mut s = Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), ..Default::default() };
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!(r["cached"], json!(crate::script_tools::SYNTH_CHUNK), "{}", r["failed"]);
    assert_eq!(r["synthesized"], json!(n - crate::script_tools::SYNTH_CHUNK));
    assert!(r["failed"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ids_that_differ_only_in_case_would_share_a_file_and_are_refused() {
    let dir = temp("case");
    let path = dir.join("c.fcscript.json");
    std::fs::write(&path, r#"{"scenes":[{"id":"s","lines":[{"id":"A1","kind":"narration","text":"a"},{"id":"a1","kind":"narration","text":"b"}]}]}"#).unwrap();
    let mut s = Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), ..Default::default() };
    let e = s.execute("script.synthesize", json!({"path": path, "out": dir.join("o")})).unwrap_err().to_string();
    assert!(e.contains("would write"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scripts_with_the_same_name_in_different_folders_do_not_share_audio() {
    let data = temp("data");
    let one = temp("season1");
    let two = temp("season2");
    for d in [&one, &two] {
        std::fs::write(d.join("ep1.fcscript.json"), r#"{"scenes":[{"id":"s","lines":[{"id":"s01","kind":"narration","text":"こんにちは"}]}]}"#).unwrap();
    }
    std::fs::write(one.join(".fcscript.json"), r#"{"scenes":[{"id":"s","lines":[{"id":"s01","kind":"narration","text":"x"}]}]}"#).unwrap();
    std::fs::write(one.join("ep1.v2.json"), r#"{"scenes":[{"id":"s","lines":[{"id":"s01","kind":"narration","text":"x"}]}]}"#).unwrap();
    let mut s = Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), data_dir: Some(data.clone()), ..Default::default() };
    let mut outs = Vec::new();
    for p in [one.join("ep1.fcscript.json"), two.join("ep1.fcscript.json"), one.join(".fcscript.json"), one.join("ep1.v2.json")] {
        let r = run(&mut s, "script.synthesize", json!({"path": p}));
        assert_eq!(r["synthesized"], json!(1), "{r}");
        outs.push(r["out"].as_str().unwrap().to_string());
    }
    let unique: std::collections::HashSet<&String> = outs.iter().collect();
    assert_eq!(unique.len(), 4, "{outs:?}");
    assert!(outs[0].contains("ep1-") && outs[3].contains("ep1.v2-") && outs[2].contains("script-"), "{outs:?}");
    for d in [&data, &one, &two] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn rewriting_a_script_keeps_the_fields_this_build_does_not_know() {
    let (mut s, path, dir) = setup("keep");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["notes"] = json!("keep me");
    v["scenes"][0]["lines"][0]["page"] = json!(12);
    v["characters"]["marina"]["voice"] = json!({"instruct": "calm"});
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    // marina's voice has an instruct but no recording yet: validation allows a voice with neither
    // only when a speaker is named, so give it one
    v["characters"]["marina"]["voice"]["speaker"] = json!("Ono_Anna");
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    run(&mut s, "script.pickVoices", json!({"path": path, "write": true, "overwrite": true}));
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["notes"], json!("keep me"));
    assert_eq!(after["scenes"][0]["lines"][0]["page"], json!(12));
    assert_eq!(after["characters"]["marina"]["voice"]["refAudio"], json!("m1.wav"));
    assert_eq!(after["characters"]["marina"]["voice"]["instruct"], json!("calm"));
    assert_eq!(after["characters"]["marina"]["voice"]["speaker"], json!("Ono_Anna"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reference_re_recorded_at_the_same_length_is_a_new_voice() {
    let (mut s, path, dir) = setup("mtime");
    let out = dir.join("tts");
    run(&mut s, "script.pickVoices", json!({"path": path, "write": true}));
    assert_eq!(run(&mut s, "script.synthesize", json!({"path": path, "out": out}))["synthesized"], json!(2));
    // same size, different take (quieter), modified later
    let m1 = dir.join("m1.wav");
    let size = std::fs::metadata(&m1).unwrap().len();
    write_wav16(&m1, &tone(&[(1.0, 0.0), (2.0, 0.4), (1.0, 0.0), (2.0, 0.4)]), RATE).unwrap();
    assert_eq!(std::fs::metadata(&m1).unwrap().len(), size);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
    std::fs::OpenOptions::new().write(true).open(&m1).unwrap().set_modified(later).unwrap();
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(1), json!(1)), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// By hand, with the real model: speaks the script named by `FILMCRAFT_REAL_SCRIPT` through the
/// Python in `FILMCRAFT_TTS_PYTHON` (downloads the models on first use; needs a GPU to be quick).
///
/// `FILMCRAFT_TTS_PYTHON=…/python.exe FILMCRAFT_REAL_SCRIPT=…/story.fcscript.json FILMCRAFT_REAL_OUT=…
///  cargo test -p filmcraft-engine --lib real_model -- --ignored --nocapture`
#[test]
#[ignore = "needs Python with qwen-tts, the models and a script; run by hand"]
fn real_model_speaks_a_script_and_the_mouths_follow() {
    let (Some(script), Some(out)) = (std::env::var_os("FILMCRAFT_REAL_SCRIPT"), std::env::var_os("FILMCRAFT_REAL_OUT")) else {
        eprintln!("FILMCRAFT_REAL_SCRIPT / FILMCRAFT_REAL_OUT not set: skipped");
        return;
    };
    let (script, out) = (script.to_string_lossy().to_string(), out.to_string_lossy().to_string());
    let mut s = Session::default();
    let info = run(&mut s, "speech.synthesizer", json!({}));
    eprintln!("synthesizer: {info}");
    assert_eq!(info["ok"], json!(true), "{info}");
    let t0 = std::time::Instant::now();
    let r = run(&mut s, "script.synthesize", json!({"path": script, "out": out}));
    eprintln!("first run ({:.1} s): {r}", t0.elapsed().as_secs_f64());
    assert!(r["failed"].as_array().unwrap().is_empty(), "{r}");
    let n = r["synthesized"].as_u64().unwrap();
    assert!(n > 0);
    for l in r["lines"].as_array().unwrap() {
        let secs = l["seconds"].as_f64().unwrap();
        assert!((0.3..30.0).contains(&secs), "{l}");
        assert!(std::path::Path::new(l["path"].as_str().unwrap()).is_file());
    }
    let r2 = run(&mut s, "script.synthesize", json!({"path": script, "out": out}));
    assert_eq!((r2["synthesized"].clone(), r2["cached"].clone()), (json!(0), json!(n)), "{r2}");
    let lip = run(&mut s, "script.lipsync", json!({"path": script, "out": out}));
    eprintln!("lipsync: {lip}");
    assert_eq!(lip["lines"].as_u64().unwrap(), n);
}

/// `to` written relative to the current folder (`None` when there is no such path, e.g. another
/// drive).
fn relative_from_cwd(to: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let (cwd, to) = (std::fs::canonicalize(cwd).ok()?, std::fs::canonicalize(to).ok()?);
    let (a, b): (Vec<_>, Vec<_>) = (cwd.components().collect(), to.components().collect());
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return None;
    }
    let mut rel = PathBuf::new();
    for _ in common..a.len() {
        rel.push("..");
    }
    for c in &b[common..] {
        rel.push(c);
    }
    Some(rel)
}

/// A setup() script in the workspace's `target/` (so it has a path relative to the tests' folder),
/// with voices picked, a data folder for the default output and the script's absolute path.
fn setup_in_target(name: &str) -> (Session, PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("script-tests");
    std::fs::create_dir_all(&root).unwrap();
    let (mut s, _, tmp_dir) = setup(name);
    let dir = root.join(tmp_dir.file_name().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["m1.wav", "m2.wav", "story.fcscript.json"] {
        std::fs::copy(tmp_dir.join(f), dir.join(f)).unwrap();
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let dir = std::fs::canonicalize(&dir).unwrap();
    let dir = PathBuf::from(dir.to_string_lossy().trim_start_matches(r"\\?\"));
    std::fs::create_dir_all(dir.join("data")).unwrap();
    s.data_dir = Some(dir.join("data"));
    let path = dir.join("story.fcscript.json");
    run(&mut s, "script.pickVoices", json!({"path": path, "write": true}));
    (s, path, dir)
}

#[test]
fn the_cache_does_not_care_how_the_script_path_was_written() {
    let (mut s, abs, dir) = setup_in_target("pathform");
    let rel = relative_from_cwd(&abs).unwrap();
    assert!(rel.is_relative(), "{rel:?}");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let dotted = dir.join("sub").join("..").join("story.fcscript.json");
    let r = run(&mut s, "script.synthesize", json!({"path": rel}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(2), json!(0)), "{r}");
    let out = r["out"].clone();
    // the same script by its absolute path, and by a path with `..` in it: nothing to speak
    for p in [&abs, &dotted, &rel] {
        let r = run(&mut s, "script.synthesize", json!({"path": p}));
        assert_eq!((r["synthesized"].clone(), r["cached"].clone(), r["out"].clone()), (json!(0), json!(2), out.clone()), "{p:?}: {r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_manifest_from_before_the_path_free_key_is_still_a_cache() {
    use crate::script_tools::{Manifest, line_key, load, read_manifest};
    use filmcraft_speech::synth::Synthesizer;
    let (mut s, abs, dir) = setup_in_target("legacykey");
    let r = run(&mut s, "script.synthesize", json!({"path": abs}));
    assert_eq!(r["synthesized"], json!(2), "{r}");
    let out = PathBuf::from(r["out"].as_str().unwrap());
    // what older builds wrote when the script came as `E:/…/story.fcscript.json` (the eve tools):
    // the hash with the reference's path as written, `E:/…\m1.wav` on Windows
    let fwd = abs.to_string_lossy().replace('\\', "/");
    let l = load(&s, &json!({"path": fwd}), "test").unwrap();
    let synth_id = FixedSynthesizer::default().id();
    let mut m: Manifest = read_manifest(&out);
    for (_, line) in l.script.lines_in_route() {
        if let Some(e) = m.lines.get_mut(&line.id) {
            let k = line_key(&l, line, &l.script.language, "", &synth_id);
            assert_eq!(e.hash, k.hash);
            let old = k.legacy_hashes.first().unwrap().clone();
            // a preset speaker (kojiro) has no reference, and so the same hash in both forms
            assert_eq!(old != k.hash, k.voice.ref_audio.is_some(), "{}", line.id);
            e.hash = old;
        }
    }
    std::fs::write(out.join("manifest.json"), serde_json::to_string(&m).unwrap()).unwrap();
    // a later run by the relative path still finds them, and rewrites them in the new form
    let rel = relative_from_cwd(&abs).unwrap();
    let r = run(&mut s, "script.synthesize", json!({"path": rel}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(0), json!(2)), "{r}");
    let m = read_manifest(&out);
    for (_, line) in l.script.lines_in_route() {
        if let Some(e) = m.lines.get(&line.id) {
            assert_eq!(e.hash, line_key(&l, line, &l.script.language, "", &synth_id).hash, "{}", line.id);
        }
    }
    let r = run(&mut s, "script.synthesize", json!({"path": abs}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(0), json!(2)), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_labelled_line_is_spoken_from_the_voice_style_of_its_label() {
    use crate::script_tools::{line_key, load};
    use filmcraft_speech::synth::Synthesizer;
    let dir = temp("styles");
    write_wav16(&dir.join("calm.wav"), &tone(&[(3.0, 0.5)]), RATE).unwrap();
    write_wav16(&dir.join("angry.wav"), &tone(&[(2.0, 0.9)]), RATE).unwrap();
    let mut script = json!({
        "title": "styles",
        "characters": {"koj": {"name": "小次郎", "voice": {"refAudio": "calm.wav", "refText": "落ち着いた文。"}}},
        "scenes": [{"id": "a", "lines": [
            {"id": "a1", "speaker": "koj", "text": "このウジ虫野郎！", "instruct": "怒り（強）"},
            {"id": "a2", "speaker": "koj", "text": "ふむ。"},
            {"id": "a3", "speaker": "koj", "text": "おい。", "instruct": "怒りっぽく"}
        ]}]
    });
    let path = dir.join("story.fcscript.json");
    std::fs::write(&path, serde_json::to_string_pretty(&script).unwrap()).unwrap();
    let mut s = Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), ..Default::default() };
    let synth_id = FixedSynthesizer::default().id();
    // the hashes before the voice had styles
    let l = load(&s, &json!({"path": path}), "test").unwrap();
    let before: Vec<String> = l.script.lines_in_route().map(|(_, line)| line_key(&l, line, "Japanese", "", &synth_id).hash).collect();
    let out = dir.join("tts");
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!(r["synthesized"], json!(3), "{r}");
    // add an angry style: only the line labelled 怒り changes voice (and is spoken again)
    script["characters"]["koj"]["voice"]["styles"] = json!({"怒り": {"refAudio": "angry.wav", "refText": "怒った文！"}});
    std::fs::write(&path, serde_json::to_string_pretty(&script).unwrap()).unwrap();
    let l = load(&s, &json!({"path": path}), "test").unwrap();
    let keys: Vec<_> = l.script.lines_in_route().map(|(_, line)| line_key(&l, line, "Japanese", "", &synth_id)).collect();
    let name = |k: &crate::script_tools::LineKey| k.voice.ref_audio.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
    assert_eq!(name(&keys[0]).as_deref(), Some("angry.wav"));
    assert_eq!(keys[0].voice.ref_text.as_deref(), Some("怒った文！"));
    assert_eq!(name(&keys[1]).as_deref(), Some("calm.wav"));
    assert_eq!(name(&keys[2]).as_deref(), Some("calm.wav"), "怒りっぽく is not the label 怒り");
    assert_ne!(keys[0].hash, before[0]);
    assert_eq!((keys[1].hash.clone(), keys[2].hash.clone()), (before[1].clone(), before[2].clone()), "unlabelled lines keep their cache");
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(1), json!(2)), "{r}");
    // a new take of the angry reference redoes only the angry line
    write_wav16(&dir.join("angry.wav"), &tone(&[(2.5, 0.9)]), RATE).unwrap();
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!((r["synthesized"].clone(), r["cached"].clone()), (json!(1), json!(2)), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
