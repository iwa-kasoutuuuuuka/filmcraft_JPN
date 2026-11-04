//! The Animated Chart editor, driven headless through the control channel: Graphics ▸ New Layer ▸
//! Animated Chart… opens it, the type, theme, palette and table controls change the chart, Add to
//! Timeline places it, Edit Chart… reopens it on the clip and Apply / Cancel change it or not.

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
    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
        self.frames(2);
    }
    fn editor(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["chart_editor"].clone()
    }
}

#[test]
fn new_chart_from_the_editor_then_edit_apply_and_cancel() {
    let mut d = Driver::demo();
    let r = d.ok("ui.menu.invoke", json!({"id": "graphics.newChart"}));
    assert_eq!(r["dialog"], "chartEditor");
    d.frames(3);
    assert!(d.editor().is_object(), "the editor is open");

    d.click("chartEditor.kind.column");
    d.click("chartEditor.theme.light");
    d.click("chartEditor.palette.Ocean");
    d.click("chartEditor.tab.data");
    d.click("chartEditor.addRow");
    let e = d.editor();
    assert_eq!((e["chart"]["kind"].as_str(), e["chart"]["theme"].as_str()), (Some("column"), Some("light")));
    let rows = e["chart"]["categories"].as_array().map_or(0, Vec::len);
    assert_eq!(rows, 7, "the sample's 6 rows and the added one");

    d.click("chartEditor.ok");
    assert!(d.editor().is_null(), "Add to Timeline closes the editor");
    let c = d.exec("graphics.chart", json!({}));
    assert_eq!((c["chart"]["kind"].as_str(), c["chart"]["theme"].as_str()), (Some("column"), Some("light")));
    assert_eq!(c["chart"]["categories"].as_array().map_or(0, Vec::len), 7);
    let clip = c["clip"].as_u64().unwrap();

    // Edit Chart… on the clip: Cancel keeps it, Apply changes it in place
    d.ok("ui.menu.invoke", json!({"id": "graphics.setChart"}));
    d.frames(3);
    assert_eq!(d.editor()["clip"].as_u64(), Some(clip));
    d.click("chartEditor.kind.pie");
    d.click("chartEditor.cancel");
    assert_eq!(d.exec("graphics.chart", json!({"clip": clip}))["chart"]["kind"], "column");
    d.ok("ui.menu.invoke", json!({"id": "graphics.setChart"}));
    d.frames(3);
    d.click("chartEditor.kind.pie");
    d.click("chartEditor.ok");
    assert_eq!(d.exec("graphics.chart", json!({"clip": clip}))["chart"]["kind"], "pie");
    d.exec("edit.undo", json!({}));
    assert_eq!(d.exec("graphics.chart", json!({"clip": clip}))["chart"]["kind"], "column", "Apply is one undo step");
}

#[test]
fn a_pasted_table_replaces_the_data() {
    let mut d = Driver::demo();
    d.ok("ui.menu.invoke", json!({"id": "graphics.newChart"}));
    d.frames(3);
    d.click("chartEditor.tab.data");
    d.click("chartEditor.paste");
    d.ok("ui.type", json!({"text": "国,2020,2021\n日本,1,2\n韓国,3,4"}));
    d.frames(2);
    d.click("chartEditor.importTable");
    let e = d.editor();
    assert_eq!(e["chart"]["categories"], json!(["日本", "韓国"]), "{e}");
    assert_eq!(e["chart"]["steps"], json!(["2020", "2021"]));
    assert_eq!(e["chart"]["values"], json!([[1.0, 2.0], [3.0, 4.0]]));
}

/// A 40×30 magenta PNG in a fresh temp folder.
fn flag_png() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("fc-chart-ui-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("flag.png");
    let img = filmcraft_testkit::golden::Rgba8::new(40, 30, (0..40 * 30).flat_map(|_| [255u8, 0, 255, 255]).collect());
    filmcraft_testkit::golden::write_png(&path, &img).unwrap();
    path.to_string_lossy().into_owned()
}

impl Driver {
    fn label(&mut self, id: &str) -> String {
        let els = self.ok("ui.elements", json!({ "prefix": id }));
        els.as_array().and_then(|a| a.iter().find(|e| e["id"] == id)).map(|e| e["label"].as_str().unwrap_or("").to_string()).unwrap_or_default()
    }
}

