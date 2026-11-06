//! Generate Narration (Graphics and Titles ▸ Generate Narration…, `tts.synthesize` without
//! parameters): text-to-speech through the AivisSpeech or VOICEVOX app running on this computer.
//!
//! The dialog lists the lines (voice, text, speed), takes a pasted script (`話者: セリフ` per
//! line), previews a line (the WAV opens in the system's player), and **Generate** renders the
//! audio on a worker thread, then runs `tts.synthesize {prepared}` to import and place it with
//! captions as one undo step. Afterwards it shows the credits of every voice the sequence uses
//! (`tts.credits`) with Copy, Add to Credit Roll (`tts.addCreditRoll`) and Export Description
//! Text (`tts.exportCredits`), and each voice's terms.
//!
//! The open dialog lives in `UiState::narration` (serde), so agents can read and fill it with
//! `ui.get` / `ui.set`. Automation ids (`narration.*`): `engine.<aivisspeech|voicevox>`,
//! `refresh`, `getEngine`, `voice.<i>` (and `voice.<i>.<k>` in its list), `text.<i>`,
//! `speed.<i>`, `up.<i>`, `down.<i>`, `remove.<i>`, `preview.<i>`, `terms.<i>`, `add`, `script`,
//! `addScript`, `captions`, `creditRoll` (no roll yet) / `updateCreditRoll` (a roll exists), `pause`, `autoLevel`, `duck`, `generate`, `cancel`,
//! `copyCredits`, `addCreditRoll`, `exportCredits`, `openTerms`, `closeTerms`.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Color32, RichText};
use filmcraft_engine::narration::{self, Rendered};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;

type Elems = Vec<(String, egui::Rect, String)>;

/// An engine as `tts.engines` reported it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EngineStatus {
    pub id: String,
    pub name: String,
    pub url: String,
    pub running: bool,
    pub version: String,
    pub error: String,
    pub home_url: String,
}

/// A voice of the selected engine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoiceRow {
    /// `Speaker/Style` (what lines refer to).
    pub voice: String,
    pub speaker: String,
    pub style: String,
    pub id: i64,
    pub uuid: String,
}

/// One line of the narration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DraftLine {
    pub text: String,
    /// `Speaker/Style`, a speaker name or a style id ("" = the default voice).
    pub voice: String,
    pub speed: f64,
}

impl Default for DraftLine {
    fn default() -> Self {
        Self { text: String::new(), voice: String::new(), speed: 1.0 }
    }
}

/// A voice's terms, as `tts.voiceTerms` returned them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TermsView {
    pub speaker: String,
    pub credit: String,
    pub policy: String,
    pub license: String,
    pub url: String,
    pub note: String,
}

/// The Generate Narration dialog.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NarrationDraft {
    pub engine: String,
    pub engines: Vec<EngineStatus>,
    pub voices: Vec<VoiceRow>,
    pub lines: Vec<DraftLine>,
    /// The script paste box.
    pub script: String,
    pub captions: bool,
    /// Add / update the credit roll at the end after generating (`creditRoll`).
    pub credit_roll: bool,
    /// The sequence has a credit roll and the user unticked "Update the credit roll": Generate
    /// sends `creditRoll: false` and leaves it alone.
    pub keep_credit_roll: bool,
    /// The sequence has a credit roll (refreshed every frame the dialog shows).
    pub roll_exists: bool,
    /// Silence after each line (seconds).
    pub pause: f64,
    /// Dialogue type + loudness match for the narration clips (`autoLevel`).
    pub auto_level: bool,
    /// Duck the other audio under the narration (`duck`).
    pub duck: bool,
    /// Rendering in the background.
    pub busy: bool,
    pub token: u64,
    /// The `tts.synthesize` parameters of the generation in progress.
    pub pending: Value,
    pub status: String,
    pub error: String,
    /// Credits of the last generation.
    pub credits: Vec<String>,
    pub credit_notes: Vec<String>,
    pub terms: Option<TermsView>,
}

/// The background render's result slot (egui temp data: it is not serde).
type Slot = Arc<Mutex<Option<Result<Rendered, String>>>>;

fn slot_id() -> egui::Id {
    egui::Id::new("narration-render-slot")
}

fn push(elems: &mut Elems, id: impl Into<String>, r: &egui::Response, label: impl Into<String>) {
    elems.push((format!("narration.{}", id.into()), r.rect, label.into()));
}

