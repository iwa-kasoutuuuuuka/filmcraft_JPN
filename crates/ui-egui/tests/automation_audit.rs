//! Driving the app as an agent does. Automation coverage audit: every interactive widget egui reports to AccessKit (buttons, check
//! boxes, combo boxes, text inputs, sliders, drag values) in each workspace and mode must lie
//! mostly inside a registered automation element about its own size (a panel-sized element around
//! it does not count), so agents can reach it by id (CLAUDE.md:
//! "everything is agent-drivable"). Prints what is not covered and fails if anything is. Covers
//! each workspace and mode in its default state (menus and dialogs are not opened).
//! Also: control-channel typing reaches text fields while the window lacks the OS focus, and
//! placeholder text follows the UI language.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use egui_kittest::kittest::NodeT;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    /// Simulate a window without the OS focus (an agent driving the app in the background).
    unfocused: bool,
    /// Simulate a minimized window.
    minimized: bool,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, unfocused: false, minimized: false };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            if self.unfocused {
                raw.focused = false;
            }
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().minimized = Some(self.minimized);
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params}: {v}");
        v["result"].clone()
    }

    /// Registered automation rectangles (x, y, w, h).
    fn registered(&mut self) -> Vec<[f32; 4]> {
        let v = self.ok("ui.elements", json!({}));
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let r = &e["rect"];
                [r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32, r[2].as_f64().unwrap() as f32, r[3].as_f64().unwrap() as f32]
            })
            .collect()
    }

    /// Interactive AccessKit nodes outside every registered element: (role, label, centre).
    fn uncovered(&mut self) -> Vec<(String, String, (f32, f32))> {
        use egui::accesskit::Role;
        let reg = self.registered();
        let mut out = Vec::new();
        for n in self.harness.root().children_recursive() {
            let a = n.accesskit_node();
            let role = a.role();
            // standard widgets by role, custom-drawn ones by their click action; labels (a
            // truncated label is clickable for its tooltip) and full-width bars (header and
            // status bar backgrounds) are not controls
            let standard = matches!(
                role,
                Role::Button | Role::CheckBox | Role::RadioButton | Role::ComboBox | Role::TextInput | Role::Slider | Role::SpinButton | Role::Link
            );
            // (scroll bars: `ui.scroll` reaches them)
            if role == Role::Label || role == Role::ScrollBar || (!standard && !a.data().supports_action(egui::accesskit::Action::Click)) {
                continue;
            }
            let Some(b) = a.bounding_box() else { continue };
            if b.width() < 2.0 || b.height() < 2.0 || b.width() >= 1000.0 {
                continue;
            }
            let (cx, cy) = (((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
            if cx < 0.0 || cy < 0.0 || cx > 1600.0 || cy > 980.0 {
                continue;
            }
            // covered by an element about its own size: a panel- or lane-sized element around a
            // small control does not make the control reachable by id
            let area = (b.width() * b.height()) as f32;
            let inside = reg
                .iter()
                .filter(|r| r[2] * r[3] <= (4.0 * area).max(160.0 * 160.0))
                // most of the control lies inside the element (a control clipped by a scrolled
                // panel keeps the element of its full size)
                .any(|r| {
                    let ox = ((b.x1 as f32).min(r[0] + r[2] + 1.0) - (b.x0 as f32).max(r[0] - 1.0)).max(0.0);
                    let oy = ((b.y1 as f32).min(r[1] + r[3] + 1.0) - (b.y0 as f32).max(r[1] - 1.0)).max(0.0);
                    let centre = cx >= r[0] - 1.0 && cx <= r[0] + r[2] + 1.0 && cy >= r[1] - 1.0 && cy <= r[1] + r[3] + 1.0;
                    centre || ox * oy >= 0.5 * area
                });
            if !inside {
                out.push((format!("{role:?} {:.0}x{:.0}", b.width(), b.height()), a.label().unwrap_or_default(), (cx, cy)));
            }
        }
        out
    }
}

#[test]
fn interactive_widgets_have_automation_ids() {
    let mut d = Driver::demo();
    let mut total = 0;
    // select a clip so the Effect Controls / Lumetri / Properties panels have content
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let clip = seq["video"][0]["items"][0]["clip"].as_u64().unwrap();
    d.ok("engine.execute", json!({"command": "timeline.select", "params": {"clips": [clip]}}));
    for ws in filmcraft_ui_egui::dock::WORKSPACES {
        d.ok("ui.set", json!({"workspace": ws}));
        d.frames(4);
        let miss = d.uncovered();
        println!("== {ws}: {} uncovered", miss.len());
        for (role, label, (x, y)) in &miss {
            println!("   {role} {label:?} at ({x:.0}, {y:.0})");
        }
        total += miss.len();
    }
    for mode in ["export", "import"] {
        d.ok("ui.set", json!({"mode": mode}));
        d.frames(4);
        let miss = d.uncovered();
        println!("== mode {mode}: {} uncovered", miss.len());
        for (role, label, (x, y)) in &miss {
            println!("   {role} {label:?} at ({x:.0}, {y:.0})");
        }
        total += miss.len();
    }
    println!("TOTAL {total}");
    assert_eq!(total, 0, "interactive widgets without an automation id (listed above)");
}

/// Typing through the control channel reaches a text field while the window does not have the OS
/// focus (egui ignores keys for an unfocused window; agents drive the app in the background).
#[test]
fn control_typing_works_in_an_unfocused_window() {
    let mut d = Driver::demo();
    d.unfocused = true;
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(3);
    d.ok("engine.execute", json!({"command": "captions.newTrack", "params": {"format": "Subtitle", "language": "ja"}}));
    d.ok("engine.execute", json!({"command": "captions.add", "params": {"text": "New caption", "seconds": 1.0, "durationSeconds": 2.0}}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "text.tab.Captions"}));
    d.frames(2);
    let v = d.ok("ui.elements", json!({"prefix": "text.captions."}));
    let field = v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str()).find(|id| id.ends_with(".text")).map(str::to_string).expect("caption text field");
    d.ok("ui.click", json!({"id": field}));
    d.frames(2);
    d.ok("ui.key", json!({"key": "Ctrl+A"}));
    d.ok("ui.type", json!({"text": "夕日の海"}));
    // (a multi-line field: Escape leaves it, which commits the text)
    d.ok("ui.key", json!({"key": "Escape"}));
    d.frames(3);
    let list = d.ok("engine.execute", json!({"command": "captions.list", "params": {}}));
    assert!(list.to_string().contains("夕日の海"), "the caption text was typed: {list}");
}

