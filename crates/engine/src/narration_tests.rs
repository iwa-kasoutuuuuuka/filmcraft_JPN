//! Tests of [`crate::narration`] against a fake VOICEVOX / AivisSpeech engine: a tiny HTTP server
//! on 127.0.0.1 in a thread that serves canned `/version`, `/speakers`, `/speaker_info`,
//! `/aivm_models`, `/audio_query` and `/synthesis` answers (the WAV is `0.1 s × characters /
//! speedScale` of silence, so durations are predictable).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use super::*;
use crate::narration::{parse_script, resolve_voice};
use filmcraft_speech::tts::{Speaker, Style, TtsEngine};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Good,
    GarbageJson,
    NotWav,
    HugeLength,
    QueryError,
}

const RATE: u32 = 24_000;

fn wav_of(seconds: f64) -> Vec<u8> {
    let frames = (seconds * RATE as f64).round() as usize;
    let data = frames * 2;
    let mut v = Vec::with_capacity(44 + data);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&RATE.to_le_bytes());
    v.extend_from_slice(&(RATE * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data as u32).to_le_bytes());
    // a quiet tone, so the clip is not digital silence
    for i in 0..frames {
        let x = ((i as f64 * 0.05).sin() * 1000.0) as i16;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(x) => {
                        out.push(x);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let q = target.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| pct_decode(v)))
}

fn speakers_json() -> Value {
    json!([
        {"name": "ずんだもん", "speaker_uuid": "uuid-zunda", "styles": [{"name": "ノーマル", "id": 3, "type": "talk"}, {"name": "あまあま", "id": 1}], "version": "0.1.0"},
        {"name": "四国めたん", "speaker_uuid": "uuid-metan", "styles": [{"name": "ノーマル", "id": 2}], "version": "0.1.0"},
        {"name": "歌手", "speaker_uuid": "uuid-sing", "styles": [{"name": "歌", "id": 6000, "type": "sing"}], "version": "0.1.0"}
    ])
}

fn audio_query(text: &str) -> Value {
    // one accent phrase per piece between 、/。 with a pause after every piece but the last
    let pieces: Vec<&str> = text.split(['、', '。']).filter(|p| !p.is_empty()).collect();
    let n = pieces.len();
    let phrases: Vec<Value> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let moras: Vec<Value> = p.chars().map(|_| json!({"text": "ア", "consonant": null, "consonant_length": null, "vowel": "a", "vowel_length": 0.1, "pitch": 5.5})).collect();
            json!({"moras": moras, "accent": 1, "pause_mora": if i + 1 < n { json!({"text": "、", "vowel": "pau", "vowel_length": 0.1, "pitch": 0.0}) } else { Value::Null }, "is_interrogative": false})
        })
        .collect();
    json!({"accent_phrases": phrases, "speedScale": 1.0, "pitchScale": 0.0, "intonationScale": 1.0, "volumeScale": 1.0,
           "prePhonemeLength": 0.0, "postPhonemeLength": 0.0, "outputSamplingRate": RATE, "outputStereo": false, "kana": text})
}

