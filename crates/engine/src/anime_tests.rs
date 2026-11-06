use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::anime::{Audio, Movie, Opts, PlanEnv, plan, split_caption};
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
    /// Movies: seconds (0 for a still) and whether they have sound.
    movies: HashMap<String, (f64, bool)>,
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
    fn movie(&mut self, path: &Path) -> Option<Movie> {
        let (d, audio) = *self.movies.get(path.to_str()?)?;
        Some(Movie { path: path.to_path_buf(), duration: Tick::from_seconds_f64(d), audio })
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

// ---------------------------------------------------------------------------------------------
// a mouth picture for each vowel

fn vowel_story() -> Script {
    Script::from_json(
        r#"{
        "characters": {"m": {"name": "m", "expressions": {
            "normal": {"image": "m.png", "openImage": "mo.png", "vowelImages": {"a": "va.png", "i": "vi.png", "u": "vu.png"}}}}},
        "scenes": [{"id": "s1", "lines": [
            {"id": "a1", "speaker": "m", "text": "あいうえ"},
            {"id": "a2", "speaker": "m", "text": "漢字《かんじ》"}]}]}"#,
    )
    .unwrap()
}

#[test]
fn each_vowel_gets_its_own_mouth_and_unpictured_vowels_use_the_open_mouth() {
    let rate = FrameRate::from_f64(24.0);
    let mut f = Fake::default();
    f.audio.insert("a1".into(), 2.0);
    f.audio.insert("a2".into(), 2.0);
    f.mouth = vec![(0.0, 2.0)];
    let pl = plan(&vowel_story(), rate, &Opts::default(), &mut f);
    let a1 = pl.cues.iter().find(|c| c.line == "a1").unwrap();
    let vowels: Vec<Option<char>> = a1.shapes.iter().map(|s| s.2).collect();
    assert_eq!(vowels, vec![Some('a'), Some('i'), Some('u'), Some('e')], "{:?}", a1.shapes);
    assert!(a1.shapes.windows(2).all(|w| w[0].1 <= w[1].0), "apart");
    assert!(a1.shapes.iter().all(|(a, b, _)| *a >= a1.start && *b <= a1.end && rate.snap_nearest(*a) == *a));
    // e has no picture of its own: the build falls back to `open`; the plan keeps the vowel
    // the reading is かんじ: a, closed, i
    let a2 = pl.cues.iter().find(|c| c.line == "a2").unwrap();
    assert_eq!(a2.shapes.iter().map(|s| s.2).collect::<Vec<_>>(), vec![Some('a'), Some('i')], "{:?}", a2.shapes);
    assert!(a2.shapes[0].1 < a2.shapes[1].0, "ん leaves the mouth shut between");
    // the sprite lists the pictures it has
    let sp = a1.sprite.as_ref().unwrap();
    assert_eq!(sp.vowels.keys().copied().collect::<Vec<_>>(), vec!['a', 'i', 'u']);
}

#[test]
fn a_missing_or_misnamed_vowel_picture_is_a_warning_not_a_failure() {
    let rate = FrameRate::from_f64(24.0);
    let mut s = vowel_story();
    if let Some(c) = s.characters.get_mut("m")
        && let Some(sp) = c.expressions.get_mut("normal")
    {
        sp.vowel_images.insert("x".into(), "vx.png".into());
        sp.vowel_images.insert("".into(), "none.png".into());
    }
    let mut f = Fake::default();
    f.audio.insert("a1".into(), 1.0);
    f.mouth = vec![(0.0, 1.0)];
    f.missing = vec!["va.png".into()];
    let pl = plan(&s, rate, &Opts::default(), &mut f);
    assert!(pl.warnings.iter().any(|w| w.contains("va.png")), "{:?}", pl.warnings);
    assert!(pl.warnings.iter().filter(|w| w.contains("not one of")).count() >= 2, "{:?}", pl.warnings);
    let a1 = pl.cues.iter().find(|c| c.line == "a1").unwrap();
    assert!(!a1.shapes.is_empty());
}

