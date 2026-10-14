//! Tests of [`crate::creator`]: YouTube chapters, beat markers, Cut to the Beat and Make Short
//! Video on the demo project and generated music (120 BPM).

use std::sync::Arc;

use filmcraft_audio_dsp::remix as dsp;
use filmcraft_project::{ClipId, Transcript, Word};
use filmcraft_speech::FixedTranscriber;
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;
use crate::creator::{chapters, youtube_time};

const SR: u32 = 48_000;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn secs(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

#[test]
fn youtube_times() {
    assert_eq!(youtube_time(Tick::ZERO), "0:00");
    assert_eq!(youtube_time(secs(65.9)), "1:05");
    assert_eq!(youtube_time(secs(600.0)), "10:00");
    assert_eq!(youtube_time(secs(3725.0)), "1:02:05");
    assert_eq!(youtube_time(Tick(-5)), "0:00");
    assert_eq!(youtube_time(Tick(i64::MAX)), youtube_time(Tick(i64::MAX)), "no overflow");
}

/// Chapter markers become YouTube's list: a first chapter at 0:00 is added, titles are one line,
/// and the rules YouTube checks are reported.
#[test]
fn chapters_follow_youtube_rules() {
    let mut empty = Session::default();
    empty.execute("file.newSequence", json!({"name": "e"})).unwrap();
    assert!(empty.execute("markers.exportChapters", json!({})).is_err(), "disabled without markers");
    let mut s = demo();
    // (the demo's own markers are plain markers: they are not chapters)
    s.execute("markers.addChapter", json!({"seconds": 10.0, "name": "Ocean"})).unwrap();
    s.execute("markers.addChapter", json!({"seconds": 20.0, "name": "City\nat night"})).unwrap();
    // a plain marker does not count while there are chapter markers
    s.execute("markers.add", json!({"seconds": 15.0, "name": "note to self"})).unwrap();
    let r = s.execute("markers.exportChapters", json!({"firstTitle": "Opening"})).unwrap();
    assert_eq!(r["text"], "0:00 Opening\n0:10 Ocean\n0:20 City at night\n", "{r}");
    // the demo is 27 s long: the last chapter is 7 s, too short for YouTube
    let w: Vec<&str> = r["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("City at night") && w[0].contains("10 s"), "{w:?}");

    // written to a file as well
    let dir = std::env::temp_dir().join(format!("filmcraft-chapters-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chapters.txt");
    s.execute("markers.exportChapters", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "0:00 Intro\n0:10 Ocean\n0:20 City at night\n");
    let _ = std::fs::remove_dir_all(&dir);

    // a chapter at 0 s keeps its own title; two chapters fewer than 3 are reported
    let q = s.active_sequence().unwrap().clone();
    let mut q2 = q.clone();
    q2.markers.retain(|m| m.name == "note to self");
    let (list, warnings) = chapters(&q2, "Intro");
    assert_eq!(list.iter().map(|(t, n)| format!("{} {n}", youtube_time(*t))).collect::<Vec<_>>(), ["0:00 Intro", "0:15 note to self"]);
    assert!(warnings.iter().any(|w| w.contains("at least 3")), "{warnings:?}");
}

/// The music clip: 32 s of generated 120 BPM music on A3, starting at `at` seconds, selected.
fn with_music(s: &mut Session, at: f64) -> ClipId {
    let (m, _) = dsp::test_music(SR, 120.0, 16, 3);
    let inter: Vec<f32> = (0..m[0].len()).flat_map(|i| [m[0][i], m[1][i]]).collect();
    let bytes: Arc<[u8]> = crate::previews::write_wav_f32(&inter, SR).into();
    let item = crate::commands::import_bytes(s, "/music.wav", bytes, None).unwrap();
    let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A3", "seconds": at})).unwrap();
    let c = ClipId(r["clips"][0].as_u64().unwrap());
    s.execute("timeline.select", json!({"clips": [c.0]})).unwrap();
    c
}

/// The beat grid of the music as placed: every half second from `at`, within a frame.
fn on_grid(t: Tick, at: f64, frame: Tick) -> bool {
    let x = t.seconds() - at;
    let k = (x / 0.5).round();
    (x - k * 0.5).abs() <= frame.seconds() + 0.011
}

#[test]
fn beat_markers() {
    let mut s = demo();
    assert!(s.execute("markers.addBeats", json!({})).is_err(), "disabled without a music clip selected");
    let at = 0.23;
    with_music(&mut s, at);
    let frame = s.sequence_rate().frame_duration();
    let n0 = s.active_sequence().unwrap().markers.len();
    let r = s.execute("markers.addBeats", json!({})).unwrap();
    assert!((r["bpm"].as_f64().unwrap() - 120.0).abs() < 1.0, "{r}");
    let added = r["added"].as_u64().unwrap() as usize;
    // 64 beats in 32 s; the sequence (27 s) cuts nothing off: the clip is the music
    assert!((60..=64).contains(&added), "{r}");
    let q = s.active_sequence().unwrap();
    let beats: Vec<Tick> = q.markers.iter().filter(|m| m.name.starts_with("Beat")).map(|m| m.start).collect();
    assert_eq!(beats.len(), added);
    assert!(beats.iter().all(|t| on_grid(*t, at, frame)), "markers off the beat");
    // again: nothing new where markers already are
    assert_eq!(s.execute("markers.addBeats", json!({})).unwrap()["added"], 0);
    // undo removes them in one step (the second command added nothing and left no step)
    s.undo();
    assert_eq!(s.active_sequence().unwrap().markers.len(), n0);
    // every fourth beat (one a bar)
    let r = s.execute("markers.addBeats", json!({"every": 4, "color": "Mango"})).unwrap();
    let bars = r["added"].as_u64().unwrap() as usize;
    assert!((15..=16).contains(&bars), "{r}");
    assert!(s.execute("markers.addBeats", json!({"every": 0})).is_err());
    assert!(s.execute("markers.addBeats", json!({"every": 1000})).is_err());
    assert!(s.execute("markers.addBeats", json!({"color": "plaid"})).is_err());
}

/// Every cut between adjacent clips on V1 moves to the nearest beat, the linked audio on A1 with
/// it, and one undo puts them back.
#[test]
fn cut_to_the_beat() {
    let mut s = demo();
    let at = 0.23;
    with_music(&mut s, at);
    let frame = s.sequence_rate().frame_duration();
    let before = s.active_sequence().unwrap().clone();
    let cuts = |q: &filmcraft_project::Sequence, track: usize, video: bool| -> Vec<Tick> {
        let t = if video { &q.video_tracks[track] } else { &q.audio_tracks[track] };
        t.items.windows(2).filter(|w| w[0].end() == w[1].start).map(|w| w[1].start).collect()
    };
    let v0 = cuts(&before, 0, true);
    assert!(v0.iter().any(|t| !on_grid(*t, at, frame)), "the demo's cuts are off this beat grid");
    let r = s.execute("sequence.cutToBeats", json!({})).unwrap();
    assert_eq!(r["cuts"].as_u64(), Some(v0.len() as u64), "{r}");
    assert!(r["moved"].as_u64().unwrap() >= 1, "{r}");
    let q = s.active_sequence().unwrap();
    let v1 = cuts(q, 0, true);
    assert_eq!(v1.len(), v0.len());
    let moved_on: Vec<&Tick> = v1.iter().filter(|t| on_grid(**t, at, frame)).collect();
    assert_eq!(moved_on.len() as u64, r["moved"].as_u64().unwrap() + r["onBeat"].as_u64().unwrap(), "{r} {v1:?}");
    // the picture and its sound are cut at the same places, and the sequence keeps its length
    assert_eq!(cuts(q, 0, false), v1, "linked audio follows the picture");
    assert_eq!(q.duration(), before.duration());
    q.check().unwrap();
    s.undo();
    assert_eq!(cuts(s.active_sequence().unwrap(), 0, true), v0);
    // a track that does not exist, a locked track
    assert!(s.execute("sequence.cutToBeats", json!({"track": "V9"})).is_err());
    let tid = s.active_sequence().unwrap().video_tracks[0].id;
    s.execute("timeline.trackLock", json!({"track": tid.0, "on": true})).ok();
    if s.active_sequence().unwrap().video_tracks[0].locked {
        assert!(s.execute("sequence.cutToBeats", json!({})).is_err());
    }
}

/// Speech-free music has no use for beats: a clip of silence is refused with a reason.
#[test]
fn no_beat_is_an_error_not_a_crash() {
    let mut s = demo();
    let bytes: Arc<[u8]> = crate::previews::write_wav_f32(&vec![0.0; (SR as usize) * 8], SR).into();
    let item = crate::commands::import_bytes(&mut s, "/quiet.wav", bytes, None).unwrap();
    let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A3", "seconds": 0.0})).unwrap();
    s.execute("timeline.select", json!({"clips": [r["clips"][0]]})).unwrap();
    let e = s.execute("markers.addBeats", json!({})).unwrap_err().to_string();
    assert!(e.contains("no beat"), "{e}");
    assert!(s.execute("sequence.cutToBeats", json!({})).is_err());
}

/// A transcript for the first A1 clip of the demo (as in `transcript_tests`).
fn transcribed() -> Session {
    let mut s = demo();
    let q = s.active_sequence().unwrap();
    let a = &q.audio_tracks[0].items[0];
    let (item, sin) = (a.item, a.source_in);
    let mut t = Transcript { language: "ja".into(), ..Default::default() };
    for (text, a, b) in [("今日", 0.2, 0.5), ("は", 0.5, 0.6), ("海", 0.7, 1.0), ("に", 1.0, 1.1), ("来", 1.2, 1.4), ("ました。", 1.4, 1.9)] {
        t.words.push(Word::new(text, sin + secs(a), sin + secs(b)));
    }
    t.normalize();
    s.transcriber = Some(Arc::new(FixedTranscriber { transcript: t, id: "fixed".into() }));
    s.execute("transcript.generate", json!({"items": [item.0]})).unwrap();
    s
}

/// One step makes a vertical sequence with captions styled for it; the original is untouched,
/// and one undo removes everything.
#[test]
fn make_short_video() {
    let mut s = transcribed();
    let original = s.state.active_sequence.unwrap();
    let orig = s.active_sequence().unwrap().clone();
    let undo0 = s.history.undo.len();
    let r = s.execute("sequence.makeShort", json!({})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1080), Some(1920)), "{r}");
    assert!(r["skipped"].as_array().unwrap().is_empty(), "{r}");
    let q = s.active_sequence().unwrap();
    assert_ne!(s.state.active_sequence, Some(original));
    let tr = q.caption_tracks.first().expect("captions");
    assert!(!tr.captions.is_empty());
    assert_eq!(tr.captions.iter().map(|c| c.text.replace('\n', "")).collect::<String>(), "今日は海に来ました。");
    assert!((tr.style.margin - 0.22).abs() < 1e-6 && tr.style.size >= 60.0);
    assert_eq!(s.project.sequence(original).unwrap(), &orig, "the original is not changed");
    assert_eq!(s.history.undo.len(), undo0 + 1, "one undo step");
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Make Short Video"));
    s.undo();
    assert!(s.project.sequence(r["sequence"].as_u64().map(filmcraft_project::ItemId).unwrap()).is_none());

    // without a transcript the captions are skipped, with the reason
    let mut s = demo();
    let r = s.execute("sequence.makeShort", json!({"removeSilence": false, "aspect": "1:1"})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1080), Some(1080)));
    assert!(r["skipped"][0].as_str().unwrap().contains("transcript"), "{r}");
    // bad parameters are refused and leave nothing behind
    let n = s.project.items.len();
    assert!(s.execute("sequence.makeShort", json!({"aspect": "3:7"})).is_err());
    assert!(s.execute("sequence.makeShort", json!({"captions": "yes"})).is_err());
    assert!(s.execute("sequence.makeShort", json!({"thresholdDb": 20.0})).is_err());
    assert_eq!(s.project.items.len(), n);
}

