//! The Source monitor's own transport through the control channel: with the Source monitor
//! focused, the arrow / Home / End keys move the Source playhead (they moved the sequence's), M
//! adds a clip marker (it added a sequence marker), and Space / J / K / L and the Play button play
//! the Source clip (it did not play at all).

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
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
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn ok(&mut self, m: &str, p: Value) -> Value {
        let (req, reply) = ControlRequest::new(m, p.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{m} {p}: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {m}");
    }
    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }
}

#[test]
fn source_monitor_keys_and_playback_act_on_the_source_clip() {
    let mut d = Driver::new();
    let media = d.app().session.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.video.is_some())).map(|i| i.id.0).unwrap();
    d.ok("engine.execute", json!({"command": "source.open", "params": {"item": media}}));
    d.ok("engine.execute", json!({"command": "source.setPlayhead", "params": {"seconds": 1.0}}));
    d.ok("engine.execute", json!({"command": "playhead.set", "params": {"seconds": 5.0}}));
    d.ok("ui.set", json!({"focused": "Source"}));
    d.frames(2);
    let seq_ph = d.app().session.playhead();
    // stepping
    let s0 = d.app().session.state.source_playhead;
    d.ok("ui.key", json!({"key": "Right"}));
    d.frames(2);
    assert!(d.app().session.state.source_playhead > s0, "Right steps the Source playhead");
    d.ok("ui.key", json!({"key": "Home"}));
    d.frames(2);
    assert_eq!(d.app().session.state.source_playhead, filmcraft_time::Tick::ZERO, "Home: the clip's start");
    assert_eq!(d.app().session.playhead(), seq_ph, "the sequence playhead stays");
    // a clip marker, not a sequence marker
    let seq_markers = d.app().session.active_sequence().unwrap().markers.len();
    d.ok("ui.key", json!({"key": "M"}));
    d.frames(2);
    assert_eq!(d.app().session.active_sequence().unwrap().markers.len(), seq_markers, "no sequence marker");
    let clip_markers = d.app().session.project.item(filmcraft_engine::project::ItemId(media)).and_then(|i| i.as_media()).map(|m| m.markers.len()).unwrap();
    assert!(clip_markers >= 1, "a clip marker on the Source clip");
    // Space plays the Source clip; the sequence stays put
    d.ok("ui.key", json!({"key": "Space"}));
    assert!(d.app().source_playing(), "Space plays the Source clip");
    assert!(!d.app().playback.playing, "not the sequence");
    let before = d.app().session.state.source_playhead;
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        d.frames(1);
    }
    assert!(d.app().session.state.source_playhead > before, "the Source playhead advances while playing");
    assert_eq!(d.app().session.playhead(), seq_ph);
    d.ok("ui.key", json!({"key": "K"}));
    d.frames(1);
    assert!(!d.app().source_playing(), "K stops");
    // the transport's Play button too
    d.ok("ui.click", json!({"id": "source.transport.src.play"}));
    d.frames(2);
    assert!(d.app().source_playing(), "the Play button plays the Source clip");
    // playing the sequence stops the Source playback
    d.ok("ui.set", json!({"focused": "Timeline"}));
    d.ok("ui.key", json!({"key": "Space"}));
    d.frames(2);
    assert!(!d.app().source_playing() && d.app().playback.playing);
}