#[test]
fn the_build_places_the_vowel_pictures_on_the_mouth_track() {
    let dir = temp("vowels");
    for (f, c) in [("bg.bmp", [10, 20, 80]), ("m.bmp", [200, 200, 200]), ("mo.bmp", [220, 180, 180]), ("va.bmp", [255, 0, 0]), ("vi.bmp", [0, 255, 0])] {
        bmp(&dir.join(f), 32, 18, c);
    }
    write_wav16(&dir.join("v.wav"), &tone(&[(2.0, 0.5)]), RATE).unwrap();
    let script = r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.bmp", "openImage": "mo.bmp", "vowelImages": {"a": "va.bmp", "i": "vi.bmp"}}}}},
        "scenes": [{"id": "s1", "background": "bg.bmp", "lines": [{"id": "a1", "speaker": "m", "text": "あいうえ", "voiceFile": "v.wav"}]}]}"#;
    let path = dir.join("story.fcscript.json");
    std::fs::write(&path, script).unwrap();
    let mut s = session();
    let r = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("tts")}));
    assert_eq!(r["clips"]["mouths"], json!(4), "{r}");
    let q = s.active_sequence().unwrap().clone();
    let v3 = &q.video_tracks[2].items;
    assert_eq!(v3.len(), 4);
    assert!(v3.windows(2).all(|w| w[0].end() <= w[1].start));
    let sources: std::collections::BTreeSet<_> = v3.iter().map(|i| i.item).collect();
    assert_eq!(sources.len(), 3, "a, i and (for う and え) the plain open mouth");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_line_of_closed_beats_keeps_the_mouth_shut_when_the_sprite_has_vowel_pictures() {
    let dir = temp("closed_beats");
    for (f, c) in [("bg.bmp", [10, 20, 80]), ("m.bmp", [200, 200, 200]), ("mo.bmp", [220, 180, 180]), ("va.bmp", [255, 0, 0])] {
        bmp(&dir.join(f), 32, 18, c);
    }
    write_wav16(&dir.join("v.wav"), &tone(&[(1.0, 0.5)]), RATE).unwrap();
    let script = r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.bmp", "openImage": "mo.bmp", "vowelImages": {"a": "va.bmp"}}}}},
        "scenes": [{"id": "s1", "background": "bg.bmp", "lines": [{"id": "a1", "speaker": "m", "text": "ん", "voiceFile": "v.wav"}]}]}"#;
    let path = dir.join("story.fcscript.json");
    std::fs::write(&path, script).unwrap();
    let mut s = session();
    let r = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("tts")}));
    assert_eq!(r["clips"]["mouths"], json!(0), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------
// movies, sub CGs, two-shots and fades

fn snapped(rate: FrameRate, secs: f64) -> Tick {
    rate.snap_nearest(Tick::from_seconds_f64(secs))
}

#[test]
fn a_movie_lasts_its_own_length_and_its_sound_wins_over_the_effect() {
    let rate = FrameRate::from_f64(24.0);
    let s = Script::from_json(
        r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.png"}}}},
        "scenes": [{"id": "s1", "background": "bg.png", "lines": [
            {"id": "a1", "speaker": "m", "text": "待って。"},
            {"id": "v1", "kind": "direction", "video": "op.mp4", "sfx": "door.wav"},
            {"id": "a2", "speaker": "m", "text": "どうして？"},
            {"id": "v2", "kind": "direction", "video": "gone.mp4"},
            {"id": "v3", "kind": "direction", "video": "still.png"},
            {"id": "v4", "kind": "direction", "video": "quiet.mp4", "sfx": "door.wav"},
            {"id": "a3", "speaker": "m", "text": "ここ。", "video": "op.mp4"}]}]}"#,
    )
    .unwrap();
    let mut f = fake();
    f.audio.insert("a3".into(), 1.0);
    f.movies.insert("op.mp4".into(), (3.1, true));
    f.movies.insert("still.png".into(), (0.0, false));
    f.movies.insert("quiet.mp4".into(), (2.0, false));
    let pl = plan(&s, rate, &Opts::default(), &mut f);
    let cue = |id: &str| pl.cues.iter().find(|c| c.line == id).unwrap();
    let one = rate.frame_duration();
    // the movie sets the line's length, on the frame grid, and the next line follows it
    let v1 = cue("v1");
    assert_eq!(v1.end - v1.start, snapped(rate, 3.1));
    // 3.1 s is 74.4 frames: the line ends on the grid and the movie is cut there
    assert_eq!(v1.video_len(one), v1.end - v1.start);
    assert_eq!(cue("a2").start, v1.end);
    assert!(v1.sprite.is_none() && v1.captions.is_empty());
    // its own sound plays: the line's sound effect does not, and the build says so
    assert!(v1.sfx.is_none());
    assert!(pl.warnings.iter().any(|w| w.contains("v1") && w.contains("video's own sound")), "{:?}", pl.warnings);
    // a missing movie: warned about, the direction lasts the default second
    let v2 = cue("v2");
    assert!(v2.video.is_none() && pl.warnings.iter().any(|w| w.contains("gone.mp4")));
    assert_eq!(v2.end - v2.start, Tick::from_seconds_f64(1.0));
    // a still has no length of its own: the default second, and shown all of it
    let v3 = cue("v3");
    assert!(v3.video.is_some());
    assert_eq!((v3.end - v3.start, v3.video_len(one)), (Tick::from_seconds_f64(1.0), Tick::from_seconds_f64(1.0)));
    // a silent movie keeps the effect
    let v4 = cue("v4");
    assert!(v4.sfx.is_some() && v4.video.is_some());
    assert_eq!(v4.end - v4.start, Tick::from_seconds_f64(2.0));
    // a movie on a spoken line is not shown (validate warns about it)
    assert!(cue("a3").video.is_none());
}

