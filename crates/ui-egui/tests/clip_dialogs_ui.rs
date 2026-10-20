//! Headless UI tests of the Edit / Clip / File menu dialogs (M3.10): Paste Attributes, Make
//! Subclip, Frame Hold Options, Modify ▸ Audio Channels, the Close Project prompt, and nested
//! menus (Clip ▸ Video Options ▸ Time Interpolation), and New ▸ Color Matte.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_engine::project::{ItemKind, Label};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            // the control channel's clicks and keys enter through the input hook
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
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
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    fn menu(&mut self, id: &str) -> Value {
        let r = self.ok("ui.menu.invoke", json!({"id": id}));
        self.frames(3);
        r
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }

    fn has(&mut self, id: &str) -> bool {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == json!(id))
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn v1(&mut self, i: usize) -> filmcraft_engine::project::TrackItem {
        self.app().session.active_sequence().unwrap().video_tracks[0].items[i].clone()
    }

    fn item_named(&mut self, name: &str) -> u64 {
        self.app().session.project.items.values().find(|i| i.name == name).unwrap().id.0
    }
}

#[test]
fn paste_attributes_dialog() {
    let mut d = Driver::new();
    let (a, b) = (d.v1(0), d.v1(2));
    d.exec("effects.setParam", json!({"clip": a.id.0, "effect": "motion", "param": "rotation", "value": 45.0}));
    d.exec("effects.setParam", json!({"clip": a.id.0, "effect": "opacity", "param": "opacity", "value": 25.0}));
    d.exec("effects.apply", json!({"clips": [a.id.0], "effect": "gaussian_blur"}));
    d.exec("timeline.select", json!({"clips": [a.id.0]}));
    d.exec("edit.copy", json!({}));
    d.exec("timeline.select", json!({"clips": [b.id.0]}));
    assert_eq!(d.menu("edit.pasteAttributes")["dialog"], "pasteAttributes");
    for id in
        ["pasteAttributes.motion", "pasteAttributes.opacity", "pasteAttributes.volume", "pasteAttributes.effect.gaussian_blur", "pasteAttributes.scaleTimes"]
    {
        assert!(d.has(id), "{id}");
    }
    // untick Opacity and the blur, then OK
    d.click("pasteAttributes.opacity");
    d.click("pasteAttributes.effect.gaussian_blur");
    d.click("pasteAttributes.ok");
    assert!(d.app().ui.clip_dialog.is_none(), "closed on OK");
    let got = d.v1(2);
    assert_eq!(got.effect("motion").unwrap().param("rotation").unwrap().value.as_f64(), Some(45.0));
    assert_eq!(got.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(100.0));
    assert!(got.effect("gaussian_blur").is_none());
    // Cancel leaves the clip alone; with params the command runs directly (no dialog)
    d.menu("edit.removeAttributes");
    d.click("removeAttributes.cancel");
    assert!(d.app().ui.clip_dialog.is_none());
    assert_eq!(d.v1(2).effect("motion").unwrap().param("rotation").unwrap().value.as_f64(), Some(45.0));
    d.exec("edit.removeAttributes", json!({"opacity": false}));
    assert_eq!(d.v1(2).effect("motion").unwrap().param("rotation").unwrap().value.as_f64(), Some(0.0));
}

