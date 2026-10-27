use std::sync::Arc;

use serde_json::json;

use crate::Session;
use filmcraft_project::{ItemId, Transcript, Word};
use filmcraft_speech::FixedTranscriber;
use filmcraft_time::Tick;

/// The demo project with a fake transcriber whose transcript fits the first A1 clip's media:
/// "Hello um world." (Speaker 1), a 1.2 s pause, "Second speaker here." (Speaker 2).
fn session() -> (Session, ItemId, Tick) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let a = &q.audio_tracks[0].items[0];
    let (item, sin) = (a.item, a.source_in);
    let sec = |x: f64| sin + Tick::from_seconds_f64(x);
    let mut t = Transcript { language: "en".into(), ..Default::default() };
    for (text, a, b, sp) in
        [("Hello", 0.2, 0.5, 0), ("um", 0.6, 0.9, 0), ("world.", 1.0, 1.4, 0), ("Second", 2.6, 3.0, 1), ("speaker", 3.0, 3.5, 1), ("here.", 3.5, 4.0, 1)]
    {
        let mut w = Word::new(text, sec(a), sec(b));
        w.speaker = Some(sp);
        t.words.push(w);
    }
    t.normalize();
    s.transcriber = Some(Arc::new(FixedTranscriber { transcript: t, id: "fixed".into() }));
    let start = a_start(&s);
    (s, item, start)
}

fn a_start(s: &Session) -> Tick {
    s.active_sequence().unwrap().audio_tracks[0].items[0].start
}

fn words(s: &mut Session) -> Vec<String> {
    let r = s.execute("transcript.inspect", json!({})).unwrap();
    r["words"].as_array().unwrap().iter().map(|w| w["text"].as_str().unwrap().to_string()).collect()
}

#[test]
fn generate_inspect_search_and_rename() {
    let (mut s, item, start) = session();
    assert!(s.execute("transcript.select", json!({"from": 0})).is_err(), "disabled before transcribing");
    let r = s.execute("transcript.generate", json!({"items": [item.0]})).unwrap();
    assert_eq!(r["items"][0]["words"], 6, "{r}");
    assert_eq!(r["items"][0]["source"], "fixed");
    assert_eq!(s.project.transcripts[&item].words.len(), 6);

    let r = s.execute("transcript.inspect", json!({})).unwrap();
    assert_eq!(words(&mut s), ["Hello", "um", "world.", "Second", "speaker", "here."]);
    assert_eq!(r["words"][0]["start"].as_i64().unwrap(), (start + Tick::from_seconds_f64(0.2)).0, "mapped to sequence time");
    assert_eq!(r["paragraphs"].as_array().unwrap().len(), 2, "speaker change splits paragraphs: {}", r["paragraphs"]);
    assert_eq!(r["speakers"], json!(["Speaker 1", "Speaker 2"]));

    let r = s.execute("transcript.search", json!({"query": "second spea"})).unwrap();
    assert_eq!(r["matches"], json!([{"from": 3, "to": 4, "start": r["matches"][0]["start"], "end": r["matches"][0]["end"]}]));

    s.execute("transcript.renameSpeaker", json!({"speaker": "Speaker 2", "name": "Ann"})).unwrap();
    let r = s.execute("transcript.inspect", json!({})).unwrap();
    assert_eq!(r["words"][3]["speaker"], "Ann");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.transcripts[&item].speakers[1].name, "Speaker 2");
    s.execute("transcript.renameSpeaker", json!({"speaker": 0, "item": item.0, "name": "Bo"})).unwrap();
    assert_eq!(s.project.transcripts[&item].speakers[0].name, "Bo");
    assert!(s.execute("transcript.renameSpeaker", json!({"speaker": "Nobody", "name": "X"})).is_err());

    // transcribing is one undo step
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.transcripts.is_empty());
}