#[test]
fn a_scene_that_hides_characters_keeps_voices_and_subtitles_but_places_no_sprite() {
    let script = Script::from_json(
        r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.png", "openImage": "mo.png"}, "normal_blink": {"image": "mb.png"}}}},
        "scenes": [
          {"id": "s1", "lines": [{"id": "a", "speaker": "m", "text": "待って。", "slot": 3}]},
          {"id": "s2", "hideCharacters": true, "background": "cg.png", "lines": [
            {"id": "b", "speaker": "m", "text": "あのときは。", "slot": 3},
            {"id": "c", "speaker": "m", "text": "ね。"}]},
          {"id": "s3", "lines": [{"id": "d", "speaker": "m", "text": "ほら。"}]}]}"#,
    )
    .unwrap();
    assert!(script.scenes[1].hide_characters && !script.scenes[0].hide_characters);
    let rate = FrameRate::from_f64(24.0);
    let mut f = Fake::default();
    for id in ["a", "b", "c", "d"] {
        f.audio.insert(id.into(), 3.0);
    }
    f.mouth = vec![(0.2, 0.7)];
    let pl = plan(&script, rate, &Opts::default(), &mut f);
    let cue = |id: &str| pl.cues.iter().find(|c| c.line == id).unwrap();
    // the right-hand character of s1 ends with s1; s2 has no one on screen; s3 shows m again
    let runs: Vec<(&str, u8, Tick, Tick)> = pl.runs.iter().map(|r| (r.speaker.as_str(), r.slot, r.start, r.end)).collect();
    assert_eq!(runs, vec![("m", 3, cue("a").start, pl.scenes[0].end), ("m", 2, cue("d").start, cue("d").end)]);
    for id in ["b", "c"] {
        let c = cue(id);
        assert!(c.sprite.is_none() && c.mouth.is_empty(), "{id}");
        assert!(c.voice.is_some() && !c.captions.is_empty(), "{id}");
    }
    assert!(!cue("a").mouth.is_empty() && !cue("d").mouth.is_empty());
    // written back only when set
    let json = serde_json::to_string(&script).unwrap();
    assert_eq!(json.matches("hideCharacters").count(), 1, "{json}");
}

fn two_shot() -> Script {
    Script::from_json(
        r#"{"characters": {
            "m": {"name": "m", "expressions": {"normal": {"image": "m.png", "openImage": "mo.png"}, "smile": {"image": "ms.png"}, "normal_blink": {"image": "mb.png"}}},
            "k": {"name": "k", "expressions": {"normal": {"image": "k.png", "openImage": "ko.png"}}}},
        "scenes": [
          {"id": "s1", "lines": [
            {"id": "l1", "speaker": "m", "text": "待って。", "slot": 1},
            {"id": "l2", "speaker": "k", "text": "なに？", "slot": 3},
            {"id": "l3", "speaker": "m", "text": "あのね。"},
            {"id": "n1", "kind": "narration", "text": "風が吹いた。"},
            {"id": "l4", "speaker": "k", "text": "ひとりで。", "slot": 2},
            {"id": "l5", "speaker": "m", "text": "ねえ。", "slot": 3},
            {"id": "l6", "speaker": "m", "text": "ほら。", "slot": 1, "expression": "smile"},
            {"id": "l7", "speaker": "k", "text": "うん。", "slot": 1}]},
          {"id": "s2", "lines": [
            {"id": "l8", "speaker": "m", "text": "朝。", "slot": 1},
            {"id": "d1", "kind": "direction"}]}]}"#,
    )
    .unwrap()
}

#[test]
fn a_two_shot_keeps_the_listener_on_screen_and_moves_only_the_speakers_mouth() {
    let rate = FrameRate::from_f64(24.0);
    let mut f = Fake::default();
    for id in ["l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8"] {
        f.audio.insert(id.into(), 4.0);
    }
    f.mouth = vec![(0.2, 0.7), (1.0, 1.4)];
    let pl = plan(&two_shot(), rate, &Opts::default(), &mut f);
    let cue = |id: &str| pl.cues.iter().find(|c| c.line == id).unwrap();
    let slots: Vec<(&str, u8)> = pl.cues.iter().map(|c| (c.line.as_str(), c.slot)).collect();
    // l3 asks for no place and stays where m stands; l4 in the centre ends the two-shot
    assert_eq!(slots, vec![("l1", 1), ("l2", 3), ("l3", 1), ("n1", 2), ("l4", 2), ("l5", 3), ("l6", 1), ("l7", 1), ("l8", 1), ("d1", 2)]);
    assert_eq!(pl.runs.len(), 7, "{:?}", pl.runs.iter().map(|r| (r.speaker.clone(), r.slot, r.start, r.end)).collect::<Vec<_>>());
    // m stands at the left through k's line and the narration, until k takes the centre
    let m1 = &pl.runs[0];
    assert_eq!((m1.slot, m1.start, m1.end), (1, cue("l1").start, cue("n1").end));
    assert_eq!(m1.speaking, vec![(cue("l1").start, cue("l1").end), (cue("l3").start, cue("l3").end)]);
    let k1 = &pl.runs[1];
    assert_eq!((k1.speaker.as_str(), k1.slot, k1.start, k1.end), ("k", 3, cue("l2").start, cue("n1").end));
    assert_eq!(k1.speaking, vec![(cue("l2").start, cue("l2").end)]);
    // the centre shows k alone
    assert_eq!((pl.runs[2].slot, pl.runs[2].start, pl.runs[2].end), (2, cue("l4").start, cue("l4").end));
    // m moves from the right to the left (one place at a time); a new expression is a new clip
    assert_eq!((pl.runs[3].slot, pl.runs[3].end), (3, cue("l5").end));
    assert_eq!((pl.runs[4].slot, pl.runs[4].start, pl.runs[4].end), (1, cue("l6").start, cue("l6").end));
    assert_eq!(pl.runs[4].sprite.closed, PathBuf::from("ms.png"));
    // k takes the left from m
    assert_eq!((pl.runs[5].speaker.as_str(), pl.runs[5].slot, pl.runs[5].start), ("k", 1, cue("l7").start));
    // a two-shot ends with its scene, through lines with no one on screen
    let last = pl.runs.last().unwrap();
    assert_eq!((last.slot, last.start, last.end), (1, cue("l8").start, pl.scenes[1].end));
    assert_eq!(last.end, cue("d1").end);
    // blinks only while the character speaks
    assert!(!m1.blinks.is_empty());
    for r in &pl.runs {
        for (a, b) in &r.blinks {
            assert!(r.speaking.iter().any(|(x, y)| a >= x && b <= y), "a blink outside speech: {a:?} {r:?}");
        }
    }
    // the mouth moves with the speaker's line only
    assert!(!cue("l2").mouth.is_empty());
    for c in &pl.cues {
        assert!(c.mouth.iter().all(|(a, b)| *a >= c.start && *b <= c.end));
    }
}

