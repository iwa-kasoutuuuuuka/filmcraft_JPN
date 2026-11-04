//! The Animated Chart editor (Graphics ▸ New Layer ▸ Animated Chart…, and Edit Chart… on a chart
//! clip's right-click menu): the chart type on the left, a playable preview or the data table in
//! the middle, the look on the right; **Add to Timeline** runs `graphics.newChart`, **Apply** runs
//! `graphics.setChart` on the clip being edited.
//!
//! The open editor lives in `UiState::chart_editor` (serde), so agents can read and set the chart
//! being edited with `ui.get` / `ui.set` and click the buttons.
//!
//! Automation ids (`chartEditor.*`): `kind.<barRace|bar|column|line|area|pie|donut>`,
//! `tab.preview`, `tab.data`, `play`, `time`, `seconds`, `title`, `theme.dark`, `theme.light`,
//! `palette.<name>`, `font`, `showValues`, `showStep`, `showGrid`, `showLegend`, `background`,
//! `maxBars`, `decimals`, `intro`, `hold`, `step.<c>`, `category.<r>`, `cell.<r>.<c>`, `addRow`,
//! `addColumn`, `removeRow`, `removeColumn`, `paste`, `importTable`, `copyTable`, `ok`,
//! `cancel`.

use std::hash::{Hash, Hasher};

use egui::{Color32, RichText, Sense, Stroke, vec2};
use filmcraft_project::chart::{ChartKind, ChartSpec, ChartTheme, MAX_CATEGORIES, MAX_STEPS, PALETTES};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::state::{ChartEditorDraft, ChartTab};

type Elems = Vec<(String, egui::Rect, String)>;

fn push(elems: &mut Elems, id: impl Into<String>, r: &egui::Response, label: impl Into<String>) {
    elems.push((format!("chartEditor.{}", id.into()), r.rect, label.into()));
}

/// Chart type names in the interface language.
pub fn kind_name(k: ChartKind) -> &'static str {
    match k {
        ChartKind::BarRace => crate::i18n::t("Bar Chart Race", "棒グラフレース"),
        ChartKind::Bar => crate::i18n::t("Bar Chart", "横棒グラフ"),
        ChartKind::Column => crate::i18n::t("Column Chart", "縦棒グラフ"),
        ChartKind::Line => crate::i18n::t("Line Chart", "折れ線グラフ"),
        ChartKind::Area => crate::i18n::t("Area Chart", "面グラフ"),
        ChartKind::Pie => crate::i18n::t("Pie Chart", "円グラフ"),
        ChartKind::Donut => crate::i18n::t("Donut Chart", "ドーナツグラフ"),
    }
}

fn palette_name(n: &str) -> &'static str {
    match n {
        "Vivid" => crate::i18n::t("Vivid", "ビビッド"),
        "Ocean" => crate::i18n::t("Ocean", "オーシャン"),
        "Sunset" => crate::i18n::t("Sunset", "サンセット"),
        "Forest" => crate::i18n::t("Forest", "フォレスト"),
        "Pastel" => crate::i18n::t("Pastel", "パステル"),
        "Mono" => crate::i18n::t("Mono", "モノクロ"),
        _ => "",
    }
}

/// `graphics.newChart` / `graphics.setChart` without params open the editor.
pub fn route(app: &mut FilmcraftApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if params.as_object().is_some_and(|m| !m.is_empty()) {
        return None;
    }
    let s = &app.session;
    let draft = match id {
        "graphics.newChart" => {
            if s.active_sequence().is_none() {
                return Some(Err(crate::i18n::t("Open a sequence first", "先にシーケンスを開いてください").into()));
            }
            // mid-race, so the preview shows the chart before Play is pressed
            ChartEditorDraft { clip: None, chart: ChartSpec::sample(ChartKind::BarRace), seconds: 8.0, time: 4.0, ..ChartEditorDraft::default() }
        }
        "graphics.setChart" => {
            let clip = filmcraft_engine::graphics::target_clip(s, &Value::Null)?;
            let chart = filmcraft_engine::charts::clip_chart(s, clip)?;
            let (seconds, start) =
                s.active_sequence().and_then(|q| q.find_item(clip)).map_or((8.0, 0.0), |(_, it)| (it.duration.seconds(), it.start.seconds()));
            // the frame under the playhead, else the middle of the clip
            let at = s.playhead().seconds() - start;
            let time = if (0.0..=seconds).contains(&at) { at } else { seconds / 2.0 };
            ChartEditorDraft { clip: Some(clip.0), chart, seconds, time, ..ChartEditorDraft::default() }
        }
        _ => return None,
    };
    app.ui.chart_editor = Some(draft);
    Some(Ok(json!({"dialog": "chartEditor"})))
}