#[test]
fn select_extract_and_lift_by_words() {
    let (mut s, item, _) = session();
    s.execute("transcript.generate", json!({"items": [item.0]})).unwrap();
    let rate = s.sequence_rate();
    let before = s.active_sequence().unwrap().duration();

    let r = s.execute("transcript.select", json!({"from": 3, "to": 5})).unwrap();
    let (a, b) = (Tick(r["start"].as_i64().unwrap()), Tick(r["end"].as_i64().unwrap()));
    let q = s.active_sequence().unwrap();
    assert_eq!(q.mark_in, Some(a));
    assert_eq!(q.mark_out, Some(b - rate.frame_duration()), "Out is the last frame inside");
    assert_eq!(rate.snap(a), a, "frame aligned");
    assert_eq!(s.playhead(), a);

    // Extract "um": the sequence gets shorter by the word's frames and the word is gone
    let r = s.execute("transcript.extract", json!({"from": 1})).unwrap();
    let cut = Tick(r["end"].as_i64().unwrap()) - Tick(r["start"].as_i64().unwrap());
    assert!(cut > Tick::ZERO);
    assert_eq!(s.active_sequence().unwrap().duration(), before - cut);
    assert!(!words(&mut s).contains(&"um".to_string()), "{:?}", words(&mut s));
    s.active_sequence().unwrap().check().unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_sequence().unwrap().duration(), before);

    // Lift leaves a gap: same duration, word gone
    s.execute("transcript.lift", json!({"from": 1, "to": 1})).unwrap();
    assert_eq!(s.active_sequence().unwrap().duration(), before);
    assert_eq!(words(&mut s), ["Hello", "world.", "Second", "speaker", "here."]);
    assert!(s.execute("transcript.extract", json!({"from": 99})).is_err());
}

#[test]
fn remove_fillers_pauses_and_create_captions() {
    let (mut s, item, _) = session();
    s.execute("transcript.generate", json!({"items": [item.0]})).unwrap();
    let before = s.active_sequence().unwrap().duration();
    let r = s.execute("transcript.removeFillers", json!({})).unwrap();
    assert_eq!(r["removed"], 1, "{r}");
    assert!(!words(&mut s).contains(&"um".to_string()));
    let r = s.execute("transcript.removePauses", json!({"minSeconds": 1.0, "keepSeconds": 0.1})).unwrap();
    assert_eq!(r["removed"], 1, "{r}");
    let after = s.active_sequence().unwrap().duration();
    assert!(after < before - Tick::from_seconds_f64(1.0), "the pause and the filler are gone");
    assert_eq!(words(&mut s).len(), 5);
    s.active_sequence().unwrap().check().unwrap();

    let r = s.execute("transcript.createCaptions", json!({"maxChars": 32})).unwrap();
    assert_eq!(r["captions"], 2, "one caption per speaker: {r}");
    let q = s.active_sequence().unwrap();
    let tr = &q.caption_tracks[0];
    assert_eq!(tr.captions[0].text, "Hello world.");
    assert_eq!(tr.captions[1].speaker.as_deref(), Some("Speaker 2"));
    q.check().unwrap();
}

#[test]
fn set_delete_and_models() {
    let (mut s, item, _) = session();
    let t = json!({"language": "en", "words": [
        {"text": "b", "start": 2000, "end": 3000, "speaker": 1},
        {"text": "a", "start": 0, "end": 1000},
    ]});
    let r = s.execute("transcript.set", json!({"item": item.0, "transcript": t})).unwrap();
    assert_eq!(r["words"], 2);
    let tr = &s.project.transcripts[&item];
    assert_eq!(tr.words[0].text, "a", "normalized: sorted");
    assert_eq!(tr.speakers.len(), 2, "missing speaker labels added");
    assert_eq!(tr.source, "imported");
    assert!(s.execute("transcript.set", json!({"item": 999_999, "transcript": {}})).is_err());
    assert!(s.execute("transcript.set", json!({"item": item.0, "transcript": {"words": 3}})).is_err());

    // survives save/load
    let bytes = filmcraft_format::encode(&s.project, true);
    let back = filmcraft_format::decode(&bytes).unwrap();
    assert_eq!(back.project.transcripts[&item].words.len(), 2);

    s.execute("transcript.delete", json!({"items": [item.0]})).unwrap();
    assert!(s.project.transcripts.is_empty());
    assert!(s.execute("transcript.delete", json!({})).is_err());

    let m = s.execute("transcript.models", json!({})).unwrap();
    assert_eq!(m["available"], filmcraft_speech::available());
    assert!(m["models"].as_array().unwrap().iter().any(|x| x["id"] == "whisper-base"));
}