#[test]
fn a_sub_cg_stays_until_the_next_one_a_movie_or_the_end_of_the_scene() {
    let rate = FrameRate::from_f64(24.0);
    let s = Script::from_json(
        r#"{"scenes": [
          {"id": "s1", "lines": [
            {"id": "c1", "kind": "narration", "text": "一。", "subCg": "cg1.png"},
            {"id": "c2", "kind": "narration", "text": "二。"},
            {"id": "c3", "kind": "narration", "text": "三。", "subCg": "cg2.png", "subCgAnchor": [0.25, 0.75]},
            {"id": "c4", "kind": "narration", "text": "四。", "subCg": ""},
            {"id": "c5", "kind": "narration", "text": "五。", "subCg": "cg3.png"},
            {"id": "mv", "kind": "direction", "video": "op.mp4"},
            {"id": "c6", "kind": "narration", "text": "六。", "subCg": "nope.png"}]},
          {"id": "s2", "transition": "fade", "lines": [
            {"id": "c7", "kind": "narration", "text": "七。", "subCg": "cg4.png", "subCgAnchor": [3, 0.5]}]}]}"#,
    )
    .unwrap();
    let mut f = Fake::default();
    f.movies.insert("op.mp4".into(), (2.0, false));
    f.missing = vec!["nope.png".into()];
    let pl = plan(&s, rate, &Opts::default(), &mut f);
    let cue = |id: &str| pl.cues.iter().find(|c| c.line == id).unwrap();
    let got: Vec<(&str, Tick, Tick, Option<(f64, f64)>)> = pl.sub_cgs.iter().map(|g| (g.path.to_str().unwrap(), g.start, g.end, g.anchor)).collect();
    assert_eq!(
        got,
        vec![
            ("cg1.png", cue("c1").start, cue("c3").start, None),
            ("cg2.png", cue("c3").start, cue("c4").start, Some((0.25, 0.75))),
            ("cg3.png", cue("c5").start, cue("mv").start, None),
            // an anchor off the frame is the middle (validate reports it)
            ("cg4.png", cue("c7").start, pl.scenes[1].end, None),
        ]
    );
    assert!(pl.warnings.iter().any(|w| w.contains("nope.png")), "{:?}", pl.warnings);
    // the scene's transition is carried to the plan
    assert_eq!(pl.scenes.iter().map(|s| s.fade).collect::<Vec<_>>(), vec![false, true]);
}

/// The colour at pixel (x, y) of the active sequence at `t`: (r, g, b).
fn pixel(s: &Session, t: Tick, x: usize, y: usize) -> (f32, f32, f32) {
    let seq = s.state.active_sequence.unwrap();
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let img = filmcraft_render::render_sequence(&s.project, seq, t, filmcraft_render::RenderOptions { scale: 1.0, ..Default::default() }, &provider);
    let i = (y * img.w + x) * 4;
    (img.px[i], img.px[i + 1], img.px[i + 2])
}

fn motion(ti: &filmcraft_project::TrackItem, param: &str) -> filmcraft_project::Param {
    ti.effects.iter().find(|e| e.effect == "motion").and_then(|e| e.params.get(param)).unwrap().clone()
}

fn pos(ti: &filmcraft_project::TrackItem) -> (f64, f64) {
    let p = motion(ti, "position").value.as_vec2().unwrap();
    (p.x, p.y)
}

