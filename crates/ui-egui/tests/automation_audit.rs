//! Automation coverage audit: every interactive widget egui reports to AccessKit (buttons, check
//! boxes, combo boxes, text inputs, sliders, drag values) in each workspace and mode must lie
//! inside a registered automation element, so agents can reach it by id (CLAUDE.md:
//! "everything is agent-drivable"). Prints what is not covered and fails if anything is. Covers
//! each workspace and mode in its default state (menus and dialogs are not opened).

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
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
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

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{method} {params}: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method}");
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
            if role == Role::Label || (!standard && !a.data().supports_action(egui::accesskit::Action::Click)) {
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
            let inside = reg.iter().any(|r| cx >= r[0] - 1.0 && cx <= r[0] + r[2] + 1.0 && cy >= r[1] - 1.0 && cy <= r[1] + r[3] + 1.0);
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