/// An engine error in the interface language ("X is not running at URL — start the X app").
pub fn error_text(e: &str) -> String {
    if !crate::i18n::is_japanese() {
        return e.to_string();
    }
    if let Some((engine, rest)) = e.split_once(" is not running at ") {
        let url = rest.split(" — ").next().unwrap_or(rest);
        let engine = engine.rsplit(": ").next().unwrap_or(engine);
        return format!("{engine} が {url} で起動していません。{engine} アプリを起動してください。");
    }
    if e.contains("narration is not available in this build") {
        return "このビルドではナレーションを使えません(Web 版、または tts 機能なしのビルド)。".into();
    }
    e.to_string()
}

/// `tts.synthesize` without parameters opens the dialog.
pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let bare = params.as_object().is_none_or(|m| m.is_empty());
    // the menu's Add Narration Credit Roll / Export Narration Credits: say what happened
    if bare && matches!(id, "tts.addCreditRoll" | "tts.exportCredits") {
        let r = app.session.execute(id, json!({})).map_err(|e| error_text(&e.to_string()));
        app.ui.status = match &r {
            Ok(v) if id == "tts.exportCredits" => {
                crate::tf!("Wrote the description text to {}.", "説明欄用テキストを {} に書き出しました。", v["path"].as_str().unwrap_or_default())
            }
            Ok(v) => roll_message(v),
            Err(e) => e.clone(),
        };
        return Some(r);
    }
    if id != "tts.synthesize" || !bare {
        return None;
    }
    if app.session.active_sequence().is_none() {
        return Some(Err(crate::i18n::t("Open a sequence first", "先にシーケンスを開いてください").into()));
    }
    let prefs = app.session.prefs.narration.clone();
    let mut d = NarrationDraft {
        engine: prefs.default_engine().id().to_string(),
        captions: prefs.captions,
        credit_roll: prefs.credit_roll,
        pause: prefs.pause_seconds,
        auto_level: prefs.auto_level,
        duck: prefs.duck,
        lines: vec![DraftLine { voice: prefs.default_voice.clone(), ..Default::default() }],
        ..Default::default()
    };
    refresh(app, &mut d, false);
    app.ui.narration = Some(d);
    Some(Ok(json!({"dialog": "narration"})))
}

/// A `tts.engines` row (`version` / `error` are null when they do not apply).
fn engine_status(e: &Value) -> EngineStatus {
    let s = |k: &str| e[k].as_str().unwrap_or_default().to_string();
    EngineStatus {
        id: s("id"),
        name: s("name"),
        url: s("url"),
        running: e["running"] == true,
        version: s("version"),
        error: s("error"),
        home_url: s("homeUrl"),
    }
}

/// Ask the engines whether they run, and the selected one for its voices.
fn refresh(app: &mut FilmcraftApp, d: &mut NarrationDraft, force: bool) {
    d.error.clear();
    if let Ok(v) = app.session.execute("tts.engines", json!({})) {
        d.engines = v["engines"].as_array().map(|a| a.iter().map(engine_status).collect()).unwrap_or_default();
    }
    d.voices.clear();
    match app.session.execute("tts.voices", json!({"engine": d.engine, "refresh": force})) {
        Ok(v) => {
            d.voices = v["voices"].as_array().map(|a| a.iter().filter_map(|e| serde_json::from_value(e.clone()).ok()).collect()).unwrap_or_default();
            // lines without a voice get the first one, so the list shows what will speak
            if let Some(first) = d.voices.first().map(|v| v.voice.clone()) {
                for l in d.lines.iter_mut().filter(|l| l.voice.is_empty()) {
                    l.voice = first.clone();
                }
            }
        }
        Err(e) => d.error = error_text(&e.to_string()),
    }
}

fn params_of(d: &NarrationDraft) -> Value {
    let lines: Vec<Value> = d
        .lines
        .iter()
        .filter(|l| !l.text.trim().is_empty())
        .map(|l| {
            let mut v = json!({"text": l.text, "speed": l.speed});
            if !l.voice.trim().is_empty() {
                v["voice"] = json!(l.voice);
            }
            v
        })
        .collect();
    let mut p = json!({"engine": d.engine, "lines": lines, "captions": d.captions, "pause": d.pause, "autoLevel": d.auto_level, "duck": d.duck});
    if d.roll_exists {
        // a roll exists: it follows the new narration unless "Update the credit roll" is off
        if d.keep_credit_roll {
            p["creditRoll"] = json!(false);
        }
    } else if d.credit_roll {
        p["creditRoll"] = json!(true);
    }
    p
}