/// A caption added with the Text panel's + button carries placeholder text in the UI's language.
#[test]
fn added_caption_placeholder_follows_the_ui_language() {
    let mut d = Driver::demo();
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(3);
    d.ok("engine.execute", json!({"command": "captions.newTrack", "params": {"format": "Subtitle", "language": "ja"}}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "text.tab.Captions"}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "text.captions.add"}));
    d.frames(3);
    let list = d.ok("engine.execute", json!({"command": "captions.list", "params": {}}));
    let text = list["tracks"][0]["captions"][0]["text"].as_str().unwrap_or_default().to_string();
    let want = if filmcraft_ui_egui::i18n::is_japanese() { "新規キャプション" } else { "New caption" };
    assert_eq!(text, want, "{list}");
}

/// An edit point grabbed and pulled quickly ripple-trims it: the drag is hit-tested where the
/// button went down, not where egui's drag threshold was crossed (that used to land on the
/// clip's body, so a quick ripple trim moved the clip instead).
#[test]
fn quick_drag_from_an_edit_point_trims() {
    let mut d = Driver::demo();
    // the music bed on A2 spans the edit: with sync lock on, a ripple there is refused (Premiere too)
    d.ok("engine.execute", json!({"command": "timeline.setTrack", "params": {"track": "A2", "syncLock": false}}));
    let v1 = |d: &mut Driver| d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}))["video"][0]["items"].clone();
    let before = v1(&mut d);
    let (clip, next) = (before[3]["clip"].as_u64().unwrap(), before[4]["clip"].as_u64().unwrap());
    let v = d.ok("ui.elements", json!({"prefix": format!("timeline.clip.{clip}")}));
    let r = v.as_array().unwrap().iter().find(|e| e["id"] == format!("timeline.clip.{clip}")).unwrap()["rect"].clone();
    let (right, cy) = (r[0].as_f64().unwrap() + r[2].as_f64().unwrap(), r[1].as_f64().unwrap() + r[3].as_f64().unwrap() / 2.0);
    d.ok("ui.set", json!({"tool": "Ripple"}));
    d.frames(2);
    // 2 pt inside the out point, 40 pt to the left in a few big steps
    d.ok("ui.drag", json!({"from": {"x": right - 2.0, "y": cy}, "to": {"x": right - 42.0, "y": cy}, "steps": 3}));
    d.frames(3);
    let after = v1(&mut d);
    let find = |items: &Value, id: u64| items.as_array().unwrap().iter().find(|i| i["clip"] == id).cloned().unwrap();
    let (a, b) = (find(&after, clip), find(&after, next));
    assert_eq!(a["start"], before[3]["start"], "the clip did not move: {a}");
    assert!(a["duration"].as_u64() < before[3]["duration"].as_u64(), "its out point was trimmed: {a}");
    assert_eq!(b["start"].as_u64(), Some(a["start"].as_u64().unwrap() + a["duration"].as_u64().unwrap()), "ripple: the next clip follows without a gap");
}

