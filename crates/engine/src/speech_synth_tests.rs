use std::sync::Arc;

use serde_json::json;

use crate::Session;
use filmcraft_project::ItemId;
use filmcraft_speech::FixedSynthesizer;

fn session(name: &str) -> (Session, std::path::PathBuf) {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name": "tts"})).ok();
    s.synthesizer = Some(Arc::new(FixedSynthesizer::default()));
    let dir = std::env::temp_dir().join(format!("filmcraft-tts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (s, dir)
}

#[test]
fn lines_become_wavs_and_media_items_in_one_undo_step() {
    let (mut s, dir) = session("lines");
    let n_items = s.project.items.len();
    let r = s
        .execute(
            "speech.synthesize",
            json!({"lines": [
                {"id": "l1", "text": "こんにちは", "refAudio": "ref.wav", "refText": "x"},
                {"id": "l2", "text": "さようなら、また明日"},
                {"id": "bad", "text": "FAIL"},
            ], "out": dir.to_string_lossy()}),
        )
        .unwrap();
    assert_eq!(r["synthesizer"], json!("fixed"));
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{r}");
    assert_eq!(lines[0]["id"], json!("l1"));
    assert!(lines[0]["path"].as_str().unwrap().ends_with("l1.wav"));
    assert!((lines[0]["seconds"].as_f64().unwrap() - 0.75).abs() < 1e-6);
    assert!((lines[1]["seconds"].as_f64().unwrap() - 1.5).abs() < 1e-6);
    let item = ItemId(lines[0]["item"].as_u64().unwrap_or_else(|| panic!("{r}")));
    let clip = s.project.item(item).unwrap();
    assert!(clip.has_audio());
    assert!((clip.duration().seconds() - 0.75).abs() < 0.01);
    assert_eq!(s.project.items.len(), n_items + 2);
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    assert_eq!(r["failed"][0]["id"], json!("bad"));
    // one undo step removes both imports
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), n_items);
    assert!(dir.join("l1.wav").is_file() && dir.join("l2.wav").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_text_without_import_and_odd_ids() {
    let (mut s, dir) = session("one");
    let n_items = s.project.items.len();
    let r = s.execute("speech.synthesize", json!({"text": "テスト", "id": "../evil/名前 ?", "import": false, "out": dir.to_string_lossy()})).unwrap();
    assert_eq!(s.project.items.len(), n_items);
    let path = r["lines"][0]["path"].as_str().unwrap();
    assert!(path.ends_with("..evil名前.wav"), "{path}");
    assert!(std::path::Path::new(path).is_file());
    assert!(r["lines"][0].get("item").is_none());
    let r = s.execute("speech.synthesize", json!({"lines": [{"id": 7, "text": "a"}, {"text": "b"}], "import": false, "out": dir.to_string_lossy()})).unwrap();
    assert!(r["lines"][0]["path"].as_str().unwrap().ends_with("7.wav"));
    assert!(r["lines"][1]["path"].as_str().unwrap().ends_with("2.wav"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_input_is_refused_before_anything_runs() {
    let (mut s, dir) = session("bad");
    let out = dir.to_string_lossy().to_string();
    assert!(s.execute("speech.synthesize", json!({"out": out})).is_err());
    assert!(s.execute("speech.synthesize", json!({"lines": [{"id": "x"}], "out": out})).is_err());
    assert!(s.execute("speech.synthesize", json!({"lines": [{"id": "a", "text": "x"}, {"id": "a", "text": "y"}], "out": out})).is_err());
    assert!(s.execute("speech.synthesize", json!({"lines": [{"id": "a", "text": "  "}], "out": out})).is_err());
    assert!(!dir.exists());
    // every line failing is an error, not an empty success
    let e = s.execute("speech.synthesize", json!({"text": "FAIL", "out": out})).unwrap();
    assert_eq!(e["lines"].as_array().unwrap().len(), 0);
    assert_eq!(e["failed"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_python_the_error_names_what_was_looked_for() {
    let mut s = Session::default();
    let dir = std::env::temp_dir().join(format!("filmcraft-tts-nopy-{}", std::process::id()));
    let e = s
        .execute("speech.synthesize", json!({"text": "x", "out": dir.to_string_lossy(), "script": dir.join("missing.py").to_string_lossy()}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("missing.py") && e.contains("README"), "{e}");
    let e = s
        .execute("speech.synthesize", json!({"text": "x", "out": dir.to_string_lossy(), "python": dir.join("no-such-python").to_string_lossy()}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("no-such-python"), "{e}");
    let info = s.execute("speech.synthesizer", json!({"script": dir.join("missing.py").to_string_lossy()})).unwrap();
    assert_eq!(info["installed"], json!(false));
    assert_eq!(info["scriptFound"], json!(false));
    assert_eq!(info["ok"], json!(false));
    assert!(info["probe"]["error"].as_str().unwrap().contains("missing.py"));
    s.synthesizer = Some(Arc::new(FixedSynthesizer::default()));
    let info = s.execute("speech.synthesizer", json!({})).unwrap();
    assert_eq!(info["installed"], json!(true));
    assert_eq!(info["id"], json!("fixed"));
}
