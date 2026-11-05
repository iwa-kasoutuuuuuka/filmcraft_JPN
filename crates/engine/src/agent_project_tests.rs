//! A project file beside an agent's export (`crate::agent_project`): where it goes, that it never
//! overwrites another file, the per-command switches, hostile params, and media collected from
//! the temp folder.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;

/// A scratch directory under the workspace `target/` (not the OS temp folder), removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let d = filmcraft_testkit::workspace_root().join("target").join("agent-project-tests").join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The demo project in a headless (agent) session.
fn agent() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.save_project_on_export = true;
    s
}

/// A short WAV export of the active sequence (the fastest format) with extra params.
fn export(s: &mut Session, video: &str, extra: Value) -> Value {
    let mut p = json!({"path": video, "format": "wav", "range": "custom", "startSeconds": 0.0, "endSeconds": 0.25, "wait": true});
    if let (Some(o), Value::Object(e)) = (p.as_object_mut(), extra) {
        o.extend(e);
    }
    s.execute("file.exportMedia", p).unwrap()
}

fn tracks(s: &mut Session) -> Value {
    s.execute("sequence.inspect", json!({})).unwrap()["tracks"].clone()
}

#[test]
fn desktop_sessions_save_nothing_by_default() {
    let d = Scratch::new("off");
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let r = export(&mut s, &d.path("clip.wav"), json!({}));
    assert!(r.get("project").is_none(), "{r}");
    assert!(!Path::new(&d.path("clip.fcproj")).exists());
    assert!(s.path.is_none());
    // …unless the command asks for it
    let r = export(&mut s, &d.path("clip.wav"), json!({"saveProject": true}));
    assert_eq!(r["project"], json!(d.path("clip.fcproj")));
}

#[test]
fn agent_export_saves_a_project_beside_the_video_that_reopens() {
    let d = Scratch::new("beside");
    let mut s = agent();
    s.execute("timeline.razor", json!({"seconds": 1.0})).unwrap();
    let before = tracks(&mut s);
    let r = export(&mut s, &d.path("my video.wav"), json!({}));
    let proj = d.path("my video.fcproj");
    assert_eq!(r["project"], json!(proj), "{r}");
    assert_eq!(s.path.as_deref(), Some(proj.as_str()), "the session adopts the project file");
    assert_eq!(s.saved_revision, s.revision, "saved, not dirty");

    let mut back = Session::default();
    back.execute("file.open", json!({"path": proj})).unwrap();
    assert_eq!(tracks(&mut back), before, "the reopened project has the exported sequence");

    // the next export of the same session saves to its own file again
    s.execute("timeline.razor", json!({"seconds": 2.0})).unwrap();
    let r = export(&mut s, &d.path("second.wav"), json!({}));
    assert_eq!(r["project"], json!(proj));
    assert!(!Path::new(&d.path("second.fcproj")).exists());
}

#[test]
fn an_existing_project_file_is_never_overwritten() {
    let d = Scratch::new("numbered");
    std::fs::write(d.path("clip.fcproj"), b"someone else's project").unwrap();
    let mut s = agent();
    let r = export(&mut s, &d.path("clip.wav"), json!({}));
    assert_eq!(r["project"], json!(d.path("clip 2.fcproj")), "{r}");
    assert_eq!(std::fs::read(d.path("clip.fcproj")).unwrap(), b"someone else's project");
    let mut s = agent();
    let r = export(&mut s, &d.path("clip.wav"), json!({}));
    assert_eq!(r["project"], json!(d.path("clip 3.fcproj")), "{r}");
}

#[test]
fn per_command_switches() {
    let d = Scratch::new("switches");
    let mut s = agent();
    let r = export(&mut s, &d.path("a.wav"), json!({"saveProject": false}));
    assert!(r.get("project").is_none() && !Path::new(&d.path("a.fcproj")).exists(), "{r}");
    // an explicit projectPath (the extension is added) may replace a file: the caller named it
    std::fs::write(d.path("named.fcproj"), b"old").unwrap();
    let r = export(&mut s, &d.path("a.wav"), json!({"projectPath": d.path("named")}));
    assert_eq!(r["project"], json!(d.path("named.fcproj")), "{r}");
    let mut back = Session::default();
    back.execute("file.open", json!({"path": d.path("named.fcproj")})).unwrap();
    // projectPath with saveProject:false saves nothing
    let r = export(&mut s, &d.path("b.wav"), json!({"projectPath": d.path("other.fcproj"), "saveProject": false}));
    assert!(r.get("project").is_none() && !Path::new(&d.path("other.fcproj")).exists(), "{r}");
}