fn hash_of(v: &impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// The preview picture of `d` at its time, `w` pixels wide, through a one-entry texture cache.
fn preview_texture(ctx: &egui::Context, d: &ChartEditorDraft, canvas: (u32, u32), w: u32) -> egui::TextureHandle {
    let h = ((f64::from(w) * f64::from(canvas.1) / f64::from(canvas.0.max(1))).round() as u32).clamp(1, 2048);
    let json = serde_json::to_string(&d.chart).unwrap_or_default();
    let key = hash_of(&(json, (d.time * 1000.0).round() as i64, (d.seconds * 1000.0).round() as i64, canvas, w));
    let id = egui::Id::new("chart-editor-preview");
    if let Some((k, t)) = ctx.data(|m| m.get_temp::<(u64, egui::TextureHandle)>(id))
        && k == key
    {
        return t;
    }
    let px = filmcraft_render::chart::chart_rgba(&d.chart, d.time, d.seconds, canvas, w, h);
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
    let t = ctx.load_texture("chart-editor-preview", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|m| m.insert_temp(id, (key, t.clone())));
    t
}

/// A checkerboard behind the preview, so a chart without its card shows as transparent.
fn checker(ui: &egui::Ui, r: egui::Rect) {
    let p = ui.painter_at(r);
    p.rect_filled(r, 4.0, Color32::from_gray(60));
    let s = 12.0;
    let (nx, ny) = ((r.width() / s).ceil() as i32, (r.height() / s).ceil() as i32);
    for y in 0..ny {
        for x in (0..nx).filter(|x| (x + y) % 2 == 0) {
            let min = r.min + vec2(x as f32 * s, y as f32 * s);
            p.rect_filled(egui::Rect::from_min_size(min, vec2(s, s)).intersect(r), 0.0, Color32::from_gray(76));
        }
    }
}

fn kinds_column(ui: &mut egui::Ui, d: &mut ChartEditorDraft, elems: &mut Elems) {
    ui.vertical(|ui| {
        ui.set_width(150.0);
        ui.label(RichText::new(crate::i18n::t("Chart", "チャート")).strong());
        for k in ChartKind::ALL {
            let r = ui.add_sized([150.0, 26.0], egui::Button::selectable(d.chart.kind == k, kind_name(k)));
            push(elems, format!("kind.{}", k.id()), &r, k.name());
            if r.clicked() {
                d.chart.kind = k;
            }
        }
    });
}

fn preview_tab(ui: &mut egui::Ui, d: &mut ChartEditorDraft, canvas: (u32, u32), elems: &mut Elems) {
    let w = 560.0f32;
    let h = w * canvas.1 as f32 / canvas.0.max(1) as f32;
    let (r, _) = ui.allocate_exact_size(vec2(w, h.min(420.0)), Sense::hover());
    checker(ui, r);
    let tex = preview_texture(ui.ctx(), d, canvas, w.round() as u32);
    ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let r = ui.button(if d.playing { "⏸" } else { "▶" });
        push(elems, "play", &r, "Play");
        if r.clicked() {
            if !d.playing && d.time >= d.seconds - 1e-3 {
                d.time = 0.0;
            }
            d.playing = !d.playing;
        }
        let max = d.seconds.max(0.1);
        ui.spacing_mut().slider_width = 300.0;
        let r = ui.add(egui::Slider::new(&mut d.time, 0.0..=max).show_value(false));
        push(elems, "time", &r, "Time");
        if r.dragged() {
            d.playing = false;
        }
        ui.label(format!("{:.1}s /", d.time));
        let r = ui.add_enabled(d.clip.is_none(), egui::DragValue::new(&mut d.seconds).range(1.0..=600.0).speed(0.1).suffix("s"));
        push(elems, "seconds", &r, "Duration");
        if d.clip.is_some() {
            r.on_hover_text(crate::i18n::t("The clip's length: trim it on the timeline", "クリップの長さです。タイムラインでトリミングして変えます"));
        }
    });
    if d.playing {
        let dt = f64::from(ui.ctx().input(|i| i.stable_dt)).min(0.1);
        d.time += dt;
        if d.time >= d.seconds {
            d.time = d.seconds;
            d.playing = false;
        }
        ui.ctx().request_repaint();
    }
}

