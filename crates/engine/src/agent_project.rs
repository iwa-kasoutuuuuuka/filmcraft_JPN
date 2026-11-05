//! Project file next to an agent's export.
//!
//! When an AI agent or a script makes a video headlessly (`filmcraft-cli exec | run | export`, the
//! headless MCP server), it also gets the edit as a FilmCraft project, so the user can open it in
//! the desktop app and keep editing. After a successful `file.exportMedia`, `export.quick` or
//! export queue run the project is saved:
//!
//! - to `projectPath` when the command is given one;
//! - else to the project's own file when it has one (opened with `--project`, or saved before);
//! - else to `<video name>.fcproj` beside the video. An existing file there is never overwritten:
//!   the name gets a number (`clip 2.fcproj`).
//!
//! Before saving, media referenced by a relative path is made absolute (the app has a different
//! working directory), and media that lives in the OS temp folder (generated audio, extracted
//! files, an agent's scratch renders) is copied into a `<project name> Media` folder beside the
//! project, since temp folders get cleaned. The command result carries `project` (the saved path)
//! or `projectError` (the export itself still succeeded).
//!
//! It is on by default in headless sessions ([`Session::save_project_on_export`], set by the CLI
//! and its MCP server; `--no-project` turns it off) and off in the desktop app. Any caller can
//! override it per command with `saveProject: true | false`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filmcraft_project::{ItemKind, MediaRef};
use serde_json::{Value, json};

use crate::commands::{bool_p, str_p};
use crate::{EngineError, Result, Session};

/// Media larger than this stays where it is in the temp folder (copying it would be slow).
const MAX_COLLECT_BYTES: u64 = 2 << 30;
/// Numbered names tried for a free project file before giving up.
const MAX_NAME_TRIES: u32 = 999;

/// Whether this export saves the project: `saveProject`, else `projectPath` given, else the
/// session's default.
pub(crate) fn wanted(s: &Session, p: &Value) -> bool {
    match bool_p(p, "saveProject") {
        Some(b) => b,
        None => str_p(p, "projectPath").is_some() || s.save_project_on_export,
    }
}

/// After a successful export to `video`: save the project if wanted and merge the outcome into
/// `out` (`project` + `collectedMedia`, or `projectError`).
pub(crate) fn after_export(s: &mut Session, p: &Value, video: &str, out: &mut Value) {
    if !wanted(s, p) {
        return;
    }
    let add = match save(s, p, video) {
        Ok(v) => v,
        Err(e) => json!({"projectError": e.to_string()}),
    };
    if let (Some(o), Value::Object(a)) = (out.as_object_mut(), add) {
        o.extend(a);
    }
}

fn save(s: &mut Session, p: &Value, video: &str) -> Result<Value> {
    let path = match str_p(p, "projectPath").map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => with_fcproj(&absolute(x)),
        None => match s.path.clone() {
            Some(x) => x,
            None => free_path(s, &project_path_for(&absolute(video)))?,
        },
    };
    absolutize_media(s);
    let collected = collect_temp_media(s, &path, &crate::temp_dir());
    crate::commands::write_project(s, &path, true)?;
    Ok(json!({"project": path, "collectedMedia": collected}))
}

/// An absolute form of `p` (relative to the working directory); `p` itself when that fails.
fn absolute(p: &str) -> String {
    if cfg!(target_arch = "wasm32") || p.is_empty() {
        return p.to_string();
    }
    std::path::absolute(p).map(|a| a.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string())
}

fn with_fcproj(p: &str) -> String {
    if p.to_ascii_lowercase().ends_with(".fcproj") { p.to_string() } else { format!("{p}.fcproj") }
}

/// `<dir>/<video stem>.fcproj`.
pub(crate) fn project_path_for(video: &str) -> String {
    let v = Path::new(video);
    let stem = v.file_stem().map(|x| x.to_string_lossy().into_owned()).filter(|x| !x.is_empty()).unwrap_or_else(|| "Project".into());
    let file = format!("{stem}.fcproj");
    match v.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.join(file).to_string_lossy().into_owned(),
        None => file,
    }
}

/// `want`, or `want` with a number (`name 2.fcproj`, …) when a file already exists there.
fn free_path(s: &Session, want: &str) -> Result<String> {
    if s.services.file_size(want).is_err() {
        return Ok(want.to_string());
    }
    let stem = want.strip_suffix(".fcproj").unwrap_or(want);
    for k in 2..=MAX_NAME_TRIES {
        let c = format!("{stem} {k}.fcproj");
        if s.services.file_size(&c).is_err() {
            return Ok(c);
        }
    }
    Err(EngineError::Other(format!("{want}: no free project file name (pass `projectPath`)")))
}