#[test]
fn the_build_places_a_movie_a_sub_cg_a_two_shot_and_a_fade() {
    let dir = temp("eve");
    for (f, c) in [
        ("bg1.bmp", [10, 20, 200]),
        ("bg2.bmp", [10, 200, 20]),
        ("m.bmp", [200, 200, 200]),
        ("mo.bmp", [220, 180, 180]),
        ("mb.bmp", [150, 150, 150]),
        ("k.bmp", [100, 100, 100]),
        ("ko.bmp", [120, 90, 90]),
    ] {
        bmp(&dir.join(f), 32, 18, c);
    }
    bmp(&dir.join("cg.bmp"), 12, 10, [250, 10, 10]);
    crate::media_test_util::make_movie(&dir.join("mv.mp4"), filmcraft_media::DemoScene::Aurora, 64, 36, 48);
    for v in ["v1", "v2", "v3", "v4"] {
        write_wav16(&dir.join(format!("{v}.wav")), &tone(&[(0.3, 0.0), (0.6, 0.5), (0.3, 0.0), (0.6, 0.5)]), RATE).unwrap();
    }
    let script = json!({
        "characters": {
            "m": {"name": "m", "expressions": {"normal": {"image": "m.bmp", "openImage": "mo.bmp"}, "blink": {"image": "mb.bmp"}}},
            "k": {"name": "k", "expressions": {"normal": {"image": "k.bmp", "openImage": "ko.bmp"}}}},
        "scenes": [
          {"id": "s1", "background": "bg1.bmp", "transition": "fade", "lines": [
            {"id": "a1", "speaker": "m", "text": "待って。", "voiceFile": "v1.wav", "slot": 1},
            {"id": "a2", "speaker": "k", "text": "なに？", "voiceFile": "v2.wav", "slot": 3, "subCg": "cg.bmp", "subCgAnchor": [0.25, 0.25]},
            {"id": "mv", "kind": "direction", "video": "mv.mp4"},
            {"id": "a3", "speaker": "m", "text": "見た？", "voiceFile": "v3.wav"}]},
          {"id": "s2", "background": "bg2.bmp", "lines": [
            {"id": "a4", "speaker": "k", "text": "うん。", "voiceFile": "v4.wav"}]}]});
    let path = dir.join("eve.fcscript.json");
    std::fs::write(&path, script.to_string()).unwrap();
    let mut s = session();
    let n_items = s.project.items.len();
    let r = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("tts"), "width": 320, "height": 180, "fps": 24}));
    let c = &r["clips"];
    assert_eq!((c["videos"].clone(), c["videoSounds"].clone(), c["subCgs"].clone(), c["fades"].clone()), (json!(1), json!(1), json!(1), json!(1)), "{r}");
    assert_eq!(c["backgrounds"], json!(3), "the first scene's background is cut around the movie: {r}");
    let names: Vec<&str> = r["tracks"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(names, vec!["Background", "Character", "Mouth", "Blink", "Left", "Left Mouth", "Left Blink", "Right", "Right Mouth", "Right Blink", "Sub CG"]);
    let q = s.active_sequence().unwrap().clone();
    let one = q.settings.frame_rate.frame_duration();
    assert_eq!(q.video_tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), names);
    let voices = &q.audio_tracks[0].items;
    assert_eq!(voices.len(), 4);
    let (a1, a2, a3) = (voices[0].start, voices[1].start, voices[2].start);

    // V1: background, the movie for its own two seconds, background again, the next scene
    let v1 = &q.video_tracks[0].items;
    assert_eq!(v1.len(), 4);
    assert!(v1.windows(2).all(|w| w[0].end() == w[1].start), "V1 has no holes");
    let movie = &v1[1];
    assert_eq!(s.project.item(movie.item).unwrap().name, "mv.mp4");
    assert_eq!(movie.duration, Tick::from_seconds_f64(2.0));
    assert_eq!(movie.end(), a3, "the next line follows the movie");
    assert_eq!(v1[0].item, v1[2].item, "the same background before and after");
    // the background goes on pushing in where it left off
    let key = |ti: &filmcraft_project::TrackItem, k: usize| motion(ti, "scale").keyframes[k].value.as_f64().unwrap();
    assert!(key(&v1[2], 0) > key(&v1[0], 0) && key(&v1[2], 0) >= key(&v1[0], 1) - 0.01);
    // A3: the movie's sound with it
    let a3_items = &q.audio_tracks[2].items;
    assert_eq!(a3_items.len(), 1);
    assert_eq!((a3_items[0].start, a3_items[0].duration, a3_items[0].item), (movie.start, movie.duration, movie.item));

    // the fade: from nothing onto the first background, at the scene's start; the second cuts
    let fades = &q.video_tracks[0].transitions;
    assert_eq!(fades.len(), 1);
    assert_eq!((fades[0].start, fades[0].duration, fades[0].from, fades[0].to), (Tick::ZERO, Tick(one.0 * 12), None, Some(v1[0].id)));
    let black = pixel(&s, Tick::ZERO, 160, 90);
    let half = pixel(&s, Tick(one.0 * 6), 160, 90);
    let full = pixel(&s, Tick(one.0 * 13), 160, 90);
    assert!(black.2 < 0.05, "starts black: {black:?}");
    assert!(half.2 > black.2 + 0.1 && half.2 < full.2 - 0.1, "half way: {half:?} of {full:?}");

    // the two-shot: m at the left through k's line, k at the right, until the movie
    let left = &q.video_tracks[4].items;
    let right = &q.video_tracks[7].items;
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].start, left[0].end(), pos(&left[0])), (a1, movie.start, (80.0, 90.0)));
    assert_eq!(right.len(), 1);
    assert_eq!((right[0].start, right[0].end(), pos(&right[0])), (a2, movie.start, (240.0, 90.0)));
    // each mouth on its speaker's side, at its place, inside its own line
    let left_mouth = &q.video_tracks[5].items;
    let right_mouth = &q.video_tracks[8].items;
    assert!(!left_mouth.is_empty() && !right_mouth.is_empty());
    assert!(left_mouth.iter().all(|m| m.start >= a1 && m.end() <= a2 && pos(m) == (80.0, 90.0)));
    assert!(right_mouth.iter().all(|m| m.start >= a2 && m.end() <= movie.start && pos(m) == (240.0, 90.0)));
    assert!(q.video_tracks[6].items.iter().all(|b| b.start >= a1 && b.end() <= a2), "m blinks only while speaking");
    assert!(q.video_tracks[9].items.is_empty(), "k has no blink picture");
    // after the movie m is alone in the centre, as imported
    let center = &q.video_tracks[1].items;
    assert_eq!(center.len(), 2);
    assert_eq!((center[0].start, pos(&center[0])), (a3, (160.0, 90.0)));
    assert!(q.video_tracks[2].items.iter().all(|m| m.start >= a3));

    // the sub CG: from k's line to the movie, its own 12x10 pixels, centred at a quarter
    let cg = &q.video_tracks[10].items;
    assert_eq!(cg.len(), 1);
    assert_eq!((cg[0].start, cg[0].end(), pos(&cg[0]), cg[0].scale_to_frame), (a2, movie.start, (80.0, 45.0), false));
    assert_eq!(motion(&cg[0], "scale").value.as_f64(), Some(100.0));
    let mid = a2 + Tick(one.0 * 2);
    let inside = pixel(&s, mid, 80, 45);
    let outside = pixel(&s, mid, 80 + 9, 45);
    assert!(inside.0 > 0.8 && inside.2 < 0.2, "the sub CG is drawn on top: {inside:?}");
    assert!(outside.0 < 0.8 || outside.2 > 0.2, "and is no larger than itself: {outside:?}");

    // the check finds nothing wrong, and one undo takes it all back
    let chk = run(&mut s, "anime.check", json!({"sequence": r["sequence"]}));
    assert_eq!(chk["ok"], json!(true), "{chk}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.project.items.len(), n_items);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_script_without_the_new_fields_builds_the_four_tracks_as_before() {
    let pr = project("plain", "");
    let mut s = session();
    let r = run(&mut s, "anime.build", params(&pr));
    assert_eq!(r["tracks"], json!(["Background", "Character", "Mouth", "Blink"]), "{r}");
    let c = &r["clips"];
    assert_eq!((c["videos"].clone(), c["subCgs"].clone(), c["fades"].clone()), (json!(0), json!(0), json!(0)));
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks.len(), 4);
    assert!(q.video_tracks[0].transitions.is_empty());
    // a fade with nothing to fade onto is a warning, not a failure
    let text = std::fs::read_to_string(&pr.path).unwrap().replace(r#"{"id": "s2", "background": "bg2.bmp","#, r#"{"id": "s2", "transition": "fade","#);
    std::fs::write(&pr.path, text).unwrap();
    let r = run(&mut s, "anime.build", params(&pr));
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("nothing on the background track")), "{r}");
    assert_eq!(r["clips"]["fades"], json!(0));
    // out-of-range layout options are refused
    assert!(s.execute("anime.build", json!({"path": pr.path, "leftX": 2})).is_err());
    assert!(s.execute("anime.build", json!({"path": pr.path, "transitionSeconds": 0})).is_err());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