/// What `tts.addCreditRoll` (or the roll part of `tts.synthesize`) did, in the interface language.
fn roll_message(v: &Value) -> String {
    let t = crate::i18n::t;
    match v["skipped"].as_str() {
        Some("locked") => {
            t("The credit roll is on a locked track, so it was not changed.", "エンドロールのトラックがロックされているため、変更しませんでした。")
        }
        Some("edited") => t(
            "The credit roll's text was edited, so it was not changed (Add to Credit Roll rewrites it).",
            "エンドロールの文字が編集されているため、変更しませんでした(「エンドロールに追加」で作り直せます)。",
        ),
        _ if v["updated"] == true && v["changed"] == false => t("The credit roll is up to date.", "エンドロールは最新です。"),
        _ if v["updated"] == true => t("Updated the credit roll.", "エンドロールのクレジットを更新しました。"),
        _ => t("Added a credit roll at the end.", "エンドロールにクレジットを追加しました。"),
    }
    .to_string()
}

/// The credits of every voice the sequence uses (falls back to `fallback`, the generation's own).
fn load_credits(app: &mut FilmcraftApp, d: &mut NarrationDraft, fallback: &[Value]) {
    let creds = app.session.execute("tts.credits", json!({})).ok().and_then(|v| v["credits"].as_array().cloned()).filter(|a| !a.is_empty());
    let creds = creds.unwrap_or_else(|| fallback.to_vec());
    d.credits = creds.iter().filter_map(|c| c["credit"].as_str().map(str::to_string)).collect();
    d.credit_notes = creds.iter().filter_map(|c| c["note"].as_str().map(str::to_string)).collect();
    d.credit_notes.dedup();
}

/// Add Narration Credit Roll / Export Narration Credits from the dialog.
fn credit_action(app: &mut FilmcraftApp, d: &mut NarrationDraft, export: bool) {
    d.error.clear();
    let id = if export { "tts.exportCredits" } else { "tts.addCreditRoll" };
    match app.session.execute(id, json!({})) {
        Ok(v) => {
            d.status = if export {
                crate::tf!("Wrote the description text to {}.", "説明欄用テキストを {} に書き出しました。", v["path"].as_str().unwrap_or_default())
            } else {
                roll_message(&v)
            };
            app.ui.status = d.status.clone();
        }
        Err(e) => d.error = error_text(&e.to_string()),
    }
}

/// Add the lines of a pasted script, mapping speaker names to voices of the engine.
fn add_script(d: &mut NarrationDraft) {
    let parsed = narration::parse_script(&d.script);
    if parsed.is_empty() {
        return;
    }
    // an empty first line is replaced
    if d.lines.len() == 1 && d.lines.first().is_some_and(|l| l.text.trim().is_empty()) {
        d.lines.clear();
    }
    let default = d.voices.first().map(|v| v.voice.clone()).unwrap_or_default();
    for (who, text) in parsed.into_iter().take(narration::MAX_LINES) {
        let voice = match who {
            Some(n) => d.voices.iter().find(|v| v.voice == n || v.speaker == n).map(|v| v.voice.clone()).unwrap_or(n),
            None => d.lines.last().map(|l| l.voice.clone()).unwrap_or_else(|| default.clone()),
        };
        d.lines.push(DraftLine { text, voice, speed: 1.0 });
    }
    d.script.clear();
}

/// Start rendering in the background (wasm: right away; it reports "not available").
fn generate(app: &mut FilmcraftApp, ctx: &egui::Context, d: &mut NarrationDraft) {
    d.error.clear();
    d.credits.clear();
    d.credit_notes.clear();
    let p = params_of(d);
    if p["lines"].as_array().is_none_or(Vec::is_empty) {
        d.error = crate::i18n::t("Type the text of at least one line.", "少なくとも 1 行、セリフを入力してください。").into();
        return;
    }
    let job = match narration::prepare(&app.session, &p, "tts.synthesize") {
        Ok(j) => j,
        Err(e) => {
            d.error = error_text(&e.to_string());
            return;
        }
    };
    let slot: Slot = Arc::new(Mutex::new(None));
    d.token = narration::next_token();
    d.pending = p;
    d.busy = true;
    d.status = crate::i18n::t("Generating narration…", "ナレーションを生成しています…").into();
    ctx.data_mut(|m| m.insert_temp(slot_id(), slot.clone()));
    let work = move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| narration::render(&job).map_err(|e| e.to_string())))
            .unwrap_or_else(|_| Err("internal error while synthesizing (see the crash log)".into()));
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(work);
    #[cfg(target_arch = "wasm32")]
    work();
}