/// Create English Captions: the speech model's translation becomes a caption track marked as
/// English; the clips' transcripts are not touched, and one undo removes the track.
#[test]
fn english_captions_from_a_translation() {
    let mut s = demo();
    let q = s.active_sequence().unwrap();
    let a = &q.audio_tracks[0].items[0];
    let sin = a.source_in;
    let mut t = Transcript { language: "en".into(), ..Default::default() };
    for (text, a, b) in
        [("Hello.", 0.2, 0.6), ("Today", 1.0, 1.3), ("we", 1.3, 1.4), ("are", 1.4, 1.6), ("at", 1.6, 1.7), ("the", 1.7, 1.8), ("sea.", 1.8, 2.2)]
    {
        t.words.push(Word::new(text, sin + secs(a), sin + secs(b)));
    }
    t.normalize();
    s.transcriber = Some(Arc::new(FixedTranscriber { transcript: t, id: "fixed".into() }));
    let undo0 = s.history.undo.len();
    let r = s.execute("transcript.createTranslatedCaptions", json!({"model": "fixed"})).unwrap();
    assert!(r["captions"].as_u64().unwrap() >= 1, "{r}");
    let q = s.active_sequence().unwrap();
    let tr = q.caption_tracks.first().unwrap();
    assert_eq!((tr.name.as_str(), tr.language.as_str()), ("English", "en"));
    let text = tr.captions.iter().map(|c| c.text.replace('\n', " ")).collect::<Vec<_>>().join(" ");
    assert!(text.starts_with("Hello.") && text.ends_with("sea."), "{text}");
    assert!(s.project.transcripts.is_empty(), "the translation is not kept as a transcript");
    assert_eq!(s.history.undo.len(), undo0 + 1);
    assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Create English Captions"));
    s.undo();
    assert!(s.active_sequence().unwrap().caption_tracks.is_empty());
}
