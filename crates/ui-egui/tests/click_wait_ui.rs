//! Automation input by id waits for the element: a click sent right after a menu opened a dialog
//! (before the dialog was laid out where it settles) lands on the dialog's control, and an id that
//! never shows up is an error instead of a silent no-op.

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
    /// The reply and how many frames it took.
    fn call(&mut self, method: &str, params: Value) -> (Value, usize) {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for n in 1..=600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return (v, n);
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (r, _) = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn editor(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["chart_editor"].clone()
    }
}

#[test]
fn a_click_right_after_opening_a_dialog_reaches_it() {
    let mut d = Driver::demo();
    // no frames in between: the dialog has not been drawn (or only in its sizing pass) yet
    d.ok("ui.menu.invoke", json!({"id": "graphics.newChart"}));
    d.ok("ui.click", json!({"id": "chartEditor.kind.line"}));
    d.frames(2);
    assert_eq!(d.editor()["chart"]["kind"], "line", "the click landed on the Line button");

    // the same for a drag sourced from an element that is just appearing
    d.ok("ui.click", json!({"id": "chartEditor.cancel"}));
    d.frames(2);
    assert!(d.editor().is_null());
    d.ok("ui.menu.invoke", json!({"id": "graphics.newChart"}));
    d.ok("ui.click", json!({"id": "chartEditor.kind.pie"}));
    d.frames(2);
    assert_eq!(d.editor()["chart"]["kind"], "pie");
}

#[test]
fn input_aimed_at_an_unknown_id_is_an_error() {
    let mut d = Driver::demo();
    let bogus = "no.such.element";
    for (method, params) in [
        ("ui.click", json!({"id": bogus})),
        ("ui.move", json!({"id": bogus})),
        ("ui.scroll", json!({"id": bogus, "dy": -40.0})),
        ("ui.drag", json!({"from": {"id": bogus}, "to": {"x": 10.0, "y": 10.0}})),
        ("ui.drag", json!({"from": {"x": 10.0, "y": 10.0}, "to": {"id": bogus}})),
        ("ui.timeline.hit", json!({"id": bogus})),
        ("ui.screenshot", json!({"id": bogus})),
    ] {
        let (r, frames) = d.call(method, params.clone());
        assert_eq!(r["ok"], false, "{method} {params}: {r}");
        let e = r["error"].as_str().unwrap_or_default();
        assert!(e.contains("no on-screen element") && e.contains(bogus), "{method}: {e}");
        assert!(frames <= 30, "{method} gave up after a bounded wait ({frames} frames)");
    }
    // the app still answers normally afterwards
    d.ok("ui.click", json!({"id": "tools.Razor"}));
}

/// A click by id on a control under an open modal dialog (Settings) is an error: it would land on
/// the dialog's backdrop and do nothing. Before, it replied ok. Controls in the dialog still work.
#[test]
fn a_click_behind_a_modal_dialog_is_an_error() {
    let mut d = Driver::demo();
    let tool = d.ok("ui.elements", json!({"prefix": "tools."}));
    let behind = tool.as_array().and_then(|a| a.first()).and_then(|e| e["id"].as_str()).map(str::to_string).expect("a tool button");
    assert_eq!(d.call("ui.click", json!({"id": behind})).0["ok"], true, "reachable while nothing is open");
    d.ok("ui.menu.invoke", json!({"id": "app.settings.audio"}));
    d.frames(3);
    let (r, _) = d.call("ui.click", json!({"id": behind}));
    assert_eq!(r["ok"], false, "{r}");
    assert!(r["error"].as_str().unwrap_or("").contains("behind an open dialog"), "{r}");
    // the dialog's own controls are clickable
    d.ok("ui.click", json!({"id": "settings.category.general"}));
    d.ok("ui.click", json!({"id": "settings.cancel"}));
    d.frames(3);
    assert_eq!(d.call("ui.click", json!({"id": behind})).0["ok"], true, "reachable again once it closes");
}