fn respond(mut c: TcpStream, mode: Mode, aivis: bool) {
    let _ = c.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let len: usize = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse().unwrap_or(0)))
        .unwrap_or(0);
    while buf.len() < head_end + len {
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let body = buf.get(head_end..).unwrap_or_default().to_vec();
    let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/");
    let (status, ctype, out): (u16, &str, Vec<u8>) = match path {
        "/version" => (200, "application/json", b"\"0.20.0\"".to_vec()),
        "/speakers" if mode == Mode::GarbageJson => (200, "application/json", b"{{{ not json".to_vec()),
        "/speakers" if mode == Mode::HugeLength => {
            let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999999999999\r\nConnection: close\r\n\r\n[");
            return;
        }
        "/speakers" => (200, "application/json", speakers_json().to_string().into_bytes()),
        "/speaker_info" => {
            let uuid = query_param(&target, "speaker_uuid").unwrap_or_default();
            (200, "application/json", json!({"policy": format!("# 利用規約 {uuid}\nクレジット表記が必要です。"), "portrait": "iVBORw0K", "style_infos": []}).to_string().into_bytes())
        }
        "/aivm_models" if aivis => (
            200,
            "application/json",
            json!({"m1": {"manifest": {"uuid": "m1", "name": "Zunda Model", "license": "ACML 1.0", "speakers": [{"uuid": "uuid-zunda", "name": "ずんだもん"}]}}}).to_string().into_bytes(),
        ),
        "/audio_query" if mode == Mode::QueryError => (422, "application/json", br#"{"detail":"text is bad"}"#.to_vec()),
        "/audio_query" => {
            let text = query_param(&target, "text").unwrap_or_default();
            (200, "application/json", audio_query(&text).to_string().into_bytes())
        }
        "/synthesis" if mode == Mode::NotWav => (200, "audio/wav", b"this is not a wav".to_vec()),
        "/synthesis" => {
            let q: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let chars = q["kana"].as_str().unwrap_or("").chars().filter(|c| !matches!(c, '、' | '。')).count();
            let speed = q["speedScale"].as_f64().unwrap_or(1.0);
            (200, "audio/wav", wav_of(0.1 * chars as f64 / speed))
        }
        _ => (404, "application/json", br#"{"detail":"Not Found"}"#.to_vec()),
    };
    let reason = if status == 200 { "OK" } else { "Error" };
    let _ =
        c.write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len()).as_bytes());
    let _ = c.write_all(&out);
    let _ = c.flush();
}

/// A fake engine; returns its base URL.
fn fake(mode: Mode, aivis: bool) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            std::thread::spawn(move || respond(c, mode, aivis));
        }
    });
    url
}

/// A port nothing listens on.
fn closed_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    drop(l);
    format!("http://{a}")
}