#[test]
fn category_pictures_follow_their_rows_and_stay_on_the_clip() {
    let mut d = Driver::demo();
    let png = flag_png();
    let p = png.clone();
    d.harness.state_mut().hooks.pick_files = Some(Box::new(move |_| vec![p.clone()]));
    d.ok("ui.menu.invoke", json!({"id": "graphics.newChart"}));
    d.frames(3);
    d.click("chartEditor.tab.data");
    // Choose… on the second row imports the file and shows it in the row
    d.click("chartEditor.image.1");
    d.frames(2);
    let e = d.editor();
    let item = e["chart"]["images"][1]["item"].as_u64().unwrap_or_else(|| panic!("the picture is set: {e}"));
    assert_eq!(e["chart"]["images"][1]["path"], png.as_str());
    assert!(e["chart"]["images"][0].is_null());
    assert!(d.label("chartEditor.image.1").contains("flag"), "the cell names the picture: {}", d.label("chartEditor.image.1"));
    // a second Choose… of the same file reuses the imported item
    d.click("chartEditor.chooseImage.2");
    assert_eq!(d.editor()["chart"]["images"][2]["item"].as_u64(), Some(item));
    d.click("chartEditor.clearImage.2");
    assert!(d.editor()["chart"]["images"][2].is_null());
    // moving a row carries its picture
    let names = d.editor()["chart"]["categories"].clone();
    d.click("chartEditor.rowUp.1");
    let e = d.editor();
    assert_eq!(e["chart"]["categories"][0], names[1]);
    assert_eq!(e["chart"]["images"][0]["item"].as_u64(), Some(item));
    assert!(e["chart"]["images"][1].is_null());
    d.click("chartEditor.imageShape.rounded");
    d.click("chartEditor.tab.preview");
    d.frames(2);
    d.click("chartEditor.ok");
    assert!(d.editor().is_null());
    let c = d.exec("graphics.chart", json!({}));
    let clip = c["clip"].as_u64().unwrap();
    assert_eq!(c["chart"]["images"][0]["item"].as_u64(), Some(item), "the clip keeps the picture: {c}");
    assert_eq!(c["chart"]["imageShape"], "rounded");

    // a picture set by command shows in the reopened editor; deleting a row keeps the rest aligned
    let last = c["chart"]["categories"].as_array().and_then(|a| a.last()).and_then(Value::as_str).unwrap().to_string();
    let mut by_name = serde_json::Map::new();
    by_name.insert(last.clone(), json!(item));
    d.exec("graphics.setChart", json!({"clip": clip, "images": by_name}));
    d.ok("ui.menu.invoke", json!({"id": "graphics.setChart"}));
    d.frames(3);
    d.click("chartEditor.tab.data");
    let n = d.editor()["chart"]["categories"].as_array().map_or(0, Vec::len);
    assert!(d.label(&format!("chartEditor.image.{}", n - 1)).contains("flag"));
    d.click("chartEditor.deleteRow.0");
    let e = d.editor();
    assert_eq!(e["chart"]["categories"].as_array().map_or(0, Vec::len), n - 1);
    assert!(e["chart"]["images"][0].is_null());
    assert_eq!(e["chart"]["images"][n - 2]["item"].as_u64(), Some(item));
    d.click("chartEditor.ok");
    let c = d.exec("graphics.chart", json!({"clip": clip}));
    assert_eq!(c["chart"]["images"][n - 2]["item"].as_u64(), Some(item));
    assert_eq!(c["chart"]["categories"][n - 2], last.as_str());

    // a file that cannot be imported shows an error and changes nothing
    d.harness.state_mut().hooks.pick_files = Some(Box::new(|_| vec!["Z:/no/such/flag.png".into()]));
    d.ok("ui.menu.invoke", json!({"id": "graphics.setChart"}));
    d.frames(3);
    d.click("chartEditor.tab.data");
    let before = d.editor()["chart"].clone();
    d.click("chartEditor.image.0");
    let e = d.editor();
    assert_eq!(e["chart"], before);
    assert!(!e["error"].as_str().unwrap_or("").is_empty(), "{e}");
}