#[test]
fn make_subclip_frame_hold_and_audio_channel_dialogs() {
    let mut d = Driver::new();
    let ocean = d.item_named("Ocean_Sunset.mp4");
    d.exec("source.open", json!({"item": ocean}));
    d.exec("project.setMarks", json!({"item": ocean, "in": filmcraft_time::FrameRate::FPS_23_976.tick_of(24).0}));
    d.menu("clip.makeSubclip");
    assert_eq!(d.app().ui.clip_dialog.as_ref().unwrap().params["startFrame"], 24);
    for id in ["makeSubclip.name", "makeSubclip.startFrame", "makeSubclip.endFrame", "makeSubclip.restrictTrims", "makeSubclip.ok"] {
        assert!(d.has(id), "{id}");
    }
    d.ok("ui.set", json!({"clipDialog": {"name": "Waves", "endFrame": 48}}));
    d.click("makeSubclip.ok");
    let sub = d.app().session.project.items.values().find(|i| i.name == "Waves").cloned().expect("subclip made");
    let ItemKind::Subclip { range, .. } = sub.kind else { panic!() };
    assert_eq!(range.duration, filmcraft_time::FrameRate::FPS_23_976.tick_of(24));

    // Frame Hold Options on the selected clip: Hold On ▸ Playhead
    let c = d.v1(1);
    d.exec("timeline.select", json!({"clips": [c.id.0]}));
    d.exec("playhead.set", json!({"time": (c.start + filmcraft_time::FrameRate::FPS_23_976.tick_of(5)).0}));
    d.menu("clip.frameHoldOptions");
    assert_eq!(d.app().ui.clip_dialog.as_ref().unwrap().params["enabled"], false, "Hold On starts unticked for a clip without a hold");
    d.click("frameHold.enabled");
    d.click("frameHold.holdOn.playhead");
    d.click("frameHold.holdFilters");
    d.click("frameHold.ok");
    let got = d.v1(1);
    assert_eq!(got.frame_hold, Some(c.source_in + filmcraft_time::FrameRate::FPS_23_976.tick_of(5)));
    assert!(got.hold_filters);

    // Modify ▸ Audio Channels (Shift+G) on a project item: Mono makes one clip per channel
    d.exec("project.select", json!({"items": [ocean]}));
    d.menu("clip.audioChannels");
    assert!(d.has("audioChannels.clip.0.ch.1"));
    d.click("audioChannels.format.mono");
    assert_eq!(d.app().ui.clip_dialog.as_ref().unwrap().params["clips"], json!([[0], [1]]));
    d.click("audioChannels.ok");
    let m = d.app().session.project.item(filmcraft_engine::project::ItemId(ocean)).unwrap().as_media().unwrap().interpret.audio_channels.clone().unwrap();
    assert_eq!(m.clips, vec![vec![0], vec![1]]);
}

#[test]
fn close_project_prompt_and_nested_menus() {
    let mut d = Driver::new();
    // nested menu paths reach the menu list
    let menus = d.ok("ui.menu.list", json!({}));
    let item = menus.as_array().unwrap().iter().find(|m| m["id"] == "clip.timeInterpolation.frameBlending").unwrap().clone();
    assert_eq!(item["path"], json!(["Clip", "Video Options", "Time Interpolation"]));
    let labels: Vec<&str> = menus.as_array().unwrap().iter().filter(|m| m["path"] == json!(["Edit", "Label"])).filter_map(|m| m["label"].as_str()).collect();
    assert_eq!(labels.len(), 17);
    // label from the menu
    let c = d.v1(0);
    d.exec("timeline.select", json!({"clips": [c.id.0]}));
    d.menu("edit.label.yellow");
    assert_eq!(d.v1(0).label, Label::Yellow);
    // the project is dirty: Close Project asks
    assert!(d.app().session.is_dirty());
    assert_eq!(d.menu("file.closeProject")["dialog"], "closeProject");
    assert!(d.has("closeProject.save") && d.has("closeProject.dontSave"));
    d.click("closeProject.cancel");
    assert!(!d.app().session.project.items.is_empty());
    d.menu("file.closeProject");
    d.click("closeProject.dontSave");
    assert!(d.app().session.project.items.is_empty(), "closed without saving");
    assert!(d.app().ui.clip_dialog.is_none());
}

