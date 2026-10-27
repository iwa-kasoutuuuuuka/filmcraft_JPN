//! Export mode (header "Export"), laid out like Premiere 26: Destinations and the export queue on
//! the left; the Media File settings in the middle (File Name, Location, Preset, Format, then the
//! Video, Audio, Multiplexer, Captions, Effects and Metadata sections); the preview, Range,
//! Scaling, the Summary with the estimated file size, and Send to Queue / Export on the right.
//! Also the Preset Manager (Preset ▸ More presets…) and the header's Quick Export popup.
//!
//! Every setting lives in [`ExportUi`] (serde, `ui.inspect` / `ui.set {"export": {…}}`) and every
//! action runs an engine command (`file.exportMedia`, `export.queue.*`, `export.presets.*`,
//! `export.quick`).
//!
//! Automation ids: `export.fileName`, `export.location`, `export.preset`, `export.preset.more`,
//! `export.format`, `export.section.<video|audio|multiplexer|captions|effects|metadata>`,
//! `export.video.*`, `export.audio.*`, `export.effects.*`, `export.metadata.*`, `export.range`,
//! `export.scaling`, `export.summary`, `export.estimate`, `export.sendToQueue`, `export.button`,
//! `export.queue.start|stop|clear`, `export.queue.item.<id>` and `….<id>.<up|down|cancel|retry|remove>`;
//! Preset Manager: `presetManager.search`, `presetManager.favoritesOnly`,
//! `presetManager.item.<name>`, `presetManager.favorite.<name>`, `presetManager.saveName`,
//! `presetManager.save`, `presetManager.delete`, `presetManager.import`, `presetManager.export`,
//! `presetManager.ok`, `presetManager.cancel`; Quick Export: `quickExport.path`,
//! `quickExport.preset.<name>`, `quickExport.go`, `quickExport.close`.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use filmcraft_engine::export::presets::{DEFAULT_PRESET, preset_key};
use filmcraft_engine::export::{
    AudioCodec, BitrateMode, ExportSettings, FieldOrder, Format, H264Profile, Multiplexer, MxfVideoCodec, Placement, Scaling, TextOverlay, builtin_presets,
    format_bytes,
};
use filmcraft_engine::time::{FrameRate, Tick};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::frames::{FrameKey, Target};
use crate::theme::Tokens;

/// Export mode state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExportUi {
    /// The preset the settings came from ("Custom" once edited).
    pub preset: String,
    pub settings: ExportSettings,
    /// File name without extension, and the folder.
    pub file_name: String,
    pub location: String,
    /// `entire` | `inOut` | `workArea` | `custom`.
    pub range: String,
    pub custom_start: f64,
    pub custom_end: f64,
    /// Expanded settings sections.
    pub open_sections: Vec<String>,
    /// The sequence the file name was taken from.
    pub for_sequence: Option<u64>,
    /// Preset Manager (None = closed).
    pub manager: Option<PresetManager>,
    /// Quick Export popup.
    pub quick_open: bool,
    pub quick_path: String,
    pub quick_preset: String,
}

impl Default for ExportUi {
    fn default() -> Self {
        let preset = filmcraft_engine::export::presets::find_builtin(DEFAULT_PRESET).map(|p| p.settings).unwrap_or_default();
        ExportUi {
            preset: DEFAULT_PRESET.into(),
            settings: preset,
            file_name: String::new(),
            location: String::new(),
            range: "entire".into(),
            custom_start: 0.0,
            custom_end: 10.0,
            open_sections: vec!["video".into(), "audio".into()],
            for_sequence: None,
            manager: None,
            quick_open: false,
            quick_path: String::new(),
            quick_preset: String::new(),
        }
    }
}

/// The Preset Manager dialog.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PresetManager {
    pub query: String,
    pub favorites_only: bool,
    pub selected: String,
    pub save_name: String,
}

pub const CUSTOM: &str = "Custom";

/// Collected automation ids (registered after drawing, so widgets can borrow the app freely).
#[derive(Default)]
struct Reg(Vec<(String, Rect, String)>);

impl Reg {
    fn add(&mut self, id: impl Into<String>, r: Rect, label: impl Into<String>) {
        self.0.push((id.into(), r, label.into()));
    }
    fn flush(self, app: &mut FilmcraftApp) {
        for (id, r, l) in self.0 {
            app.auto.add(&id, r, &l);
        }
    }
}

/// A built-in export preset or category name in the UI language (names the user gave are kept).
fn preset_display(name: &str) -> std::borrow::Cow<'_, str> {
    if crate::i18n::is_japanese() { preset_in_japanese(name) } else { name.into() }
}

/// [`preset_display`] whatever the UI language. Product and platform names stay as they are.
fn preset_in_japanese(name: &str) -> std::borrow::Cow<'_, str> {
    let fixed = match name {
        // categories
        "Match Source" => "ソースに合わせる",
        "Web & Social" => "Web と SNS",
        "Broadcast" => "放送",
        "Image Sequence" => "連番画像",
        "Animated GIF" => "アニメーション GIF",
        "Audio Only" => "オーディオのみ",
        "User" => "ユーザー",
        // presets
        "Match Source – Adaptive High Bitrate" => "ソースに合わせる – アダプティブ高ビットレート",
        "Match Source – Adaptive Medium Bitrate" => "ソースに合わせる – アダプティブ中ビットレート",
        "Match Source – Adaptive Low Bitrate" => "ソースに合わせる – アダプティブ低ビットレート",
        "Match Source – AV1" => "ソースに合わせる – AV1",
        "AV1 1080p Full HD" => "AV1 1080p フル HD",
        "High Quality 1080p HD" => "高品質 1080p HD",
        "High Quality 2160p 4K" => "高品質 2160p 4K",
        "Social Vertical 1080×1920" => "SNS 縦型 1080×1920",
        "PNG Sequence" => "PNG 連番",
        "TIFF Sequence" => "TIFF 連番",
        "BMP Sequence" => "BMP 連番",
        "Animated GIF 640×360" => "アニメーション GIF 640×360",
        "Waveform Audio 48 kHz 16-bit" => "WAV オーディオ 48 kHz 16 ビット",
        "Waveform Audio 48 kHz 24-bit" => "WAV オーディオ 48 kHz 24 ビット",
        "AIFF 48 kHz 16-bit" => "AIFF 48 kHz 16 ビット",
        _ => return name.into(),
    };
    fixed.into()
}

/// A Summary line (the export crate words it in English, as agents read it) in the UI language.
fn summary_text(s: &str) -> std::borrow::Cow<'_, str> {
    if crate::i18n::is_japanese() { summary_in_japanese(s).into() } else { s.into() }
}

/// [`summary_text`] whatever the UI language: the summary's fixed words, longest first.
fn summary_in_japanese(s: &str) -> String {
    let mut out = s.to_string();
    // "keyframe every 48 frames"
    if let Some(i) = out.find("keyframe every ")
        && let Some(n) = out.get(i + 15..).and_then(|r| r.split(' ').next()).map(str::to_string)
    {
        let en = format!("keyframe every {n} frames");
        out = out.replace(&en, &format!("キーフレーム間隔 {n} フレーム"));
    }
    // "Uncompressed 16-bit PCM"
    for bits in [16, 24] {
        out = out.replace(&format!("Uncompressed {bits}-bit PCM"), &format!("非圧縮 {bits} ビット PCM"));
    }
    for (en, ja) in [
        ("Image sequence", "連番画像"),
        ("Upper First", "上位フィールドから"),
        ("Lower First", "下位フィールドから"),
        ("Progressive", "プログレッシブ"),
        ("auto level", "レベル自動"),
        ("average ", "平均 "),
        ("VBR, 1 pass", "VBR 1 パス"),
        ("VBR, 2 pass", "VBR 2 パス"),
        ("Target ", "ターゲット "),
        ("Max ", "最大 "),
        ("quality ", "画質 "),
        ("No video", "ビデオなし"),
        ("No audio", "オーディオなし"),
        ("Stereo", "ステレオ"),
        ("Mono", "モノラル"),
    ] {
        out = out.replace(en, ja);
    }
    out.replace(", ", "、")
}

fn settings_json(s: &ExportSettings) -> Value {
    serde_json::to_value(s).unwrap_or_default()
}

/// Apply a preset by name (from the engine's library).
pub fn apply_preset(app: &mut FilmcraftApp, name: &str) -> bool {
    match app.session.export_presets.find(name) {
        Some(p) => {
            app.ui.export.settings = p.settings;
            app.ui.export.preset = p.name;
            true
        }
        None => {
            app.ui.status = crate::tf!("No export preset named `{name}`", "`{name}` という書き出しプリセットはありません");
            false
        }
    }
}

/// Full output path of the current settings.
pub fn output_path(ex: &ExportUi) -> String {
    let name = if ex.file_name.trim().is_empty() { "Export" } else { ex.file_name.trim() };
    let dir = expand_home(ex.location.trim());
    let file = format!("{name}.{}", ex.settings.extension());
    if dir.is_empty() { file } else { std::path::Path::new(&dir).join(file).to_string_lossy().to_string() }
}