/// A minimized window drops synthetic input and presents no frames: UI requests say so at once
/// (they used to be dropped silently or time out), while engine commands keep working.
#[test]
fn minimized_window_reports_ui_requests_instead_of_dropping_them() {
    let mut d = Driver::demo();
    d.minimized = true;
    d.frames(2);
    for (method, params) in [("ui.key", json!({"key": "C"})), ("ui.click", json!({"x": 100.0, "y": 100.0})), ("ui.elements", json!({}))] {
        let v = d.call(method, params);
        assert_eq!(v["ok"], json!(false), "{method}: {v}");
        assert!(v["error"].as_str().unwrap_or_default().contains("minimized"), "{method}: {v}");
    }
    d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    d.ok("ui.set", json!({"tool": "Razor"}));
    d.minimized = false;
    d.frames(2);
    d.ok("ui.key", json!({"key": "V"}));
    assert_eq!(d.ok("ui.inspect", json!({}))["ui"]["tool"], "Selection");
}

/// A new, empty sequence shows its first minute, and a project item dropped near its start snaps
/// to the start (the timeline used to fit "1 s", zoomed in to single frames, so the first clip
/// dropped "at the start" landed a frame or more in, leaving black at the head).
#[test]
fn first_clip_dropped_near_the_start_of_an_empty_sequence_starts_at_zero() {
    let mut d = Driver::demo();
    // an item at the top of the Project panel (the demo's media sit in bins)
    let matte = d.ok("engine.execute", json!({"command": "file.newColorMatte", "params": {"color": "#3366ff"}}))["item"].as_u64().unwrap();
    d.ok("engine.execute", json!({"command": "file.newSequence", "params": {"name": "Empty"}}));
    d.frames(30);
    let rect = |d: &mut Driver, id: &str| -> [f64; 4] {
        let v = d.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no {id}")).clone();
        let r: Vec<f64> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        [r[0], r[1], r[2], r[3]]
    };
    let tracks = rect(&mut d, "timeline.tracks");
    let pps = d.ok("ui.inspect", json!({}))["ui"]["timeline"]["pps"].as_f64().unwrap();
    let shown = tracks[2] / pps;
    assert!((40.0..=90.0).contains(&shown), "an empty sequence shows about a minute, not {shown:.2} s");
    // drop it 6 pt into V1 (a fraction of a second, within snapping distance)
    let from = format!("project.item.{matte}");
    let v1 = rect(&mut d, "timeline.track.V1.target");
    d.ok("ui.drag", json!({"from": {"id": from}, "to": {"x": tracks[0] + 6.0, "y": v1[1] + v1[3] / 2.0}, "steps": 12}));
    d.frames(3);
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    assert_eq!(seq["video"][0]["items"][0]["start"], json!(0), "the clip starts at the sequence start: {}", seq["video"][0]["items"]);
}

