//! Runtime smoke test of the real `filmcraft-cli` binary, as a user drives it: a project saved,
//! reopened, edited and exported, all under a path with spaces and non-ASCII (Japanese) names.
//! On Windows this is the check that paths, file replacement and the process exit codes work
//! outside the unit tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn work_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("filmcraft cli {tag} {}", std::process::id())).join("プロジェクト フォルダ");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn cli(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_filmcraft-cli")).arg("--data-dir").arg(data).args(args).output().unwrap()
}

fn ok(data: &Path, args: &[&str]) -> Value {
    let o = cli(data, args);
    assert!(o.status.success(), "{args:?} failed: {}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
}

#[test]
fn help_and_version() {
    let d = work_dir("help");
    for flag in ["--help", "-h", "help"] {
        let o = cli(&d, &[flag]);
        assert!(o.status.success(), "{flag}");
        assert!(String::from_utf8_lossy(&o.stdout).contains("SUBCOMMANDS"), "{flag}");
    }
    let o = cli(&d, &["frobnicate"]);
    assert_eq!(o.status.code(), Some(2), "unknown subcommand is a usage error");
}

#[test]
fn save_reopen_edit_and_export_under_a_japanese_path() {
    let d = work_dir("workflow");
    let data = d.join("data");
    let proj = d.join("編集 テスト.fcproj");
    let p = proj.to_str().unwrap();

    // --save-as is honoured by every subcommand (inspect used to ignore it)
    ok(&data, &["--demo", "--save-as", p, "inspect", "project"]);
    assert!(proj.is_file(), "inspect --save-as wrote nothing");

    // reopen, edit, save in place
    let seq = ok(&data, &["--project", p, "--compact", "inspect", "sequence"]);
    let clips_before = seq.to_string().matches("\"clip\":").count();
    ok(&data, &["--project", p, "--save", "exec", "timeline.razor", "seconds=1"]);
    let seq = ok(&data, &["--project", p, "--compact", "inspect", "sequence"]);
    assert!(seq.to_string().matches("\"clip\":").count() > clips_before, "the razor cut was saved");

    // export a custom range (`--range START,END`) and read the file back
    let out = d.join("書き出し 結果.mp4");
    ok(&data, &["--project", p, "export", out.to_str().unwrap(), "--range", "0,1"]);
    let info = ok(&data, &["probe", out.to_str().unwrap()]);
    assert_eq!(info["video"]["codec"].as_str().map(|c| c.starts_with("H.264")), Some(true), "{info}");
    let secs = info["duration"].as_i64().unwrap_or(0) as f64 / 254_016_000_000.0;
    assert!((secs - 1.0).abs() < 0.1, "1 s range exported, got {secs} s");

    // a still frame to a Japanese file name
    let png = d.join("フレーム.png");
    let o = cli(&data, &["--project", p, "render", "--seconds", "0.5", "--scale", "0.25", "--out", png.to_str().unwrap()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(std::fs::metadata(&png).unwrap().len() > 0);

    // a `run` script saved with a UTF-8 BOM (Windows PowerShell 5, Notepad) runs
    let script = d.join("手順.jsonl");
    std::fs::write(&script, "\u{feff}{\"id\":\"timeline.razor\",\"params\":{\"seconds\":2}}\n").unwrap();
    let o = cli(&data, &["--project", p, "run", script.to_str().unwrap()]);
    assert!(o.status.success(), "BOM script: {}", String::from_utf8_lossy(&o.stderr));

    // a bad --range is a usage error, not a crash
    assert_eq!(cli(&data, &["--project", p, "export", out.to_str().unwrap(), "--range", "0,x"]).status.code(), Some(2));
    let _ = std::fs::remove_dir_all(d.parent().unwrap());
}

/// A 16-bit mono 48 kHz WAV of `n` samples of a quiet ramp.
fn wav(n: u32) -> Vec<u8> {
    let data = n * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&48_000u32.to_le_bytes());
    v.extend_from_slice(&96_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    for i in 0..n {
        v.extend_from_slice(&(((i % 200) as i16 - 100) * 20).to_le_bytes());
    }
    v
}

/// What an agent does: a `run` script that edits and exports. The export leaves a FilmCraft
/// project beside the video that reopens with the same sequence and media (imported by a path
/// relative to the script's working directory), and never overwrites an existing project.
#[test]
fn a_script_export_also_writes_a_project_that_reopens() {
    let d = work_dir("agent project");
    let data = d.join("data");
    std::fs::write(d.join("ナレーション.wav"), wav(24_000)).unwrap();
    let video = d.join("作品 1.mp4");
    let script = d.join("edit.jsonl");
    let export = serde_json::json!({"id": "file.exportMedia", "params": {"path": video, "format": "h264", "range": "custom", "startSeconds": 0, "endSeconds": 1, "wait": true}});
    let lines = [
        r#"{"id":"file.import","params":{"paths":["ナレーション.wav"]}}"#.to_string(),
        r#"{"id":"timeline.razor","params":{"seconds":1}}"#.to_string(),
        export.to_string(),
        r#"{"id":"sequence.inspect"}"#.to_string(),
    ];
    std::fs::write(&script, lines.join("\n")).unwrap();
    // run from the project folder, so the import path is relative
    let run = |extra: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_filmcraft-cli"))
            .current_dir(&d)
            .arg("--data-dir")
            .arg(&data)
            .arg("--demo")
            .args(extra)
            .arg("run")
            .arg(&script)
            .output()
            .unwrap();
        assert!(o.status.success(), "run failed: {}", String::from_utf8_lossy(&o.stderr));
        let out: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        out
    };
    let out = run(&[]);
    let proj = d.join("作品 1.fcproj");
    assert_eq!(out[2]["result"]["project"].as_str().map(PathBuf::from), Some(proj.clone()), "{:?}", out[2]);
    assert!(video.is_file() && proj.is_file());

    // reopened from another working directory: same sequence, no missing media
    let elsewhere = data.clone();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let reopen = |args: &[&str]| {
        let o = Command::new(env!("CARGO_BIN_EXE_filmcraft-cli"))
            .current_dir(&elsewhere)
            .arg("--data-dir")
            .arg(&data)
            .arg("--project")
            .arg(&proj)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice::<Value>(&o.stdout).unwrap()
    };
    let seq = reopen(&["--compact", "inspect", "sequence"]);
    assert_eq!(seq["tracks"], out[3]["result"]["tracks"], "the reopened project has the exported sequence");
    let missing = reopen(&["--compact", "exec", "media.findMissing"]);
    assert_eq!(missing["missing"].as_array().map(Vec::len), Some(0), "{missing}");

    // the same script again: a numbered project, the first one untouched
    let first = std::fs::read(&proj).unwrap();
    let out = run(&[]);
    assert_eq!(out[2]["result"]["project"].as_str().map(PathBuf::from), Some(d.join("作品 1 2.fcproj")));
    assert_eq!(std::fs::read(&proj).unwrap(), first);

    // --no-project: the video only
    let out = run(&["--no-project"]);
    assert!(out[2]["result"].get("project").is_none(), "{:?}", out[2]);
    assert!(!d.join("作品 1 3.fcproj").exists());

    // `export` reports the project, and saves into the --project file it was given
    let o = cli(&data, &["--project", proj.to_str().unwrap(), "export", d.join("again.mp4").to_str().unwrap(), "--range", "0,0.5"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["project"].as_str().map(PathBuf::from), Some(proj.clone()), "{v}");
    assert!(String::from_utf8_lossy(&o.stderr).contains("project saved to"));
    assert!(!d.join("again.fcproj").exists());
    let _ = std::fs::remove_dir_all(d.parent().unwrap());
}