/// Engine params for the current settings (`file.exportMedia` / `export.queue.add`).
pub fn export_params(app: &FilmcraftApp) -> Value {
    let ex = &app.ui.export;
    let mut p = json!({"path": output_path(ex), "settings": settings_json(&ex.settings), "range": ex.range});
    if ex.range == "custom" {
        p["startSeconds"] = json!(ex.custom_start);
        p["endSeconds"] = json!(ex.custom_end);
    }
    p
}

fn sync_sequence(app: &mut FilmcraftApp) {
    let Some(seq) = app.session.state.active_sequence else { return };
    if app.ui.export.for_sequence != Some(seq.0) || app.ui.export.file_name.is_empty() {
        app.ui.export.file_name = app.session.project.item(seq).map(|i| i.name.clone()).unwrap_or_else(|| "Sequence".into());
        app.ui.export.for_sequence = Some(seq.0);
    }
    if app.ui.export.location.is_empty() {
        app.ui.export.location = filmcraft_engine::export_tools::default_export_dir(&app.session).to_string_lossy().to_string();
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let left = Rect::from_min_size(rect.min, vec2(300.0, rect.height()));
    let right = Rect::from_min_max(pos2(rect.max.x - (rect.width() * 0.4).clamp(360.0, 640.0), rect.min.y), rect.max);
    let mid = Rect::from_min_max(pos2(left.max.x + 4.0, rect.min.y), pos2(right.min.x - 4.0, rect.max.y));
    for r in [left, mid, right] {
        ui.painter().rect_filled(r, t.radius, t.panel_bg);
    }
    let Some(seq_id) = app.session.state.active_sequence else {
        crate::dock::placeholder(ui, mid, &t, crate::i18n::t("Open a sequence to export", "書き出すシーケンスを開いてください"));
        queue_column(app, ui, left);
        return;
    };
    sync_sequence(app);
    let before = settings_json(&app.ui.export.settings);
    queue_column(app, ui, left);
    settings_column(app, ui, mid);
    preview_column(app, ui, right, seq_id);
    // editing any setting turns the preset into "Custom"
    if settings_json(&app.ui.export.settings) != before
        && app.ui.export.preset != CUSTOM
        && app.session.export_presets.find(&app.ui.export.preset).is_none_or(|p| settings_json(&p.settings) != settings_json(&app.ui.export.settings))
    {
        app.ui.export.preset = CUSTOM.into();
    }
    preset_manager(app, ui.ctx());
    if app.session.export_queue.is_active() || app.session.jobs.iter().any(|j| !j.progress.finished.load(std::sync::atomic::Ordering::Relaxed)) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------------------------
// left: destinations + queue
// ---------------------------------------------------------------------------------------------

fn queue_column(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut reg = Reg::default();
    let p = ui.painter();
    p.text(rect.min + vec2(14.0, 20.0), Align2::LEFT_CENTER, crate::i18n::t("Destinations", "書き出し先"), Tokens::semibold(13.0), t.text);
    let mf = Rect::from_min_size(rect.min + vec2(8.0, 38.0), vec2(rect.width() - 16.0, 26.0));
    p.rect_filled(mf, 4.0, t.row_selected);
    p.text(pos2(mf.min.x + 10.0, mf.center().y), Align2::LEFT_CENTER, crate::i18n::t("Media File", "メディアファイル"), Tokens::ui(12.5), t.text);
    reg.add("export.destination.mediaFile", mf, "Media File");
    let qtop = mf.max.y + 18.0;
    let n = app.session.export_queue.items.len();
    p.text(pos2(rect.min.x + 14.0, qtop), Align2::LEFT_CENTER, crate::tf!("Queue ({n})", "キュー ({n})"), Tokens::semibold(13.0), t.text);
    let body = Rect::from_min_max(pos2(rect.min.x + 8.0, qtop + 14.0), pos2(rect.max.x - 8.0, rect.max.y - 40.0));
    let items: Vec<Value> = app.session.execute("export.queue.list", json!({})).ok().and_then(|v| v["items"].as_array().cloned()).unwrap_or_default();
    let mut action: Option<(&'static str, Value)> = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("export-queue"));
    egui::ScrollArea::vertical().id_salt("export-queue-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        if items.is_empty() {
            ui.label(
                egui::RichText::new(crate::i18n::t(
                    "Send exports here with Send to Queue, then start the queue.",
                    "「キューに送信」で書き出しをここに追加し、キューを開始します。",
                ))
                .color(t.text_dim)
                .size(11.5),
            );
        }
        for (i, it) in items.iter().enumerate() {
            let id = it["id"].as_u64().unwrap_or(0);
            let status = it["status"].as_str().unwrap_or("");
            let frame = egui::Frame::NONE.fill(if i % 2 == 0 { t.row_alt } else { Color32::TRANSPARENT }).inner_margin(egui::Margin::same(6));
            let r = frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let file = std::path::Path::new(it["path"].as_str().unwrap_or("")).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                    ui.label(egui::RichText::new(format!("{} → {file}", it["sequenceName"].as_str().unwrap_or(""))).size(12.0).color(t.text));
                    let preset = it["preset"].as_str().filter(|s| !s.is_empty()).unwrap_or(CUSTOM);
                    let (label, col) = match status {
                        "ready" => ("Ready".to_string(), t.text_dim),
                        "encoding" => (crate::tf!("Encoding {:.0}%", "エンコード中 {:.0}%", it["progress"].as_f64().unwrap_or(0.0) * 100.0), t.accent),
                        "done" => ("Done".to_string(), t.render_green),
                        "failed" => (crate::tf!("Failed: {}", "失敗: {}", it["error"].as_str().unwrap_or("")), t.danger),
                        _ => ("Cancelled".to_string(), t.text_faint),
                    };
                    ui.label(egui::RichText::new(format!("{preset} · {label}")).size(11.0).color(col));
                    if status == "encoding" {
                        ui.add(egui::ProgressBar::new(it["progress"].as_f64().unwrap_or(0.0) as f32).desired_height(6.0));
                    }
                    ui.horizontal(|ui| {
                        let mut b = |ui: &mut egui::Ui, key: &'static str, text: &str, enabled: bool, cmd: &'static str, params: Value| {
                            let r = ui.add_enabled(enabled, egui::Button::new(egui::RichText::new(text).size(11.0)).small());
                            reg.add(format!("export.queue.item.{id}.{key}"), r.rect, text);
                            if r.clicked() {
                                action = Some((cmd, params));
                            }
                        };
                        b(ui, "up", "Up", i > 0 && status != "encoding", "export.queue.move", json!({"id": id, "by": -1}));
                        b(ui, "down", "Down", i + 1 < items.len(), "export.queue.move", json!({"id": id, "by": 1}));
                        if matches!(status, "ready" | "encoding") {
                            b(ui, "cancel", "Cancel", true, "export.queue.cancel", json!({"id": id}));
                        } else {
                            b(ui, "retry", "Retry", true, "export.queue.retry", json!({"id": id, "start": true}));
                        }
                        b(ui, "remove", "Remove", status != "encoding", "export.queue.remove", json!({"id": id}));
                    });
                })
                .response;
            reg.add(format!("export.queue.item.{id}"), r.rect, status);
        }
    });
    // footer
    let running = app.session.export_queue.running;
    let has_ready = items.iter().any(|i| i["status"] == "ready");
    let foot = Rect::from_min_max(pos2(rect.min.x + 8.0, rect.max.y - 34.0), pos2(rect.max.x - 8.0, rect.max.y - 6.0));
    let mut f = ui.new_child(egui::UiBuilder::new().max_rect(foot).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let r = f.add_enabled(!running && has_ready, egui::Button::new(crate::i18n::t("Start Queue", "キューを開始")));
    reg.add("export.queue.start", r.rect, "Start Queue");
    if r.clicked() {
        action = Some(("export.queue.start", json!({})));
    }
    let r = f.add_enabled(running, egui::Button::new(crate::i18n::t("Stop", "停止")));
    reg.add("export.queue.stop", r.rect, "Stop");
    if r.clicked() {
        action = Some(("export.queue.stop", json!({})));
    }
    let r = f.add_enabled(
        items.iter().any(|i| matches!(i["status"].as_str(), Some("done" | "failed" | "cancelled"))),
        egui::Button::new(crate::i18n::t("Clear Finished", "完了分をクリア")),
    );
    reg.add("export.queue.clear", r.rect, "Clear Finished");
    if r.clicked() {
        action = Some(("export.queue.clear", json!({})));
    }
    reg.flush(app);
    if let Some((cmd, p)) = action
        && let Err(e) = app.session.execute(cmd, p)
    {
        app.ui.status = e.to_string();
    }
}

// ---------------------------------------------------------------------------------------------
// middle: settings
// ---------------------------------------------------------------------------------------------

const LABEL_W: f32 = 132.0;