/// Every file reference in the project: (item, is proxy, path).
fn file_refs(s: &Session) -> Vec<(filmcraft_project::ItemId, bool, String)> {
    let mut out = Vec::new();
    for (id, it) in &s.project.items {
        let ItemKind::Media(m) = &it.kind else { continue };
        if let MediaRef::File { path } = &m.media {
            out.push((*id, false, path.clone()));
        }
        if let Some(MediaRef::File { path }) = &m.proxy {
            out.push((*id, true, path.clone()));
        }
    }
    out
}

fn set_ref(s: &mut Session, id: filmcraft_project::ItemId, proxy: bool, to: String) {
    let Some(m) = Arc::make_mut(&mut s.project).item_mut(id).and_then(|i| i.as_media_mut()) else { return };
    let r = if proxy { m.proxy.as_mut() } else { Some(&mut m.media) };
    if let Some(MediaRef::File { path }) = r {
        *path = to;
    }
}

/// Media paths relative to the working directory become absolute (the desktop app opens the
/// project from elsewhere).
fn absolutize_media(s: &mut Session) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    for (id, proxy, path) in file_refs(s) {
        if path.is_empty() || !Path::new(&path).is_relative() {
            continue;
        }
        let abs = absolute(&path);
        if abs != path {
            set_ref(s, id, proxy, abs);
        }
    }
}

/// Whether `b` holds the same bytes as `a` (of `size` bytes). Big files are compared by size and
/// their first and last MiB, as the media identity fingerprint does.
fn same_content(a: &Path, b: &Path, size: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    const MIB: u64 = 1 << 20;
    if std::fs::metadata(b).map(|m| m.len()).ok() != Some(size) {
        return false;
    }
    let chunk = |p: &Path, from: SeekFrom, n: u64| -> Option<Vec<u8>> {
        let mut f = std::fs::File::open(p).ok()?;
        f.seek(from).ok()?;
        let mut v = Vec::new();
        f.take(n).read_to_end(&mut v).ok()?;
        Some(v)
    };
    if size <= 2 * MIB {
        return chunk(a, SeekFrom::Start(0), size).is_some_and(|x| chunk(b, SeekFrom::Start(0), size) == Some(x));
    }
    let tail = SeekFrom::Start(size.saturating_sub(MIB));
    let head_eq = chunk(a, SeekFrom::Start(0), MIB).is_some_and(|x| chunk(b, SeekFrom::Start(0), MIB) == Some(x));
    head_eq && chunk(a, tail, MIB).is_some_and(|x| chunk(b, tail, MIB) == Some(x))
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Copy media that lives under `temp` into `<project dir>/<project name> Media/` and point the
/// project at the copies. Nothing is copied when the project itself is saved under `temp`, for
/// image sequences (many files) or for files over [`MAX_COLLECT_BYTES`]. Failures leave the
/// reference as it was.
pub(crate) fn collect_temp_media(s: &mut Session, project_path: &str, temp: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    if cfg!(target_arch = "wasm32") {
        return out;
    }
    let temp = canonical(temp);
    let proj = Path::new(project_path);
    let Some(dir) = proj.parent() else { return out };
    if canonical(dir).starts_with(&temp) {
        return out;
    }
    let stem = proj.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "Project".into());
    let media_dir = dir.join(format!("{stem} Media"));
    for (id, proxy, path) in file_refs(s) {
        let src = Path::new(&path);
        if !canonical(src).starts_with(&temp) {
            continue;
        }
        let seq = s.project.item(id).and_then(|i| i.as_media()).is_some_and(|m| m.info.kind == filmcraft_media::MediaKind::ImageSequence);
        let size = std::fs::metadata(src).map(|m| m.len()).ok();
        let (false, Some(size)) = (seq, size) else { continue };
        if size > MAX_COLLECT_BYTES {
            continue;
        }
        let Some(name) = src.file_name() else { continue };
        if std::fs::create_dir_all(&media_dir).is_err() {
            return out;
        }
        let (st, ext) =
            (src.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(), src.extension().map(|x| x.to_string_lossy().into_owned()));
        let mut dest = media_dir.join(name);
        let mut k = 2u32;
        // a file of that name with other content is never overwritten
        while dest.exists() && !same_content(src, &dest, size) {
            if k > MAX_NAME_TRIES {
                break;
            }
            dest = media_dir.join(match &ext {
                Some(e) => format!("{st} {k}.{e}"),
                None => format!("{st} {k}"),
            });
            k = k.saturating_add(1);
        }
        let ok = if dest.exists() {
            // an earlier save already collected this file
            same_content(src, &dest, size)
        } else {
            std::fs::copy(src, &dest).is_ok()
        };
        if !ok {
            continue;
        }
        let to = dest.to_string_lossy().into_owned();
        out.push(json!({"item": id.0, "from": path, "to": to}));
        set_ref(s, id, proxy, to);
    }
    out
}