#[test]
fn hostile_project_params_never_fail_the_export() {
    let d = Scratch::new("hostile");
    let mut s = agent();
    // a folder that does not exist: the video is written, the project error is reported
    let bad = d.0.join("no such folder").join("p.fcproj").to_string_lossy().into_owned();
    let r = export(&mut s, &d.path("v.wav"), json!({"projectPath": bad}));
    assert!(Path::new(&d.path("v.wav")).exists());
    assert!(r["projectError"].is_string() && r.get("project").is_none(), "{r}");
    // wrong types and empty strings fall back to the defaults
    for extra in [json!({"saveProject": "yes"}), json!({"projectPath": 7}), json!({"projectPath": "   "}), json!({"saveProject": null})] {
        let mut s = agent();
        let r = export(&mut s, &d.path("w.wav"), extra.clone());
        assert!(r["project"].is_string() || r["projectError"].is_string(), "{extra}: {r}");
    }
    assert!(Path::new(&d.path("w.fcproj")).exists());
    // a failed export saves nothing
    let mut s = agent();
    let r = s.execute(
        "file.exportMedia",
        json!({"path": d.path("x.wav"), "format": "wav", "range": "custom", "startSeconds": 5.0, "endSeconds": 1.0, "wait": true}),
    );
    if r.is_err() {
        assert!(!Path::new(&d.path("x.fcproj")).exists());
    }
}

#[test]
fn quick_export_and_queue_save_the_project_too() {
    let d = Scratch::new("queue");
    let mut s = agent();
    let r = s
        .execute(
            "export.quick",
            json!({"path": d.path("quick.wav"), "format": "wav", "range": "custom", "startSeconds": 0.0, "endSeconds": 0.25, "wait": true}),
        )
        .unwrap();
    assert_eq!(r["project"], json!(d.path("quick.fcproj")), "{r}");

    let mut s = agent();
    let q = json!({"path": d.path("queued.wav"), "format": "wav", "range": "custom", "startSeconds": 0.0, "endSeconds": 0.25, "start": true, "wait": true});
    let r = s.execute("export.queue.add", q).unwrap();
    assert_eq!(r["project"], json!(d.path("queued.fcproj")), "{r}");
    assert!(r["items"].as_array().is_some_and(|i| i.iter().all(|x| x["status"] == "done")), "{r}");

    // added without starting: nothing yet; export.queue.start saves
    let mut s = agent();
    let q = json!({"path": d.path("later.wav"), "format": "wav", "range": "custom", "startSeconds": 0.0, "endSeconds": 0.25});
    let r = s.execute("export.queue.add", q).unwrap();
    assert!(r.get("project").is_none(), "{r}");
    let r = s.execute("export.queue.start", json!({"wait": true})).unwrap();
    assert_eq!(r["project"], json!(d.path("later.fcproj")), "{r}");
}

#[test]
fn media_in_the_temp_folder_is_collected_beside_the_project() {
    let d = Scratch::new("collect");
    // a stand-in for the OS temp folder, holding a generated WAV
    let temp = d.0.join("temp");
    std::fs::create_dir_all(&temp).unwrap();
    let wav = temp.join("voice 1.wav");
    std::fs::write(&wav, crate::voiceover::write_wav_f32(&[0.25; 4800], 48_000)).unwrap();
    let mut s = agent();
    s.execute("file.import", json!({"paths": [wav.to_string_lossy()]})).unwrap();
    let proj = d.path("film.fcproj");
    let got = crate::agent_project::collect_temp_media(&mut s, &proj, &temp);
    let to = d.0.join("film Media").join("voice 1.wav");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["to"], json!(to.to_string_lossy()));
    assert_eq!(std::fs::read(&to).unwrap(), std::fs::read(&wav).unwrap());
    let points_at_copy =
        s.project.items.values().filter_map(|i| i.as_media()).any(|m| m.media == filmcraft_project::MediaRef::File { path: to.to_string_lossy().into_owned() });
    assert!(points_at_copy, "the project points at the copy");

    // collected again (a later save): the copy is reused, not duplicated
    s.execute("file.import", json!({"paths": [wav.to_string_lossy()]})).unwrap();
    let got = crate::agent_project::collect_temp_media(&mut s, &proj, &temp);
    assert!(got.len() <= 1, "{got:?}");
    assert!(!d.0.join("film Media").join("voice 1 2.wav").exists());

    // another file with the same name gets a numbered copy; the first is kept
    let other = temp.join("x");
    std::fs::create_dir_all(&other).unwrap();
    let wav2 = other.join("voice 1.wav");
    std::fs::write(&wav2, crate::voiceover::write_wav_f32(&[-0.5; 9600], 48_000)).unwrap();
    s.execute("file.import", json!({"paths": [wav2.to_string_lossy()]})).unwrap();
    let got = crate::agent_project::collect_temp_media(&mut s, &proj, &temp);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["to"], json!(d.0.join("film Media").join("voice 1 2.wav").to_string_lossy()));
    assert_eq!(std::fs::read(&to).unwrap(), std::fs::read(&wav).unwrap());

    // a project saved inside the temp folder collects nothing
    let mut s = agent();
    s.execute("file.import", json!({"paths": [wav.to_string_lossy()]})).unwrap();
    assert!(crate::agent_project::collect_temp_media(&mut s, &temp.join("p.fcproj").to_string_lossy(), &temp).is_empty());
}

#[test]
fn project_path_for_odd_video_names() {
    use crate::agent_project::project_path_for;
    assert_eq!(project_path_for("clip.mp4"), "clip.fcproj");
    assert_eq!(project_path_for(""), "Project.fcproj");
    assert_eq!(project_path_for(".mp4"), ".mp4.fcproj");
    let p = project_path_for(&Path::new("dir").join("a.b.mov").to_string_lossy());
    assert_eq!(p, Path::new("dir").join("a.b.fcproj").to_string_lossy());
}
