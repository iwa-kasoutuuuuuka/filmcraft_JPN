//! File ▸ Export ▸ AAF… and OMF…: the export settings dialogs. OK asks for the file name (the
//! host's save panel) and runs `file.exportAaf` / `file.exportOmf` with the chosen settings; with
//! parameters (`path` …) the menu commands run directly.
//!
//! Automation ids (`<p>` is `aafExport` or `omfExport`): `<p>.mixdownVideo` (AAF),
//! `<p>.breakoutToMono`, `<p>.audio.<embedded|separate|linked>` (linked: AAF only),
//! `<p>.audioFormat.<wav|aiff|mxf>` (mxf: AAF only), `<p>.sampleRate.<rate>`, `<p>.bitDepth.<16|24>`, `<p>.trimAudio`,
//! `<p>.handles` (frames), `<p>.renderAudioEffects`, `<p>.title` (OMF), `<p>.ok`, `<p>.cancel`.

use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Debug, PartialEq)]
struct Draft {
    omf: bool,
    title: String,
    mixdown_video: bool,
    breakout: bool,
    audio: &'static str,
    format: &'static str,
    sample_rate: u32,
    bits: u16,
    trim: bool,
    handles: i64,
    render_effects: bool,
}

fn draft_id() -> egui::Id {
    egui::Id::new("interchange-export-draft")
}

const RATES: [u32; 4] = [32_000, 44_100, 48_000, 96_000];

/// Open the AAF or OMF settings dialog (`id` is the menu command).
pub fn open(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str) -> Result<Value, String> {
    let omf = id == "file.exportOmf";
    let seq = app.session.state.active_sequence.and_then(|s| app.session.project.item(s));
    let title = seq.map(|i| i.name.clone()).unwrap_or_default();
    let sr = app.session.active_sequence().map(|q| q.settings.sample_rate).filter(|r| RATES.contains(r)).unwrap_or(48_000);
    let d = Draft {
        omf,
        title,
        mixdown_video: false,
        breakout: false,
        audio: "embedded",
        format: "wav",
        sample_rate: sr,
        bits: 16,
        trim: omf,
        handles: if omf { 30 } else { 0 },
        render_effects: false,
    };
    ctx.data_mut(|m| m.insert_temp(draft_id(), Some(d)));
    Ok(json!({"dialog": if omf { "omfExport" } else { "aafExport" }}))
}