#[test]
fn generate_without_a_transcriber() {
    let (mut s, item, _) = session();
    s.transcriber = None;
    let e = s.execute("transcript.generate", json!({"items": [item.0], "model": "nope"})).unwrap_err().to_string();
    assert!(e.contains("unknown speech model"), "{e}");
    if !filmcraft_speech::available() {
        let e = s.execute("transcript.generate", json!({"items": [item.0]})).unwrap_err().to_string();
        assert!(e.contains("whisper") && e.contains("not available"), "{e}");
        #[cfg(not(feature = "speech-download"))]
        assert!(s.execute("transcript.downloadModel", json!({})).unwrap_err().to_string().contains("not available"));
    }
    // defaults to the media of the open sequence's audio clips
    s.transcriber = Some(Arc::new(FixedTranscriber { transcript: Transcript::default(), id: "empty".into() }));
    let r = s.execute("transcript.generate", json!({})).unwrap();
    assert!(r["items"].as_array().unwrap().len() >= 2, "{r}");
}

/// Speech models belong to the session's data directory. Before, `transcript.models` and
/// `transcript.downloadModel` used the default directory even for a session started with another
/// one (`--data-dir`), so its downloads landed in the user's own application data.
#[test]
fn models_live_in_the_sessions_data_directory() {
    let dir = crate::media_test_util::tmp_dir("speech-models-dir");
    let (mut s, _, _) = session();
    s.prefs_path = Some(dir.join("preferences.json"));
    assert_eq!(crate::transcript::models_dir(&s), Some(dir.join("models")));
    let m = s.execute("transcript.models", json!({})).unwrap();
    assert_eq!(std::path::Path::new(m["dir"].as_str().unwrap()), dir.join("models"), "{m}");
    assert!(m["models"].as_array().unwrap().iter().all(|x| x["installed"] == false), "nothing is installed in a fresh directory");
    // an explicit data directory (the CLI's `--data-dir`) wins over where the preferences are
    let other = crate::media_test_util::tmp_dir("speech-models-dir-2");
    s.data_dir = Some(other.clone());
    assert_eq!(crate::transcript::models_dir(&s), Some(other.join("models")));
    // a session without a data directory of its own uses the default one
    s.data_dir = None;
    s.prefs_path = None;
    assert_eq!(crate::transcript::models_dir(&s), crate::autosave::default_data_dir().map(|d| d.join("models")));
}

#[test]
fn hostile_speakers_and_caption_rules_are_refused_or_clamped() {
    let (mut s, item, _) = session();
    // a huge speaker index grew the label list to 4 billion entries
    let t = json!({"words": [{"text": "a", "start": 0, "end": 1000, "speaker": u32::MAX}]});
    assert!(s.execute("transcript.set", json!({"item": item.0, "transcript": t})).is_err());
    s.execute("transcript.generate", json!({"items": [item.0]})).unwrap();
    // extreme layout values overflowed the caption maths
    for p in [
        json!({"maxChars": u64::MAX, "lines": u64::MAX, "gapFrames": i64::MAX, "minSeconds": 1e300, "maxSeconds": 1e300}),
        json!({"maxChars": 0, "lines": 0, "gapFrames": i64::MIN, "minSeconds": -1e300, "maxSeconds": -1e300}),
    ] {
        s.execute("transcript.createCaptions", p).unwrap();
        s.active_sequence().unwrap().check().unwrap();
    }
}

