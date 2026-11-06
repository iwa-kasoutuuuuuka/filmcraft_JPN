//! One synthesizer interface for both text-to-speech routes: Qwen3-TTS (faked by
//! [`FixedSynthesizer`]) and the local AivisSpeech / VOICEVOX engines (faked by the HTTP engine of
//! `narration_tests`), in `speech.synthesize`, `script.synthesize` and `anime.run`, with the
//! voices' credits on what they make.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::narration_credits::{CREDIT_KEY, VOICE_KEY};
use crate::narration_tests::{Mode, fake};
use crate::speech_synth::Route;
use filmcraft_project::ItemId;
use filmcraft_speech::FixedSynthesizer;
use filmcraft_speech::tts::TtsEngine;

fn temp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("filmcraft-route-{name}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session whose Qwen3 route is the fixed synthesizer and whose AivisSpeech / VOICEVOX
/// addresses are fake engines.
fn session() -> Session {
    let mut s = Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), ..Default::default() };
    s.execute("file.newProject", json!({"name": "route"})).ok();
    s.prefs.narration.aivisspeech_url = fake(Mode::Good, true);
    s.prefs.narration.voicevox_url = fake(Mode::Good, false);
    s
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

fn write_script(path: &Path, characters: Value) {
    let script = json!({
        "title": "route",
        "characters": characters,
        "scenes": [{"id": "s", "lines": [
            {"id": "z1", "speaker": "zunda", "text": "こんにちは、ずんだもんなのだ。"},
            {"id": "m1", "speaker": "metan", "text": "よろしくね。"},
            {"id": "n1", "kind": "narration", "text": "ふたりは出会った。"}
        ]}]
    });
    std::fs::write(path, serde_json::to_string_pretty(&script).unwrap()).unwrap();
}

fn line<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["lines"].as_array().unwrap().iter().find(|l| l["id"] == id).unwrap_or_else(|| panic!("no line {id}: {r}"))
}

#[test]
fn routes_parse_and_agree_with_the_script_model() {
    assert_eq!(Route::from_id("qwen3"), Some(Route::Qwen3));
    assert_eq!(Route::from_id(" Qwen3-TTS "), Some(Route::Qwen3));
    assert_eq!(Route::from_id("AivisSpeech"), Some(Route::Local(TtsEngine::AivisSpeech)));
    assert_eq!(Route::from_id("VOICEVOX"), Some(Route::Local(TtsEngine::Voicevox)));
    assert_eq!(Route::from_id("mystery"), None);
    assert_eq!(Route::DEFAULT, Route::Qwen3);
    // the script model names the same local engines (it cannot see the speech crate)
    for id in filmcraft_project::script::LOCAL_ENGINES {
        let v = filmcraft_project::script::VoiceRef { engine: Some(id.to_uppercase()), ..Default::default() };
        assert_eq!(v.local_engine(), Some(id));
        assert_eq!(Route::from_id(id).map(Route::id), Some(id));
    }
    for e in TtsEngine::ALL {
        assert!(filmcraft_project::script::LOCAL_ENGINES.contains(&e.id()));
    }
    let mut s = session();
    let e = s.execute("speech.synthesize", json!({"engine": "mystery", "text": "x"})).unwrap_err().to_string();
    assert!(e.contains("unknown engine `mystery`"), "{e}");
}