#[test]
fn subclip_in_the_source_monitor_and_project_menu() {
    let mut d = Driver::new();
    let r24 = filmcraft_time::FrameRate::FPS_23_976;
    let ocean = d.item_named("Ocean_Sunset.mp4");
    d.exec("project.setMarks", json!({"item": ocean, "in": r24.tick_of(24).0, "out": r24.tick_of(71).0}));
    d.exec("project.select", json!({"items": [ocean]}));
    let sub = d.exec("clip.makeSubclip", json!({"name": "Waves Sub"}))["item"].as_u64().unwrap();
    d.exec("source.open", json!({"item": sub}));
    d.frames(4);
    assert_eq!(d.app().session.state.source_playhead, r24.tick_of(24), "opens at the subclip's In");
    // the scrub bar spans the subclip: its left end is the In, its right end the last frame
    let bar = d.ok("ui.elements", json!({"prefix": "source.scrubBar"}))[0]["rect"].clone();
    let (x, y, w, h) = (bar[0].as_f64().unwrap(), bar[1].as_f64().unwrap(), bar[2].as_f64().unwrap(), bar[3].as_f64().unwrap());
    d.ok("ui.click", json!({"x": x + w - 1.0, "y": y + h / 2.0}));
    d.frames(3);
    assert_eq!(d.app().session.state.source_playhead, r24.tick_of(71));
    d.ok("ui.click", json!({"x": x + 1.0, "y": y + h / 2.0}));
    d.frames(3);
    assert_eq!(d.app().session.state.source_playhead, r24.tick_of(24));
    // Project panel ▸ right-click ▸ Edit Subclip… opens the dialog; Convert to Master Clip converts
    d.ok("ui.panel.show", json!({"panel": "Project"}));
    d.exec("project.view.set", json!({"view": "list"}));
    d.exec("project.select", json!({"items": [sub]}));
    d.frames(4);
    // the subclip is made in its master clip's bin: expand it
    let bin = d.app().session.project.root.parent_of(filmcraft_engine::project::ItemId(sub));
    if let Some(b) = bin.filter(|b| *b != d.app().session.project.root.id) {
        d.click(&format!("project.bin.{}.toggle", b.0));
        d.frames(3);
    }
    let row = format!("project.item.{sub}");
    if d.has(&row) {
        d.ok("ui.click", json!({"id": row, "button": "right"}));
        d.frames(3);
        assert!(d.has("project.itemMenu.editSubclip") && d.has("project.itemMenu.convertToMaster"));
        d.click("project.itemMenu.editSubclip");
        assert_eq!(d.app().ui.clip_dialog.as_ref().map(|c| c.command.clone()).as_deref(), Some("clip.editSubclip"));
        d.click("editSubclip.cancel");
        d.ok("ui.click", json!({"id": row, "button": "right"}));
        d.frames(3);
        d.click("project.itemMenu.convertToMaster");
        assert!(d.app().session.project.item(filmcraft_engine::project::ItemId(sub)).unwrap().as_media().is_some(), "converted");
    } else {
        panic!("subclip row not shown: {:?}", d.ok("ui.elements", json!({"prefix": "project.item."})));
    }
}

/// File ▸ New ▸ Color Matte… asks for the colour (picker or hex), name, size and duration; it used
/// to make a grey matte straight away with no way to pick a colour (issue #29).
#[test]
fn color_matte_dialog_picks_the_colour() {
    let mut d = Driver::new();
    d.menu("file.newColorMatte");
    for id in ["colorMatte.color", "colorMatte.colorHex", "colorMatte.name", "colorMatte.width", "colorMatte.height", "colorMatte.seconds", "colorMatte.ok"] {
        assert!(d.has(id), "{id}");
    }
    d.ok("ui.set", json!({"clipDialog": {"color": "#ff8000", "name": "Orange", "seconds": 3.0}}));
    d.click("colorMatte.ok");
    assert!(d.app().ui.clip_dialog.is_none(), "OK closes the dialog");
    let it = d.app().session.project.items.values().find(|i| i.name == "Orange").cloned().expect("matte made");
    let rate = d.app().session.active_sequence().unwrap().settings.frame_rate;
    let src = d.app().session.media.cached(it.id).expect("matte source");
    let f = src.video_frame(filmcraft_media::FrameRequest::full(rate.tick_of(1))).unwrap().to_rgba8();
    assert_eq!(&f[..3], &[255, 128, 0], "the picked colour");
}

/// The other New Item commands open a dialog too (name, frame size, duration), as in Premiere;
/// they used to create the item with defaults straight away.
#[test]
fn synthetic_item_dialogs_set_name_size_and_duration() {
    let mut d = Driver::new();
    for (cmd, pre) in [
        ("file.newAdjustmentLayer", "adjustmentLayer"),
        ("file.newBarsAndTone", "barsAndTone"),
        ("file.newBlackVideo", "blackVideo"),
        ("file.newCountingLeader", "countingLeader"),
        ("file.newTransparentVideo", "transparentVideo"),
    ] {
        d.menu(cmd);
        for f in ["name", "width", "height", "seconds", "ok"] {
            assert!(d.has(&format!("{pre}.{f}")), "{pre}.{f}");
        }
        let name = format!("My {pre}");
        d.ok("ui.set", json!({"clipDialog": {"name": name, "width": 640, "height": 360, "seconds": 2.0}}));
        d.click(&format!("{pre}.ok"));
        assert!(d.app().ui.clip_dialog.is_none(), "{cmd}: OK closes the dialog");
        let it = d.app().session.project.items.values().find(|i| i.name == name).cloned().unwrap_or_else(|| panic!("{cmd} made {name}"));
        let size = match &it.kind {
            ItemKind::AdjustmentLayer { width, height, .. } => (*width, *height),
            _ => d.app().session.media.cached(it.id).and_then(|s| s.info().video.as_ref().map(|v| (v.width, v.height))).unwrap_or_default(),
        };
        assert_eq!(size, (640, 360), "{cmd}");
    }
}

