//! Workflow acceptance: one real job, end to end, driven only through engine commands (what the
//! CLI, MCP and the UI dispatch), with ffmpeg / ffprobe judging the delivered files.
//!
//! An interview and a B-roll shot (made by ffmpeg) are imported; the interview is cut and its
//! head ripple-deleted; the B-roll goes on V2, is trimmed and brightened with Lumetri; a Japanese
//! caption is added and exported as SRT; the sequence is delivered as H.264 / AAC with loudness
//! normalisation; and the edit survives an FCPXML round trip.
//! Each step checks the timeline, so a regression names the step that broke.

use std::path::{Path, PathBuf};
use std::process::Command;

use filmcraft_engine::Session;
use serde_json::{Value, json};

const TPS: f64 = 254_016_000_000.0;

fn secs(t: &Value) -> f64 {
    t.as_i64().unwrap_or(i64::MIN) as f64 / TPS
}

fn out_dir() -> PathBuf {
    let d = filmcraft_testkit::fixtures_dir("engine").join(format!("workflow 受け入れ {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(ff: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new(ff).args(["-v", "error", "-y"]).args(args).output().unwrap();
    assert!(o.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, p.clone()).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// Video track `n` of the active sequence: (clip id, start s, end s, item id) per clip.
fn track(s: &mut Session, n: usize) -> Vec<(u64, f64, f64, u64)> {
    let q = run(s, "sequence.inspect", json!({}));
    q["video"][n]["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    let start = secs(&c["start"]);
                    (c["clip"].as_u64().unwrap(), start, start + secs(&c["duration"]), c["item"].as_u64().unwrap_or(0))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.05
}

#[test]
fn interview_cut_broll_colour_captions_and_delivery() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let d = out_dir();

    // ---- footage: a 6 s interview (picture + quiet speech-like tone) and a 4 s silent B-roll
    let interview = d.join("インタビュー.mp4");
    ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:sample_rate=48000,volume=0.05",
            "-t",
            "6",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            interview.to_str().unwrap(),
        ],
    );
    let broll = d.join("B-roll 夜景.mov");
    ffmpeg(&ff, &["-f", "lavfi", "-i", "mandelbrot=size=640x360:rate=25", "-t", "4", "-c:v", "libx264", "-pix_fmt", "yuv420p", broll.to_str().unwrap()]);

    let mut s = Session::default();
    // ---- 1. import
    let r = run(&mut s, "file.import", json!({"paths": [interview, broll]}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    let items: Vec<u64> = r["items"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    let [iv, br] = items[..] else { panic!("two items: {r}") };

    // ---- 2. a sequence from the interview (its settings follow the clip)
    run(&mut s, "file.newSequenceFromClip", json!({"items": [iv]}));
    let v1 = track(&mut s, 0);
    assert_eq!(v1.len(), 1);
    assert!(close(v1[0].2, 6.0), "interview fills 6 s: {v1:?}");

    // ---- 3. cut at 1 s and ripple-delete the head (the linked audio goes with it)
    run(&mut s, "timeline.razor", json!({"seconds": 1.0}));
    let v1 = track(&mut s, 0);
    assert_eq!(v1.len(), 2, "razor made two clips: {v1:?}");
    run(&mut s, "edit.rippleDelete", json!({"clips": [v1[0].0]}));
    let v1 = track(&mut s, 0);
    assert_eq!(v1.len(), 1);
    assert!(close(v1[0].1, 0.0) && close(v1[0].2, 5.0), "rippled to 0..5 s: {v1:?}");
    let q = run(&mut s, "sequence.inspect", json!({}));
    let a1 = q["audio"][0]["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(a1.len(), 1, "linked audio rippled too: {a1:?}");
    assert!(close(secs(&a1[0]["duration"]), 5.0));

    // ---- 4. B-roll on V2 at 2 s, trimmed to 2 s
    run(&mut s, "timeline.place", json!({"item": br, "track": "V2", "seconds": 2.0}));
    let v2 = track(&mut s, 1);
    assert_eq!(v2.len(), 1, "B-roll on V2: {v2:?}");
    assert!(close(v2[0].1, 2.0) && close(v2[0].2, 6.0), "{v2:?}");
    run(&mut s, "timeline.trim", json!({"clip": v2[0].0, "edge": "out", "mode": "regular", "delta": (-2.0 * TPS) as i64}));
    let v2 = track(&mut s, 1);
    assert!(close(v2[0].1, 2.0) && close(v2[0].2, 4.0), "trimmed to 2..4 s: {v2:?}");
    let broll_clip = v2[0].0;

    // ---- 5. colour: Lumetri exposure +1 on the B-roll
    run(&mut s, "effects.apply", json!({"clips": [broll_clip], "effect": "lumetri"}));
    run(&mut s, "effects.setParam", json!({"clip": broll_clip, "effect": "lumetri", "param": "exposure", "value": 1.0}));

    // ---- 6. a Japanese caption, exported as SRT
    run(&mut s, "captions.newTrack", json!({"format": "Subtitle", "language": "ja"}));
    run(&mut s, "captions.add", json!({"text": "こんにちは、FilmCraft です", "seconds": 0.6, "durationSeconds": 2.0}));
    let srt = d.join("字幕.srt");
    run(&mut s, "captions.export", json!({"path": srt}));
    let text = std::fs::read_to_string(&srt).expect("SRT is UTF-8");
    assert!(text.contains("こんにちは、FilmCraft です"), "{text}");
    assert!(text.contains("00:00:00,600 --> 00:00:02,600"), "{text}");

    // ---- 7. deliver: H.264 / AAC, loudness normalised to -16 LUFS
    let mp4 = d.join("納品 -16LUFS.mp4");
    let r = run(
        &mut s,
        "file.exportMedia",
        json!({"path": mp4, "format": "h264", "wait": true, "settings": {"effects": {"loudness": {"enabled": true, "target_lufs": -16.0, "true_peak_dbtp": -1.0}}}}),
    );
    assert!(r["result"]["frames"].as_u64().is_some_and(|f| f == 125), "5 s at 25 fps: {r}");
    let probe =
        Command::new(&fp).args(["-v", "error", "-show_entries", "stream=codec_name,width,height:format=duration", "-of", "json"]).arg(&mp4).output().unwrap();
    let pj: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let codecs: Vec<&str> = pj["streams"].as_array().unwrap().iter().filter_map(|x| x["codec_name"].as_str()).collect();
    assert_eq!(codecs, ["h264", "aac"], "{pj}");
    let dur: f64 = pj["format"]["duration"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0);
    assert!((dur - 5.0).abs() < 0.1, "duration {dur}");
    // integrated loudness by ffmpeg's EBU R128 meter
    let o = Command::new(&ff).args(["-hide_banner", "-nostats", "-i"]).arg(&mp4).args(["-af", "ebur128", "-f", "null", "-"]).output().unwrap();
    let log = String::from_utf8_lossy(&o.stderr);
    let lufs: f64 = log
        .rsplit("Integrated loudness:")
        .next()
        .and_then(|t| t.split("I:").nth(1))
        .and_then(|t| t.split_whitespace().next())
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("no ebur128 summary: {log}"));
    assert!((lufs + 16.0).abs() <= 1.0, "integrated loudness {lufs} LUFS, wanted -16 ±1");

    // the B-roll is on top from 2 to 4 s and brighter than the source (Lumetri +1 stop)
    let luma = |file: &Path, at: f64| -> f64 {
        let raw = ffmpeg(&ff, &["-ss", &at.to_string(), "-i", file.to_str().unwrap(), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"]);
        raw.iter().map(|&v| v as f64).sum::<f64>() / raw.len().max(1) as f64
    };
    let (out_broll, src_broll, out_interview) = (luma(&mp4, 3.0), luma(&broll, 1.0), luma(&mp4, 1.0));
    assert!(out_broll > src_broll + 10.0, "Lumetri brightened the B-roll: {out_broll:.1} vs source {src_broll:.1}");
    assert!((out_broll - out_interview).abs() > 5.0, "V2 covers V1 at 3 s");

    // ---- 8. interchange: FCPXML out and back in keeps the edit
    let xml = d.join("編集.fcpxml");
    run(&mut s, "file.exportFcpxml", json!({"path": xml}));
    let mut s2 = Session::default();
    let r = run(&mut s2, "file.import", json!({"paths": [xml]}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    let seq = s2.project.items.values().find(|i| matches!(i.kind, filmcraft_project::ItemKind::Sequence(_))).map(|i| i.id);
    let seq = seq.expect("a sequence came back");
    run(&mut s2, "sequence.open", json!({"item": seq.0}));
    let (v1, v2) = (track(&mut s2, 0), track(&mut s2, 1));
    assert!(v1.len() == 1 && close(v1[0].1, 0.0) && close(v1[0].2, 5.0), "V1 after the round trip: {v1:?}");
    assert!(v2.len() == 1 && close(v2[0].1, 2.0) && close(v2[0].2, 4.0), "V2 after the round trip: {v2:?}");

    let _ = std::fs::remove_dir_all(&d);
}

/// A second job, end to end: two camera angles of one event (a shared clap in their sound) are
/// synchronised into a multi-camera source sequence by audio, cut between angles, mixed (one
/// track down, one muted), captioned and burned in, sent through the export queue, and the
/// project survives save and reopen.
#[test]
fn multicam_mix_burned_captions_queue_and_reopen() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let d = filmcraft_testkit::fixtures_dir("engine").join(format!("workflow マルチカム {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();

    // ---- two angles: different pictures, the same clap 1 s / 1.5 s into each recording
    let mut angles = Vec::new();
    for (i, (src, delay)) in [("testsrc2=size=640x360:rate=25", 1000), ("mandelbrot=size=640x360:rate=25", 1500)].iter().enumerate() {
        let p = d.join(format!("カメラ{}.mp4", i + 1));
        let clap = format!("sine=frequency=1000:sample_rate=48000:duration=0.1,adelay={delay}|{delay},apad=whole_dur=6");
        ffmpeg(
            &ff,
            &[
                "-f",
                "lavfi",
                "-i",
                src,
                "-f",
                "lavfi",
                "-i",
                &clap,
                "-t",
                "6",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                p.to_str().unwrap(),
            ],
        );
        angles.push(p);
    }
    let mut s = Session::default();
    let r = run(&mut s, "file.import", json!({"paths": angles.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>()}));
    let items: Vec<u64> = r["items"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    assert_eq!(items.len(), 2, "{r}");

    // ---- multi-camera source sequence, synchronised by audio
    let mc = run(&mut s, "clip.createMulticam", json!({"items": items, "method": "audio", "name": "イベント マルチカム"}));
    let mc_item = mc["item"].as_u64().or_else(|| mc["sequence"].as_u64()).unwrap_or_else(|| panic!("multicam item: {mc}"));
    run(&mut s, "file.newSequence", json!({"name": "本編", "width": 640, "height": 360, "fps": 25}));
    run(&mut s, "timeline.place", json!({"item": mc_item, "track": "V1", "seconds": 0.0}));
    let v1 = track(&mut s, 0);
    assert_eq!(v1.len(), 1, "{v1:?}");
    // cut to camera 2 at 2 s
    run(&mut s, "playhead.set", json!({"seconds": 2.0}));
    run(&mut s, "timeline.select", json!({"clips": [v1[0].0]}));
    // Ctrl+2: an edit at the playhead and camera 2 from there (a plain 2 only switches the clip)
    run(&mut s, "multicam.cutToCamera", json!({"camera": 2, "time": (2.0 * TPS) as i64}));
    let v1 = track(&mut s, 0);
    assert_eq!(v1.len(), 2, "the cut splits the multi-camera clip: {v1:?}");
    assert!(close(v1[1].1, 2.0), "{v1:?}");

    // ---- mix: A1 down 6 dB
    run(&mut s, "mixer.setStrip", json!({"strip": "A1", "volumeDb": -6.0}));
    let mixer = run(&mut s, "mixer.inspect", json!({}));
    assert!(mixer.to_string().contains("-6"), "{mixer}");

    // ---- captions, burned in
    run(&mut s, "captions.newTrack", json!({"format": "Subtitle", "language": "ja"}));
    run(&mut s, "captions.add", json!({"text": "乾杯！", "seconds": 0.5, "durationSeconds": 3.0}));
    let list = run(&mut s, "captions.list", json!({}));
    assert!(list.to_string().contains("乾杯"), "{list}");

    // ---- the export queue: plain and burned-in versions
    let plain = d.join("本編.mp4");
    let burned = d.join("本編 字幕入り.mp4");
    run(&mut s, "export.queue.add", json!({"format": "h264", "path": plain.to_string_lossy()}));
    run(&mut s, "export.queue.add", json!({"format": "h264", "path": burned.to_string_lossy(), "burnCaptions": true}));
    let q = run(&mut s, "export.queue.list", json!({}));
    assert_eq!(q["items"].as_array().map(Vec::len).or_else(|| q.as_array().map(Vec::len)), Some(2), "{q}");
    run(&mut s, "export.queue.start", json!({"wait": true}));
    for p in [&plain, &burned] {
        let o = Command::new(&fp).args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(p).output().unwrap();
        let dur: f64 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0.0);
        assert!(dur > 5.0, "{} lasts {dur} s", p.display());
    }
    // the burned-in version differs from the plain one where the caption is (1 s in)
    let grab = |p: &Path| ffmpeg(&ff, &["-ss", "1.0", "-i", p.to_str().unwrap(), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"]);
    let (a, b) = (grab(&plain), grab(&burned));
    let diff = a.iter().zip(&b).filter(|(x, y)| x.abs_diff(**y) > 40).count();
    assert!(diff > 500, "the caption is burned in ({diff} pixels differ)");

    // ---- save and reopen
    let proj = d.join("イベント.fcproj");
    run(&mut s, "file.saveAs", json!({"path": proj.to_string_lossy()}));
    let before = run(&mut s, "sequence.inspect", json!({}));
    let mut t = Session::default();
    run(&mut t, "file.open", json!({"path": proj.to_string_lossy()}));
    let after = run(&mut t, "sequence.inspect", json!({}));
    assert_eq!(before["video"], after["video"], "the cut survives save / open");
    assert_eq!(before["captions"].to_string(), after["captions"].to_string());
    let _ = std::fs::remove_dir_all(&d);
}