fn out_dir(name: &str) -> String {
    std::env::temp_dir().join(format!("filmcraft-narration-{}-{name}", std::process::id())).to_string_lossy().into_owned()
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn secs(t: i64) -> f64 {
    t as f64 / TICKS_PER_SECOND as f64
}

#[test]
fn voices_engines_and_terms_from_the_engine() {
    let url = fake(Mode::Good, false);
    let mut s = demo();
    let v = s.execute("tts.voices", json!({"engine": "voicevox", "url": url})).unwrap();
    let rows = v["voices"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "singing styles are left out: {v}");
    assert_eq!(rows[0]["voice"], "ずんだもん/ノーマル");
    assert_eq!(rows[0]["id"], 3);
    assert_eq!(rows[0]["uuid"], "uuid-zunda");

    s.prefs.narration.voicevox_url = url.clone();
    s.prefs.narration.aivisspeech_url = closed_url();
    let e = s.execute("tts.engines", json!({})).unwrap();
    let list = e["engines"].as_array().unwrap();
    let vv = list.iter().find(|x| x["id"] == "voicevox").unwrap();
    let av = list.iter().find(|x| x["id"] == "aivisspeech").unwrap();
    assert_eq!((vv["running"].as_bool(), vv["version"].as_str()), (Some(true), Some("0.20.0")));
    assert_eq!(av["running"], false);
    assert!(av["error"].as_str().unwrap().contains("start the AivisSpeech app"), "{av}");
    // all engines: the one that answers lists its voices, the other one an error
    let all = s.execute("tts.voices", json!({})).unwrap();
    assert_eq!(all["voices"].as_array().unwrap().len(), 3);
    assert_eq!(all["errors"][0]["engine"], "aivisspeech");

    let t = s.execute("tts.voiceTerms", json!({"engine": "voicevox", "voice": "四国めたん"})).unwrap();
    assert_eq!(t["uuid"], "uuid-metan");
    assert!(t["policy"].as_str().unwrap().contains("利用規約 uuid-metan"));
    assert_eq!(t["credit"], "VOICEVOX:四国めたん");
}

#[test]
fn multi_voice_narration_back_to_back_with_captions_credits_and_one_undo() {
    let url = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = url;
    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1})).unwrap();
    let a_new = format!("A{}", s.active_sequence().unwrap().audio_tracks.len());
    let items0 = s.project.items.len();
    let undo0 = s.history.undo.len();
    let ctracks0 = s.active_sequence().unwrap().caption_tracks.len();
    let dir = out_dir("multi");
    let r = s
        .execute(
            "tts.synthesize",
            json!({
                "engine": "voicevox",
                "lines": [
                    {"text": "こんにちは", "voice": "ずんだもん"},
                    {"text": "よろしくお願いします", "voice": "四国めたん/ノーマル", "pauseAfter": 0.5, "speed": 2.0},
                    {"text": "またね", "voice": 3}
                ],
                "pause": 0.25,
                "seconds": 2.0,
                "track": a_new,
                "dir": dir,
            }),
        )
        .unwrap();
    let clips: Vec<u64> = r["clips"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(clips.len(), 3);
    assert_eq!(r["track"], a_new);
    // durations are the WAVs' (0.1 s a character / speed), back to back with the pauses
    let q = s.active_sequence().unwrap();
    let rate = q.settings.frame_rate;
    let tr = q.audio_tracks.iter().find(|t| t.id.0 == r["trackId"].as_u64().unwrap()).unwrap();
    let placed: Vec<_> = clips.iter().map(|c| tr.items.iter().find(|i| i.id.0 == *c).unwrap().clone()).collect();
    let want = [0.5, 0.5, 0.3];
    let pauses = [0.25, 0.5, 0.25];
    // the first line starts on the first frame at or after 2 s
    let t0 = Tick::from_seconds_f64(2.0);
    assert!(placed[0].start >= t0 && placed[0].start - t0 < rate.frame_duration(), "{:?}", placed[0].start);
    for (i, it) in placed.iter().enumerate() {
        assert!((it.duration.seconds() - want[i]).abs() < 1e-6, "line {i}: {}", it.duration.seconds());
        if i > 0 {
            let prev = &placed[i - 1];
            let gap_end = prev.start + prev.duration + Tick::from_seconds_f64(pauses[i - 1]);
            assert!(it.start >= gap_end && it.start - gap_end < rate.frame_duration(), "line {i} starts at the next frame after the pause");
        }
    }
    // the files exist next to each other with the voice in the name
    let files: Vec<&str> = r["files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(files[0].ends_with("Narration_001_ずんだもん-ノーマル.wav"), "{}", files[0]);
    assert!(files[1].ends_with("Narration_002_四国めたん-ノーマル.wav"), "{}", files[1]);
    for f in &files {
        assert!(std::fs::metadata(f).unwrap().len() > 44);
    }
    // captions: exactly the line texts, timed to the audio, speakers named (dialogue)
    let ct = r["captionTrack"].as_u64().unwrap();
    let q = s.active_sequence().unwrap();
    let ctr = q.caption_tracks.iter().find(|t| t.id.0 == ct).unwrap();
    assert_eq!(ctr.language, "ja");
    let texts: Vec<&str> = ctr.captions.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["こんにちは", "よろしくお願いします", "またね"]);
    for (c, it) in ctr.captions.iter().zip(&placed) {
        assert_eq!(c.start, it.start);
        assert!((c.end() - it.end()).0.abs() <= rate.frame_duration().0, "caption ends with its line");
    }
    let who: Vec<_> = ctr.captions.iter().map(|c| c.speaker.clone().unwrap_or_default()).collect();
    assert_eq!(who, ["ずんだもん", "四国めたん", "ずんだもん"]);
    // credits: one per voice
    let credits: Vec<&str> = r["credits"].as_array().unwrap().iter().map(|c| c["credit"].as_str().unwrap()).collect();
    assert_eq!(credits, ["VOICEVOX:ずんだもん", "VOICEVOX:四国めたん"]);
    assert_eq!(r["creditText"], "VOICEVOX:ずんだもん\nVOICEVOX:四国めたん");
    assert!(r["credits"][0]["termsUrl"].as_str().unwrap().starts_with("https://"));
    // one undo step removes clips, items and the caption track
    assert_eq!(s.history.undo.len(), undo0 + 1);
    assert_eq!(s.history.undo.last().unwrap().0, "Synthesize Narration");
    assert_eq!(s.project.items.len(), items0 + 3);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), items0);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.caption_tracks.len(), ctracks0);
    assert!(q.all_tracks().all(|t| t.items.iter().all(|i| !clips.contains(&i.id.0))));
    // the WAVs stay on disk (never deleted); a second run picks new names
    let r2 = s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "こんにちは", "voice": "ずんだもん", "dir": dir, "captions": false})).unwrap();
    assert!(r2["files"][0].as_str().unwrap().ends_with("Narration_001_ずんだもん-ノーマル_2.wav"), "{}", r2["files"][0]);
    assert!(r2["captionTrack"].is_null());
}

