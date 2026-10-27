//! Graphics UI, driven headless through the control channel (like `scripted.rs`): the Type tool
//! creates a text layer on the Program monitor, typing and keys edit it, the Properties panel's
//! controls style it, the Selection tool moves it.
//!
//! With `FILMCRAFT_UI_SHOTS=<dir>` the test also renders the UI with wgpu (works without a
//! window or an unlocked screen) and writes PNGs there for review.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    shots: Option<std::path::PathBuf>,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let shots = std::env::var_os("FILMCRAFT_UI_SHOTS").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if shots.is_some() {
            b = b.wgpu().with_pixels_per_point(1.0);
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, shots };
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
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn rect(&mut self, id: &str) -> [f64; 4] {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id}: {v}")).clone();
        let r: Vec<f64> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        [r[0], r[1], r[2], r[3]]
    }
    fn shot(&mut self, name: &str) {
        let Some(dir) = self.shots.clone() else { return };
        self.frames(8);
        let img = self.harness.render().expect("wgpu render");
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn type_tool_edit_style_and_move() {
    let mut d = Driver::demo();
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.exec("playhead.set", json!({"seconds": 2}));
    d.ok("ui.set", json!({"tool": "Type"}));
    d.frames(3);
    let pic = d.rect("program.picture");
    let (px, py) = (pic[0] + pic[2] * 0.2, pic[1] + pic[3] * 0.35);
    d.ok("ui.click", json!({"x": px, "y": py}));
    d.ok("ui.type", json!({"text": "Night Drive"}));
    d.ok("ui.key", json!({"key": "Enter"}));
    d.ok("ui.type", json!({"text": "through the city"}));
    let list = d.exec("graphics.list", json!({}));
    assert_eq!(list["layers"][0]["text"], "Night Drive\nthrough the city", "{list}");
    // caret editing: Backspace ×4, select the word "Drive" with Shift+arrows… then retype
    for _ in 0..4 {
        d.ok("ui.key", json!({"key": "Backspace"}));
    }
    d.ok("ui.type", json!({"text": "town"}));
    d.ok("ui.key", json!({"key": "ArrowUp"}));
    d.ok("ui.key", json!({"key": "End"}));
    for _ in 0..5 {
        d.ok("ui.key", json!({"key": "Shift+ArrowLeft"}));
    }
    d.ok("ui.type", json!({"text": "Ride"}));
    let list = d.exec("graphics.list", json!({}));
    assert_eq!(list["layers"][0]["text"], "Night Ride\nthrough the town", "{list}");
    // one undo step for the whole typing session (after "New Graphic")
    let hist = d.exec("history.list", json!({}));
    let undo: Vec<&str> = hist["undo"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(&undo[undo.len() - 2..], ["New Graphic", "Edit Text"], "{undo:?}");
    d.shot("type-tool-editing");

    // style through the Properties panel controls
    d.ok("ui.key", json!({"key": "Escape"}));
    d.ok("ui.set", json!({"tool": "Selection"}));
    d.ok("ui.click", json!({"id": "graphics.prop.faux_bold"}));
    d.ok("ui.click", json!({"id": "graphics.prop.align.1"}));
    d.ok("ui.click", json!({"id": "graphics.prop.stroke"}));
    d.ok("ui.click", json!({"id": "graphics.prop.shadow"}));
    let clip = list["clip"].as_u64().unwrap();
    d.exec("graphics.set", json!({"clip": clip, "props": {"font_style": "Bold", "fill_color": "#ffd23f", "size": 120}}));
    let seq = d.exec("sequence.inspect", json!({}));
    let s = seq.to_string();
    assert!(s.contains("graphic_text"), "graphic layer in the sequence");
    let list2 = d.exec("graphics.list", json!({"clip": clip}));
    let q0 = list2["layers"][0]["quad"][0][0].as_f64().unwrap();
    // selection-tool drag moves the layer
    let lr = d.rect(&format!("program.layer.{clip}.0"));
    let pic = d.rect("program.picture");
    // a point on the layer that is inside the picture
    let x0 = lr[0].max(pic[0]);
    let x1 = (lr[0] + lr[2]).min(pic[0] + pic[2]);
    let (cx, cy) = ((x0 + x1) / 2.0, lr[1] + lr[3] * 0.3);
    let from = json!({"x": cx, "y": cy});
    let to = json!({"x": cx + 60.0, "y": cy + 20.0});
    d.ok("ui.drag", json!({"from": from, "to": to, "steps": 6}));
    let list3 = d.exec("graphics.list", json!({"clip": clip}));
    let q1 = list3["layers"][0]["quad"][0][0].as_f64().unwrap();
    assert!(q1 > q0 + 20.0, "moved right: {q0} → {q1}");
    d.shot("properties-styled");

    // shapes: rectangle tool drag
    d.ok("ui.set", json!({"tool": "Rectangle"}));
    let pic = d.rect("program.picture");
    d.ok("ui.drag", json!({"from": {"x": pic[0] + pic[2] * 0.15, "y": pic[1] + pic[3] * 0.75}, "to": {"x": pic[0] + pic[2] * 0.85, "y": pic[1] + pic[3] * 0.9}, "steps": 5}));
    let list4 = d.exec("graphics.list", json!({"clip": clip}));
    assert_eq!(list4["layers"].as_array().unwrap().len(), 2, "{list4}");
    assert_eq!(list4["layers"][1]["kind"], "Rectangle");
    d.exec("graphics.arrangeLayer", json!({"clip": clip, "layer": 1, "to": "back"}));
    d.ok("ui.set", json!({"tool": "Selection"}));
    d.shot("shape-layer");
}

/// Japanese input on the Program monitor's text tool: the input method's composition shows at
/// the caret without changing the layer, keys while composing belong to the input method, and the
/// committed text is typed (it was dropped: converted Japanese never reached the layer).
#[test]
fn input_method_composition_types_japanese_into_a_text_layer() {
    let mut d = Driver::demo();
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.exec("playhead.set", json!({"seconds": 2}));
    d.ok("ui.set", json!({"tool": "Type"}));
    d.frames(3);
    let pic = d.rect("program.picture");
    d.ok("ui.click", json!({"x": pic[0] + pic[2] * 0.2, "y": pic[1] + pic[3] * 0.35}));
    d.ok("ui.type", json!({"text": "Sky "}));
    let ime = |d: &mut Driver, ev: egui::ImeEvent| {
        d.harness.input_mut().events.push(egui::Event::Ime(ev));
        d.frames(2);
    };
    ime(&mut d, egui::ImeEvent::Preedit { text: "ゆうひ".into(), active_range_chars: None });
    let list = d.exec("graphics.list", json!({}));
    assert_eq!(list["layers"][0]["text"], "Sky ", "composition is not typed yet: {list}");
    let pre = d.ok("ui.elements", json!({"prefix": "program.textEdit.preedit"}));
    assert_eq!(pre[0]["label"], "ゆうひ", "the composition shows at the caret");
    // Enter while composing converts in the input method: no new line in the layer
    d.harness.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    d.frames(2);
    ime(&mut d, egui::ImeEvent::Commit("夕日".into()));
    let list = d.exec("graphics.list", json!({}));
    assert_eq!(list["layers"][0]["text"], "Sky 夕日", "{list}");
    assert!(d.ok("ui.elements", json!({"prefix": "program.textEdit.preedit"})).as_array().unwrap().is_empty(), "composition ended");
    // typing continues after the committed text
    d.ok("ui.type", json!({"text": "!"}));
    assert_eq!(d.exec("graphics.list", json!({}))["layers"][0]["text"], "Sky 夕日!");
}

/// The input method around the text tool: the clause being converted is kept for drawing, the
/// candidate window is placed at the caret (not over the whole layer), a platform that reports the
/// commit again as typed text does not type it twice, and an edit that ends while composing types
/// the marked text instead of losing it.
#[test]
fn input_method_clause_caret_area_and_ending_while_composing() {
    let mut d = Driver::demo();
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.exec("playhead.set", json!({"seconds": 2}));
    d.ok("ui.set", json!({"tool": "Type"}));
    d.frames(3);
    let pic = d.rect("program.picture");
    d.ok("ui.click", json!({"x": pic[0] + pic[2] * 0.2, "y": pic[1] + pic[3] * 0.35}));
    d.ok("ui.type", json!({"text": "A big title line"}));
    let ime = |d: &mut Driver, evs: Vec<egui::Event>| {
        d.harness.input_mut().events.extend(evs);
        d.frames(2);
    };
    ime(&mut d, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "きょうは".into(), active_range_chars: Some(0..2) })]);
    let state = d.ok("ui.inspect", json!({}));
    let ed = &state["ui"]["gfx_edit"];
    assert_eq!(ed["preedit_clause"], json!([0, 2]), "the converted clause is kept: {ed}");
    // the candidate window goes to the caret and composition, not the layer's whole box
    let out = d.harness.output().platform_output.ime.expect("the text tool asks for the input method");
    let layer = d.rect("program.textEdit");
    let plate = d.rect("program.textEdit.preedit");
    assert!(out.rect.width() < layer[2] as f32, "IME area {:?} is narrower than the layer {layer:?}", out.rect);
    assert!(out.rect.contains(egui::pos2(plate[0] as f32 + 1.0, plate[1] as f32 + 1.0)), "IME area {:?} covers the composition {plate:?}", out.rect);
    // commit reported twice (as IME commit and as text): typed once
    ime(&mut d, vec![egui::Event::Ime(egui::ImeEvent::Commit("今日は".into())), egui::Event::Text("今日は".into())]);
    let list = d.exec("graphics.list", json!({}));
    assert_eq!(list["layers"][0]["text"], "A big title line今日は", "{list}");
    // compose again, then end the edit by clicking empty picture with the Type tool: the marked
    // text is typed into the layer it was composed in
    ime(&mut d, vec![egui::Event::Ime(egui::ImeEvent::Preedit { text: "はれ".into(), active_range_chars: None })]);
    d.ok("ui.click", json!({"x": pic[0] + pic[2] * 0.6, "y": pic[1] + pic[3] * 0.8}));
    d.frames(3);
    let list = d.exec("graphics.list", json!({}));
    let texts: Vec<&str> = list["layers"].as_array().unwrap().iter().filter_map(|l| l["text"].as_str()).collect();
    assert!(texts.contains(&"A big title line今日ははれ"), "the marked text was kept: {texts:?}");
}