/// A labelled settings row.
/// Bitrate Encoding, Target / Maximum Bitrate and Key Frame Distance of the delivery codecs
/// (H.264, HEVC), with the bitrate modes the codec offers.
fn delivery_bitrate_rows(ui: &mut egui::Ui, reg: &mut Reg, s: &mut ExportSettings, t: &Tokens, modes: &[BitrateMode]) {
    row(ui, t, "Bitrate Encoding", |ui| {
        let o = modes;
        let labels: Vec<(String, bool)> = o.iter().map(|m| (m.label().to_string(), true)).collect();
        if let Some(i) = combo(ui, reg, "export.video.bitrateMode", s.bitrate_mode.label(), &labels, 140.0) {
            s.bitrate_mode = o[i];
        }
    });
    if let Some(bpp) = s.adaptive_bitrate {
        row(ui, t, "Target Bitrate", |ui| {
            ui.label(egui::RichText::new(crate::tf!("Adaptive ({bpp} bits per pixel)", "アダプティブ({bpp} ビット/ピクセル)")).size(12.0));
            let r = ui.small_button(crate::i18n::t("Set", "設定"));
            reg.add("export.video.fixedBitrate", r.rect, "Set a fixed bitrate");
            if r.clicked() {
                s.adaptive_bitrate = None;
            }
        });
    } else {
        row(ui, t, "Target Bitrate", |ui| {
            let mut mbps = s.bitrate_kbps as f64 / 1000.0;
            if drag(ui, reg, "export.video.target", &mut mbps, 0.1..=800.0, 0.1, " Mbps", 1) {
                s.bitrate_kbps = (mbps * 1000.0).round() as u32;
            }
        });
        if s.bitrate_mode != BitrateMode::Cbr {
            row(ui, t, "Maximum Bitrate", |ui| {
                let mut mbps = s.max_bitrate_kbps.unwrap_or(s.bitrate_kbps * 3 / 2) as f64 / 1000.0;
                if drag(ui, reg, "export.video.max", &mut mbps, 0.1..=1000.0, 0.1, " Mbps", 1) {
                    s.max_bitrate_kbps = Some((mbps * 1000.0).round() as u32);
                }
            });
        }
    }
    row(ui, t, "Key Frame Distance", |ui| {
        let mut on = s.keyframe_distance.is_some();
        if check(ui, reg, "export.video.keyframeOn", &mut on, "") {
            s.keyframe_distance = on.then_some(48);
        }
        if let Some(k) = s.keyframe_distance.as_mut() {
            let mut v = *k as f64;
            drag(ui, reg, "export.video.keyframe", &mut v, 1.0..=600.0, 1.0, " frames", 0);
            *k = v as u32;
        }
    });
}

fn row<R>(ui: &mut egui::Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(LABEL_W, 20.0), egui::Label::new(egui::RichText::new(crate::i18n::tr_export(label)).color(t.text_dim).size(12.0)).truncate());
        add(ui)
    })
    .inner
}

/// A combo box over `options` (label, enabled); returns the chosen index.
fn combo(ui: &mut egui::Ui, reg: &mut Reg, id: &str, selected: &str, options: &[(String, bool)], width: f32) -> Option<usize> {
    let mut chosen = None;
    let r = egui::ComboBox::from_id_salt(id).selected_text(crate::i18n::tr_export(selected)).width(width).show_ui(ui, |ui| {
        for (i, (label, enabled)) in options.iter().enumerate() {
            let resp = ui.add_enabled(*enabled, egui::Button::selectable(label == selected, crate::i18n::tr_export(label)));
            let resp = if *enabled { resp } else { resp.on_disabled_hover_text(crate::i18n::tr_export("Not supported by FilmCraft's encoders yet")) };
            reg.add(format!("{id}.option.{i}"), resp.rect, label.clone());
            if resp.clicked() {
                chosen = Some(i);
            }
        }
    });
    reg.add(id, r.response.rect, selected);
    chosen
}

fn opts(labels: &[&str]) -> Vec<(String, bool)> {
    labels.iter().map(|l| (l.to_string(), true)).collect()
}

fn section(ui: &mut egui::Ui, reg: &mut Reg, open: &mut Vec<String>, key: &str, title: &str, t: &Tokens) -> bool {
    let is_open = open.iter().any(|k| k == key);
    let (r, now) = crate::widgets::section_header(ui, egui::Id::new(("export-sec", key)), title, is_open, t, true);
    reg.add(format!("export.section.{key}"), r.rect, title);
    if now != is_open {
        if now {
            open.push(key.into());
        } else {
            open.retain(|k| k != key);
        }
    }
    now
}

fn drag(ui: &mut egui::Ui, reg: &mut Reg, id: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, speed: f64, suffix: &str, decimals: usize) -> bool {
    let r = ui.add(egui::DragValue::new(v).range(range).speed(speed).suffix(suffix).max_decimals(decimals));
    reg.add(id, r.rect, format!("{v}"));
    r.changed()
}

fn check(ui: &mut egui::Ui, reg: &mut Reg, id: &str, v: &mut bool, label: &str) -> bool {
    let r = ui.checkbox(v, crate::i18n::tr_export(label));
    reg.add(id, r.rect, label);
    r.changed()
}

fn text(ui: &mut egui::Ui, reg: &mut Reg, id: &str, v: &mut String, hint: &str) {
    let r = ui.add(egui::TextEdit::singleline(v).hint_text(crate::i18n::tr_ui(hint)).desired_width(f32::INFINITY));
    reg.add(id, r.rect, hint);
}

const RATES: [(&str, FrameRate); 11] = [
    ("10", FrameRate { num: 10, den: 1 }),
    ("12", FrameRate { num: 12, den: 1 }),
    ("15", FrameRate { num: 15, den: 1 }),
    ("23.976", FrameRate::FPS_23_976),
    ("24", FrameRate::FPS_24),
    ("25", FrameRate::FPS_25),
    ("29.97", FrameRate::FPS_29_97),
    ("30", FrameRate::FPS_30),
    ("50", FrameRate::FPS_50),
    ("59.94", FrameRate::FPS_59_94),
    ("60", FrameRate::FPS_60),
];

const PARS: [(&str, Option<(u32, u32)>); 6] = [
    ("Square Pixels (1.0)", None),
    ("D1/DV NTSC (0.9091)", Some((10, 11))),
    ("D1/DV PAL (1.0940)", Some((59, 54))),
    ("HD Anamorphic 1080 (1.333)", Some((4, 3))),
    ("DVCPRO HD (1.5)", Some((3, 2))),
    ("Anamorphic 2:1 (2.0)", Some((2, 1))),
];