/// The background render finished: place it.
fn poll(app: &mut FilmcraftApp, ctx: &egui::Context, d: &mut NarrationDraft) {
    if !d.busy {
        return;
    }
    let Some(slot) = ctx.data(|m| m.get_temp::<Slot>(slot_id())) else {
        d.busy = false;
        return;
    };
    let Some(result) = slot.lock().unwrap_or_else(PoisonError::into_inner).take() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        return;
    };
    d.busy = false;
    d.status.clear();
    let rendered = match result {
        Ok(r) => r,
        Err(e) => {
            d.error = error_text(&e);
            return;
        }
    };
    app.session.narration.prepared = Some(narration::Prepared { token: d.token, rendered });
    let mut p = d.pending.clone();
    p["prepared"] = json!(d.token);
    match app.session.execute("tts.synthesize", p) {
        Ok(v) => {
            let creds = v["credits"].as_array().cloned().unwrap_or_default();
            load_credits(app, d, &creds);
            let n = v["clips"].as_array().map_or(0, Vec::len);
            let track = v["track"].as_str().unwrap_or_default().to_string();
            d.status = crate::tf!("Placed {} narration clips on {}.", "{} 個のナレーションクリップを {} に配置しました。", n, track);
            let ducked = v["mix"]["ducked"].as_array().map_or(0, Vec::len);
            if ducked > 0 {
                d.status.push(' ');
                d.status.push_str(&crate::tf!("Ducked {} clips under it.", "その下の {} 個のクリップを下げました。", ducked));
            }
            if v["creditRoll"].is_object() {
                d.status.push(' ');
                d.status.push_str(&roll_message(&v["creditRoll"]));
            }
            app.ui.status = d.status.clone();
        }
        Err(e) => d.error = error_text(&e.to_string()),
    }
}

fn voice_terms(app: &mut FilmcraftApp, d: &mut NarrationDraft, voice: &str) {
    match app.session.execute("tts.voiceTerms", json!({"engine": d.engine, "voice": voice})) {
        Ok(v) => {
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            d.terms =
                Some(TermsView { speaker: s("speaker"), credit: s("credit"), policy: s("policy"), license: s("license"), url: s("termsUrl"), note: s("note") });
        }
        Err(e) => d.error = error_text(&e.to_string()),
    }
}

fn preview(app: &mut FilmcraftApp, ctx: &egui::Context, d: &mut NarrationDraft, i: usize) {
    let Some(l) = d.lines.get(i).cloned() else { return };
    if l.text.trim().is_empty() {
        return;
    }
    let mut p = json!({"engine": d.engine, "text": l.text, "speed": l.speed});
    if !l.voice.is_empty() {
        p["voice"] = json!(l.voice);
    }
    match app.session.execute("tts.preview", p) {
        Ok(v) => {
            let path = v["path"].as_str().unwrap_or_default().replace('\\', "/");
            d.status = crate::tf!("Preview: {}", "試聴: {}", path);
            if !path.is_empty() && !cfg!(target_arch = "wasm32") {
                let url = if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") };
                crate::links::open(ctx, &url);
            }
        }
        Err(e) => d.error = error_text(&e.to_string()),
    }
}