#[test]
fn a_blink_waits_for_the_mouth_to_close_and_a_sprite_can_carry_its_own_blink() {
    let rate = FrameRate::from_f64(24.0);
    let script = Script::from_json(
        r#"{"characters": {"m": {"name": "m", "expressions": {
            "normal": {"image": "m.png", "openImage": "mo.png", "blink": "mb.png"}}}},
          "scenes": [{"id": "s1", "lines": [{"id": "a1", "speaker": "m", "text": "あいうえお"}]}]}"#,
    )
    .unwrap();
    let mut f = Fake::default();
    f.audio.insert("a1".into(), 12.0);
    // the mouth is open nearly all the time: closed only at 3.0-3.5 s and 7.0-7.6 s
    f.mouth = vec![(0.0, 3.0), (3.5, 7.0), (7.6, 12.0)];
    let pl = plan(&script, rate, &Opts::default(), &mut f);
    let r = &pl.runs[0];
    assert_eq!(r.sprite.blink.as_deref(), Some(Path::new("mb.png")), "the sprite's own blink picture");
    assert!(!r.blinks.is_empty(), "{r:?}");
    let open = &pl.cues[0].mouth;
    assert_eq!(open.len(), 3, "{open:?}");
    for (a, b) in &r.blinks {
        assert!(open.iter().all(|(x, y)| b <= x || a >= y), "blink {a:?}..{b:?} over an open mouth {open:?}");
        assert_eq!(rate.snap_nearest(*a), *a);
        assert!(*a >= r.start && *b <= r.end);
    }
    assert!(r.blinks.windows(2).all(|w| w[0].1 < w[1].0), "blinks apart");
    // a sprite without mouth pictures has nothing to wait for: the blinks keep their times
    let still = Script::from_json(
        r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.png", "blink": "mb.png"}}}},
          "scenes": [{"id": "s1", "lines": [{"id": "a1", "speaker": "m", "text": "あいうえお"}]}]}"#,
    )
    .unwrap();
    let mut f = Fake::default();
    f.audio.insert("a1".into(), 12.0);
    f.mouth = vec![(0.0, 12.0)];
    let pl = plan(&still, rate, &Opts::default(), &mut f);
    assert!(pl.cues[0].mouth.is_empty());
    assert!(pl.runs[0].blinks.len() >= 2, "{:?}", pl.runs[0].blinks);
    // an empty `blink` falls back to the expression called `blink`
    let fallback = Script::from_json(
        r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.png", "blink": " "}, "blink": {"image": "b2.png"}}}},
          "scenes": [{"id": "s1", "lines": [{"id": "a1", "speaker": "m", "text": "あ"}]}]}"#,
    )
    .unwrap();
    let pl = plan(&fallback, rate, &Opts::default(), &mut fake());
    assert_eq!(pl.runs[0].sprite.blink.as_deref(), Some(Path::new("b2.png")));
}