#[test]
fn script_cast_long_line_captions_and_aivisspeech_credits() {
    let url = fake(Mode::Good, true);
    let mut s = demo();
    s.prefs.narration.aivisspeech_url = url;
    let script = "# 台本\nずんだもん: 今日はいい天気なのだ。散歩に行くのだ。\nめたん「そうね」\n\n";
    let r = s
        .execute("tts.synthesize", json!({"script": script, "cast": {"めたん": "四国めたん"}, "captionMaxChars": 12, "dir": out_dir("script"), "time": 0}))
        .unwrap();
    assert_eq!(r["engine"], "aivisspeech");
    assert_eq!(r["lines"][0]["voice"]["speaker"], "ずんだもん");
    assert_eq!(r["lines"][1]["voice"]["speaker"], "四国めたん");
    assert_eq!(r["lines"][1]["text"], "そうね");
    // the long line became two captions split at the pause (0.1 s a mora + 0.1 s pause)
    let ct = r["captionTrack"].as_u64().unwrap();
    let q = s.active_sequence().unwrap();
    let ctr = q.caption_tracks.iter().find(|t| t.id.0 == ct).unwrap();
    let texts: Vec<&str> = ctr.captions.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["今日はいい天気なのだ。", "散歩に行くのだ。", "そうね"]);
    assert!(secs(ctr.captions[1].start.0) > 0.9 && secs(ctr.captions[1].start.0) < 1.2, "{}", secs(ctr.captions[1].start.0));
    // AivisSpeech: model licence from /aivm_models; the other voice is "check the licence"
    let c = r["credits"].as_array().unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(c[0]["credit"], "AivisSpeech:ずんだもん");
    assert_eq!(c[0]["license"], "ACML 1.0");
    assert_eq!(c[0]["model"], "Zunda Model");
    assert!(c[0]["modelUrl"].as_str().unwrap().ends_with("/aivm-models/m1"));
    assert!(c[1]["license"].is_null() && c[1]["checkLicense"] == true);
    // an unknown speaker in a script is an error, not a silent default
    let e = s.execute("tts.synthesize", json!({"script": "だれか: こんにちは", "dir": out_dir("script2")})).unwrap_err().to_string();
    assert!(e.contains("no voice named `だれか`"), "{e}");
}