fn settings_column(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut reg = Reg::default();
    let mut open_manager = false;
    let mut chosen_preset: Option<String> = None;
    let (seq_w, seq_h, seq_rate, seq_sr) = app
        .session
        .active_sequence()
        .map(|q| (q.settings.width, q.settings.height, q.settings.frame_rate, q.settings.sample_rate))
        .unwrap_or((1920, 1080, FrameRate::FPS_24, 48_000));
    let has_captions = app.session.active_sequence().is_some_and(|q| !q.caption_tracks.is_empty());
    let all_presets = app.session.export_presets.all();
    let favs: Vec<String> = all_presets.iter().filter(|p| app.session.export_presets.is_favorite(&p.name)).map(|p| p.name.clone()).collect();
    let mut pick_folder = false;
    let mut pick_overlay = false;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(14.0, 10.0))).id_salt("export-settings"));
    let ex = &mut app.ui.export;
    egui::ScrollArea::vertical().id_salt("export-settings-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.label(egui::RichText::new(crate::i18n::t("Media File", "メディアファイル")).strong().size(14.0));
        ui.add_space(6.0);
        row(ui, &t, crate::i18n::t("File Name", "ファイル名"), |ui| {
            text(ui, &mut reg, "export.fileName", &mut ex.file_name, crate::i18n::t("Sequence name", "シーケンス名"))
        });
        row(ui, &t, crate::i18n::t("Location", "保存先"), |ui| {
            let r = ui.small_button("…");
            reg.add("export.location.browse", r.rect, "Choose folder");
            pick_folder = r.clicked();
            text(ui, &mut reg, "export.location", &mut ex.location, crate::i18n::t("Folder", "フォルダー"));
        });
        // Preset: favourites, then every preset by category, then More presets…
        row(ui, &t, crate::i18n::t("Preset", "プリセット"), |ui| {
            let r =
                egui::ComboBox::from_id_salt("export.preset").selected_text(preset_display(&ex.preset)).width(ui.available_width() - 4.0).show_ui(ui, |ui| {
                    if !favs.is_empty() {
                        ui.label(egui::RichText::new(crate::i18n::t("Favorites", "お気に入り")).color(t.text_dim).size(11.0));
                        for f in &favs {
                            if ui.selectable_label(*f == ex.preset, preset_display(f)).clicked() {
                                chosen_preset = Some(f.clone());
                            }
                        }
                        ui.separator();
                    }
                    let mut cat = String::new();
                    for p in &all_presets {
                        if p.category != cat {
                            cat = p.category.clone();
                            ui.label(egui::RichText::new(preset_display(&cat)).color(t.text_dim).size(11.0));
                        }
                        if ui.selectable_label(p.name == ex.preset, preset_display(&p.name)).clicked() {
                            chosen_preset = Some(p.name.clone());
                        }
                    }
                    ui.separator();
                    if ui.button(crate::i18n::t("More presets…", "その他のプリセット…")).clicked() {
                        open_manager = true;
                    }
                });
            reg.add("export.preset", r.response.rect, ex.preset.clone());
        });
        ui.horizontal(|ui| {
            ui.add_space(LABEL_W + 8.0);
            let r = ui.link(crate::i18n::t("More presets…", "その他のプリセット…"));
            reg.add("export.preset.more", r.rect, "More presets…");
            if r.clicked() {
                open_manager = true;
            }
        });
        row(ui, &t, crate::i18n::t("Format", "形式"), |ui| {
            let labels: Vec<(String, bool)> = Format::ALL.iter().map(|f| (f.label().to_string(), filmcraft_engine::export::available(*f))).collect();
            if let Some(i) = combo(ui, &mut reg, "export.format", ex.settings.format.label(), &labels, ui.available_width() - 4.0) {
                let f = Format::ALL[i];
                if f != ex.settings.format {
                    // a format switch starts from that format's defaults (keeping size / rate / range)
                    let keep = ex.settings.clone();
                    ex.settings = ExportSettings {
                        frame_size: keep.frame_size,
                        frame_rate: keep.frame_rate,
                        effects: keep.effects,
                        metadata: keep.metadata,
                        ..filmcraft_engine::export::presets::format_defaults(f)
                    };
                }
            }
        });
        ui.add_space(8.0);
        let s = &mut ex.settings;
        if s.has_video() && section(ui, &mut reg, &mut ex.open_sections, "video", crate::i18n::t("Video", "ビデオ"), &t) {
            video_section(ui, &mut reg, s, &t, seq_w, seq_h);
        }
        if (s.has_audio()
            || !s.has_video()
            || s.format.is_delivery()
            || matches!(s.format, Format::ProRes | Format::DnxHr | Format::Mjpeg)
            || s.format.is_mxf())
            && !s.is_image_sequence()
            && s.format != Format::Gif
            && section(ui, &mut reg, &mut ex.open_sections, "audio", crate::i18n::t("Audio", "オーディオ"), &t)
        {
            audio_section(ui, &mut reg, s, &t, seq_sr);
        }
        if s.format.is_delivery() && section(ui, &mut reg, &mut ex.open_sections, "multiplexer", crate::i18n::t("Multiplexer", "マルチプレクサー"), &t)
        {
            row(ui, &t, crate::i18n::t("Multiplexer", "多重化"), |ui| {
                let cur = if s.multiplexer == Multiplexer::Mp4 { "MP4" } else { "QuickTime" };
                if let Some(i) = combo(ui, &mut reg, "export.multiplexer", cur, &opts(&["MP4", "QuickTime"]), 160.0) {
                    s.multiplexer = if i == 0 { Multiplexer::Mp4 } else { Multiplexer::Mov };
                }
            });
        }
        if s.has_video() && section(ui, &mut reg, &mut ex.open_sections, "captions", crate::i18n::t("Captions", "キャプション"), &t) {
            if !has_captions {
                ui.label(
                    egui::RichText::new(crate::i18n::t("The sequence has no caption track.", "シーケンスにキャプショントラックがありません。"))
                        .color(t.text_dim)
                        .size(11.5),
                );
            }
            row(ui, &t, crate::i18n::t("Export Options", "書き出しオプション"), |ui| {
                let modes = [
                    crate::i18n::t("None", "なし"),
                    crate::i18n::t("Burn Captions Into Video", "ビデオにキャプションを焼き付け"),
                    crate::i18n::t("Create Sidecar File", "サイドカーファイルを作成"),
                ];
                let cur = if s.burn_captions {
                    modes[1]
                } else if s.caption_sidecar.is_some() {
                    modes[2]
                } else {
                    modes[0]
                };
                if let Some(i) = combo(ui, &mut reg, "export.captions.mode", cur, &opts(&modes), 220.0) {
                    s.burn_captions = i == 1;
                    s.caption_sidecar = (i == 2).then(|| s.caption_sidecar.clone().unwrap_or_else(|| "srt".into()));
                }
            });
            if let Some(kind) = s.caption_sidecar.clone() {
                row(ui, &t, crate::i18n::t("File Format", "ファイル形式"), |ui| {
                    let cur = if kind == "vtt" { "WebVTT (.vtt)" } else { "SubRip Subtitle (.srt)" };
                    if let Some(i) = combo(ui, &mut reg, "export.captions.format", cur, &opts(&["SubRip Subtitle (.srt)", "WebVTT (.vtt)"]), 220.0) {
                        s.caption_sidecar = Some(if i == 0 { "srt" } else { "vtt" }.into());
                    }
                });
            }
        }
        if section(ui, &mut reg, &mut ex.open_sections, "effects", crate::i18n::t("Effects", "エフェクト"), &t) {
            pick_overlay = effects_section(ui, &mut reg, s, &t);
        }
        if matches!(s.format, Format::H264 | Format::Hevc | Format::Av1 | Format::ProRes | Format::DnxHr | Format::Mjpeg)
            && section(ui, &mut reg, &mut ex.open_sections, "metadata", crate::i18n::t("Metadata", "メタデータ"), &t)
        {
            let m = &mut s.metadata;
            for (key, label, v) in [
                ("title", crate::i18n::t("Title", "タイトル"), &mut m.title),
                ("creator", crate::i18n::t("Creator", "作成者"), &mut m.creator),
                ("copyright", crate::i18n::t("Copyright", "著作権"), &mut m.copyright),
                ("description", crate::i18n::t("Description", "説明"), &mut m.description),
                ("comment", crate::i18n::t("Comment", "コメント"), &mut m.comment),
            ] {
                row(ui, &t, label, |ui| text(ui, &mut reg, &format!("export.metadata.{key}"), v, ""));
            }
        }
        let _ = seq_rate;
    });
    if let Some(name) = chosen_preset {
        apply_preset(app, &name);
    }
    if open_manager {
        app.ui.export.manager = Some(PresetManager { selected: app.ui.export.preset.clone(), ..Default::default() });
    }
    if pick_folder && let Some(d) = app.hooks.pick_folder.as_mut().and_then(|f| f()) {
        app.ui.export.location = d;
    }
    if pick_overlay && let Some(f) = app.hooks.pick_open_file.as_mut().and_then(|f| f("Image", &["png", "jpg", "jpeg"])) {
        app.ui.export.settings.effects.image_overlay.path = f;
        app.ui.export.settings.effects.image_overlay.enabled = true;
    }
    reg.flush(app);
}