#[derive(Debug)]
struct Dropped(std::path::PathBuf);
impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// Files dropped on the window: a folder imports its media (it failed with "access denied"),
/// an unsupported file is reported (errors were dropped silently), and a project file opens,
/// asking about unsaved changes first (it was refused as "unsupported media").
#[test]
fn dropped_folders_media_and_project_files() {
    let mut d = Driver::demo();
    let dir = std::env::temp_dir().join(format!("fc-drop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let folder = dir.join("素材フォルダ");
    std::fs::create_dir_all(&folder).unwrap();
    let wav = filmcraft_media::wav::write_wav16(&[0.0; 9600], 2, 48_000);
    for n in ["ナレーション.wav", "b-roll.wav"] {
        std::fs::write(folder.join(n), &wav).unwrap();
    }
    std::fs::write(folder.join("notes.txt"), b"not media").unwrap();
    let drop = |d: &mut Driver, paths: &[std::path::PathBuf]| {
        let files: Vec<egui::DroppedFileHandle> = paths.iter().map(|p| std::sync::Arc::new(Dropped(p.clone())) as egui::DroppedFileHandle).collect();
        d.harness.input_mut().dropped_files = files;
        d.frames(3);
    };
    let n0 = d.harness.state().session.project.items.len();
    drop(&mut d, std::slice::from_ref(&folder));
    assert_eq!(d.harness.state().session.project.items.len(), n0 + 2, "the folder's two audio files: {}", d.harness.state().ui.status);
    // an unsupported file says so
    let txt = folder.join("notes.txt");
    drop(&mut d, &[txt]);
    assert!(d.harness.state().ui.status.contains("notes.txt"), "{}", d.harness.state().ui.status);
    // a project file opens, after the unsaved-changes prompt
    let proj = dir.join("保存済み.fcproj");
    let mut other = Session::default();
    other.execute("file.newBin", json!({"name": "From the other project"})).unwrap();
    other.execute("file.saveAs", json!({"path": proj.to_string_lossy()})).unwrap();
    drop(&mut d, &[proj]);
    assert_eq!(d.harness.state().ui.clip_dialog.as_ref().map(|x| x.command.clone()).as_deref(), Some("app.replaceProject"));
    d.ok("ui.click", json!({"id": "replaceProject.dontSave"}));
    d.frames(3);
    let bins = format!("{:?}", d.harness.state().session.project.root.children);
    assert!(bins.contains("From the other project"), "{bins}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `engine.execute` replies list the parameters a command ignored, so an agent sees a misspelt
/// key instead of a silent no-op.
#[test]
fn ignored_parameters_come_back_as_warnings() {
    let mut d = Driver::demo();
    let v = d.call("engine.execute", json!({"command": "mixer.setStrip", "params": {"strip": "A1", "volume": -6.0}}));
    assert_eq!(v["ok"], json!(true), "{v}");
    assert!(v["warnings"][0].as_str().is_some_and(|w| w.contains("volume")), "{v}");
    let v = d.call("engine.execute", json!({"command": "mixer.setStrip", "params": {"strip": "A1", "volumeDb": -6.0}}));
    assert!(v.get("warnings").is_none(), "{v}");
}