#[test]
fn sprites_can_stand_on_the_bottom_and_backgrounds_can_fill_the_frame() {
    let pr = project("align", "");
    let mut s = session();
    run(&mut s, "script.synthesize", params(&pr));
    let mut p = params(&pr);
    for (k, v) in [("width", json!(64)), ("height", json!(48)), ("spriteScale", json!(200)), ("spriteAlign", json!("bottom")), ("backgroundFit", json!("fill"))]
    {
        p[k] = v;
    }
    run(&mut s, "anime.build", p);
    let q = s.active_sequence().unwrap().clone();
    // the 32x18 background covers 64x48: the larger ratio, 48 / 18, then it pushes in from there
    let bg = &q.video_tracks[0].items[0];
    assert!(!bg.scale_to_frame);
    let k0 = motion(bg, "scale").keyframes[0].value.as_f64().unwrap();
    assert!((k0 - 48.0 / 18.0 * 100.0).abs() < 0.01, "{k0}");
    // the sprite (32x18 at 200 %, so 64x36) stands on the bottom: its centre 18 px above it;
    // its mouth and blink land exactly on it
    assert!(!q.video_tracks[1].items.is_empty() && !q.video_tracks[2].items.is_empty() && !q.video_tracks[3].items.is_empty());
    for t in 1..=3 {
        for ti in &q.video_tracks[t].items {
            assert_eq!(pos(ti), (32.0, 30.0), "track {t}");
        }
    }
    // and in the frame: the bottom row is the sprite's grey, the top row the background's blue
    let at = q.video_tracks[1].items[0].start;
    let (r, g, b) = pixel(&s, at, 32, 47);
    assert!((r - g).abs() < 0.05 && (g - b).abs() < 0.05 && r > 0.4, "{:?}", (r, g, b));
    let (r, _, b) = pixel(&s, at, 32, 2);
    assert!(b > r, "{:?}", (r, b));
    // the defaults are as before: fitted, centred as imported
    run(&mut s, "anime.build", json!({"path": pr.path, "out": pr.dir.join("tts"), "width": 64, "height": 48}));
    let q = s.active_sequence().unwrap().clone();
    assert!(q.video_tracks[0].items[0].scale_to_frame);
    assert_eq!(pos(&q.video_tracks[1].items[0]), (32.0, 24.0));
    // anything else is refused
    assert!(s.execute("anime.build", json!({"path": pr.path, "spriteAlign": "top"})).is_err());
    assert!(s.execute("anime.build", json!({"path": pr.path, "backgroundFit": 3})).is_err());
    let _ = std::fs::remove_dir_all(&pr.dir);
}

// ---------------------------------------------------------------------------------------------
// sound words: subtitled, never spoken, their sound on the effects track and under the voices

#[test]
fn a_sound_word_is_subtitled_for_its_sound_and_never_waits_for_a_voice() {
    let s = Script::from_json(
        r#"{"characters": {"m": {"name": "m", "expressions": {"normal": {"image": "m.png", "openImage": "mo.png"}}}},
        "scenes": [{"id": "s", "background": "bg.png", "lines": [
            {"id": "crash", "kind": "narration", "text": "ガシャーン！", "spoken": false, "sfx": "crash.wav"},
            {"id": "thud", "kind": "narration", "text": "ドテッ。", "spoken": false, "sfx": "thud.wav"},
            {"id": "jiri", "kind": "narration", "text": "ジリッ‥‥。", "spoken": false},
            {"id": "guu", "speaker": "m", "text": "ぐぅ～。", "spoken": false, "voiceFile": "guu_voice.wav"},
            {"id": "a1", "speaker": "m", "text": "待って。"}]}]}"#,
    )
    .unwrap();
    let rate = FrameRate::from_f64(24.0);
    let o = Opts::default();
    let mut f = fake();
    for (k, v) in [("crash.wav", 1.6), ("thud.wav", 0.15), ("guu", 3.0), ("guu_voice.wav", 3.0)] {
        f.audio.insert(k.into(), v);
    }
    let pl = plan(&s, rate, &o, &mut f);
    let cue = |id: &str| pl.cues.iter().find(|c| c.line == id).unwrap();
    let len = |id: &str| (cue(id).end - cue(id).start).seconds();
    // as long as its sound (plus the pause after a line), never shorter than a reading of it
    assert!((len("crash") - (1.6 + 0.35)).abs() < 0.05, "{}", len("crash"));
    assert!((len("thud") - (0.8 + 0.35)).abs() < 0.05, "{}", len("thud"));
    // no sound: the time to read it (6 characters at 7 a second)
    assert!((len("jiri") - (6.0 / 7.0 + 0.35)).abs() < 0.05, "{}", len("jiri"));
    for id in ["crash", "thud", "jiri", "guu"] {
        let c = cue(id);
        assert!(c.voice.is_none() && c.mouth.is_empty() && !c.estimated, "{id}: {c:?}");
        assert!(!c.captions.is_empty(), "{id} is subtitled");
        assert!(!pl.warnings.iter().any(|w| w.contains(&format!("line {id}:"))), "{:?}", pl.warnings);
    }
    assert!(cue("crash").sfx.is_some() && cue("jiri").sfx.is_none());
    // the recording of an unspoken line is not played, and the speaker still stands there
    assert!(cue("guu").sprite.is_some() && (len("guu") - (0.8 + 0.35)).abs() < 0.05, "{}", len("guu"));
    // the spoken line is unchanged: its voice and its mouth
    assert!(cue("a1").voice.is_some() && !cue("a1").mouth.is_empty());
}