fn video_section(ui: &mut egui::Ui, reg: &mut Reg, s: &mut ExportSettings, t: &Tokens, seq_w: u32, seq_h: u32) {
    // frame size
    let mut match_size = s.frame_size.is_none();
    row(ui, t, "Frame Size", |ui| {
        if check(ui, reg, "export.video.matchSize", &mut match_size, "Match Source") {
            s.frame_size = if match_size { None } else { Some((seq_w, seq_h)) };
        }
    });
    if let Some((w, h)) = s.frame_size.as_mut() {
        row(ui, t, "", |ui| {
            let (mut fw, mut fh) = (*w as f64, *h as f64);
            drag(ui, reg, "export.video.width", &mut fw, 16.0..=8192.0, 1.0, "", 0);
            ui.label("×");
            drag(ui, reg, "export.video.height", &mut fh, 16.0..=8192.0, 1.0, "", 0);
            (*w, *h) = (fw as u32, fh as u32);
        });
    }
    // frame rate
    let mut match_rate = s.frame_rate.is_none();
    row(ui, t, "Frame Rate", |ui| {
        if check(ui, reg, "export.video.matchRate", &mut match_rate, "Match Source") {
            s.frame_rate = if match_rate { None } else { Some(FrameRate::FPS_29_97) };
        }
        if let Some(r) = s.frame_rate {
            let cur = RATES.iter().find(|(_, x)| *x == r).map(|(l, _)| l.to_string()).unwrap_or_else(|| r.label());
            let labels: Vec<(String, bool)> = RATES.iter().map(|(l, _)| (l.to_string(), true)).collect();
            if let Some(i) = combo(ui, reg, "export.video.fps", &cur, &labels, 90.0) {
                s.frame_rate = Some(RATES[i].1);
            }
        }
    });
    row(ui, t, "Field Order", |ui| {
        let o = [FieldOrder::Progressive, FieldOrder::UpperFirst, FieldOrder::LowerFirst];
        let labels: Vec<(String, bool)> = o.iter().map(|f| (f.label().to_string(), *f == FieldOrder::Progressive)).collect();
        if let Some(i) = combo(ui, reg, "export.video.fieldOrder", s.field_order.label(), &labels, 160.0) {
            s.field_order = o[i];
        }
    });
    row(ui, t, "Aspect", |ui| {
        let cur = PARS.iter().find(|(_, p)| *p == s.pixel_aspect).map(|(l, _)| l.to_string()).unwrap_or_else(|| "Custom".into());
        let labels: Vec<(String, bool)> = PARS.iter().map(|(l, _)| (l.to_string(), true)).collect();
        if let Some(i) = combo(ui, reg, "export.video.aspect", &cur, &labels, 220.0) {
            s.pixel_aspect = PARS[i].1;
        }
    });
    if s.format.is_mxf() {
        row(ui, t, "Video Codec", |ui| {
            let labels: Vec<(String, bool)> = MxfVideoCodec::ALL.iter().map(|c| (c.label().to_string(), true)).collect();
            if let Some(i) = combo(ui, reg, "export.video.mxfCodec", s.mxf_video_codec.label(), &labels, 180.0) {
                s.mxf_video_codec = MxfVideoCodec::ALL[i];
            }
        });
    }
    match s.video_format() {
        Format::Hevc => {
            row(ui, t, "Profile", |ui| {
                ui.label(egui::RichText::new("Main").size(12.0));
            });
            row(ui, t, "Performance", |ui| {
                ui.label(egui::RichText::new(crate::i18n::tr_export("Hardware Encoding")).size(12.0));
            });
            delivery_bitrate_rows(ui, reg, s, t, &[BitrateMode::Cbr, BitrateMode::Vbr1Pass]);
        }
        Format::Av1 => {
            row(ui, t, "Profile", |ui| {
                ui.label(egui::RichText::new("Main (8-bit 4:2:0)").size(12.0));
            });
            row(ui, t, "Performance", |ui| {
                ui.label(egui::RichText::new(crate::i18n::tr_export("Software Encoding")).size(12.0));
            });
            // rav1e encodes one pass towards an average bitrate
            delivery_bitrate_rows(ui, reg, s, t, &[BitrateMode::Vbr1Pass]);
        }
        Format::H264 => {
            row(ui, t, "Profile", |ui| {
                let o = [H264Profile::Baseline, H264Profile::Main, H264Profile::High];
                let labels: Vec<(String, bool)> = o.iter().map(|p| (p.label().to_string(), true)).collect();
                if let Some(i) = combo(ui, reg, "export.video.profile", s.h264_profile.label(), &labels, 120.0) {
                    s.h264_profile = o[i];
                }
            });
            row(ui, t, "Level", |ui| {
                let levels: [Option<u8>; 13] =
                    [None, Some(30), Some(31), Some(32), Some(40), Some(41), Some(42), Some(50), Some(51), Some(52), Some(60), Some(61), Some(62)];
                let lab = |l: &Option<u8>| l.map(|l| format!("{}.{}", l / 10, l % 10)).unwrap_or_else(|| "Auto".into());
                let labels: Vec<(String, bool)> = levels.iter().map(|l| (lab(l), true)).collect();
                if let Some(i) = combo(ui, reg, "export.video.level", &lab(&s.h264_level), &labels, 120.0) {
                    s.h264_level = levels[i];
                }
            });
            row(ui, t, "Performance", |ui| {
                let labels: Vec<(String, bool)> = ["Hardware Encoding", "Software Encoding"].iter().map(|l| (l.to_string(), true)).collect();
                let cur = if s.hardware_encoding { "Hardware Encoding" } else { "Software Encoding" };
                if let Some(i) = combo(ui, reg, "export.video.performance", cur, &labels, 160.0) {
                    s.hardware_encoding = i == 0;
                }
            });
            delivery_bitrate_rows(ui, reg, s, t, &[BitrateMode::Cbr, BitrateMode::Vbr1Pass, BitrateMode::Vbr2Pass]);
        }
        Format::ProRes => {
            row(ui, t, "Profile", |ui| {
                let keys = ["proxy", "lt", "standard", "hq", "4444", "4444xq"];
                let labels =
                    ["Apple ProRes 422 Proxy", "Apple ProRes 422 LT", "Apple ProRes 422", "Apple ProRes 422 HQ", "Apple ProRes 4444", "Apple ProRes 4444 XQ"];
                let cur = keys.iter().position(|k| *k == s.prores_profile).unwrap_or(3);
                if let Some(i) = combo(ui, reg, "export.video.proresProfile", labels.get(cur).copied().unwrap_or_default(), &opts(&labels), 220.0)
                    && let Some(k) = keys.get(i)
                {
                    s.prores_profile = (*k).into();
                }
            });
            // 4444 / 4444 XQ carry the sequence's alpha (transparent where nothing is drawn)
            if s.supports_alpha() {
                row(ui, t, "Alpha", |ui| {
                    check(ui, reg, "export.video.alpha", &mut s.alpha, "Include Alpha Channel");
                });
            }
        }
        Format::DnxHr => {
            row(ui, t, "Profile", |ui| {
                let keys = ["lb", "sq", "hq", "hqx"];
                let labels = ["DNxHR LB", "DNxHR SQ", "DNxHR HQ", "DNxHR HQX (10-bit)"];
                let cur = keys.iter().position(|k| *k == s.dnx_profile).unwrap_or(2);
                if let Some(i) = combo(ui, reg, "export.video.dnxProfile", labels[cur], &opts(&labels), 220.0) {
                    s.dnx_profile = keys[i].into();
                }
            });
        }
        Format::Mjpeg => {
            row(ui, t, "Quality", |ui| {
                let mut q = s.quality as f64;
                drag(ui, reg, "export.video.quality", &mut q, 1.0..=100.0, 1.0, "", 0);
                s.quality = q as u8;
            });
        }
        _ => {}
    }
    row(ui, t, "", |ui| check(ui, reg, "export.video.maxDepth", &mut s.render_at_max_depth, "Render at Maximum Depth"));
    row(ui, t, "", |ui| check(ui, reg, "export.video.maxQuality", &mut s.max_render_quality, "Use Maximum Render Quality"));
}

fn audio_section(ui: &mut egui::Ui, reg: &mut Reg, s: &mut ExportSettings, t: &Tokens, seq_sr: u32) {
    let audio_only = !s.has_video();
    if !audio_only {
        row(ui, t, "", |ui| check(ui, reg, "export.audio.include", &mut s.include_audio, "Export Audio"));
        if !s.include_audio {
            return;
        }
    }
    row(ui, t, "Audio Format", |ui| {
        // MP4 carries AAC: H.264 in MP4, and AV1 (MP4 only)
        let mp4_aac = s.format == Format::Av1 || (s.format == Format::H264 && s.multiplexer == Multiplexer::Mp4);
        let fixed = mp4_aac || audio_only || s.format.is_mxf();
        let cur = match s.audio_codec() {
            AudioCodec::Aac => "AAC",
            _ => "Uncompressed (PCM)",
        };
        let labels = vec![("AAC".to_string(), !audio_only), ("Uncompressed (PCM)".to_string(), !mp4_aac)];
        if fixed {
            ui.label(cur);
        } else if let Some(i) = combo(ui, reg, "export.audio.codec", cur, &labels, 180.0) {
            s.audio.codec = if i == 0 { AudioCodec::Aac } else { AudioCodec::Pcm };
        }
    });
    row(ui, t, "Sample Rate", |ui| {
        let rates: [Option<u32>; 5] = [None, Some(32_000), Some(44_100), Some(48_000), Some(96_000)];
        let lab = |r: &Option<u32>| r.map(|r| format!("{r} Hz")).unwrap_or_else(|| format!("{} ({seq_sr} Hz)", crate::i18n::tr_export("Match Source")));
        let labels: Vec<(String, bool)> = rates.iter().map(|r| (lab(r), true)).collect();
        if let Some(i) = combo(ui, reg, "export.audio.sampleRate", &lab(&s.audio.sample_rate), &labels, 180.0) {
            s.audio.sample_rate = rates[i];
        }
    });
    row(ui, t, "Channels", |ui| {
        let cur = match s.audio.channels {
            1 => "Mono",
            6 => "5.1",
            _ => "Stereo",
        };
        if let Some(i) = combo(ui, reg, "export.audio.channels", cur, &opts(&["Mono", "Stereo", "5.1"]), 120.0) {
            s.audio.channels = [1, 2, 6][i.min(2)];
        }
    });
    if s.audio_codec() == AudioCodec::Aac {
        row(ui, t, "Audio Bitrate", |ui| {
            let rates = [128u32, 160, 192, 256, 320];
            let labels: Vec<(String, bool)> = rates.iter().map(|r| (format!("{r} kbps"), true)).collect();
            if let Some(i) = combo(ui, reg, "export.audio.bitrate", &format!("{} kbps", s.audio.bitrate_kbps), &labels, 120.0) {
                s.audio.bitrate_kbps = rates[i];
            }
        });
    } else {
        row(ui, t, "Sample Size", |ui| {
            let cur = if s.audio.bits >= 24 { "24 bit" } else { "16 bit" };
            if let Some(i) = combo(ui, reg, "export.audio.bits", cur, &opts(&["16 bit", "24 bit"]), 120.0) {
                s.audio.bits = if i == 0 { 16 } else { 24 };
            }
        });
    }
}

fn placement_combo(ui: &mut egui::Ui, reg: &mut Reg, id: &str, p: &mut Placement) {
    let labels: Vec<(String, bool)> = Placement::ALL.iter().map(|x| (x.label().to_string(), true)).collect();
    if let Some(i) = combo(ui, reg, id, p.label(), &labels, 150.0) {
        *p = Placement::ALL[i];
    }
}

fn text_overlay(ui: &mut egui::Ui, reg: &mut Reg, key: &str, o: &mut TextOverlay, t: &Tokens, text_label: &str) {
    row(ui, t, text_label, |ui| text(ui, reg, &format!("export.effects.{key}.text"), &mut o.text, ""));
    row(ui, t, "Position", |ui| placement_combo(ui, reg, &format!("export.effects.{key}.position"), &mut o.placement));
    row(ui, t, "Size / Opacity", |ui| {
        let (mut sz, mut op) = (o.size_percent as f64, o.opacity as f64);
        drag(ui, reg, &format!("export.effects.{key}.size"), &mut sz, 1.0..=50.0, 0.2, " %", 1);
        drag(ui, reg, &format!("export.effects.{key}.opacity"), &mut op, 0.0..=100.0, 1.0, " %", 0);
        (o.size_percent, o.opacity) = (sz as f32, op as f32);
    });
}

