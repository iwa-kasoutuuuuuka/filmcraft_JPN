//! Headless UI test of the UI language (Settings ▸ Appearance ▸ Language):
//! - in English nothing on screen is Japanese, in every workspace, mode and the dialogs opened here;
//! - in Japanese the UI is translated, while automation ids and labels stay English, so agents
//!   drive the app the same way in every language.
//!
//! One test: the language is process-wide, and tests of one binary share the process.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::i18n::{self, Language};
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
                assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    /// Open a screen: run its command (false when the command is disabled now) and let it draw.
    fn open(&mut self, command: &str) -> bool {
        let (req, reply) = ControlRequest::new("engine.execute", json!({"command": command, "params": {}}));
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                self.frames(3);
                return v["ok"] == json!(true);
            }
        }
        panic!("no reply to {command}");
    }

    fn close(&mut self) {
        self.ok("ui.key", json!({"key": "Escape"}));
        self.frames(2);
    }

    fn set_language(&mut self, l: Language) {
        self.exec("prefs.set", json!({"key": "appearance.language", "value": l.pref_name()}));
        self.frames(3);
    }

    /// Every piece of text painted in the last frame.
    fn painted(&self) -> Vec<String> {
        fn walk(s: &egui::Shape, out: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for c in &self.harness.output().shapes {
            walk(&c.shape, &mut out);
        }
        out
    }

    /// (id, label) of every automation element on screen.
    fn elements(&mut self) -> Vec<(String, String)> {
        let v = self.ok("ui.elements", json!({}));
        v.as_array().unwrap().iter().map(|e| (e["id"].as_str().unwrap_or_default().to_string(), e["label"].as_str().unwrap_or_default().to_string())).collect()
    }
}

/// Kana, kanji and full-width forms.
fn japanese(s: &str) -> bool {
    // (a language's own name in the language picker is the same in every language)
    s != Language::Japanese.native_name() && s.chars().any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x9FFF | 0xFF00..=0xFFEF))
}

/// The screens visited in each language: (description, command). Workspaces, every Settings page
/// and the in-app dialogs that open without a file picker. A command that is disabled in this
/// state (nothing selected) is skipped.
fn screens() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&str, String)> = ["editing", "assembly", "color", "effects", "audio", "captionsandgraphics", "allpanels"]
        .into_iter()
        .map(|w| ("workspace", format!("window.workspace.{w}")))
        .collect();
    for page in [
        "general",
        "appearance",
        "audio",
        "audioHardware",
        "autoSave",
        "color",
        "graphics",
        "labels",
        "media",
        "mediaAnalysis",
        "mediaCache",
        "memory",
        "playback",
        "plugins",
        "timeline",
        "trim",
    ] {
        v.push(("Settings page", format!("app.settings.{page}")));
    }
    for dialog in [
        "app.about",
        "app.keyboardShortcuts",
        "file.exportAaf",
        "file.exportOmf",
        "file.replaceFonts",
        "clip.audioGain",
        "sequence.deleteTracks",
        "audio.voiceover.settings",
        "trim.monitor",
    ] {
        v.push(("dialog", dialog.to_string()));
    }
    v
}

#[test]
fn english_shows_no_japanese_and_automation_labels_stay_english() {
    let mut d = Driver::demo();

    // ---- English: no Japanese anywhere
    d.set_language(Language::English);
    assert_eq!(i18n::language(), Language::English);
    for mode in ["edit", "import", "export"] {
        d.ok("ui.set", json!({"mode": mode}));
        d.frames(3);
        let jp: Vec<String> = d.painted().into_iter().filter(|t| japanese(t)).collect();
        assert!(jp.is_empty(), "Japanese text in English, {mode} mode: {jp:?}");
    }
    d.ok("ui.set", json!({"mode": "edit"}));
    let mut opened = 0;
    for (what, cmd) in screens() {
        if !d.open(&cmd) {
            continue;
        }
        opened += 1;
        let jp: Vec<String> = d.painted().into_iter().filter(|t| japanese(t)).collect();
        assert!(jp.is_empty(), "Japanese text in English after {cmd} ({what}): {jp:?}");
        assert!(d.painted().iter().any(|t| t == "File"), "the menu bar is English after {cmd}");
        d.close();
    }
    assert!(opened >= 25, "most screens open in the demo project: {opened}");
    let english_labels = d.elements();

    // ---- Japanese: translated, with the same automation ids and labels
    d.set_language(Language::Japanese);
    assert_eq!(i18n::language(), Language::Japanese);
    d.exec("window.workspace.editing", json!({}));
    d.frames(3);
    let painted = d.painted();
    for word in ["ファイル", "編集", "プロジェクト", "タイムライン"] {
        assert!(painted.iter().any(|t| t.contains(word)), "{word} is on screen in Japanese");
    }
    let mut jp: std::collections::BTreeSet<(String, String)> = Default::default();
    for mode in ["edit", "import", "export"] {
        d.ok("ui.set", json!({"mode": mode}));
        d.frames(3);
        jp.extend(d.elements().into_iter().filter(|(_, label)| japanese(label)));
    }
    d.ok("ui.set", json!({"mode": "edit"}));
    for (_, cmd) in screens() {
        if d.open(&cmd) {
            jp.extend(d.elements().into_iter().filter(|(_, label)| japanese(label)));
            d.close();
        }
    }
    assert!(jp.is_empty(), "Japanese automation labels: {jp:#?}");
    // the screen both runs ended on registers the same elements with the same labels
    d.exec("window.workspace.allpanels", json!({}));
    d.frames(3);
    let japanese_labels = d.elements();
    let differ: Vec<String> = english_labels
        .iter()
        .filter_map(|(id, en)| {
            japanese_labels.iter().find(|(i, _)| i == id).filter(|(_, l)| l != en).map(|(_, l)| format!("{id}: {en:?} in English, {l:?} in Japanese"))
        })
        .collect();
    assert!(differ.is_empty(), "automation labels that depend on the language: {differ:#?}");

    // unknown values fall back to the default language instead of failing
    assert_eq!(Language::from_pref("EN", Some("ja-JP")), Language::English);
    assert_eq!(Language::from_pref("ja", Some("en-US")), Language::Japanese);
    // Auto follows the system: Japanese there, English elsewhere, Japanese when it does not say
    for (system, want) in [
        (Some("ja-JP"), Language::Japanese),
        (Some("ja"), Language::Japanese),
        (Some("en-US"), Language::English),
        (Some("fr-FR"), Language::English),
        (Some("jam"), Language::English),
        (None, Language::Japanese),
    ] {
        assert_eq!(Language::from_pref("auto", system), want, "{system:?}");
        assert_eq!(Language::from_pref("xx", system), want, "unknown values mean Auto");
    }
}