/// Closing the window with unsaved changes asks first (it used to close at once, and the next
/// launch offered the changes as if FilmCraft had crashed): Cancel keeps the app and the changes;
/// Don't Save discards them (and their recovery journal) and lets the window close.
#[test]
fn closing_the_window_with_unsaved_changes_asks_first() {
    let mut d = Driver::new();
    let close = |d: &mut Driver| {
        d.harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        d.frames(3);
    };
    d.exec("file.newBin", json!({"name": "Unsaved"}));
    assert!(d.app().session.is_dirty());
    close(&mut d);
    assert_eq!(d.app().ui.clip_dialog.as_ref().map(|x| x.command.clone()).as_deref(), Some("app.quit"), "the prompt opened");
    for id in ["quitProject.save", "quitProject.dontSave", "quitProject.cancel"] {
        assert!(d.has(id), "{id}");
    }
    d.click("quitProject.cancel");
    assert!(d.app().ui.clip_dialog.is_none() && d.app().session.is_dirty(), "Cancel keeps the changes");
    close(&mut d);
    d.click("quitProject.dontSave");
    assert!(d.app().ui.clip_dialog.is_none());
    assert!(!d.app().session.is_dirty(), "Don't Save discards the changes");
    // nothing unsaved now: closing does not ask again
    close(&mut d);
    assert!(d.app().ui.clip_dialog.is_none());
}

/// Clip ▸ Speed/Duration… (Ctrl+R) and the Properties panel's speed button open the dialog; it used
/// to run with no parameters (silently resetting the speed to 100 %), the timeline's context menu
/// set 50 % straight away, and the Properties button did nothing.
#[test]
fn speed_duration_dialog_sets_speed_reverse_and_interpolation() {
    let mut d = Driver::new();
    let c = d.v1(0);
    d.exec("timeline.select", json!({"clips": [c.id.0]}));
    d.exec("clip.speedDuration", json!({"speed": 50.0}));
    d.frames(2);
    // the menu command opens the dialog with the clip's current speed instead of resetting it
    d.menu("clip.speedDuration");
    assert_eq!(d.app().ui.clip_dialog.as_ref().unwrap().params["speed"], json!(50.0));
    assert!((d.v1(0).speed - 0.5).abs() < 1e-9, "opening the dialog changes nothing");
    for id in ["speedDuration.speed", "speedDuration.reverse", "speedDuration.ripple", "speedDuration.interpolation.frameBlending", "speedDuration.ok"] {
        assert!(d.has(id), "{id}");
    }
    d.ok("ui.set", json!({"clipDialog": {"speed": 200.0}}));
    d.click("speedDuration.reverse");
    d.click("speedDuration.interpolation.frameBlending");
    d.click("speedDuration.ok");
    let got = d.v1(0);
    assert!((got.speed - 2.0).abs() < 1e-9 && got.reverse, "speed {} reverse {}", got.speed, got.reverse);
    assert_eq!(got.time_interpolation, filmcraft_engine::project::TimeInterpolation::FrameBlending);
    // Cancel leaves the clip alone
    d.menu("clip.speedDuration");
    d.ok("ui.set", json!({"clipDialog": {"speed": 10.0}}));
    d.click("speedDuration.cancel");
    assert!((d.v1(0).speed - 2.0).abs() < 1e-9);
    // the Properties panel's speed button opens the same dialog
    d.ok("ui.panel.show", json!({"panel": "Properties"}));
    d.frames(3);
    d.click("properties.speed");
    assert_eq!(d.app().ui.clip_dialog.as_ref().map(|x| x.command.clone()).as_deref(), Some("clip.speedDuration"));
    d.click("speedDuration.cancel");
    // its Crop keyframe buttons toggle animation (they did nothing)
    d.click("properties.section.Crop");
    if !d.has("properties.crop.left.animate") {
        d.click("properties.section.Crop");
    }
    d.click("properties.crop.left.animate");
    let animated = d.v1(0).effect("crop").and_then(|e| e.param("left")).is_some_and(|p| p.is_animated());
    assert!(animated, "Crop ▸ Left is animated");
}