#[test]
fn speech_synthesize_through_a_local_engine_credits_its_items() {
    let mut s = session();
    let dir = temp("speech");
    let undo0 = s.history.undo.len();
    let r = run(
        &mut s,
        "speech.synthesize",
        json!({"engine": "voicevox", "lines": [
            {"id": "a", "text": "こんにちは", "speaker": "四国めたん/ノーマル"},
            {"id": "b", "text": "またね"}
        ], "out": dir}),
    );
    assert_eq!((r["engine"].as_str(), r["synthesizer"].as_str()), (Some("voicevox"), Some("voicevox")), "{r}");
    assert_eq!(r["failed"], json!([]), "{r}");
    // 0.1 s per character from the fake engine
    assert!((line(&r, "a")["seconds"].as_f64().unwrap() - 0.5).abs() < 1e-3, "{r}");
    assert_eq!(line(&r, "a")["credit"]["credit"], "VOICEVOX:四国めたん");
    // no speaker: the engine's first voice
    assert_eq!(line(&r, "b")["credit"]["credit"], "VOICEVOX:ずんだもん");
    assert_eq!(r["creditText"], "VOICEVOX:四国めたん\nVOICEVOX:ずんだもん");
    let item = ItemId(line(&r, "a")["item"].as_u64().unwrap());
    let md = &s.project.item(item).unwrap().metadata;
    assert_eq!(md.get(CREDIT_KEY).map(String::as_str), Some("VOICEVOX:四国めたん"));
    assert!(md.get(VOICE_KEY).is_some_and(|v| v.contains("uuid-metan")));
    assert_eq!(s.history.undo.len(), undo0 + 1, "import and credits are one undo step");
    // the credit commands see them
    let c = run(&mut s, "tts.credits", json!({"scope": "project"}));
    assert_eq!(c["creditText"], "VOICEVOX:四国めたん\nVOICEVOX:ずんだもん", "{c}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_local_engine_that_is_not_running_says_so() {
    let mut s = session();
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        drop(l);
        format!("http://{a}")
    };
    let dir = temp("down");
    let e = s.execute("speech.synthesize", json!({"engine": "aivisspeech", "url": closed, "text": "テスト", "out": dir})).unwrap_err().to_string();
    assert!(e.contains("AivisSpeech is not running at") && e.contains("start the AivisSpeech app"), "{e}");
    let i = run(&mut s, "speech.synthesizer", json!({"engine": "aivisspeech", "url": closed}));
    assert_eq!(i["ok"], false);
    assert!(i["error"].as_str().unwrap().contains("start the AivisSpeech app"), "{i}");
    let i = run(&mut s, "speech.synthesizer", json!({"engine": "aivisspeech"}));
    assert_eq!((i["ok"].as_bool(), i["version"].as_str()), (Some(true), Some("0.20.0")), "{i}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_lines_of_an_aivisspeech_character_are_cached_and_redone_after_a_voice_change() {
    let mut s = session();
    let dir = temp("aivis");
    let path = dir.join("story.fcscript.json");
    let chars = |zunda_style: &str| {
        json!({
            "zunda": {"name": "ずんだもん", "voice": {"engine": "aivisspeech", "speaker": format!("ずんだもん/{zunda_style}")}},
            "metan": {"name": "四国めたん", "voice": {"engine": "aivisspeech", "speaker": "四国めたん"}}
        })
    };
    write_script(&path, chars("ノーマル"));
    let out = dir.join("out");
    // the whole script on AivisSpeech, narration included
    let p = json!({"path": path, "out": out, "engine": "aivisspeech"});
    let r = run(&mut s, "script.synthesize", p.clone());
    assert_eq!((r["synthesized"].as_u64(), r["cached"].as_u64()), (Some(3), Some(0)), "{r}");
    assert_eq!(r["synthesizer"], "aivisspeech");
    assert_eq!(line(&r, "z1")["engine"], "aivisspeech");
    assert_eq!(line(&r, "z1")["credit"]["credit"], "AivisSpeech:ずんだもん");
    assert_eq!(line(&r, "z1")["credit"]["license"], "ACML 1.0", "the model licence from /aivm_models: {r}");
    assert!(out.join("z1.wav").is_file());
    assert_eq!(r["creditText"], "AivisSpeech:ずんだもん\nAivisSpeech:四国めたん");
    // nothing changed: nothing is spoken again, and the credits come from the manifest
    let r = run(&mut s, "script.synthesize", p.clone());
    assert_eq!((r["synthesized"].as_u64(), r["cached"].as_u64()), (Some(0), Some(3)), "{r}");
    assert_eq!(line(&r, "z1")["credit"]["credit"], "AivisSpeech:ずんだもん");
    // another style for one character: only its line
    write_script(&path, chars("あまあま"));
    let r = run(&mut s, "script.synthesize", p.clone());
    assert_eq!((r["synthesized"].as_u64(), r["cached"].as_u64()), (Some(1), Some(2)), "{r}");
    assert_eq!(line(&r, "z1")["status"], "synthesized");
    // the engine closed: the cached lines stand (no voice model to compare them with)
    let mut q = p.clone();
    q["url"] = json!("http://127.0.0.1:1");
    let r = run(&mut s, "script.synthesize", q.clone());
    assert_eq!((r["synthesized"].as_u64(), r["cached"].as_u64()), (Some(0), Some(3)), "{r}");
    // ... but a line to speak fails with the engine's own words
    let e = s
        .execute("script.synthesize", json!({"path": path, "out": out, "engine": "aivisspeech", "url": "http://127.0.0.1:1", "force": true}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("AivisSpeech is not running at http://127.0.0.1:1"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mixed_script_speaks_each_character_with_its_engine_and_credits_the_anime() {
    let mut s = session();
    let dir = temp("mixed");
    let path = dir.join("mixed.fcscript.json");
    // zunda: AivisSpeech; metan: VOICEVOX; the narrator and anyone without an engine: Qwen3
    write_script(
        &path,
        json!({
            "zunda": {"name": "ずんだもん", "voice": {"engine": "aivisspeech", "speaker": "ずんだもん/ノーマル"}},
            "metan": {"name": "四国めたん", "voice": {"engine": "VOICEVOX", "speaker": "四国めたん/ノーマル"}}
        }),
    );
    let out = dir.join("out");
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": out}));
    assert_eq!(r["synthesized"], 3, "{r}");
    assert_eq!(r["synthesizer"], "fixed", "the default route is Qwen3: {r}");
    let used: Vec<&str> = r["synthesizers"].as_array().unwrap().iter().map(|x| x["engine"].as_str().unwrap()).collect();
    assert_eq!(used, ["qwen3", "aivisspeech", "voicevox"], "{r}");
    assert_eq!(line(&r, "n1")["engine"], "qwen3");
    assert!(line(&r, "n1").get("credit").is_none(), "no credit line for a Qwen3 voice: {r}");
    assert_eq!(line(&r, "z1")["credit"]["credit"], "AivisSpeech:ずんだもん");
    assert_eq!(line(&r, "m1")["credit"]["credit"], "VOICEVOX:四国めたん");
    // the fixed synthesizer's tone (0.15 s per character) against the fake engine's 0.1 s
    let secs = |id: &str| line(&r, id)["seconds"].as_f64().unwrap();
    assert!((secs("n1") - 0.15 * 9.0).abs() < 1e-3, "{r}");
    assert!((secs("m1") - 0.1 * 5.0).abs() < 1e-3, "{r}");
    let v = run(&mut s, "script.validate", json!({"path": path}));
    assert_eq!(v["ok"], true, "{v}");

    // the whole pipeline: the anime's narration items carry the credits
    let a = run(&mut s, "anime.run", json!({"path": path, "out": out, "check": false}));
    assert_eq!(a["ok"], true, "{a}");
    let synth = a["steps"].as_array().unwrap().iter().find(|x| x["step"] == "synthesize").unwrap();
    assert_eq!(synth["cached"], 3, "{a}");
    let c = run(&mut s, "tts.credits", json!({"sequence": a["sequence"]}));
    assert_eq!(c["creditText"], "AivisSpeech:ずんだもん\nVOICEVOX:四国めたん", "{c}");
    let roll = run(&mut s, "tts.addCreditRoll", json!({}));
    assert!(roll.to_string().contains("VOICEVOX:四国めたん"), "{roll}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validate_warns_about_a_local_voice_without_a_speaker() {
    let mut s = session();
    let dir = temp("warn");
    let path = dir.join("w.fcscript.json");
    write_script(&path, json!({"zunda": {"voice": {"engine": "aivisspeech"}}, "metan": {"voice": {"engine": "voicevox", "speaker": "四国めたん"}}}));
    let v = run(&mut s, "script.validate", json!({"path": path}));
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["problems"].to_string().contains("aivisspeech voice has no speaker"), "{v}");
    // and it still speaks, in the engine's first voice
    let r = run(&mut s, "script.synthesize", json!({"path": path, "out": dir.join("o"), "ids": ["z1"]}));
    assert_eq!(line(&r, "z1")["credit"]["credit"], "AivisSpeech:ずんだもん", "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Real engines on this machine, when they are running (skipped otherwise).
#[test]
fn real_local_engines_speak_when_they_are_running() {
    for e in TtsEngine::ALL {
        let addr = e.default_url().trim_start_matches("http://").to_string();
        let Ok(sock) = addr.parse::<std::net::SocketAddr>() else { continue };
        if std::net::TcpStream::connect_timeout(&sock, std::time::Duration::from_millis(300)).is_err() {
            eprintln!("{} is not running: skipped", e.name());
            continue;
        }
        let mut s = Session::default();
        let dir = temp(e.id());
        let r = s.execute("speech.synthesize", json!({"engine": e.id(), "text": "テストです。", "out": dir, "import": false}));
        let r = match r {
            Ok(r) => r,
            Err(err) => panic!("{}: {err}", e.name()),
        };
        let l = &r["lines"][0];
        assert!(l["seconds"].as_f64().unwrap_or(0.0) > 0.2, "{r}");
        assert!(l["credit"]["credit"].as_str().unwrap_or("").starts_with(match e {
            TtsEngine::AivisSpeech => "AivisSpeech:",
            TtsEngine::Voicevox => "VOICEVOX:",
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