fn data_tab(ui: &mut egui::Ui, d: &mut ChartEditorDraft, elems: &mut Elems) {
    let c = &mut d.chart;
    let (rows, cols) = (c.categories.len(), c.steps.len());
    ui.label(
        RichText::new(crate::i18n::t(
            "Rows are categories, columns are steps (years, months…) in playing order.",
            "行が項目、列がステップ(年・月など、再生する順)です。",
        ))
        .weak(),
    );
    egui::ScrollArea::both().id_salt("chart-table").max_height(260.0).max_width(560.0).show(ui, |ui| {
        egui::Grid::new("chart-table-grid").striped(true).spacing(vec2(4.0, 3.0)).show(ui, |ui| {
            ui.label("");
            for j in 0..cols {
                if let Some(s) = c.steps.get_mut(j) {
                    let r = ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(s));
                    push(elems, format!("step.{j}"), &r, format!("Step {}", j + 1));
                }
            }
            ui.end_row();
            for i in 0..rows {
                if let Some(s) = c.categories.get_mut(i) {
                    let r = ui.add_sized([110.0, 20.0], egui::TextEdit::singleline(s));
                    push(elems, format!("category.{i}"), &r, format!("Category {}", i + 1));
                }
                if let Some(row) = c.values.get_mut(i) {
                    row.resize(cols, 0.0);
                    for (j, v) in row.iter_mut().enumerate() {
                        let speed = (v.abs() * 0.01).max(0.1);
                        let r = ui.add_sized([70.0, 20.0], egui::DragValue::new(v).speed(speed).max_decimals(6));
                        push(elems, format!("cell.{i}.{j}"), &r, format!("Value {} {}", i + 1, j + 1));
                    }
                }
                ui.end_row();
            }
        });
    });
    ui.horizontal(|ui| {
        let r = ui.add_enabled(rows < MAX_CATEGORIES, egui::Button::new(crate::i18n::t("Add Row", "行を追加")));
        push(elems, "addRow", &r, "Add Row");
        if r.clicked() {
            c.categories.push(format!("{} {}", crate::i18n::t("Item", "項目"), rows + 1));
            c.values.push(vec![0.0; cols]);
        }
        let r = ui.add_enabled(cols < MAX_STEPS, egui::Button::new(crate::i18n::t("Add Column", "列を追加")));
        push(elems, "addColumn", &r, "Add Column");
        if r.clicked() {
            let next = c.steps.last().and_then(|s| s.trim().parse::<i64>().ok()).map_or_else(|| format!("{}", cols + 1), |n| (n.saturating_add(1)).to_string());
            c.steps.push(next);
            for row in &mut c.values {
                let last = row.last().copied().unwrap_or(0.0);
                row.push(last);
            }
        }
        let r = ui.add_enabled(rows > 1, egui::Button::new(crate::i18n::t("Remove Last Row", "最後の行を削除")));
        push(elems, "removeRow", &r, "Remove Last Row");
        if r.clicked() {
            c.categories.pop();
            c.values.truncate(c.categories.len());
        }
        let r = ui.add_enabled(cols > 1, egui::Button::new(crate::i18n::t("Remove Last Column", "最後の列を削除")));
        push(elems, "removeColumn", &r, "Remove Last Column");
        if r.clicked() {
            c.steps.pop();
            for row in &mut c.values {
                row.truncate(c.steps.len());
            }
        }
    });
    ui.add_space(6.0);
    ui.label(crate::i18n::t(
        "Paste a table from a spreadsheet or CSV (a corner cell, then the steps on the first row):",
        "表計算ソフトや CSV の表を貼り付け(1 行目は左上のセルのあとにステップ名):",
    ));
    let r = ui.add(egui::TextEdit::multiline(&mut d.paste).desired_rows(4).desired_width(560.0).code_editor());
    push(elems, "paste", &r, "Paste");
    ui.horizontal(|ui| {
        let r = ui.add_enabled(!d.paste.trim().is_empty(), egui::Button::new(crate::i18n::t("Load Table", "表を読み込む")));
        push(elems, "importTable", &r, "Load Table");
        if r.clicked() {
            match ChartSpec::parse_table(&d.paste) {
                Ok((categories, steps, values)) => {
                    d.chart.categories = categories;
                    d.chart.steps = steps;
                    d.chart.values = values;
                    d.error.clear();
                }
                Err(e) => d.error = e,
            }
        }
        let r = ui.button(crate::i18n::t("Copy Table", "表をコピー"));
        push(elems, "copyTable", &r, "Copy Table");
        if r.clicked() {
            ui.ctx().copy_text(d.chart.table_text());
        }
    });
}