/// Returns whether the image overlay's Browse… was clicked.
fn effects_section(ui: &mut egui::Ui, reg: &mut Reg, s: &mut ExportSettings, t: &Tokens) -> bool {
    let mut browse = false;
    let audio = s.has_audio() || !s.has_video();
    let video = s.has_video();
    let fx = &mut s.effects;
    if video {
        check(ui, reg, "export.effects.imageOverlay", &mut fx.image_overlay.enabled, "Image Overlay");
        if fx.image_overlay.enabled {
            let o = &mut fx.image_overlay;
            row(ui, t, "Image", |ui| {
                let r = ui.small_button(crate::i18n::t("Browse…", "参照…"));
                reg.add("export.effects.imageOverlay.browse", r.rect, "Browse…");
                browse = r.clicked();
                text(ui, reg, "export.effects.imageOverlay.path", &mut o.path, "PNG or JPEG file");
            });
            row(ui, t, "Position", |ui| placement_combo(ui, reg, "export.effects.imageOverlay.position", &mut o.placement));
            row(ui, t, "Size / Opacity", |ui| {
                let (mut sz, mut op) = (o.size_percent as f64, o.opacity as f64);
                drag(ui, reg, "export.effects.imageOverlay.size", &mut sz, 1.0..=100.0, 0.5, " %", 1);
                drag(ui, reg, "export.effects.imageOverlay.opacity", &mut op, 0.0..=100.0, 1.0, " %", 0);
                (o.size_percent, o.opacity) = (sz as f32, op as f32);
            });
        }
        check(ui, reg, "export.effects.nameOverlay", &mut fx.name_overlay.enabled, "Name Overlay");
        if fx.name_overlay.enabled {
            text_overlay(ui, reg, "nameOverlay", &mut fx.name_overlay, t, "Name");
        }
        check(ui, reg, "export.effects.timecodeOverlay", &mut fx.timecode_overlay.enabled, "Timecode Overlay");
        if fx.timecode_overlay.enabled {
            text_overlay(ui, reg, "timecodeOverlay", &mut fx.timecode_overlay, t, "Prefix");
        }
        check(ui, reg, "export.effects.videoLimiter", &mut fx.video_limiter.enabled, "Video Limiter");
        if fx.video_limiter.enabled {
            row(ui, t, "Luma Min / Max", |ui| {
                let (mut lo, mut hi) = (fx.video_limiter.min_percent as f64, fx.video_limiter.max_percent as f64);
                drag(ui, reg, "export.effects.videoLimiter.min", &mut lo, 0.0..=50.0, 0.5, " %", 1);
                drag(ui, reg, "export.effects.videoLimiter.max", &mut hi, 50.0..=100.0, 0.5, " %", 1);
                (fx.video_limiter.min_percent, fx.video_limiter.max_percent) = (lo as f32, hi as f32);
            });
        }
    }
    if audio {
        check(ui, reg, "export.effects.loudness", &mut fx.loudness.enabled, "Loudness Normalization");
        if fx.loudness.enabled {
            row(ui, t, "Target Loudness", |ui| {
                drag(ui, reg, "export.effects.loudness.target", &mut fx.loudness.target_lufs, -40.0..=-5.0, 0.1, " LUFS", 1);
            });
            row(ui, t, "True Peak Limit", |ui| {
                drag(ui, reg, "export.effects.loudness.truePeak", &mut fx.loudness.true_peak_dbtp, -9.0..=0.0, 0.1, " dBTP", 1);
            });
        }
    }
    browse
}

// ---------------------------------------------------------------------------------------------
// right: preview, range, summary, buttons
// ---------------------------------------------------------------------------------------------

fn ranges() -> [(&'static str, &'static str); 4] {
    [
        ("entire", crate::i18n::t("Entire Source", "ソース全体")),
        ("inOut", crate::i18n::t("Source In/Out", "ソースのイン/アウト")),
        ("workArea", crate::i18n::t("Work Area", "ワークエリア")),
        ("custom", crate::i18n::t("Custom", "カスタム")),
    ]
}