/// Whether the dialog is open.
pub fn is_open(ctx: &egui::Context) -> bool {
    ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())).flatten().is_some()
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(Some(mut d)) = ctx.data(|m| m.get_temp::<Option<Draft>>(draft_id())) else { return };
    let p = if d.omf { "omfExport" } else { "aafExport" };
    let mut close = false;
    let mut apply = false;
    let mut elems: Vec<(String, egui::Rect, String)> = Vec::new();
    let title = if d.omf { "OMF Export Settings" } else { "AAF Export Settings" };
    egui::Window::new(crate::i18n::tr_ui(title))
        .id(egui::Id::new(("interchange-dialog", title)))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(380.0);
            egui::Grid::new("interchange-export-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                if d.omf {
                    ui.label(crate::i18n::t("OMF Title:", "OMF タイトル:"));
                    let r = ui.text_edit_singleline(&mut d.title);
                    elems.push((format!("{p}.title"), r.rect, d.title.clone()));
                    ui.end_row();
                } else {
                    ui.label("");
                    let r = ui.checkbox(&mut d.mixdown_video, crate::i18n::t("Mixdown video", "ビデオをミックスダウン"));
                    elems.push((format!("{p}.mixdownVideo"), r.rect, d.mixdown_video.to_string()));
                    ui.end_row();
                }
                ui.label(crate::i18n::t("Audio:", "オーディオ:"));
                ui.horizontal(|ui| {
                    let modes: &[(&'static str, &str)] = if d.omf {
                        &[("embedded", "Encapsulate"), ("separate", "Separate Audio")]
                    } else {
                        &[("embedded", "Embed"), ("separate", "Separate Files"), ("linked", "Link to Media")]
                    };
                    for (k, label) in modes {
                        let r = ui.radio(d.audio == *k, *label);
                        if r.clicked() {
                            d.audio = k;
                        }
                        elems.push((format!("{p}.audio.{k}"), r.rect, label.to_string()));
                    }
                });
                ui.end_row();
                ui.label(crate::i18n::t("Audio File Format:", "オーディオファイル形式:"));
                ui.horizontal(|ui| {
                    let formats: &[(&'static str, &str)] = if d.omf {
                        &[("wav", "Broadcast Wave"), ("aiff", "AIFF")]
                    } else {
                        &[("wav", "Broadcast Wave"), ("aiff", "AIFF"), ("mxf", "OP-Atom MXF")]
                    };
                    for &(k, label) in formats {
                        let r = ui.add_enabled(d.audio == "separate", egui::RadioButton::new(d.format == k, label));
                        if r.clicked() {
                            d.format = k;
                        }
                        elems.push((format!("{p}.audioFormat.{k}"), r.rect, label.to_string()));
                    }
                });
                ui.end_row();
                ui.label(crate::i18n::t("Sample Rate:", "サンプルレート:"));
                ui.horizontal(|ui| {
                    for r in RATES {
                        let b = ui.radio(d.sample_rate == r, format!("{r}"));
                        if b.clicked() {
                            d.sample_rate = r;
                        }
                        elems.push((format!("{p}.sampleRate.{r}"), b.rect, format!("{r} Hz")));
                    }
                });
                ui.end_row();
                ui.label(crate::i18n::t("Bits per Sample:", "サンプルあたりのビット数:"));
                ui.horizontal(|ui| {
                    for b in [16u16, 24] {
                        let r = ui.radio(d.bits == b, format!("{b}"));
                        if r.clicked() {
                            d.bits = b;
                        }
                        elems.push((format!("{p}.bitDepth.{b}"), r.rect, format!("{b}-bit")));
                    }
                });
                ui.end_row();
                ui.label(crate::i18n::t("Rendering:", "レンダリング:"));
                let linked = d.audio == "linked";
                let r = ui.add_enabled(!linked, egui::Checkbox::new(&mut d.trim, "Trim audio files"));
                elems.push((format!("{p}.trimAudio"), r.rect, d.trim.to_string()));
                ui.end_row();
                ui.label(crate::i18n::t("Handle Frames:", "予備フレーム:"));
                let r = ui.add_enabled(d.trim && !linked, egui::DragValue::new(&mut d.handles).range(0..=10_000));
                elems.push((format!("{p}.handles"), r.rect, d.handles.to_string()));
                ui.end_row();
                ui.label("");
                let r = ui.add_enabled(!linked, egui::Checkbox::new(&mut d.render_effects, "Render audio clip effects"));
                elems.push((format!("{p}.renderAudioEffects"), r.rect, d.render_effects.to_string()));
                ui.end_row();
                ui.label("");
                let r = ui.checkbox(&mut d.breakout, crate::i18n::t("Breakout to mono", "モノラルに分割"));
                elems.push((format!("{p}.breakoutToMono"), r.rect, d.breakout.to_string()));
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let c = ui.button(crate::i18n::t("Cancel", "キャンセル"));
                elems.push((format!("{p}.cancel"), c.rect, "Cancel".into()));
                if c.clicked() {
                    close = true;
                }
                let o = ui.add(egui::Button::new(egui::RichText::new("OK").color(egui::Color32::WHITE)).fill(app.tokens.accent));
                elems.push((format!("{p}.ok"), o.rect, "OK".into()));
                if o.clicked() {
                    apply = true;
                }
            });
        });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if apply {
        close = true;
        let (ext, filter) = if d.omf { ("omf", "OMF") } else { ("aaf", "AAF") };
        let stem = if d.title.is_empty() { "Sequence".to_string() } else { d.title.replace(['/', '\\', ':'], "_") };
        let suggested = format!("{stem}.{ext}");
        match app.hooks.pick_save_as.as_mut().and_then(|f| f(filter, &[ext], &suggested)) {
            Some(path) => {
                let mut params = json!({
                    "path": path,
                    "breakoutToMono": d.breakout,
                    "audio": d.audio,
                    "audioFormat": d.format,
                    "sampleRate": d.sample_rate,
                    "bitDepth": d.bits,
                    "trimAudio": d.trim && d.audio != "linked",
                    "handles": d.handles,
                    "renderAudioEffects": d.render_effects && d.audio != "linked",
                });
                if d.omf {
                    params["title"] = json!(d.title);
                } else {
                    params["mixdownVideo"] = json!(d.mixdown_video);
                }
                let cmd = if d.omf { "file.exportOmf" } else { "file.exportAaf" };
                match app.session.execute(cmd, params) {
                    Ok(v) => app.ui.status = crate::tf!("Exported {}", "{} を書き出しました", v["path"].as_str().unwrap_or_default()),
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
            None if app.hooks.pick_save_as.is_none() => {
                app.ui.status = crate::i18n::t(
                    "no save dialog available: run the command with a `path`",
                    "保存ダイアログを利用できません: コマンドに `path` を指定して実行してください",
                )
                .into()
            }
            None => {}
        }
    }
    ctx.data_mut(|m| m.insert_temp(draft_id(), if close { None } else { Some(d) }));
}