/// Every frame: the dialog when it is open.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.narration.take() else { return };
    poll(app, ctx, &mut d);
    d.roll_exists = filmcraft_engine::narration_credits::credit_roll_clip(&app.session).is_some();
    let mut elems: Elems = Vec::new();
    let mut close = false;
    let mut do_refresh = false;
    let mut do_generate = false;
    let mut do_preview: Option<usize> = None;
    let mut do_terms: Option<String> = None;
    let mut do_credit: Option<bool> = None;
    let mut open_url: Option<String> = None;
    let accent = app.tokens.accent;
    let ja = crate::i18n::is_japanese();
    egui::Window::new(crate::i18n::t("Generate Narration", "ナレーションを生成"))
        .collapsible(false)
        .resizable(true)
        .default_width(820.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // engines
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Engine:", "エンジン:"));
                let engines = d.engines.clone();
                for e in &engines {
                    let mark = if e.running { "●" } else { "○" };
                    let state = if e.running { crate::i18n::t("running", "起動中") } else { crate::i18n::t("not running", "未起動") };
                    let r = ui.add(egui::Button::selectable(d.engine == e.id, format!("{mark} {} ({state})", e.name)));
                    push(&mut elems, format!("engine.{}", e.id), &r, format!("{} ({})", e.name, if e.running { "running" } else { "not running" }));
                    if r.clicked() && d.engine != e.id {
                        d.engine = e.id.clone();
                        do_refresh = true;
                    }
                }
                let r = ui.button(crate::i18n::t("Refresh", "再確認"));
                push(&mut elems, "refresh", &r, "Refresh");
                if r.clicked() {
                    do_refresh = true;
                }
            });
            if let Some(e) = d.engines.iter().find(|e| e.id == d.engine && !e.running) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::tf!(
                            "Start the {} app (free), then press Refresh.",
                            "{} アプリ(無料)を起動してから「再確認」を押してください。",
                            e.name
                        ))
                        .weak(),
                    );
                    let r = ui.link(crate::tf!("Get {}", "{} を入手", e.name));
                    push(&mut elems, "getEngine", &r, format!("Get {}", e.name));
                    if r.clicked() {
                        open_url = Some(e.home_url.clone());
                    }
                });
            }
            ui.separator();
            // lines
            let voices = d.voices.clone();
            let mut action: Option<(usize, i32)> = None;
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                egui::Grid::new("narration-lines").num_columns(5).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("#");
                    ui.label(crate::i18n::t("Voice", "声"));
                    ui.label(crate::i18n::t("Text", "セリフ"));
                    ui.label(crate::i18n::t("Speed", "話速"));
                    ui.label("");
                    ui.end_row();
                    let n = d.lines.len();
                    for (i, l) in d.lines.iter_mut().enumerate() {
                        ui.label(format!("{}", i + 1));
                        let shown = if l.voice.is_empty() { crate::i18n::t("(default)", "(既定)").to_string() } else { l.voice.clone() };
                        let cb = egui::ComboBox::from_id_salt(("narration-voice", i)).selected_text(&shown).width(200.0).show_ui(ui, |ui| {
                            for (k, v) in voices.iter().enumerate() {
                                let r = ui.selectable_label(l.voice == v.voice, &v.voice);
                                push(&mut elems, format!("voice.{i}.{k}"), &r, format!("Voice {}", v.id));
                                if r.clicked() {
                                    l.voice = v.voice.clone();
                                }
                            }
                        });
                        push(&mut elems, format!("voice.{i}"), &cb.response, "Voice");
                        let r =
                            ui.add_sized([360.0, 22.0], egui::TextEdit::singleline(&mut l.text).hint_text(crate::i18n::t("Text to speak", "読み上げるセリフ")));
                        push(&mut elems, format!("text.{i}"), &r, "Text");
                        let r = ui.add(egui::DragValue::new(&mut l.speed).speed(0.01).range(0.5..=2.0).fixed_decimals(2));
                        push(&mut elems, format!("speed.{i}"), &r, "Speed");
                        ui.horizontal(|ui| {
                            let r = ui.add_enabled(i > 0, egui::Button::new("↑").small());
                            push(&mut elems, format!("up.{i}"), &r, "Move Up");
                            if r.clicked() {
                                action = Some((i, -1));
                            }
                            let r = ui.add_enabled(i + 1 < n, egui::Button::new("↓").small());
                            push(&mut elems, format!("down.{i}"), &r, "Move Down");
                            if r.clicked() {
                                action = Some((i, 1));
                            }
                            let r = ui.add(egui::Button::new("✕").small());
                            push(&mut elems, format!("remove.{i}"), &r, "Remove");
                            if r.clicked() {
                                action = Some((i, 0));
                            }
                            let r = ui.add(egui::Button::new(crate::i18n::t("Preview", "試聴")).small());
                            push(&mut elems, format!("preview.{i}"), &r, "Preview");
                            if r.clicked() {
                                do_preview = Some(i);
                            }
                            let r = ui.add(egui::Button::new(crate::i18n::t("Terms", "規約")).small());
                            push(&mut elems, format!("terms.{i}"), &r, "Terms");
                            if r.clicked() {
                                do_terms = Some(l.voice.clone());
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            match action {
                Some((i, 0)) if i < d.lines.len() => {
                    d.lines.remove(i);
                }
                Some((i, -1)) if i > 0 && i < d.lines.len() => d.lines.swap(i, i - 1),
                Some((i, 1)) if i + 1 < d.lines.len() => d.lines.swap(i, i + 1),
                _ => {}
            }
            let r = ui.button(crate::i18n::t("+ Add Line", "+ 行を追加"));
            push(&mut elems, "add", &r, "Add Line");
            if r.clicked() && d.lines.len() < narration::MAX_LINES {
                let voice = d.lines.last().map(|l| l.voice.clone()).unwrap_or_default();
                d.lines.push(DraftLine { voice, ..Default::default() });
            }
            // script
            egui::CollapsingHeader::new(crate::i18n::t("Paste a script", "台本を貼り付け"))
                .id_salt("narration-script")
                .open(if d.script.is_empty() { None } else { Some(true) })
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(crate::i18n::t(
                            "One line per line of dialogue: \"Speaker: text\". Speakers are matched to voice names.",
                            "1 行に 1 つのセリフ:「話者: セリフ」。話者は声の名前に対応づけます。",
                        ))
                        .weak()
                        .small(),
                    );
                    let r = ui.add(egui::TextEdit::multiline(&mut d.script).desired_rows(4).desired_width(f32::INFINITY));
                    push(&mut elems, "script", &r, "Script");
                    let r = ui.button(crate::i18n::t("Add Lines from Script", "台本から行を追加"));
                    push(&mut elems, "addScript", &r, "Add Lines from Script");
                    if r.clicked() {
                        add_script(&mut d);
                    }
                });
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut d.captions, crate::i18n::t("Make captions", "キャプションを作成"));
                push(&mut elems, "captions", &r, "Make captions");
                if d.roll_exists {
                    // the sequence has a roll: Generate updates it unless this is unticked
                    let mut update = !d.keep_credit_roll;
                    let r = ui.checkbox(&mut update, crate::i18n::t("Update the credit roll", "エンドロールを更新")).on_hover_text(crate::i18n::t(
                        "Add the new voices to the credit roll. Text you edited, a font you chose and where you placed it are kept.",
                        "新しい声をエンドロールに加えます。編集した文字、選んだフォント、置いた位置はそのままです。",
                    ));
                    d.keep_credit_roll = !update;
                    push(&mut elems, "updateCreditRoll", &r, "Update the credit roll");
                } else {
                    let r = ui.checkbox(&mut d.credit_roll, crate::i18n::t("Credit roll at the end", "最後にエンドロール"));
                    push(&mut elems, "creditRoll", &r, "Credit roll at the end");
                }
                ui.label(crate::i18n::t("Pause after each line:", "各行のあとの間:"));
                let r = ui.add(egui::DragValue::new(&mut d.pause).speed(0.05).range(0.0..=30.0).suffix(crate::i18n::t(" s", " 秒")));
                push(&mut elems, "pause", &r, "Pause");
            });
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut d.auto_level, crate::i18n::t("Auto-level (Dialogue, loudness match)", "音量を自動調整(会話、ラウドネスを一致)"));
                push(&mut elems, "autoLevel", &r, "Auto-level");
                let r = ui.checkbox(&mut d.duck, crate::i18n::t("Duck other audio under the narration", "ナレーション中はほかの音声を下げる"));
                push(&mut elems, "duck", &r, "Duck other audio");
            });
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(235, 90, 80), &d.error);
            }
            if !d.status.is_empty() {
                ui.horizontal(|ui| {
                    if d.busy {
                        ui.spinner();
                    }
                    ui.label(&d.status);
                });
            }
            // credits
            if !d.credits.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(crate::i18n::t(
                        "Credits — show these in the video or its description:",
                        "クレジット — 動画内または概要欄に表記してください:",
                    ))
                    .strong(),
                );
                let text = d.credits.join("\n");
                let mut shown = text.clone();
                ui.add(egui::TextEdit::multiline(&mut shown).desired_rows(d.credits.len().clamp(1, 6)).desired_width(f32::INFINITY).interactive(false));
                for n in d.credit_notes.iter().take(8) {
                    ui.label(RichText::new(n).weak().small());
                }
                ui.horizontal(|ui| {
                    let r = ui.button(crate::i18n::t("Copy Credits", "クレジットをコピー"));
                    push(&mut elems, "copyCredits", &r, "Copy Credits");
                    if r.clicked() {
                        ctx.copy_text(text);
                        app.ui.status = crate::i18n::t("Credits copied.", "クレジットをコピーしました。").into();
                    }
                    let r = ui.add_enabled(!d.busy, egui::Button::new(crate::i18n::t("Add to Credit Roll", "エンドロールに追加")));
                    push(&mut elems, "addCreditRoll", &r, "Add to Credit Roll");
                    if r.clicked() {
                        do_credit = Some(false);
                    }
                    let r = ui.button(crate::i18n::t("Export Description Text", "説明欄用テキストを書き出す"));
                    push(&mut elems, "exportCredits", &r, "Export Description Text");
                    if r.clicked() {
                        do_credit = Some(true);
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let r = ui.button(crate::i18n::t("Close", "閉じる"));
                push(&mut elems, "cancel", &r, "Close");
                if r.clicked() {
                    close = true;
                }
                let r = ui.add_enabled(!d.busy, egui::Button::new(RichText::new(crate::i18n::t("Generate", "生成")).color(Color32::WHITE)).fill(accent));
                push(&mut elems, "generate", &r, "Generate");
                if r.clicked() {
                    do_generate = true;
                }
            });
            ui.label(
                RichText::new(if ja {
                    "音声は AivisSpeech / VOICEVOX アプリが生成します。声ごとに利用規約があります(「規約」ボタン)。"
                } else {
                    "The AivisSpeech / VOICEVOX app makes the audio. Every voice has its own terms (Terms button)."
                })
                .weak()
                .small(),
            );
        });
    // terms window
    if let Some(t) = d.terms.clone() {
        let mut open = true;
        egui::Window::new(crate::i18n::t("Voice Terms", "声の利用規約")).collapsible(false).resizable(true).default_width(560.0).open(&mut open).show(
            ctx,
            |ui| {
                ui.label(RichText::new(format!("{} — {}", t.speaker, t.credit)).strong());
                if !t.license.is_empty() {
                    ui.label(crate::tf!("Model licence: {}", "モデルのライセンス: {}", t.license));
                }
                if !t.note.is_empty() {
                    ui.label(RichText::new(&t.note).weak().small());
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    let mut policy = if t.policy.is_empty() {
                        crate::i18n::t("(The engine sent no terms text.)", "(エンジンから規約の文章が届きませんでした)").to_string()
                    } else {
                        t.policy.clone()
                    };
                    ui.add(egui::TextEdit::multiline(&mut policy).desired_width(f32::INFINITY).interactive(false));
                });
                ui.horizontal(|ui| {
                    if !t.url.is_empty() {
                        let r = ui.link(crate::i18n::t("Open the terms page", "規約のページを開く"));
                        push(&mut elems, "openTerms", &r, "Open the terms page");
                        if r.clicked() {
                            open_url = Some(t.url.clone());
                        }
                    }
                    let r = ui.button(crate::i18n::t("Close", "閉じる"));
                    push(&mut elems, "closeTerms", &r, "Close Terms");
                    if r.clicked() {
                        d.terms = None;
                    }
                });
            },
        );
        if !open {
            d.terms = None;
        }
    }
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if let Some(u) = open_url {
        crate::links::open(ctx, &u);
    }
    if do_refresh {
        refresh(app, &mut d, true);
    }
    if let Some(i) = do_preview {
        preview(app, ctx, &mut d, i);
    }
    if let Some(v) = do_terms {
        voice_terms(app, &mut d, &v);
    }
    if do_generate {
        generate(app, ctx, &mut d);
    }
    if let Some(export) = do_credit {
        credit_action(app, &mut d, export);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && d.terms.is_none() && !d.busy {
        close = true;
    }
    if d.terms.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        d.terms = None;
    }
    if !close {
        app.ui.narration = Some(d);
    }
}