fn preview_column(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, seq_id: filmcraft_engine::project::ItemId) {
    let t = app.tokens;
    let mut reg = Reg::default();
    let Some(q) = app.session.active_sequence().cloned() else { return };
    let rate = q.settings.frame_rate;
    // preview (aspect of the output frame)
    let r = app.ui.export.settings.resolve(q.settings.width, q.settings.height, rate, q.settings.sample_rate);
    let pv_area = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(rect.width() - 28.0, (rect.height() * 0.42).max(160.0)));
    let pic = crate::panels::monitor::fit(pv_area, r.width as f32, r.height as f32);
    ui.painter().rect_filled(pic, 0.0, Color32::BLACK);
    if app.ui.export.settings.has_video() {
        let frame = rate.frame_at(app.session.playhead());
        let key = FrameKey { target: Target::Sequence(seq_id), frame, size: 500, revision: app.session.revision, draft: false };
        let project = app.session.project.clone();
        app.frames.request(key, rate.tick_of(frame), 0.5, &project, 0);
        if let Some(img) = app.frames.get(&key) {
            let ctx = ui.ctx().clone();
            let tex = app.texture_for(&ctx, "export-preview", key, &img);
            let inner = match app.ui.export.settings.scaling {
                Scaling::StretchToFill => pic,
                Scaling::ScaleToFit => crate::panels::monitor::fit(pic, q.settings.width as f32, q.settings.height as f32),
                Scaling::ScaleToFill => {
                    let s = (pic.width() / q.settings.width as f32).max(pic.height() / q.settings.height as f32);
                    Rect::from_center_size(pic.center(), vec2(q.settings.width as f32 * s, q.settings.height as f32 * s))
                }
            };
            ui.painter().with_clip_rect(pic).image(tex, inner, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
    } else {
        ui.painter().text(pic.center(), Align2::CENTER_CENTER, crate::i18n::t("Audio only", "オーディオのみ"), Tokens::ui(13.0), t.text_dim);
    }
    reg.add("export.preview", pic, "Preview");
    // range + scaling
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(rect.min.x + 14.0, pv_area.max.y + 8.0), pos2(rect.max.x - 14.0, rect.max.y - 50.0)))
            .id_salt("export-right"),
    );
    let ex = &mut app.ui.export;
    let has_work_area = q.work_area.is_some();
    child.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::t("Range", "範囲")).color(t.text_dim));
        let cur = ranges().iter().find(|(k, _)| *k == ex.range).map(|x| x.1).unwrap_or(crate::i18n::t("Entire Source", "ソース全体"));
        let labels: Vec<(String, bool)> = ranges().iter().map(|(k, l)| (l.to_string(), *k != "workArea" || has_work_area)).collect();
        if let Some(i) = combo(ui, &mut reg, "export.range", cur, &labels, 140.0) {
            ex.range = ranges()[i].0.into();
        }
        if ex.range == "custom" {
            drag(ui, &mut reg, "export.range.start", &mut ex.custom_start, 0.0..=86_400.0, 0.04, " s", 2);
            drag(ui, &mut reg, "export.range.end", &mut ex.custom_end, 0.0..=86_400.0, 0.04, " s", 2);
        }
    });
    if ex.settings.has_video() {
        child.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::t("Scaling", "スケーリング")).color(t.text_dim));
            let o = [
                (Scaling::ScaleToFit, crate::i18n::t("Scale To Fit", "全体に合わせて拡大/縮小")),
                (Scaling::ScaleToFill, crate::i18n::t("Scale To Fill", "フレームに合わせて拡大/縮小")),
                (Scaling::StretchToFill, crate::i18n::t("Stretch To Fill", "フレームサイズに合わせて伸縮")),
            ];
            let cur = o.iter().find(|(x, _)| *x == ex.settings.scaling).map(|x| x.1).unwrap_or(crate::i18n::t("Scale To Fit", "全体に合わせて拡大/縮小"));
            let labels: Vec<(String, bool)> = o.iter().map(|(_, l)| (l.to_string(), true)).collect();
            if let Some(i) = combo(ui, &mut reg, "export.scaling", cur, &labels, 140.0) {
                ex.settings.scaling = o[i].0;
            }
        });
    }
    // summary
    let range = range_ticks(ex, &q);
    let sum = ex.settings.summary(q.settings.width, q.settings.height, rate, q.settings.sample_rate, range.1 - range.0);
    child.add_space(8.0);
    let resp = egui::Frame::NONE
        .fill(t.field_bg)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(10))
        .show(&mut child, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(crate::i18n::t("Summary", "概要")).strong());
            egui::Grid::new("export-summary-grid").num_columns(2).spacing(vec2(10.0, 3.0)).show(ui, |ui| {
                let line = |ui: &mut egui::Ui, k: &str, v: &str| {
                    ui.label(egui::RichText::new(k).color(t.text_dim).size(11.5));
                    ui.add(egui::Label::new(egui::RichText::new(v).size(11.5)).wrap_mode(egui::TextWrapMode::Wrap).halign(egui::Align::Min));
                    ui.end_row();
                };
                line(ui, crate::i18n::t("Output", "出力"), &output_path(ex));
                line(ui, "", &summary_text(&sum.format));
                line(ui, "", &summary_text(&sum.video));
                line(ui, "", &summary_text(&sum.audio));
                line(
                    ui,
                    crate::i18n::t("Source", "ソース"),
                    &crate::tf!(
                        "Sequence, {}x{}, {} fps, {} Hz",
                        "シーケンス, {}x{}, {} fps, {} Hz",
                        q.settings.width,
                        q.settings.height,
                        rate.label(),
                        q.settings.sample_rate
                    ),
                );
                line(
                    ui,
                    crate::i18n::t("Range", "範囲"),
                    &format!("{} ({:.2} s)", ranges().iter().find(|(k, _)| *k == ex.range).map(|x| x.1).unwrap_or(""), (range.1 - range.0).seconds()),
                );
            });
        })
        .response;
    reg.add("export.summary", resp.rect, sum.video.clone());
    child.add_space(4.0);
    let est =
        child.label(egui::RichText::new(crate::tf!("Estimated file size: {}", "予測ファイルサイズ: {}", format_bytes(sum.estimated_bytes))).color(t.text_dim));
    reg.add("export.estimate", est.rect, sum.estimated_size.clone());
    // buttons
    let go = Rect::from_min_size(pos2(rect.max.x - 120.0, rect.max.y - 44.0), vec2(104.0, 30.0));
    let queue = Rect::from_min_size(pos2(go.min.x - 132.0, go.min.y), vec2(122.0, 30.0));
    let qresp = ui.interact(queue, egui::Id::new("export-queue-send"), Sense::click());
    ui.painter().rect_stroke(queue, 15.0, egui::Stroke::new(1.0, if qresp.hovered() { t.text } else { t.text_dim }), egui::StrokeKind::Inside);
    ui.painter().text(queue.center(), Align2::CENTER_CENTER, crate::i18n::t("Send to Queue", "キューに送信"), Tokens::semibold(12.5), t.text);
    reg.add("export.sendToQueue", queue, "Send to Queue");
    let resp = ui.interact(go, egui::Id::new("export-go"), Sense::click());
    ui.painter().rect_filled(go, 15.0, if resp.hovered() { t.accent_hover } else { t.accent });
    ui.painter().text(go.center(), Align2::CENTER_CENTER, crate::i18n::t("Export", "書き出し"), Tokens::semibold(13.0), Color32::WHITE);
    reg.add("export.button", go, "Export");
    reg.flush(app);
    if resp.clicked() {
        let r = app.session.execute("file.exportMedia", export_params(app));
        match r {
            Ok(v) => app.ui.status = crate::tf!("Exporting {}", "書き出し中: {}", v["path"].as_str().unwrap_or("")),
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    if qresp.clicked() {
        let mut p = export_params(app);
        if app.ui.export.preset != CUSTOM {
            // the queue shows which preset an item came from
            p["preset"] = json!(app.ui.export.preset);
        }
        match app.session.execute("export.queue.add", p) {
            Ok(_) => app.ui.status = crate::i18n::t("Added to the export queue", "書き出しキューに追加しました").into(),
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

/// The export range in sequence ticks (for the summary).
fn range_ticks(ex: &ExportUi, q: &filmcraft_engine::project::Sequence) -> (Tick, Tick) {
    let fd = q.settings.frame_rate.frame_duration();
    let end = q.duration().max(fd);
    match ex.range.as_str() {
        "inOut" => {
            let a = q.mark_in.unwrap_or(Tick::ZERO);
            (a, q.mark_out.map(|o| o + fd).unwrap_or(end).max(a + fd))
        }
        "workArea" => q.work_area.map(|w| (w.start, w.end())).unwrap_or((Tick::ZERO, end)),
        "custom" => (Tick::from_seconds_f64(ex.custom_start.max(0.0)), Tick::from_seconds_f64(ex.custom_end.max(ex.custom_start + 0.001))),
        _ => (Tick::ZERO, end),
    }
}

// ---------------------------------------------------------------------------------------------
// Preset Manager
// ---------------------------------------------------------------------------------------------

fn preset_manager(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut m) = app.ui.export.manager.clone() else { return };
    let t = app.tokens;
    let mut reg = Reg::default();
    let mut open = true;
    let mut close = false;
    let mut apply: Option<String> = None;
    let mut cmd: Option<(&'static str, Value)> = None;
    let mut pick: Option<&'static str> = None;
    let all = app.session.export_presets.all();
    let favorite = |n: &str| app.session.export_presets.is_favorite(n);
    let q = m.query.to_ascii_lowercase();
    egui::Window::new(crate::i18n::t("Preset Manager", "プリセットマネージャー"))
        .id(egui::Id::new("preset-manager"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(vec2(560.0, 520.0))
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let r = crate::widgets::search_field(ui, &mut m.query, "Search presets", 300.0, &t);
                reg.add("presetManager.search", r.rect, "Search");
                check(ui, &mut reg, "presetManager.favoritesOnly", &mut m.favorites_only, "Favorites only");
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, false]).show(ui, |ui| {
                let mut cat = String::new();
                for p in all.iter().filter(|p| {
                    (q.is_empty()
                        || p.name.to_ascii_lowercase().contains(&q)
                        || p.description.to_ascii_lowercase().contains(&q)
                        || p.category.to_ascii_lowercase().contains(&q))
                        && (!m.favorites_only || favorite(&p.name))
                }) {
                    if p.category != cat {
                        cat = p.category.clone();
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&cat).strong().color(t.text_dim).size(11.5));
                    }
                    ui.horizontal(|ui| {
                        let fav = favorite(&p.name);
                        let (sr, sresp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                        paint_star(ui.painter(), sr.shrink(2.0), fav, if fav { Color32::from_rgb(0xf5, 0xc2, 0x42) } else { t.text_faint });
                        reg.add(format!("presetManager.favorite.{}", p.name), sr, if fav { "Remove from favorites" } else { "Add to favorites" });
                        if sresp.on_hover_text(crate::i18n::t("Show in the Preset menu", "プリセットメニューに表示")).clicked() {
                            cmd = Some(("export.presets.favorite", json!({"name": p.name})));
                        }
                        let sel = preset_key(&m.selected) == preset_key(&p.name);
                        let r = list_row(ui, &t, sel, &p.name, 300.0);
                        reg.add(format!("presetManager.item.{}", p.name), r.rect, p.name.clone());
                        if r.clicked() {
                            m.selected = p.name.clone();
                        }
                        if r.double_clicked() {
                            apply = Some(p.name.clone());
                        }
                        let r = r.on_hover_text(&p.description);
                        let _ = r;
                        ui.label(egui::RichText::new(p.settings.format.label()).color(t.text_dim).size(11.0));
                        if !p.builtin {
                            ui.label(egui::RichText::new(crate::i18n::t("User", "ユーザー")).color(t.accent).size(10.5));
                        }
                    });
                }
            });
            ui.separator();
            let sel = all.iter().find(|p| preset_key(&p.name) == preset_key(&m.selected)).cloned();
            if let Some(p) = &sel {
                ui.label(egui::RichText::new(&p.description).color(t.text_dim).size(11.5));
            }
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Save current settings as:", "現在の設定を次の名前で保存:"));
                let r = ui.add(egui::TextEdit::singleline(&mut m.save_name).hint_text(crate::i18n::t("Preset name", "プリセット名")).desired_width(200.0));
                reg.add("presetManager.saveName", r.rect, "Preset name");
                let r = ui.add_enabled(!m.save_name.trim().is_empty(), egui::Button::new(crate::i18n::t("Save", "保存")));
                reg.add("presetManager.save", r.rect, "Save");
                if r.clicked() {
                    cmd = Some(("export.presets.save", json!({"name": m.save_name.trim(), "settings": settings_json(&app.ui.export.settings)})));
                    m.selected = m.save_name.trim().to_string();
                }
            });
            ui.horizontal(|ui| {
                let r = ui.button(crate::i18n::t("Import…", "読み込み…"));
                reg.add("presetManager.import", r.rect, "Import…");
                if r.clicked() {
                    pick = Some("import");
                }
                let r = ui.add_enabled(sel.is_some(), egui::Button::new(crate::i18n::t("Export…", "書き出し…")));
                reg.add("presetManager.export", r.rect, "Export…");
                if r.clicked() {
                    pick = Some("export");
                }
                let r = ui.add_enabled(sel.as_ref().is_some_and(|p| !p.builtin), egui::Button::new(crate::i18n::t("Delete", "削除")));
                reg.add("presetManager.delete", r.rect, "Delete");
                if r.clicked() {
                    cmd = Some(("export.presets.delete", json!({"name": m.selected})));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.add_enabled(sel.is_some(), egui::Button::new("OK"));
                    reg.add("presetManager.ok", r.rect, "OK");
                    if r.clicked() {
                        apply = Some(m.selected.clone());
                    }
                    let r = ui.button(crate::i18n::t("Cancel", "キャンセル"));
                    reg.add("presetManager.cancel", r.rect, "Cancel");
                    close |= r.clicked();
                });
            });
        });
    reg.flush(app);
    if let Some((c, p)) = cmd
        && let Err(e) = app.session.execute(c, p)
    {
        app.ui.status = e.to_string();
    }
    match pick {
        Some("import") => match app.hooks.pick_open_file.as_mut().and_then(|f| f("Export presets", &["json"])) {
            Some(path) => {
                if let Err(e) = app.session.execute("export.presets.import", json!({"path": path})) {
                    app.ui.status = e.to_string();
                }
            }
            None => {
                app.ui.status = crate::i18n::t(
                    "Import: no file chosen (agents: export.presets.import {path})",
                    "読み込み: ファイルが選択されていません(エージェント: export.presets.import {path})",
                )
                .into()
            }
        },
        Some(_) => match app.hooks.pick_save_as.as_mut().and_then(|f| f("Export presets", &["json"], &format!("{}.json", m.selected))) {
            Some(path) => {
                if let Err(e) = app.session.execute("export.presets.export", json!({"path": path, "names": [m.selected]})) {
                    app.ui.status = e.to_string();
                }
            }
            None => {
                app.ui.status = crate::i18n::t(
                    "Export: no file chosen (agents: export.presets.export {path, names})",
                    "書き出し: ファイルが選択されていません(エージェント: export.presets.export {path, names})",
                )
                .into()
            }
        },
        None => {}
    }
    if let Some(name) = apply
        && apply_preset(app, &name)
    {
        close = true;
    }
    app.ui.export.manager = if close || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) { None } else { Some(m) };
}

