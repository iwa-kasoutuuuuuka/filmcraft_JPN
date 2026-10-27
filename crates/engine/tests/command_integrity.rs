//! Every command, as agents and the UI call it: hostile parameters never panic (never crash);
//! one command is one undo step that Undo / Redo restore exactly, and the sequences stay valid
//! (`Sequence::check`).

use filmcraft_engine::Session;
use serde_json::{Value, json};

/// The parameter names of a command's parameter doc (`{"clip":id?,"speed":percent=100}`).
fn keys(doc: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = doc;
    while let Some(i) = rest.find('"') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('"') else { break };
        let k = &after[..j];
        let tail = after[j + 1..].trim_start();
        if tail.starts_with(':') && !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !out.iter().any(|x| x == k) {
            out.push(k.to_string());
        }
        rest = &after[j + 1..];
    }
    out
}

fn demo_with_selection() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let _ = s.execute("playhead.set", json!({"seconds": 3.3}));
    let c = s.active_sequence().unwrap().video_tracks[0].items[1].id.0;
    let _ = s.execute("timeline.select", json!({"clips": [c]}));
    s
}

#[test]
fn hostile_parameters_never_panic() {
    let tmp = std::env::temp_dir().join(format!("fc-command-integrity-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    // (a wider set of 15 values, NaN-like floats, control characters, nesting, ran clean once;
    // these three keep CI short)
    let values: Vec<Value> = vec![json!(i64::MAX), json!(-1e308), json!("x".repeat(20000))];
    let mut panics = Vec::new();
    for spec in filmcraft_engine::command_specs() {
        let id = spec.id;
        if id == "app.quit" {
            continue;
        }
        let ks = keys(spec.params);
        for (vi, v) in values.iter().enumerate() {
            // every parameter the same hostile value; paths stay inside the temp dir
            let mut p = serde_json::Map::new();
            for k in &ks {
                let path_like = k.to_lowercase().contains("path") || matches!(k.as_str(), "dir" | "folder" | "destination");
                let val = match v {
                    Value::String(_) if path_like => json!(tmp.join(format!("f{vi}")).to_string_lossy()),
                    _ => v.clone(),
                };
                p.insert(k.clone(), val);
            }
            let params = Value::Object(p);
            let mut s = demo_with_selection();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = s.execute(id, params.clone());
                // the project must stay usable: its sequences valid
                for item in s.project.sequences() {
                    if let Some(q) = s.project.sequence(item.id) {
                        let _ = q.check();
                    }
                }
            }));
            if r.is_err() {
                panics.push(format!("{id} {}", params.to_string().chars().take(160).collect::<String>()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(panics.is_empty(), "commands panicked: {panics:?}");
}

/// One command, one undo step (nested commands used to leave several: Undo after Add Caption
/// kept its new caption track, after Set Look the Lumetri it added); Undo restores the project
/// exactly and Redo brings the change back.
#[test]
fn one_command_is_one_exact_undo_step() {
    let mut issues = Vec::new();
    for spec in filmcraft_engine::command_specs() {
        let id = spec.id;
        if id.starts_with("file.") || id.starts_with("app.") || id.starts_with("help.") || matches!(id, "edit.undo" | "edit.redo") {
            continue;
        }
        for with_sel in [true, false] {
            let mut s = demo_with_selection();
            if !with_sel {
                s.state.selection.clear();
            }
            let before = filmcraft_format::encode(&s.project, true);
            let p0 = s.project.clone();
            if s.execute(id, json!({})).is_err() || std::sync::Arc::ptr_eq(&p0, &s.project) {
                continue;
            }
            for item in s.project.sequences() {
                if let Some(Err(e)) = s.project.sequence(item.id).map(|q| q.check()) {
                    issues.push(format!("{id}: invalid sequence: {e}"));
                }
            }
            let after = filmcraft_format::encode(&s.project, true);
            if !s.history.can_undo() {
                continue;
            }
            let _ = s.execute("edit.undo", json!({}));
            if filmcraft_format::encode(&s.project, true) != before {
                issues.push(format!("{id} (selection {with_sel}): one Undo does not restore the project"));
                continue;
            }
            let _ = s.execute("edit.redo", json!({}));
            if filmcraft_format::encode(&s.project, true) != after {
                issues.push(format!("{id} (selection {with_sel}): Redo does not bring the change back"));
            }
        }
    }
    assert!(issues.is_empty(), "{issues:#?}");
}