/// Retakes on the first A1 clip's media: "So the the plan. We went to the — we went to the store."
fn set_retakes(s: &mut Session, item: ItemId) {
    let sin = s.active_sequence().unwrap().audio_tracks[0].items[0].source_in;
    let sec = |x: f64| (sin + Tick::from_seconds_f64(x)).0;
    let texts = ["So", "the", "the", "plan.", "We", "went", "to", "the", "—", "we", "went", "to", "the", "store."];
    let words: Vec<_> =
        texts.iter().enumerate().map(|(i, t)| json!({"text": t, "start": sec(0.2 + i as f64 * 0.3), "end": sec(0.45 + i as f64 * 0.3)})).collect();
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "en", "words": words}})).unwrap();
}

#[test]
fn find_and_remove_repeats() {
    let (mut s, item, _) = session();
    set_retakes(&mut s, item);
    let before = s.active_sequence().unwrap().duration();
    let n = words(&mut s).len();
    let r = s.execute("transcript.findRepeats", json!({})).unwrap();
    assert_eq!(r["count"], 2, "{r}");
    assert_eq!(r["repeats"][0]["removed"], "the");
    assert_eq!(r["repeats"][0]["kept"], "the");
    assert_eq!((r["repeats"][0]["from"].as_u64(), r["repeats"][0]["to"].as_u64()), (Some(1), Some(1)));
    assert_eq!(r["repeats"][1]["removed"], "We went to the —");
    assert_eq!(r["repeats"][1]["kept"], "we went to the");
    assert!(r["repeats"][1]["end"].as_f64().unwrap() > r["repeats"][1]["start"].as_f64().unwrap());
    // finding edits nothing
    assert_eq!(words(&mut s).len(), n);
    assert_eq!(s.active_sequence().unwrap().duration(), before);

    let r = s.execute("transcript.removeRepeats", json!({})).unwrap();
    assert_eq!(r["removed"], 2, "{r}");
    assert_eq!(words(&mut s), ["So", "the", "plan.", "we", "went", "to", "the", "store."]);
    let after = s.active_sequence().unwrap().duration();
    assert!(after < before - Tick::from_seconds_f64(1.2), "{before:?} → {after:?}");
    s.active_sequence().unwrap().check().unwrap();
    assert_eq!(s.execute("transcript.findRepeats", json!({})).unwrap()["count"], 0);
    // one undo step
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(words(&mut s).len(), n);
    assert_eq!(s.active_sequence().unwrap().duration(), before);
    // only the long restart
    let r = s.execute("transcript.removeRepeats", json!({"minWords": 2})).unwrap();
    assert_eq!(r["removed"], 1, "{r}");
    assert_eq!(words(&mut s).len(), n - 5);
}

#[test]
fn hostile_repeat_params_are_clamped() {
    let (mut s, item, _) = session();
    assert!(s.execute("transcript.removeRepeats", json!({})).is_err(), "no transcript yet");
    set_retakes(&mut s, item);
    for p in [
        json!({"minWords": u64::MAX, "maxWords": u64::MAX, "maxGapSeconds": 1e300}),
        json!({"minWords": 0, "maxWords": 0, "maxGapSeconds": -1e300}),
        json!({"minWords": 9, "maxWords": 2, "fillers": [""]}),
        json!({"minWords": "x", "maxWords": -3, "maxGapSeconds": "nan", "fillers": 7}),
    ] {
        s.execute("transcript.findRepeats", p.clone()).unwrap();
        s.execute("transcript.removeRepeats", p).unwrap();
        s.active_sequence().unwrap().check().unwrap();
    }
}