fn settings_column(ui: &mut egui::Ui, d: &mut ChartEditorDraft, elems: &mut Elems) {
    let c = &mut d.chart;
    ui.vertical(|ui| {
        ui.set_width(260.0);
        ui.label(RichText::new(crate::i18n::t("Header", "ヘッダー")).strong());
        let r = ui.add(egui::TextEdit::singleline(&mut c.title).desired_width(250.0).hint_text(crate::i18n::t("Title", "タイトル")));
        push(elems, "title", &r, "Title");
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::t("Theme", "テーマ")).strong());
        ui.horizontal(|ui| {
            for (t, id, label) in
                [(ChartTheme::Light, "light", crate::i18n::t("Light", "ライト")), (ChartTheme::Dark, "dark", crate::i18n::t("Dark", "ダーク"))]
            {
                let r = ui.add_sized([120.0, 24.0], egui::Button::selectable(c.theme == t, label));
                push(elems, format!("theme.{id}"), &r, label);
                if r.clicked() {
                    c.theme = t;
                }
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::t("Color Palette", "カラーパレット")).strong());
        for (name, colors) in PALETTES {
            let on = c.palette.as_slice() == *colors || (c.palette.is_empty() && PALETTES.first().is_some_and(|p| p.0 == *name));
            let (r, resp) = ui.allocate_exact_size(vec2(250.0, 22.0), Sense::click());
            let p = ui.painter_at(r);
            if on || resp.hovered() {
                p.rect_stroke(r, 4.0, Stroke::new(if on { 2.0 } else { 1.0 }, ui.visuals().selection.bg_fill), egui::StrokeKind::Inside);
            }
            for (k, col) in colors.iter().take(8).enumerate() {
                let cr = egui::Rect::from_min_size(r.min + vec2(6.0 + k as f32 * 18.0, 4.0), vec2(14.0, 14.0));
                p.circle_filled(cr.center(), 7.0, Color32::from_rgb(col[0], col[1], col[2]));
            }
            p.text(r.min + vec2(160.0, 11.0), egui::Align2::LEFT_CENTER, palette_name(name), egui::FontId::proportional(12.0), ui.visuals().text_color());
            push(elems, format!("palette.{name}"), &resp, *name);
            if resp.clicked() {
                c.palette = colors.to_vec();
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::t("Font", "フォント")).strong());
        let shown = if c.font.is_empty() { filmcraft_text::fonts::DEFAULT_FAMILY.to_string() } else { c.font.clone() };
        let r = egui::ComboBox::from_id_salt("chart-font").selected_text(shown).width(250.0).height(360.0).show_ui(ui, |ui| {
            if !filmcraft_text::fonts::system_scanned() {
                filmcraft_text::fonts::scan_system();
            }
            for (f, _) in filmcraft_text::families() {
                if ui.selectable_label(c.font == f, &f).clicked() {
                    c.font = f;
                }
            }
        });
        push(elems, "font", &r.response, "Font");
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::t("Show", "表示")).strong());
        for (v, id, en, ja) in [
            (&mut c.show_values, "showValues", "Values", "値"),
            (&mut c.show_step, "showStep", "Step label (year…)", "ステップ(年など)を大きく表示"),
            (&mut c.show_grid, "showGrid", "Grid lines and axis", "グリッド線と軸"),
            (&mut c.show_legend, "showLegend", "Legend", "凡例"),
            (&mut c.background, "background", "Background card", "背景のカード"),
        ] {
            let r = ui.checkbox(v, crate::i18n::t(en, ja));
            push(elems, id, &r, en);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(crate::i18n::t("Format and Animation", "形式とアニメーション")).strong());
        egui::Grid::new("chart-format").num_columns(2).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
            ui.label(crate::i18n::t("Bars shown", "表示する棒の数"));
            let r = ui.add(egui::DragValue::new(&mut c.max_bars).range(1..=50));
            push(elems, "maxBars", &r, "Bars shown");
            ui.end_row();
            ui.label(crate::i18n::t("Decimals", "小数点以下の桁数"));
            let r = ui.add(egui::DragValue::new(&mut c.decimals).range(0..=6));
            push(elems, "decimals", &r, "Decimals");
            ui.end_row();
            ui.label(crate::i18n::t("Grow in", "伸びる時間"));
            let r = ui.add(egui::DragValue::new(&mut c.intro_seconds).range(0.0..=60.0).speed(0.05).suffix("s"));
            push(elems, "intro", &r, "Grow in");
            ui.end_row();
            ui.label(crate::i18n::t("Hold at end", "最後の静止時間"));
            let r = ui.add(egui::DragValue::new(&mut c.hold_seconds).range(0.0..=60.0).speed(0.05).suffix("s"));
            push(elems, "hold", &r, "Hold at end");
            ui.end_row();
        });
    });
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut d) = app.ui.chart_editor.clone() else { return };
    let canvas = app.session.active_sequence().map_or((1920, 1080), |q| (q.settings.width.max(1), q.settings.height.max(1)));
    let mut elems: Elems = Vec::new();
    let mut action: Option<&'static str> = None;
    let title = crate::i18n::t("Animated Chart", "アニメーションチャート");
    egui::Window::new(title).id(egui::Id::new("chart-editor")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            ui.horizontal_top(|ui| {
                kinds_column(ui, &mut d, &mut elems);
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.set_width(560.0);
                    ui.horizontal(|ui| {
                        for (tab, id, label) in [
                            (ChartTab::Preview, "preview", crate::i18n::t("Preview", "プレビュー")),
                            (ChartTab::Data, "data", crate::i18n::t("Data", "データ")),
                        ] {
                            let r = ui.add_sized([110.0, 24.0], egui::Button::selectable(d.tab == tab, label));
                            push(&mut elems, format!("tab.{id}"), &r, label);
                            if r.clicked() {
                                d.tab = tab;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    match d.tab {
                        ChartTab::Preview => preview_tab(ui, &mut d, canvas, &mut elems),
                        ChartTab::Data => data_tab(ui, &mut d, &mut elems),
                    }
                });
                ui.add_space(12.0);
                settings_column(ui, &mut d, &mut elems);
            });
            if !d.error.is_empty() {
                ui.colored_label(Color32::from_rgb(0xe0, 0x60, 0x60), &d.error);
            }
            ui.separator();
            ui.horizontal(|ui| {
                let r = ui.button(crate::i18n::t("Cancel", "キャンセル"));
                push(&mut elems, "cancel", &r, "Cancel");
                if r.clicked() {
                    action = Some("cancel");
                }
                let label = if d.clip.is_some() { crate::i18n::t("Apply", "適用") } else { crate::i18n::t("Add to Timeline", "タイムラインに追加") };
                let r = ui.button(RichText::new(label).strong());
                push(&mut elems, "ok", &r, label);
                if r.clicked() {
                    action = Some("ok");
                }
            });
        },
    );
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Some("cancel");
    }
    match action {
        Some("cancel") => {
            app.ui.chart_editor = None;
            return;
        }
        Some("ok") => {
            let chart = serde_json::to_value(d.chart.sanitized()).unwrap_or(Value::Null);
            let r = match d.clip {
                Some(clip) => app.session.execute("graphics.setChart", json!({"clip": clip, "chart": chart})),
                None => app.session.execute("graphics.newChart", json!({"chart": chart, "seconds": d.seconds})),
            };
            match r {
                Ok(_) => {
                    app.ui.chart_editor = None;
                    return;
                }
                Err(e) => d.error = e.to_string(),
            }
        }
        _ => {}
    }
    app.ui.chart_editor = Some(d);
}