/// A five-pointed star (favourite marker), drawn from scratch.
fn paint_star(p: &egui::Painter, r: Rect, filled: bool, color: Color32) {
    let c = r.center();
    let (ro, ri) = (r.width().min(r.height()) / 2.0, r.width().min(r.height()) / 5.0);
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            let rr = if i % 2 == 0 { ro } else { ri };
            pos2(c.x + rr * a.cos(), c.y + rr * a.sin())
        })
        .collect();
    if filled {
        // triangles from the centre keep the concave outline exact
        for i in 0..10 {
            p.add(egui::Shape::convex_polygon(vec![c, pts[i], pts[(i + 1) % 10]], color, egui::Stroke::NONE));
        }
    }
    p.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.0, color)));
}

// ---------------------------------------------------------------------------------------------
// Quick Export (header)
// ---------------------------------------------------------------------------------------------

/// Presets offered by Quick Export: favourites first, then the common delivery presets.
fn quick_presets(app: &FilmcraftApp) -> Vec<String> {
    let mut v: Vec<String> =
        app.session.export_presets.all().iter().filter(|p| app.session.export_presets.is_favorite(&p.name)).map(|p| p.name.clone()).collect();
    for p in builtin_presets().into_iter().filter(|p| p.category == "Match Source" || p.category == "Web & Social").map(|p| p.name) {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// The Quick Export popup under the header button at `anchor`.
pub fn quick_export(app: &mut FilmcraftApp, ctx: &egui::Context, anchor: egui::Pos2) {
    if !app.ui.export.quick_open {
        return;
    }
    let t = app.tokens;
    let Some(seq) = app.session.state.active_sequence else {
        app.ui.export.quick_open = false;
        app.ui.status = crate::i18n::t("Quick Export: open a sequence first", "クイック書き出し: 先にシーケンスを開いてください").into();
        return;
    };
    if app.ui.export.quick_preset.is_empty() {
        app.ui.export.quick_preset = app.session.export_queue.quick_preset.clone().unwrap_or_else(|| DEFAULT_PRESET.to_string());
    }
    let preset = app.session.export_presets.find(&app.ui.export.quick_preset);
    let ext = preset.as_ref().map(|p| p.settings.extension()).unwrap_or("mp4");
    if app.ui.export.quick_path.is_empty() || !app.ui.export.quick_path.ends_with(&format!(".{ext}")) {
        let name = app.session.project.item(seq).map(|i| i.name.clone()).unwrap_or_else(|| "Sequence".into());
        let dir = match app.ui.export.quick_path.rsplit_once(['/', '\\']) {
            Some((d, _)) if !d.is_empty() => d.to_string(),
            _ => filmcraft_engine::export_tools::default_export_dir(&app.session).to_string_lossy().to_string(),
        };
        app.ui.export.quick_path = std::path::Path::new(&dir).join(format!("{name}.{ext}")).to_string_lossy().to_string();
    }
    let est = app
        .session
        .execute("export.resolve", json!({"preset": app.ui.export.quick_preset}))
        .ok()
        .and_then(|v| v["summary"]["estimated_size"].as_str().map(str::to_string));
    let presets = quick_presets(app);
    let mut reg = Reg::default();
    let mut go = false;
    let mut close = false;
    let q = &mut app.ui.export;
    let area = egui::Area::new(egui::Id::new("quick-export")).order(egui::Order::Foreground).fixed_pos(anchor).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(340.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::t("Quick Export", "クイック書き出し")).strong().size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.small_button(crate::i18n::t("Close", "閉じる"));
                    reg.add("quickExport.close", r.rect, "Close");
                    close = r.clicked();
                });
            });
            ui.label(egui::RichText::new(crate::i18n::t("File Name & Location", "ファイル名と場所")).color(t.text_dim).size(11.5));
            let r = ui.add(egui::TextEdit::singleline(&mut q.quick_path).desired_width(f32::INFINITY));
            reg.add("quickExport.path", r.rect, "File Name & Location");
            ui.add_space(4.0);
            ui.label(egui::RichText::new(crate::i18n::t("Preset", "プリセット")).color(t.text_dim).size(11.5));
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for name in &presets {
                    let r = list_row(ui, &t, preset_key(name) == preset_key(&q.quick_preset), name, 320.0);
                    reg.add(format!("quickExport.preset.{name}"), r.rect, name.clone());
                    if r.clicked() {
                        q.quick_preset = name.clone();
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(crate::tf!("Estimated file size: {}", "予測ファイルサイズ: {}", est.as_deref().unwrap_or("–")))
                    .color(t.text_dim)
                    .size(11.5),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = ui.add(
                    egui::Button::new(egui::RichText::new(crate::i18n::t("Export", "書き出し")).color(Color32::WHITE))
                        .fill(t.accent)
                        .corner_radius(12.0)
                        .min_size(vec2(90.0, 26.0)),
                );
                reg.add("quickExport.go", r.rect, "Export");
                go = r.clicked();
            });
        });
    });
    let clicked_elsewhere = area.response.clicked_elsewhere();
    // the header click that opened the popup is not a click elsewhere
    let toggled = ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("quick-export-toggled")).unwrap_or(false));
    reg.flush(app);
    if go {
        let p = json!({"preset": app.ui.export.quick_preset, "path": app.ui.export.quick_path});
        match app.session.execute("export.quick", p) {
            Ok(v) => app.ui.status = crate::tf!("Quick Export: {}", "クイック書き出し: {}", v["path"].as_str().unwrap_or("")),
            Err(e) => app.ui.status = e.to_string(),
        }
        close = true;
    }
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) || (clicked_elsewhere && !toggled) {
        app.ui.export.quick_open = false;
    }
}

/// A left-aligned selectable list row of a fixed width.
fn list_row(ui: &mut egui::Ui, t: &Tokens, selected: bool, text: &str, width: f32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click());
    let bg = if selected {
        t.accent
    } else if resp.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(r, 3.0, bg);
    ui.painter().text(pos2(r.min.x + 6.0, r.center().y), Align2::LEFT_CENTER, text, Tokens::ui(12.5), if selected { Color32::WHITE } else { t.text });
    resp
}

fn expand_home(p: &str) -> String {
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(h)) => format!("{h}/{rest}"),
        _ => p.to_string(),
    }
}

#[cfg(test)]
mod summary_tests {
    /// The Summary's fixed words in Japanese (pure translator; the UI language is not switched).
    #[test]
    fn summary_lines_in_japanese() {
        let v = super::summary_in_japanese(
            "1920x1080 (1.0), 23.976 fps, Progressive, High auto level, VBR, 1 pass, Target 9.94 Mbps, Max 14.91 Mbps, keyframe every 48 frames",
        );
        assert_eq!(
            v,
            "1920x1080 (1.0)、23.976 fps、プログレッシブ、High レベル自動、VBR 1 パス、ターゲット 9.94 Mbps、最大 14.91 Mbps、キーフレーム間隔 48 フレーム"
        );
        assert_eq!(super::summary_in_japanese("AAC, 320 kbps, 48000 Hz, Stereo"), "AAC、320 kbps、48000 Hz、ステレオ");
        assert_eq!(super::summary_in_japanese("Uncompressed 24-bit PCM, 48000 Hz, Mono"), "非圧縮 24 ビット PCM、48000 Hz、モノラル");
    }

    /// Every built-in preset and category has a Japanese name, or is a product / platform name.
    #[test]
    fn builtin_presets_in_japanese() {
        let kept = |n: &str| ["YouTube", "Vimeo", "Apple ProRes", "Avid DNxHR", "MXF "].iter().any(|k| n.starts_with(k));
        let missing: Vec<String> = super::builtin_presets()
            .iter()
            .flat_map(|p| [p.name.clone(), p.category.clone()])
            .filter(|n| !kept(n) && super::preset_in_japanese(n).is_ascii())
            .collect();
        assert!(missing.is_empty(), "no Japanese name: {missing:?}");
    }
}