#[test]
fn engine_down_and_hostile_answers_are_errors() {
    let mut s = demo();
    let items0 = s.project.items.len();
    let undo0 = s.history.undo.len();
    let down = closed_url();
    let e = s.execute("tts.synthesize", json!({"engine": "aivisspeech", "url": down, "text": "こんにちは"})).unwrap_err().to_string();
    assert!(e.contains(&format!("AivisSpeech is not running at {down}")) && e.contains("start the AivisSpeech app"), "{e}");
    let e = s.execute("tts.voices", json!({"engine": "voicevox", "url": closed_url()})).unwrap_err().to_string();
    assert!(e.contains("VOICEVOX is not running"), "{e}");
    for (mode, want) in [(Mode::GarbageJson, "not JSON"), (Mode::NotWav, "not a WAV"), (Mode::HugeLength, "unusable"), (Mode::QueryError, "text is bad")] {
        let url = fake(mode, false);
        let e =
            s.execute("tts.synthesize", json!({"engine": "voicevox", "url": url, "text": "こんにちは", "dir": out_dir("hostile")})).unwrap_err().to_string();
        assert!(e.contains(want), "{want}: {e}");
    }
    // hostile parameters
    let url = fake(Mode::Good, false);
    for p in [
        json!({"engine": "sapi", "text": "x"}),
        json!({"engine": "voicevox", "url": "https://evil.example", "text": "x"}),
        json!({"engine": "voicevox", "url": url, "lines": []}),
        json!({"engine": "voicevox", "url": url, "lines": [{"text": "   "}]}),
        json!({"engine": "voicevox", "url": url, "lines": [{"text": "x", "voice": 99999}]}),
        json!({"engine": "voicevox", "url": url, "lines": [{"text": "x", "voice": {"nope": 1}}]}),
        json!({"engine": "voicevox", "url": url, "lines": [7]}),
        json!({"engine": "voicevox", "url": url, "text": "あ".repeat(5000)}),
        json!({"engine": "voicevox", "url": url, "lines": vec![json!("x"); 600]}),
        json!({"engine": "voicevox", "url": url, "text": "x", "track": "A99x"}),
        json!({"engine": "voicevox", "url": url, "text": "x", "captionTrack": "C42"}),
        json!({"engine": "voicevox", "url": url, "prepared": 12345, "text": "x"}),
    ] {
        assert!(s.execute("tts.synthesize", p.clone()).is_err(), "{p}");
    }
    // NaN-ish numbers are clamped, not trusted
    s.execute(
        "tts.synthesize",
        json!({"engine": "voicevox", "url": url, "text": "あい", "speed": 1e308, "pause": -5.0, "seconds": -100.0, "captionMaxChars": u64::MAX, "dir": out_dir("clamp")}),
    )
    .unwrap();
    assert_eq!(s.project.items.len(), items0 + 1, "the failures left nothing behind");
    assert_eq!(s.history.undo.len(), undo0 + 1);
}

#[test]
fn prepared_audio_is_placed_without_asking_the_engine_again() {
    let url = fake(Mode::Good, false);
    let mut s = demo();
    let p = json!({"engine": "voicevox", "url": url, "lines": [{"text": "あいう", "voice": "ずんだもん/あまあま"}], "dir": out_dir("prepared")});
    let job = crate::narration::prepare(&s, &p, "tts.synthesize").unwrap();
    let rendered = crate::narration::render(&job).unwrap();
    let token = crate::narration::next_token();
    s.narration.prepared = Some(crate::narration::Prepared { token, rendered });
    // the engine is not asked: the URL points nowhere now
    let mut p2 = p.clone();
    p2["url"] = json!(closed_url());
    p2["prepared"] = json!(token);
    let r = s.execute("tts.synthesize", p2).unwrap();
    assert_eq!(r["lines"][0]["voice"]["id"], 1);
    assert!(s.narration.prepared.is_none());
}

