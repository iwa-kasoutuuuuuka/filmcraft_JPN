use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::anime::{Audio, Opts, PlanEnv, plan, split_caption};
use filmcraft_project::script::{Script, ScriptLine};
use filmcraft_speech::FixedSynthesizer;
use filmcraft_speech::synth::write_wav16;
use filmcraft_time::{FrameRate, Tick};

const RATE: u32 = 16_000;

fn tone(pieces: &[(f64, f32)]) -> Vec<f32> {
    let mut v = Vec::new();
    for (secs, amp) in pieces {
        let t0 = v.len();
        v.extend((0..(secs * RATE as f64) as usize).map(|k| amp * ((t0 + k) as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin()));
    }
    v
}

/// A solid-colour 24-bit BMP (the importer reads them).
fn bmp(path: &Path, w: u32, h: u32, rgb: [u8; 3]) {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::new();
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&h.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&(row * h).to_le_bytes());
    b.extend_from_slice(&[0x13, 0x0b, 0, 0, 0x13, 0x0b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for _ in 0..h {
        let mut r = Vec::new();
        for _ in 0..w {
            r.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
        r.resize(row as usize, 0);
        b.extend_from_slice(&r);
    }
    std::fs::write(path, b).unwrap();
}

fn temp(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("filmcraft-anime-{name}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

// ---------------------------------------------------------------------------------------------
// the plan, with files faked

#[derive(Default)]
struct Fake {
    audio: HashMap<String, f64>,
    mouth: Vec<(f64, f64)>,
    missing: Vec<String>,
}

impl PlanEnv for Fake {
    fn resolve(&self, rel: &str) -> PathBuf {
        PathBuf::from(rel)
    }
    fn voice(&mut self, line: &ScriptLine) -> Option<Audio> {
        let d = *self.audio.get(&line.id)?;
        Some(Audio { path: PathBuf::from(format!("{}.wav", line.id)), duration: Tick::from_seconds_f64(d) })
    }
    fn audio_file(&mut self, path: &Path) -> Option<Audio> {
        let d = *self.audio.get(path.to_str()?)?;
        Some(Audio { path: path.to_path_buf(), duration: Tick::from_seconds_f64(d) })
    }
    fn image(&mut self, path: &Path) -> bool {
        !self.missing.iter().any(|m| path.to_str() == Some(m.as_str()))
    }
    fn mouth(&mut self, _: &Audio) -> Vec<(f64, f64)> {
        self.mouth.clone()
    }
}

fn story() -> Script {
    Script::from_json(
        r#"{
        "characters": {"m": {"name": "m", "expressions": {
            "normal": {"image": "m.png", "openImage": "mo.png"},
            "normal_blink": {"image": "mb.png"},
            "angry": {"image": "ma.png"}}}},
        "scenes": [
          {"id": "s1", "background": "bg1.png", "bgm": "music.wav", "lines": [
            {"id": "n1", "kind": "narration", "text": "夜だった。"},
            {"id": "a1", "speaker": "m", "text": "待って。"},
            {"id": "a2", "speaker": "m", "text": "どうして？", "pauseAfter": 1.0},
            {"id": "a3", "speaker": "m", "text": "やめて！", "expression": "angry"},
            {"id": "d1", "kind": "direction", "sfx": "door.wav"}]},
          {"id": "s2", "background": "bg2.png", "bgm": "music.wav", "lines": [
            {"id": "a4", "speaker": "m", "text": "朝になった。"}]}]}"#,
    )
    .unwrap()
}

fn fake() -> Fake {
    let mut f = Fake::default();
    for (k, v) in [("a1", 2.0), ("a2", 3.0), ("a3", 1.5), ("a4", 2.5), ("door.wav", 0.5), ("music.wav", 30.0)] {
        f.audio.insert(k.into(), v);
    }
    f.mouth = vec![(0.2, 0.7), (1.0, 1.4)];
    f
}

#[test]
fn the_plan_puts_every_line_after_the_last_on_the_frame_grid() {
    let rate = FrameRate::from_f64(24.0);
    let o = Opts::default();
    let pl = plan(&story(), rate, &o, &mut fake());
    // contiguous, on the grid
    let mut at = Tick::ZERO;
    for sc in &pl.scenes {
        assert_eq!(rate.snap_nearest(sc.start), sc.start, "{sc:?}");
        assert!(sc.start >= at);
        at = sc.end;
    }
    for w in pl.cues.windows(2) {
        assert!(w[0].end <= w[1].start, "{:?} {:?}", w[0], w[1]);
        assert_eq!(rate.snap_nearest(w[0].end), w[0].end);
    }
    assert_eq!(pl.end, pl.scenes.last().unwrap().end);
    assert_eq!(pl.scenes[0].end, pl.scenes[1].start, "scenes abut");
    // the first scene starts with a beat of background (0.6 s)
    assert_eq!(pl.cues[0].start, rate.snap_nearest(Tick::from_seconds_f64(0.6)));
    // a1: 2 s of speech and the default 0.35 s pause; a2: 3 s and its own 1 s pause
    let a1 = pl.cues.iter().find(|c| c.line == "a1").unwrap();
    assert_eq!(a1.end - a1.start, rate.snap_nearest(Tick::from_seconds_f64(2.35)));
    let a2 = pl.cues.iter().find(|c| c.line == "a2").unwrap();
    assert_eq!(a2.end - a2.start, rate.snap_nearest(Tick::from_seconds_f64(4.0)));
    // the direction lasts as long as its sound effect
    let d1 = pl.cues.iter().find(|c| c.line == "d1").unwrap();
    assert_eq!(d1.end - d1.start, Tick::from_seconds_f64(0.5));
    assert!(d1.sfx.is_some() && d1.sprite.is_none() && d1.captions.is_empty());
    // n1 has no audio: estimated, and said so
    let n1 = &pl.cues[0];
    assert!(n1.estimated && pl.warnings.iter().any(|w| w.contains("n1")), "{:?}", pl.warnings);
}

#[test]
fn consecutive_lines_in_one_picture_are_one_run_with_blinks_inside() {
    let rate = FrameRate::from_f64(24.0);
    let pl = plan(&story(), rate, &Opts::default(), &mut fake());
    // a1 + a2 share the normal picture; a3 is angry (no blink picture of its own: it falls back
    // to none, since `angry_blink` and `blink` do not exist); a4 is in the next scene
    assert_eq!(pl.runs.len(), 3, "{:?}", pl.runs.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>());
    let r = &pl.runs[0];
    let a1 = pl.cues.iter().find(|c| c.line == "a1").unwrap();
    let a2 = pl.cues.iter().find(|c| c.line == "a2").unwrap();
    assert_eq!((r.start, r.end), (a1.start, a2.end));
    assert!(!r.blinks.is_empty());
    for (a, b) in &r.blinks {
        assert!(*a >= r.start && *b <= r.end && b > a, "{a:?} {b:?}");
        assert_eq!(rate.snap_nearest(*a), *a);
    }
    assert!(r.blinks.windows(2).all(|w| w[0].1 < w[1].0), "blinks apart");
    assert!(pl.runs[1].blinks.is_empty());
    // deterministic
    assert_eq!(pl, plan(&story(), rate, &Opts::default(), &mut fake()));
    // blinking can be turned off
    let off = plan(&story(), rate, &Opts { blink: false, ..Opts::default() }, &mut fake());
    assert!(off.runs.iter().all(|r| r.blinks.is_empty()));
}

#[test]
fn the_mouth_opens_where_the_voice_is_loud_and_never_overlaps() {
    let rate = FrameRate::from_f64(24.0);
    let pl = plan(&story(), rate, &Opts::default(), &mut fake());
    let a1 = pl.cues.iter().find(|c| c.line == "a1").unwrap();
    assert_eq!(a1.mouth.len(), 2);
    assert!((a1.mouth[0].0 - a1.start).seconds() - 0.2 < 0.05);
    for c in &pl.cues {
        for (a, b) in &c.mouth {
            assert!(*a >= c.start && *b <= c.end && b > a);
        }
        assert!(c.mouth.windows(2).all(|w| w[0].1 <= w[1].0));
    }
    // a3 has no open picture: no mouth clips
    assert!(pl.cues.iter().find(|c| c.line == "a3").unwrap().mouth.is_empty());
}

#[test]
fn music_runs_across_scenes_that_share_it_and_subtitles_cover_the_speech() {
    let rate = FrameRate::from_f64(24.0);
    let pl = plan(&story(), rate, &Opts::default(), &mut fake());
    assert_eq!(pl.bgm.len(), 1, "{:?}", pl.bgm);
    assert_eq!((pl.bgm[0].start, pl.bgm[0].end), (Tick::ZERO, pl.end));
    for c in pl.cues.iter().filter(|c| c.voice.is_some()) {
        let cap = &c.captions;
        assert!(!cap.is_empty());
        assert_eq!(cap[0].0, c.start);
        assert!(cap.last().unwrap().1 <= c.end);
        assert!(cap.windows(2).all(|w| w[0].1 <= w[1].0));
    }
}

#[test]
fn missing_pictures_and_sounds_are_warned_about_and_skipped() {
    let mut f = fake();
    f.missing = vec!["bg2.png".into(), "m.png".into()];
    f.audio.remove("door.wav");
    let pl = plan(&story(), FrameRate::from_f64(24.0), &Opts::default(), &mut f);
    assert!(pl.scenes[1].background.is_none() && pl.scenes[0].background.is_some());
    assert!(pl.warnings.iter().any(|w| w.contains("bg2.png")));
    assert!(pl.warnings.iter().any(|w| w.contains("door.wav")));
    assert!(pl.cues.iter().find(|c| c.line == "a1").unwrap().sprite.is_none());
    // the direction without its sound still lasts the default second
    let d1 = pl.cues.iter().find(|c| c.line == "d1").unwrap();
    assert_eq!(d1.end - d1.start, Tick::from_seconds_f64(1.0));
}

#[test]
fn long_subtitles_break_at_sentences_then_commas_and_cover_the_time() {
    assert_eq!(split_caption("短い。", 36), vec!["短い。"]);
    assert!(split_caption("   ", 36).is_empty());
    let long = "あ".repeat(20) + "。" + &"い".repeat(20) + "。" + &"う".repeat(5);
    let parts = split_caption(&long, 24);
    assert_eq!(parts.len(), 3, "{parts:?}");
    assert!(parts.iter().all(|p| p.chars().count() <= 24));
    assert_eq!(parts.concat(), long);
    // a sentence too long for one piece breaks at commas, then anywhere
    let commas = format!("{}、{}、{}", "あ".repeat(12), "い".repeat(12), "う".repeat(12));
    let parts = split_caption(&commas, 14);
    assert!(parts.len() >= 3 && parts.iter().all(|p| p.chars().count() <= 14), "{parts:?}");
    let blob = "あ".repeat(100);
    let parts = split_caption(&blob, 30);
    assert!(parts.iter().all(|p| p.chars().count() <= 30) && parts.concat() == blob);
    // a tiny limit is raised, not obeyed to the point of absurdity
    assert!(split_caption(&blob, 1).iter().all(|p| p.chars().count() <= 8));
}

// ---------------------------------------------------------------------------------------------
// building, checking and running with real files

struct Project {
    dir: PathBuf,
    path: PathBuf,
}

fn project(name: &str, extra_lines: &str) -> Project {
    let dir = temp(name);
    for (f, c) in [("bg1.bmp", [10, 20, 80]), ("bg2.bmp", [80, 20, 10]), ("m.bmp", [200, 200, 200]), ("mo.bmp", [220, 180, 180]), ("mb.bmp", [150, 150, 150])] {
        bmp(&dir.join(f), 32, 18, c);
    }
    write_wav16(&dir.join("v1.wav"), &tone(&[(0.5, 0.0), (1.5, 0.5), (0.5, 0.0), (1.5, 0.5)]), RATE).unwrap();
    write_wav16(&dir.join("v2.wav"), &tone(&[(2.0, 0.5)]), RATE).unwrap();
    write_wav16(&dir.join("bgm.wav"), &tone(&[(2.0, 0.2)]), RATE).unwrap();
    write_wav16(&dir.join("door.wav"), &tone(&[(0.5, 0.3)]), RATE).unwrap();
    let script = format!(
        r#"{{
        "title": "Test story",
        "characters": {{"marina": {{"name": "まりな",
            "voice": {{"speaker": "Ono_Anna"}},
            "expressions": {{"normal": {{"image": "m.bmp", "openImage": "mo.bmp"}}, "blink": {{"image": "mb.bmp"}}}}}}}},
        "scenes": [
          {{"id": "s1", "background": "bg1.bmp", "bgm": "bgm.wav", "lines": [
            {{"id": "n1", "kind": "narration", "text": "夜だった。"}},
            {{"id": "a1", "speaker": "marina", "text": "待って、まだ話は終わってない。", "voiceFile": "v1.wav"}},
            {{"id": "a2", "speaker": "marina", "text": "どうして？", "voiceFile": "v2.wav"}},
            {{"id": "d1", "kind": "direction", "sfx": "door.wav"}}{extra_lines}]}},
          {{"id": "s2", "background": "bg2.bmp", "bgm": "bgm.wav", "lines": [
            {{"id": "a3", "speaker": "marina", "text": "ええ、そうよ。"}}]}}]}}"#
    );
    let path = dir.join("story.fcscript.json");
    std::fs::write(&path, script).unwrap();
    Project { dir, path }
}

fn session() -> Session {
    Session { synthesizer: Some(Arc::new(FixedSynthesizer::default())), ..Default::default() }
}

fn params(pr: &Project) -> Value {
    json!({"path": pr.path, "out": pr.dir.join("tts")})
}

#[test]
fn a_script_becomes_a_sequence_in_one_undo_step() {
    let pr = project("build", "");
    let mut s = session();
    run(&mut s, "script.synthesize", params(&pr));
    let n_seq = s.project.sequences().count();
    let n_items = s.project.items.len();
    let r = run(&mut s, "anime.build", params(&pr));
    let c = &r["clips"];
    assert_eq!(r["lines"], json!(5), "{r}");
    assert_eq!(c["backgrounds"], json!(2), "{r}");
    assert_eq!(c["characters"], json!(2), "{r}");
    assert_eq!(c["voices"], json!(4), "{r}");
    assert_eq!(c["effects"], json!(1), "{r}");
    assert!(c["mouths"].as_u64().unwrap() >= 2 && c["blinks"].as_u64().unwrap() >= 1, "{r}");
    assert!(c["music"].as_u64().unwrap() >= 2, "music repeats to fill the scenes: {r}");
    assert!(c["subtitles"].as_u64().unwrap() >= 4, "{r}");
    assert!(r["withoutAudio"].as_array().unwrap().is_empty(), "{r}");

    let q = s.active_sequence().unwrap().clone();
    assert_eq!(q.settings.frame_rate, FrameRate::from_f64(24.0));
    let one = q.settings.frame_rate.frame_duration();
    // V1: backgrounds abut from time zero, and the first one pushes in
    let v1 = &q.video_tracks[0].items;
    assert_eq!(v1.len(), 2);
    assert_eq!(v1[0].start, Tick::ZERO);
    assert_eq!(v1[0].end(), v1[1].start);
    let scale = v1[0].effects.iter().find(|e| e.effect == "motion").and_then(|e| e.params.get("scale")).unwrap();
    assert_eq!(scale.keyframes.len(), 2);
    assert!(scale.keyframes[1].value.as_f64().unwrap() > scale.keyframes[0].value.as_f64().unwrap());
    // A1: the voices in order, as long as their audio, never overlapping
    let a1 = &q.audio_tracks[0].items;
    assert_eq!(a1.len(), 4);
    assert!(a1.windows(2).all(|w| w[0].end() <= w[1].start));
    let a1_2 = a1.iter().find(|i| (i.duration.seconds() - 2.0).abs() < 0.05).expect("the 2 s recording");
    assert!((a1_2.duration - Tick::from_seconds_f64(2.0)).0.abs() <= one.0);
    // the character sits on V2 and its open mouth on V3, inside the run
    let v2 = &q.video_tracks[1].items;
    let v3 = &q.video_tracks[2].items;
    assert_eq!(v2.len(), 2);
    assert!(v3.iter().all(|m| v2.iter().any(|r| m.start >= r.start && m.end() <= r.end())));
    // subtitles
    assert!(q.caption_tracks[0].captions.iter().any(|c| c.text.contains("待って")));
    assert_eq!(q.caption_tracks[0].language, "ja");
    // names
    assert_eq!(q.video_tracks[1].name, "Character");
    assert_eq!(q.audio_tracks[0].name, "Voice");
    // one undo step takes back the sequence, the bin and the imports
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project.sequences().count(), n_seq);
    assert_eq!(s.project.items.len(), n_items);
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn building_before_speaking_estimates_the_lines_and_says_so() {
    let pr = project("estimate", "");
    let mut s = session();
    let r = run(&mut s, "anime.build", params(&pr));
    let missing: Vec<&str> = r["withoutAudio"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(missing, vec!["n1", "a3"], "{r}");
    assert_eq!(r["clips"]["voices"], json!(2));
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("no audio yet")));
    // a second build is another sequence, named apart
    let r2 = run(&mut s, "anime.build", params(&pr));
    assert_ne!(r["name"], r2["name"]);
    assert!(r2["name"].as_str().unwrap().ends_with("(2)"));
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn a_bad_script_builds_nothing() {
    let pr = project("bad", ",{\"id\": \"zz\", \"speaker\": \"ghost\", \"text\": \"x\"}");
    let mut s = session();
    let n = s.project.items.len();
    let hist = s.history.undo.len();
    let e = s.execute("anime.build", params(&pr)).unwrap_err().to_string();
    assert!(e.contains("ghost"), "{e}");
    assert_eq!((s.project.items.len(), s.history.undo.len()), (n, hist));
    std::fs::write(&pr.path, r#"{"scenes":[]}"#).unwrap();
    assert!(s.execute("anime.build", params(&pr)).unwrap_err().to_string().contains("no lines"));
    assert!(s.execute("anime.build", json!({"path": pr.path, "fps": 0})).is_err());
    assert!(s.execute("anime.build", json!({"path": pr.path, "push": 500})).is_err());
    assert!(s.execute("anime.build", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn the_check_finds_gaps_overlaps_silences_and_odd_voices() {
    let pr = project("check", "");
    let mut s = session();
    run(&mut s, "script.synthesize", params(&pr));
    let built = run(&mut s, "anime.build", params(&pr));
    let seq = built["sequence"].as_u64().unwrap();
    let r = run(&mut s, "anime.check", json!({"sequence": seq, "path": pr.path, "out": pr.dir.join("tts")}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["stats"]["voices"], json!(4));
    assert_eq!(r["stats"]["linesWithoutAudio"], json!(0));

    // break it: a voice on top of the one before, a hole in the background
    // (straight into the project: the edit commands refuse to make an overlap)
    s.edit("break", |pr, _| {
        let q = pr.sequence_mut(filmcraft_project::ItemId(seq)).unwrap();
        let start = q.audio_tracks[0].items[0].start + Tick::from_seconds_f64(0.1);
        q.audio_tracks[0].items[1].start = start;
        q.video_tracks[0].items.remove(1);
        Ok(())
    })
    .unwrap();
    let r = run(&mut s, "anime.check", json!({"sequence": seq}));
    assert_eq!(r["ok"], json!(false));
    let kinds: Vec<&str> = r["problems"].as_array().unwrap().iter().map(|p| p["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"overlap"), "{kinds:?}");
    assert!(kinds.contains(&"gap"), "{kinds:?}");
    // a script line without audio is reported when the script is named
    let r = run(&mut s, "anime.check", json!({"sequence": seq, "path": pr.path, "out": pr.dir.join("nowhere")}));
    assert!(r["stats"]["linesWithoutAudio"].as_u64().unwrap() >= 2, "{r}");
    assert!(s.execute("anime.check", json!({"sequence": 999_999})).is_err());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn a_voice_far_quieter_than_the_rest_is_flagged() {
    let dir = temp("loud");
    bmp(&dir.join("bg.bmp"), 32, 18, [1, 2, 3]);
    let mut lines = Vec::new();
    for (i, amp) in [0.5f32, 0.5, 0.5, 0.01].iter().enumerate() {
        write_wav16(&dir.join(format!("v{i}.wav")), &tone(&[(1.0, *amp)]), RATE).unwrap();
        lines.push(json!({"id": format!("l{i}"), "kind": "narration", "text": "声", "voiceFile": format!("v{i}.wav")}));
    }
    let path = dir.join("l.fcscript.json");
    std::fs::write(&path, serde_json::to_string(&json!({"scenes": [{"id": "s", "background": "bg.bmp", "lines": lines}]})).unwrap()).unwrap();
    let mut s = session();
    let b = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o"), "levelVoices": false}));
    let r = run(&mut s, "anime.check", json!({"sequence": b["sequence"]}));
    let loud: Vec<&Value> = r["problems"].as_array().unwrap().iter().filter(|p| p["kind"] == "loudness").collect();
    assert_eq!(loud.len(), 1, "{r}");
    assert!(loud[0]["message"].as_str().unwrap().contains("LU"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_call_runs_every_step_and_a_second_call_speaks_nothing() {
    let pr = project("run", "");
    let mut s = session();
    let r = run(&mut s, "anime.run", params(&pr));
    assert_eq!(r["ok"], json!(true), "{r}");
    let names: Vec<&str> = r["steps"].as_array().unwrap().iter().map(|x| x["step"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["validate", "synthesize", "build", "check"]);
    assert_eq!(r["steps"][1]["synthesized"], json!(2));
    let r2 = run(&mut s, "anime.run", params(&pr));
    assert_eq!((r2["steps"][1]["synthesized"].clone(), r2["steps"][1]["cached"].clone()), (json!(0), json!(2)), "{r2}");
    assert_ne!(r["sequence"], r2["sequence"]);
    // a step that fails stops the run and names itself
    std::fs::write(&pr.path, r#"{"scenes":[{"id":"s","lines":[{"id":"a","speaker":"nobody","text":"x"}]}]}"#).unwrap();
    let r3 = run(&mut s, "anime.run", params(&pr));
    assert_eq!((r3["ok"].clone(), r3["failedStep"].clone()), (json!(false), json!("validate")), "{r3}");
    assert!(!r3["problems"].as_array().unwrap().is_empty());
    assert!(s.execute("anime.run", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn pick_voices_and_skipping_synthesis_are_options() {
    let pr = project("opts", "");
    // marina already has a voice here; give the script a character without one
    let text = std::fs::read_to_string(&pr.path).unwrap().replace(r#""voice": {"speaker": "Ono_Anna"},"#, "");
    std::fs::write(&pr.path, text).unwrap();
    let mut s = session();
    let r = run(&mut s, "anime.run", json!({"path": pr.path, "out": pr.dir.join("tts"), "pickVoices": true, "synthesize": false}));
    let names: Vec<&str> = r["steps"].as_array().unwrap().iter().map(|x| x["step"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["validate", "pickVoices", "build", "check"], "{r}");
    // recorded lines were enough to build; the two unspoken lines are reported
    assert_eq!(r["steps"][2]["withoutAudio"].as_array().unwrap().len(), 2);
    assert!(r["steps"][3]["stats"]["linesWithoutAudio"].as_u64().unwrap() >= 2);
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn the_run_can_export_the_finished_sequence() {
    let pr = project("export", "");
    let mut s = session();
    let out = pr.dir.join("episode.mp4");
    let r = run(&mut s, "anime.run", json!({"path": pr.path, "out": pr.dir.join("tts"), "width": 320, "height": 180, "fps": 12, "export": out}));
    assert_eq!(r["ok"], json!(true), "{r}");
    let last = r["steps"].as_array().unwrap().last().unwrap();
    assert_eq!(last["step"], json!("export"), "{r}");
    let meta = std::fs::metadata(&out).unwrap_or_else(|e| panic!("{e}: {last}"));
    assert!(meta.len() > 1000, "{} bytes", meta.len());
    // the file plays: picture and sound, as long as the sequence
    let src = s.media.open_file(&out.to_string_lossy(), &*s.services).unwrap();
    let want = r["steps"][2]["seconds"].as_f64().unwrap();
    assert!(src.info().has_audio() && src.info().video.is_some());
    assert!((src.info().duration.seconds() - want).abs() < 0.25, "{} vs {want}", src.info().duration.seconds());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

// ---------------------------------------------------------------------------------------------
// regressions found by review

/// A script of `n` narration lines, each with its own recording, and the pictures to match.
fn many(name: &str, n: usize, bgm_secs: f64) -> Project {
    let dir = temp(name);
    bmp(&dir.join("bg.bmp"), 32, 18, [1, 2, 3]);
    write_wav16(&dir.join("bgm.wav"), &tone(&[(bgm_secs, 0.2)]), RATE).unwrap();
    let mut lines = Vec::new();
    for i in 0..n {
        write_wav16(&dir.join(format!("v{i}.wav")), &tone(&[(0.5, 0.4)]), RATE).unwrap();
        lines.push(json!({"id": format!("l{i}"), "kind": "narration", "text": "声", "voiceFile": format!("v{i}.wav")}));
    }
    let path = dir.join("m.fcscript.json");
    std::fs::write(&path, serde_json::to_string(&json!({"scenes": [{"id": "s", "background": "bg.bmp", "bgm": "bgm.wav", "lines": lines}]})).unwrap()).unwrap();
    Project { dir, path }
}

fn no_overlap(items: &[filmcraft_project::TrackItem]) -> bool {
    items.windows(2).all(|w| w[0].end() <= w[1].start)
}

#[test]
fn music_whose_length_is_not_a_whole_number_of_frames_does_not_overlap_itself() {
    // 1.03 s at 24 fps is 24.72 frames: the repeat used to start inside the last
    let pr = many("bgm", 12, 1.03);
    let mut s = session();
    let r = run(&mut s, "anime.build", json!({"path": pr.path, "out": pr.dir.join("o")}));
    assert!(r["clips"]["music"].as_u64().unwrap() >= 5, "{r}");
    let q = s.active_sequence().unwrap();
    assert!(no_overlap(&q.audio_tracks[1].items));
    let rate = q.settings.frame_rate;
    assert!(q.audio_tracks[1].items.iter().all(|i| rate.snap_nearest(i.start) == i.start), "on the grid");
    assert_eq!(q.audio_tracks[1].items.last().unwrap().end(), q.video_tracks[0].items.last().unwrap().end());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn a_scene_without_lines_still_gets_its_own_frame() {
    let dir = temp("empty");
    bmp(&dir.join("a.bmp"), 32, 18, [1, 2, 3]);
    bmp(&dir.join("b.bmp"), 32, 18, [3, 2, 1]);
    write_wav16(&dir.join("v.wav"), &tone(&[(0.5, 0.4)]), RATE).unwrap();
    let path = dir.join("e.fcscript.json");
    let script = json!({"scenes": [
        {"id": "a", "background": "a.bmp", "lines": [{"id": "l", "kind": "narration", "text": "声", "voiceFile": "v.wav"}]},
        {"id": "b", "background": "b.bmp", "lines": []},
        {"id": "c", "background": "a.bmp", "lines": [{"id": "m", "kind": "narration", "text": "声", "voiceFile": "v.wav"}]}]});
    std::fs::write(&path, script.to_string()).unwrap();
    let mut s = session();
    run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o"), "sceneGap": 0}));
    let v1 = &s.active_sequence().unwrap().video_tracks[0].items;
    assert_eq!(v1.len(), 3);
    assert!(no_overlap(v1) && v1[1].duration >= s.active_sequence().unwrap().settings.frame_rate.frame_duration());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_long_sound_effect_ends_with_the_sequence_and_the_check_agrees() {
    let dir = temp("sfx");
    bmp(&dir.join("bg.bmp"), 32, 18, [1, 2, 3]);
    write_wav16(&dir.join("v.wav"), &tone(&[(0.5, 0.4)]), RATE).unwrap();
    write_wav16(&dir.join("boom.wav"), &tone(&[(6.0, 0.3)]), RATE).unwrap();
    let path = dir.join("s.fcscript.json");
    std::fs::write(&path, json!({"scenes": [{"id": "s", "background": "bg.bmp", "lines": [{"id": "l", "kind": "narration", "text": "声", "voiceFile": "v.wav", "sfx": "boom.wav"}]}]}).to_string()).unwrap();
    let mut s = session();
    let b = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o")}));
    let q = s.active_sequence().unwrap();
    let end = q.video_tracks[0].items.last().unwrap().end();
    assert!(q.audio_tracks[2].items.iter().all(|i| i.end() <= end), "the effect runs past the end");
    let r = run(&mut s, "anime.check", json!({"sequence": b["sequence"]}));
    assert_eq!(r["ok"], json!(true), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn subtitles_stay_inside_their_time_even_when_it_is_short() {
    let rate = FrameRate::from_f64(24.0);
    let one = rate.frame_duration();
    let pieces = vec!["あ".repeat(36), "い".repeat(2)];
    let end = Tick(one.0 * 2);
    let out = crate::anime::spread(&pieces, Tick::ZERO, end, rate);
    assert_eq!(out.len(), 2);
    assert_eq!(out.last().unwrap().1, end);
    assert!(out.windows(2).all(|w| w[0].1 == w[1].0) && out.iter().all(|c| c.1 > c.0));
    let pieces = vec!["あ".repeat(100), "い".to_string(), "う".to_string()];
    let out = crate::anime::spread(&pieces, Tick::ZERO, Tick(one.0 * 3), rate);
    assert_eq!(out.last().unwrap().1, Tick(one.0 * 3));
    assert!(out.iter().all(|c| c.1 > c.0));
    // too little time for every piece: one subtitle, words not run together
    let out = crate::anime::spread(&["hello".to_string(), "world".to_string()], Tick::ZERO, one, rate);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].2, "hello world");
}

#[test]
fn a_build_is_one_undo_step_even_when_the_history_is_nearly_full() {
    let pr = many("history", 60, 2.0);
    let mut s = session();
    s.history.limit = 40;
    for i in 0..38 {
        run(&mut s, "file.newBin", json!({"name": format!("b{i}")}));
    }
    let before = s.history.undo.len();
    let items = s.project.items.len();
    run(&mut s, "anime.build", json!({"path": pr.path, "out": pr.dir.join("o")}));
    assert_eq!(s.history.undo.len(), before + 1, "one step");
    assert!(s.history.undo.len() <= s.history.limit);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project.items.len(), items, "all of it undone, nothing more");
    assert_eq!(s.history.undo.len(), before);
    // a build that is refused leaves no history and nothing in the project
    s.history.limit = 200;
    let redo = s.history.redo.len();
    assert!(s.execute("anime.build", json!({"path": pr.path, "out": pr.dir.join("o"), "width": 0})).is_err());
    assert_eq!(s.history.undo.len(), before);
    assert_eq!(s.history.redo.len(), redo);
    assert_eq!(s.project.items.len(), items);
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn numbered_pictures_stay_pictures_and_are_imported_in_one_bin() {
    let dir = temp("numbered");
    for (f, c) in [("bg_01.bmp", [1, 2, 3]), ("bg_02.bmp", [3, 2, 1])] {
        bmp(&dir.join(f), 32, 18, c);
    }
    write_wav16(&dir.join("v.wav"), &tone(&[(0.5, 0.4)]), RATE).unwrap();
    let path = dir.join("n.fcscript.json");
    let line = |id: &str| json!({"id": id, "kind": "narration", "text": "声", "voiceFile": "v.wav"});
    std::fs::write(
        &path,
        json!({"scenes": [{"id": "a", "background": "bg_01.bmp", "lines": [line("l1")]}, {"id": "b", "background": "bg_02.bmp", "lines": [line("l2")]}]})
            .to_string(),
    )
    .unwrap();
    let mut s = session();
    s.prefs.media.import_image_sequences = true;
    run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o")}));
    let stills: Vec<_> = s.project.items.values().filter(|i| i.name.starts_with("bg_0")).collect();
    assert_eq!(stills.len(), 2);
    assert!(
        stills.iter().all(|i| i.as_media().is_some_and(|m| matches!(m.info.kind, filmcraft_media::MediaKind::Still))),
        "numbered backgrounds were read as a sequence"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_export_stops_the_run() {
    let pr = project("badexport", "");
    let mut s = session();
    let bad_path = pr.dir.join("no-such-folder").join("deeper").join("episode.mp4");
    let r = run(&mut s, "anime.run", json!({"path": pr.path, "out": pr.dir.join("tts"), "width": 320, "height": 180, "fps": 12, "export": bad_path}));
    // either the destination was refused up front or the encode failed: never reported as done
    let last = r["steps"].as_array().unwrap().last().unwrap();
    if r["ok"] == json!(true) {
        assert!(bad_path.is_file(), "reported as exported but there is no file: {last}");
    } else {
        assert_eq!(r["failedStep"], json!("export"), "{r}");
    }
    let _ = std::fs::remove_dir_all(&pr.dir);
}

/// By hand, with the real model: runs the whole pipeline on the script named by
/// `FILMCRAFT_REAL_SCRIPT`, exporting to `FILMCRAFT_REAL_VIDEO`.
///
/// `FILMCRAFT_TTS_PYTHON=…/python.exe FILMCRAFT_REAL_SCRIPT=…/x.fcscript.json FILMCRAFT_REAL_VIDEO=…/x.mp4
///  cargo test -p filmcraft-engine --lib real_model_runs -- --ignored --nocapture`
#[test]
#[ignore = "needs Python with qwen-tts, the models and a script; run by hand"]
fn real_model_runs_the_whole_pipeline() {
    let (Some(script), Some(video)) = (std::env::var_os("FILMCRAFT_REAL_SCRIPT"), std::env::var_os("FILMCRAFT_REAL_VIDEO")) else {
        eprintln!("FILMCRAFT_REAL_SCRIPT / FILMCRAFT_REAL_VIDEO not set: skipped");
        return;
    };
    let (script, video) = (script.to_string_lossy().to_string(), video.to_string_lossy().to_string());
    let mut s = Session::default();
    let t0 = std::time::Instant::now();
    let r = run(&mut s, "anime.run", json!({"path": script, "export": video, "fps": 24}));
    eprintln!("anime.run ({:.1} s): {}", t0.elapsed().as_secs_f64(), serde_json::to_string_pretty(&r).unwrap());
    assert_eq!(r["ok"], json!(true), "{r}");
    let t1 = std::time::Instant::now();
    let r2 = run(&mut s, "anime.run", json!({"path": script, "check": false}));
    eprintln!("second run ({:.1} s): synthesized {} cached {}", t1.elapsed().as_secs_f64(), r2["steps"][1]["synthesized"], r2["steps"][1]["cached"]);
    assert_eq!(r2["steps"][1]["synthesized"], json!(0), "{r2}");
}

#[test]
fn voices_are_brought_to_one_loudness_and_the_check_hears_the_difference() {
    let dir = temp("level");
    bmp(&dir.join("bg.bmp"), 32, 18, [1, 2, 3]);
    let mut lines = Vec::new();
    for (i, amp) in [0.5f32, 0.5, 0.5, 0.15].iter().enumerate() {
        write_wav16(&dir.join(format!("v{i}.wav")), &tone(&[(1.0, *amp)]), RATE).unwrap();
        lines.push(json!({"id": format!("l{i}"), "kind": "narration", "text": "声", "voiceFile": format!("v{i}.wav")}));
    }
    let path = dir.join("l.fcscript.json");
    std::fs::write(&path, json!({"scenes": [{"id": "s", "background": "bg.bmp", "lines": lines}]}).to_string()).unwrap();
    let mut s = session();
    let flagged = |s: &mut Session, b: &Value| {
        run(s, "anime.check", json!({"sequence": b["sequence"]}))["problems"].as_array().unwrap().iter().filter(|p| p["kind"] == "loudness").count()
    };
    // not levelled: the quiet one is 10 LU down and flagged
    let b = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o"), "levelVoices": false}));
    assert_eq!((b["leveledVoices"].clone(), flagged(&mut s, &b)), (json!(0), 1));
    // levelled: only the quiet one is raised (by about 10 dB), and nothing is flagged
    let b = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o")}));
    assert_eq!(b["leveledVoices"], json!(1), "{b}");
    assert_eq!(flagged(&mut s, &b), 0);
    let gains: Vec<f64> = s.active_sequence().unwrap().audio_tracks[0].items.iter().map(|i| i.gain_db).collect();
    assert!(gains[..3].iter().all(|g| g.abs() < 0.5) && (gains[3] - 10.5).abs() < 1.5, "{gains:?}");
    // a target of the caller's choosing
    let b = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o"), "voiceLufs": -30}));
    assert_eq!(b["leveledVoices"], json!(4), "{b}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn backgrounds_fill_the_frame_unless_told_otherwise() {
    let pr = project("fit", "");
    let mut s = session();
    run(&mut s, "anime.build", json!({"path": pr.path, "out": pr.dir.join("tts")}));
    assert!(s.active_sequence().unwrap().video_tracks[0].items.iter().all(|i| i.scale_to_frame));
    assert!(s.active_sequence().unwrap().video_tracks[1].items.iter().all(|i| !i.scale_to_frame), "sprites keep their size");
    run(&mut s, "anime.build", json!({"path": pr.path, "out": pr.dir.join("tts"), "fitBackground": false}));
    assert!(s.active_sequence().unwrap().video_tracks[0].items.iter().all(|i| !i.scale_to_frame));
    let _ = std::fs::remove_dir_all(&pr.dir);
}

// ---------------------------------------------------------------------------------------------
// a game's scenario through tools/game2script

fn system_python() -> Option<String> {
    ["python", "python3"]
        .iter()
        .find(|p| std::process::Command::new(p).arg("--version").stdin(std::process::Stdio::null()).output().is_ok_and(|o| o.status.success()))
        .map(|p| p.to_string())
}

#[test]
fn a_converted_game_scenario_validates_and_builds() {
    let Some(py) = system_python() else {
        eprintln!("no python: skipped");
        return;
    };
    let dir = temp("game");
    for (f, c) in [
        ("bg/roof.bmp", [10, 20, 80]),
        ("bg/room.bmp", [80, 20, 10]),
        ("chara/marina_normal.bmp", [200, 200, 200]),
        ("chara/marina_normal_open.bmp", [220, 180, 180]),
    ] {
        std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        bmp(&dir.join(f), 32, 18, c);
    }
    for (f, secs) in [("voice/v001.wav", 2.0), ("voice/v003.wav", 1.5), ("bgm/calm.wav", 2.0), ("se/door.wav", 0.5)] {
        std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        write_wav16(&dir.join(f), &tone(&[(secs, 0.4)]), RATE).unwrap();
    }
    std::fs::create_dir_all(dir.join("scenario")).unwrap();
    // a Shift-JIS file, as old games have
    let text = "■ s01 屋上\n@pov まりな\n@bg roof\n@bgm calm\n夜だった。\n@face まりな normal\n[v001]まりな「待って、[漢字/かんじ]が読めない。」\n@se door\n小次郎「俺に任せろ。」\n@bg room\n[v003]まりな「着いた。」\n■ s02 朝\n@bg roof\n朝になった。\n";
    let (bytes, _, _) = encoding_rs_cp932(text);
    std::fs::write(dir.join("scenario/ch1.txt"), bytes).unwrap();
    let profile = dir.join("mine.json");
    std::fs::write(&profile, json!({"extends": "plain", "characters": {"まりな": {"id": "marina", "voice": {"speaker": "Ono_Anna"}}, "小次郎": {"id": "kojiro", "voice": {"speaker": "Ono_Anna"}}}}).to_string()).unwrap();
    let converter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/game2script/game2script.py");
    let out = dir.join("out").join("story.fcscript.json");
    let o = std::process::Command::new(&py)
        .args(["-X", "utf8", "-I"])
        .arg(&converter)
        .arg("--input")
        .arg(dir.join("scenario"))
        .arg("--assets")
        .arg(&dir)
        .arg("--out")
        .arg(&out)
        .arg("--profile")
        .arg(&profile)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

    let mut s = session();
    let v = run(&mut s, "script.validate", json!({"path": out}));
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(v["stats"]["scenes"], json!(3), "{v}");
    assert_eq!(v["stats"]["recorded"], json!(2), "{v}");
    let r = run(&mut s, "anime.run", json!({"path": out, "out": dir.join("tts"), "width": 320, "height": 180, "fps": 12}));
    assert_eq!(r["ok"], json!(true), "{r}");
    let build = r["steps"].as_array().unwrap().iter().find(|x| x["step"] == "build").unwrap();
    let c = &build["clips"];
    assert_eq!(c["backgrounds"], json!(3), "{build}");
    assert!(c["characters"].as_u64().unwrap() >= 1 && c["mouths"].as_u64().unwrap() >= 1, "{build}");
    assert_eq!(c["voices"], json!(5), "{build}");
    assert_eq!(c["effects"], json!(1), "{build}");
    assert!(build["withoutAudio"].as_array().unwrap().is_empty(), "{build}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The text as Shift-JIS bytes, by the converter's own reader: this test writes the file through
/// Python so the crate needs no encoding library.
fn encoding_rs_cp932(text: &str) -> (Vec<u8>, (), ()) {
    let py = system_python().unwrap();
    let o = std::process::Command::new(py)
        .args(["-X", "utf8", "-c", "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read().decode('utf-8').encode('cp932'))"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(text.as_bytes())?;
            c.wait_with_output()
        })
        .unwrap();
    (o.stdout, (), ())
}