/// New Project / Open / Demo Project on a project with unsaved changes ask first; New Project used
/// to replace it at once, dropping the changes and their recovery journal.
#[test]
fn replacing_a_project_with_unsaved_changes_asks_first() {
    let mut d = Driver::new();
    d.exec("file.newBin", json!({"name": "Unsaved work"}));
    let has_bin = |d: &mut Driver| d.app().session.project.root.children.iter().any(|c| format!("{c:?}").contains("Unsaved work"));
    assert!(d.app().session.is_dirty() && has_bin(&mut d));
    d.menu("file.newProject");
    assert_eq!(d.app().ui.clip_dialog.as_ref().map(|x| x.command.clone()).as_deref(), Some("app.replaceProject"), "the prompt opened");
    assert!(has_bin(&mut d), "nothing replaced yet");
    for id in ["replaceProject.save", "replaceProject.dontSave", "replaceProject.cancel"] {
        assert!(d.has(id), "{id}");
    }
    d.click("replaceProject.cancel");
    assert!(d.app().ui.clip_dialog.is_none() && has_bin(&mut d) && d.app().session.is_dirty(), "Cancel keeps the project");
    d.menu("file.openDemoProject");
    d.click("replaceProject.dontSave");
    assert!(d.app().ui.clip_dialog.is_none());
    assert!(!has_bin(&mut d) && !d.app().session.is_dirty(), "Don't Save: the demo project replaced it");
    // nothing unsaved: no prompt
    d.menu("file.newProject");
    assert!(d.app().ui.clip_dialog.is_none());
    assert!(d.app().session.project.items.is_empty(), "a new, empty project");
}

/// File ▸ New ▸ Sequence… opens the New Sequence dialog (presets, name, size, rate, tracks); it
/// used to make a 1080p 23.976 sequence straight away.
#[test]
fn new_sequence_dialog_picks_a_preset() {
    let mut d = Driver::new();
    let n0 = d.app().session.project.sequences().count();
    d.menu("file.newSequence");
    assert_eq!(d.app().session.project.sequences().count(), n0, "nothing made before OK");
    for id in ["newSequence.name", "newSequence.preset.0", "newSequence.width", "newSequence.fps", "newSequence.video", "newSequence.ok"] {
        assert!(d.has(id), "{id}");
    }
    d.click("newSequence.preset.8"); // Vertical 1080×1920 30
    d.ok("ui.set", json!({"clipDialog": {"name": "Reel", "video": 2}}));
    d.click("newSequence.ok");
    let seq = d.app().session.active_sequence().unwrap().clone();
    assert_eq!((seq.settings.width, seq.settings.height), (1080, 1920));
    assert!((seq.settings.frame_rate.as_f64() - 30.0).abs() < 1e-6);
    assert_eq!(seq.video_tracks.len(), 2);
    assert_eq!(d.app().session.project.sequences().count(), n0 + 1);
}

/// File ▸ Export ▸ EDL / OTIO / FCP XML / FCPXML ask where to save (they failed with "need
/// `path`" from the menu), and Export ▸ Media… opens Export mode.
#[test]
fn export_menu_items_ask_where_or_open_export_mode() {
    let mut d = Driver::new();
    let dir = std::env::temp_dir().join(format!("fc-menu-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.clone();
    d.app().hooks.pick_save_as = Some(Box::new(move |_filter: &str, exts: &[&str], suggested: &str| {
        assert!(suggested.ends_with(&format!(".{}", exts[0])), "{suggested} {exts:?}");
        Some(target.join(suggested).to_string_lossy().to_string())
    }));
    for id in ["file.exportEdl", "file.exportOtio", "file.exportFcp7Xml", "file.exportFcpxml"] {
        let r = d.ok("ui.menu.invoke", json!({"id": id}));
        let path = r["path"].as_str().map(str::to_string).or_else(|| r.as_str().map(str::to_string));
        assert!(std::fs::read_dir(&dir).unwrap().count() > 0, "{id} wrote a file: {r} {path:?}");
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4, "one file per format");
    d.menu("file.exportMedia");
    assert_eq!(format!("{:?}", d.app().ui.mode), "Export");
    let _ = std::fs::remove_dir_all(&dir);
}