#[test]
fn preview_writes_a_temporary_wav_and_leaves_the_project_alone() {
    let url = fake(Mode::Good, false);
    let mut s = demo();
    let rev = s.revision;
    let r = s.execute("tts.preview", json!({"engine": "voicevox", "url": url, "text": "テスト", "voice": "ずんだもん"})).unwrap();
    assert!((r["seconds"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert!(std::fs::metadata(r["path"].as_str().unwrap()).is_ok());
    assert_eq!(s.revision, rev);
}

#[test]
fn voices_resolve_by_name_style_and_id_and_scripts_parse() {
    let sp = vec![
        Speaker {
            name: "ずんだもん".into(),
            uuid: "u1".into(),
            styles: vec![Style { name: "ノーマル".into(), id: 3, kind: None }, Style { name: "あまあま".into(), id: 1, kind: None }],
            version: String::new(),
        },
        Speaker { name: "Anneli".into(), uuid: "u2".into(), styles: vec![Style { name: "Normal".into(), id: 888, kind: None }], version: String::new() },
    ];
    let e = TtsEngine::Voicevox;
    assert_eq!(resolve_voice(e, &sp, &json!("ずんだもん/あまあま")).unwrap().id, 1);
    assert_eq!(resolve_voice(e, &sp, &json!("ずんだもん／ノーマル")).unwrap().id, 3);
    assert_eq!(resolve_voice(e, &sp, &json!("anneli")).unwrap().id, 888);
    assert_eq!(resolve_voice(e, &sp, &json!("888")).unwrap().speaker, "Anneli");
    assert_eq!(resolve_voice(e, &sp, &json!({"speaker": "ずんだもん", "style": "あまあま"})).unwrap().id, 1);
    assert_eq!(resolve_voice(e, &sp, &json!({"id": 3})).unwrap().style, "ノーマル");
    assert_eq!(resolve_voice(e, &sp, &Value::Null).unwrap().id, 3);
    assert!(resolve_voice(e, &sp, &json!("ずんだもん/ささやき")).is_err());
    assert!(resolve_voice(e, &[], &Value::Null).is_err());
    assert!(resolve_voice(e, &sp, &json!([1])).is_err());

    let lines = parse_script("ずんだもん：こんにちは\nナレーション\n  \n# comment\n四国めたん「よろしく」\n12: 00\nA: B: C");
    assert_eq!(
        lines,
        [
            (Some("ずんだもん".into()), "こんにちは".into()),
            (None, "ナレーション".into()),
            (Some("四国めたん".into()), "よろしく".into()),
            (None, "12: 00".into()),
            (Some("A".into()), "B: C".into()),
        ]
    );
}

/// The demo's music clip on A2 (untyped, Volume −14 dB under the whole edit).
fn demo_music(s: &Session) -> filmcraft_project::TrackItem {
    s.active_sequence().unwrap().audio_tracks[1].items[0].clone()
}

#[test]
fn narration_is_auto_leveled_and_ducks_the_music_in_one_undo_step() {
    use filmcraft_project::AudioType;
    let url = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = url;
    let music0 = demo_music(&s);
    assert!(music0.essential.is_none());
    let undo0 = s.history.undo.len();
    let r = s
        .execute(
            "tts.synthesize",
            json!({"engine": "voicevox", "text": "こんにちは、今日はいい天気ですね。散歩に行きましょう", "seconds": 4.0, "dir": out_dir("mix"), "captions": false}),
        )
        .unwrap();
    let mix = &r["mix"];
    assert_eq!(mix["errors"], json!([]), "{mix}");
    let clip = r["clips"][0].as_u64().unwrap();
    assert_eq!(mix["dialogue"], json!([clip]));
    // the narration: Dialogue, loudness matched to the dialogue target (the fake tone is quiet, so up)
    let q = s.active_sequence().unwrap();
    let (_, it) = q.find_item(filmcraft_project::ClipId(clip)).unwrap();
    let es = it.essential.as_ref().unwrap();
    assert_eq!(es.kind, AudioType::Dialogue);
    assert!(es.loudness.enabled);
    assert_eq!(es.loudness.target_lufs, Some(-23.0));
    assert!(es.loudness.gain_db > 3.0, "match gain {}", es.loudness.gain_db);
    assert_eq!(mix["loudness"]["targetLufs"], -23.0);
    assert!(mix["loudness"]["clips"][0]["gainDb"].as_f64().unwrap() > 3.0, "{mix}");
    // the music under it: typed Music and ducked by 15 dB below its −14 dB while the narration speaks
    let m = demo_music(&s);
    assert_eq!(m.essential.as_ref().map(|e| e.kind), Some(AudioType::Music));
    assert!(mix["typedMusic"].as_array().unwrap().contains(&json!(m.id.0)), "{mix}");
    let ducked = mix["ducked"].as_array().unwrap();
    assert!(ducked.iter().any(|d| d["clip"] == m.id.0 && d["keyframes"].as_u64().unwrap() >= 4), "{mix}");
    let lvl = m.effect("volume").unwrap().param("level").unwrap();
    assert!(lvl.is_animated());
    let at = |sec: f64| lvl.f64_at(m.source_time_at(Tick::from_seconds_f64(sec)));
    assert!((at(4.8) - -29.0).abs() < 1e-6, "ducked under the narration: {}", at(4.8));
    assert!((at(0.5) - -14.0).abs() < 1e-6, "open before it: {}", at(0.5));
    assert_eq!(mix["reduceDb"], -15.0);
    // the narration's own track is not ducked; one undo step takes it all back
    assert_eq!(s.history.undo.len(), undo0 + 1);
    assert_eq!(s.history.undo.last().unwrap().0, "Synthesize Narration");
    s.execute("edit.undo", json!({})).unwrap();
    let m = demo_music(&s);
    assert_eq!(m, music0, "undo restores the music");
}

#[test]
fn auto_level_and_ducking_follow_params_and_settings() {
    use filmcraft_project::AudioType;
    let url = fake(Mode::Good, false);
    let mut s = demo();
    s.prefs.narration.voicevox_url = url;
    let music0 = demo_music(&s);
    // both off by parameter: the clips are placed as they are
    let r = s
        .execute(
            "tts.synthesize",
            json!({"engine": "voicevox", "text": "こんにちは", "dir": out_dir("mixoff"), "autoLevel": false, "duck": false, "seconds": 1.0}),
        )
        .unwrap();
    assert_eq!(r["mix"]["dialogue"], json!([]));
    let clip = filmcraft_project::ClipId(r["clips"][0].as_u64().unwrap());
    assert!(s.active_sequence().unwrap().find_item(clip).unwrap().1.essential.is_none());
    assert_eq!(demo_music(&s), music0);
    s.execute("edit.undo", json!({})).unwrap();
    // Settings ▸ Narration ▸ Duck off: auto-level only
    s.prefs.narration.duck = false;
    let r = s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "こんにちは", "dir": out_dir("mixlvl"), "seconds": 1.0})).unwrap();
    let clip = filmcraft_project::ClipId(r["clips"][0].as_u64().unwrap());
    assert_eq!(s.active_sequence().unwrap().find_item(clip).unwrap().1.essential.as_ref().map(|e| e.kind), Some(AudioType::Dialogue));
    assert_eq!(r["mix"]["ducked"], json!([]));
    assert_eq!(demo_music(&s), music0);
    s.execute("edit.undo", json!({})).unwrap();
    // ducking by parameter with its own depth (hostile values are clamped) and a dialogue target
    let r = s
        .execute(
            "tts.synthesize",
            json!({"engine": "voicevox", "text": "こんにちは、さようなら", "dir": out_dir("mixdeep"), "seconds": 1.0, "duck": true, "duckReduceDb": -1e9, "dialogueLufs": -16.0}),
        )
        .unwrap();
    assert_eq!(r["mix"]["reduceDb"], -40.0);
    assert_eq!(r["mix"]["loudness"]["targetLufs"], -16.0);
    let m = demo_music(&s);
    let lvl = m.effect("volume").unwrap().param("level").unwrap();
    let min = lvl.keyframes.iter().filter_map(|k| k.value.as_f64()).fold(f64::INFINITY, f64::min);
    assert!((min - -54.0).abs() < 1e-6, "{min}");
    // nothing under the narration: nothing to duck, no error
    s.execute("edit.undo", json!({})).unwrap();
    let r = s.execute("tts.synthesize", json!({"engine": "voicevox", "text": "これでおわりです", "dir": out_dir("mixend"), "seconds": 600.0})).unwrap();
    assert_eq!((r["mix"]["ducked"].clone(), r["mix"]["errors"].clone()), (json!([]), json!([])), "{}", r["mix"]);
    assert_eq!(demo_music(&s), music0);
}

/// Against a real engine, when one runs on this machine (`cargo test -- --ignored`).
#[test]
#[ignore]
fn real_engine_when_running() {
    let mut s = demo();
    let e = s.execute("tts.engines", json!({})).unwrap();
    let Some(eng) = e["engines"].as_array().unwrap().iter().find(|x| x["running"] == true).map(|x| x["id"].as_str().unwrap().to_string()) else {
        eprintln!("no engine running; skipped");
        return;
    };
    let r = s.execute("tts.synthesize", json!({"engine": eng, "text": "これはテストです。", "dir": out_dir("real")})).unwrap();
    assert!(r["lines"][0]["seconds"].as_f64().unwrap() > 0.3);
    assert!(!r["creditText"].as_str().unwrap().is_empty());
}