#[test]
fn sound_effects_sit_under_the_voices_and_the_check_measures_them() {
    let dir = temp("words");
    bmp(&dir.join("bg.bmp"), 32, 18, [1, 2, 3]);
    let mut lines = Vec::new();
    for i in 0..3 {
        write_wav16(&dir.join(format!("v{i}.wav")), &tone(&[(1.0, 0.1)]), RATE).unwrap();
        lines.push(json!({"id": format!("l{i}"), "kind": "narration", "text": "声", "voiceFile": format!("v{i}.wav")}));
    }
    // a game's effects are as loud as they go: far louder than the voices
    write_wav16(&dir.join("crash.wav"), &tone(&[(1.2, 0.9)]), RATE).unwrap();
    write_wav16(&dir.join("click.wav"), &tone(&[(0.1, 0.9)]), RATE).unwrap();
    write_wav16(&dir.join("hum.wav"), &tone(&[(2.0, 0.01)]), RATE).unwrap();
    lines.push(json!({"id": "w1", "kind": "narration", "text": "ガシャーン！", "spoken": false, "sfx": "crash.wav"}));
    lines.push(json!({"id": "w2", "kind": "narration", "text": "カチッ。", "spoken": false, "sfx": "click.wav"}));
    lines.push(json!({"id": "w3", "kind": "narration", "text": "ジリッ。", "spoken": false}));
    lines.push(json!({"id": "d1", "kind": "direction", "sfx": "hum.wav"}));
    let path = dir.join("w.fcscript.json");
    std::fs::write(&path, json!({"scenes": [{"id": "s", "background": "bg.bmp", "lines": lines}]}).to_string()).unwrap();
    let mut s = session();
    // nothing to speak: the sound words are not synthesized
    let v = run(&mut s, "script.validate", json!({"path": path}));
    assert_eq!((v["stats"]["spoken"].clone(), v["stats"]["toSynthesize"].clone()), (json!(3), json!(0)), "{v}");
    let r = run(&mut s, "anime.run", json!({"path": path, "out": dir.join("o")}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["steps"][1]["synthesized"], json!(0), "{r}");
    let b = &r["steps"][2];
    assert_eq!((b["clips"]["voices"].clone(), b["clips"]["effects"].clone(), b["clips"]["subtitles"].clone()), (json!(3), json!(3), json!(6)), "{b}");
    // all three are levelled, the click measured as one 400 ms block
    assert_eq!(b["leveledEffects"], json!(3), "{b}");
    let q = s.active_sequence().unwrap();
    assert!(q.audio_tracks[0].items.iter().all(|i| i.name.starts_with('v')), "only voices on A1");
    let gains: Vec<f64> = q.audio_tracks[2].items.iter().map(|i| i.gain_db).collect();
    assert!(gains[0] < -20.0 && gains[1] < -20.0 && gains[1] > gains[0] && gains[2] == 6.0, "{gains:?}");
    let c = &r["steps"][3];
    let (voice, effect) = (c["stats"]["medianVoiceLufs"].as_f64().unwrap(), c["stats"]["loudestEffectLufs"].as_f64().unwrap());
    assert!((voice - effect - 10.0).abs() < 1.0, "voice {voice} effect {effect}: {c}");
    assert!(!c["problems"].as_array().unwrap().iter().any(|p| p["kind"] == "effect-loud" || p["kind"] == "no-audio"), "{c}");
    // not levelled and at full gain, the crash is as loud as a voice and the check says so
    let b = run(&mut s, "anime.build", json!({"path": path, "out": dir.join("o"), "levelEffects": false, "sfxGainDb": 0}));
    assert_eq!(b["leveledEffects"], json!(0), "{b}");
    let c = run(&mut s, "anime.check", json!({"sequence": b["sequence"]}));
    let loud: Vec<&Value> = c["problems"].as_array().unwrap().iter().filter(|p| p["kind"] == "effect-loud").collect();
    assert_eq!(loud.len(), 2, "the crash and the click: {c}");
    // a hostile margin is refused, not a crash
    assert!(s.execute("anime.build", json!({"path": path, "effectsBelowVoiceLu": f64::MAX})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
