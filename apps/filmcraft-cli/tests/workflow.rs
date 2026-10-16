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
