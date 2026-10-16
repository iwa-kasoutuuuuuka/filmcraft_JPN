//! Events panel (warnings and errors of commands, background jobs and the auto-save worker, from
//! the engine's `Session::log`) and Progress panel (background jobs with progress and Cancel).
//!
//! Automation ids: Events — `events.filter`, `events.filter.<info|warning|error>`,
//! `events.clearAll`, `events.row.<id>`, `events.details`; Progress — `progress.job.<id>`,
//! `progress.cancel.<id>`, `progress.showFinished`. State: `ui.panels.events` / `ui.panels.progress`.

use egui::{Align2, Color32, Rect, RichText, Sense, Stroke, pos2, vec2};
use filmcraft_engine::panels::{Level, LogEntry};
use serde_json::json;

use crate::FilmcraftApp;
use crate::theme::Tokens;

fn level_color(l: Level) -> Color32 {
    match l {
        Level::Info => Color32::from_rgb(0x57, 0x94, 0xec),
        Level::Warning => Color32::from_rgb(0xe8, 0xb3, 0x3c),
        Level::Error => Color32::from_rgb(0xe0, 0x4f, 0x4f),
    }
}

/// A small level glyph: a dot (info), a triangle (warning), a square (error), drawn in code.
fn glyph(p: &egui::Painter, c: egui::Pos2, l: Level) {
    let col = level_color(l);
    match l {
        Level::Info => {
            p.circle_filled(c, 5.0, col);
        }
        Level::Warning => {
            p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -6.0), c + vec2(6.0, 5.0), c + vec2(-6.0, 5.0)], col, Stroke::NONE));
        }
        Level::Error => {
            p.rect_filled(Rect::from_center_size(c, vec2(10.0, 10.0)), 2.0, col);
        }
    }
    let mark = if l == Level::Info { "i" } else { "!" };
    p.text(c + vec2(0.0, if l == Level::Warning { 1.0 } else { 0.0 }), Align2::CENTER_CENTER, mark, Tokens::semibold(8.0), Color32::BLACK);
}

/// An Events message in the UI language. The engine logs English (agents and logs read it); the
/// messages it writes from fixed patterns are shown in Japanese, anything else as it is.
pub(crate) fn message_ja(msg: &str) -> std::borrow::Cow<'_, str> {
    if crate::i18n::is_japanese() { message_in_japanese(msg) } else { msg.into() }
}

/// [`message_ja`] whatever the UI language (pure, for tests).
fn message_in_japanese(msg: &str) -> std::borrow::Cow<'_, str> {
    for (en, ja) in [("Started: ", "開始: "), ("Finished: ", "完了: "), ("Cancelled: ", "キャンセル: "), ("Failed: ", "失敗: ")] {
        if let Some(rest) = msg.strip_prefix(en) {
            return format!("{ja}{}", job_label_in_japanese(rest)).into();
        }
    }
    // "Hardware decoding unavailable for <codec>: <why>. Playback and export decode it in software, which is slower."
    if let Some(rest) = msg.strip_prefix("Hardware decoding unavailable for ")
        && let Some((codec, why)) = rest.split_once(": ")
    {
        let why = why.trim_end_matches(". Playback and export decode it in software, which is slower.");
        let why = match why {
            "Windows has no HEVC decoder; install the HEVC Video Extensions from the Microsoft Store" => {
                "Windows に HEVC デコーダーがありません。Microsoft Store から「HEVC ビデオ拡張機能」をインストールしてください"
            }
            "Windows has no H.264 decoder (an N edition without the Media Feature Pack?)" => {
                "Windows に H.264 デコーダーがありません (Media Feature Pack のない N エディションの可能性があります)"
            }
            other => other,
        };
        return format!("{} のハードウェアデコードを使えません: {why}。再生と書き出しはソフトウェアでデコードするため遅くなります。", codec.to_uppercase())
            .into();
    }
    if msg == "Export queue finished" {
        return "書き出しキューが完了しました".into();
    }
    if let Some(n) = msg.strip_prefix("Export queue finished (").and_then(|r| r.strip_suffix(" failed)")) {
        return format!("書き出しキューが完了しました ({n} 件失敗)").into();
    }
    msg.into()
}

/// A background job's label (as the engine names jobs) in Japanese.
pub(crate) fn job_label_ja(label: &str) -> std::borrow::Cow<'_, str> {
    if crate::i18n::is_japanese() { job_label_in_japanese(label) } else { label.into() }
}

fn job_label_in_japanese(label: &str) -> std::borrow::Cow<'_, str> {
    // "(3 clips)" → "(3 クリップ)"
    let clips = |s: &str| s.replace(" clips)", " クリップ)").replace(" clip)", " クリップ)");
    if label == "Rendering audio previews" {
        return "オーディオプレビューをレンダリング".into();
    }
    if let Some(n) = label.strip_prefix("Rendering ").and_then(|r| r.split_once(" preview segment")).map(|(n, _)| n) {
        return format!("プレビューをレンダリング ({n} セグメント)").into();
    }
    if let Some((mask, dir)) = label.strip_prefix("Track ").and_then(|r| r.rsplit_once(" (")) {
        let dir = if dir.starts_with("backward") { "後方" } else { "前方" };
        return format!("マスクをトラック: {mask} ({dir})").into();
    }
    for (en, ja) in [
        ("Export ", "書き出し: "),
        ("Queue: ", "キュー: "),
        ("Project Manager: ", "プロジェクトマネージャー: "),
        ("Create Proxies ", "プロキシを作成 "),
        ("Scene Edit Detection ", "シーン編集の検出 "),
    ] {
        if let Some(rest) = label.strip_prefix(en) {
            return format!("{ja}{}", clips(rest)).into();
        }
    }
    label.into()
}

/// A job's progress status (as the export pipeline words it) in the UI language.
fn status_ja(s: &str) -> std::borrow::Cow<'_, str> {
    if crate::i18n::is_japanese() { status_in_japanese(s) } else { s.into() }
}

fn status_in_japanese(s: &str) -> std::borrow::Cow<'_, str> {
    if s == "Measuring loudness" {
        return "ラウドネスを測定中".into();
    }
    // "Exporting 125 frames (H.264)"
    if let Some((n, fmt)) = s.strip_prefix("Exporting ").and_then(|r| r.split_once(" frames ")) {
        return format!("{n} フレームを書き出し中 {fmt}").into();
    }
    // "Done in 3.2s"
    if let Some(t) = s.strip_prefix("Done in ") {
        return format!("完了 ({t})").into();
    }
    s.into()
}

fn ago(ms: u64) -> String {
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(ms);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=4 => crate::i18n::t("just now", "たった今").into(),
        5..=59 => crate::tf!("{s} s ago", "{s} 秒前"),
        60..=3599 => crate::tf!("{} min ago", "{} 分前", s / 60),
        _ => crate::tf!("{} h ago", "{} 時間前", s / 3600),
    }
}

pub fn events(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let st = app.ui.panels.events.clone();
    let min = Level::from_name(&st.level).unwrap_or(Level::Info);
    let entries: Vec<LogEntry> = app.session.log.entries.iter().rev().filter(|e| e.level >= min).cloned().collect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 6.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    let mut clear = false;
    let mut level = None;
    let mut select = None;
    // toolbar: [All ▾]  counts …  [Clear All]
    ui.horizontal(|ui| {
        let label = |l: Level| match l {
            Level::Info => "All Events",
            Level::Warning => "Warnings and Errors",
            Level::Error => "Errors Only",
        };
        // shown in the UI language; automation labels stay English
        let shown = |l: Level| match l {
            Level::Info => crate::i18n::t("All Events", "すべてのイベント"),
            Level::Warning => crate::i18n::t("Warnings and Errors", "警告とエラー"),
            Level::Error => crate::i18n::t("Errors Only", "エラーのみ"),
        };
        let r = egui::ComboBox::from_id_salt("events-filter").selected_text(shown(min)).width(150.0).show_ui(ui, |ui| {
            for l in [Level::Info, Level::Warning, Level::Error] {
                let r = ui.selectable_label(min == l, shown(l));
                elems.push((format!("events.filter.{}", l.label().to_ascii_lowercase()), r.rect, label(l).into()));
                if r.clicked() {
                    level = Some(l);
                }
            }
        });
        elems.push(("events.filter".into(), r.response.rect, label(min).into()));
        let log = &app.session.log;
        for l in [Level::Error, Level::Warning, Level::Info] {
            let (gr, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            glyph(ui.painter(), gr.center(), l);
            ui.label(RichText::new(log.count(l).to_string()).color(t.text_dim));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.button(crate::i18n::t("Clear All", "すべて消去"));
            elems.push(("events.clearAll".into(), r.rect, "Clear All".into()));
            clear = r.clicked();
        });
    });
    ui.add_space(4.0);
    let details_h = if st.selected.is_some() { 70.0 } else { 0.0 };
    let list_h = (ui.available_height() - details_h).max(20.0);
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        if entries.is_empty() {
            ui.label(RichText::new(crate::i18n::t("No events.", "イベントはありません。")).color(t.text_faint));
        }
        for (n, e) in entries.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            let selected = st.selected == Some(e.id);
            if selected {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 0.0, t.hover);
            } else if n % 2 == 1 {
                ui.painter().rect_filled(r, 0.0, t.row_alt);
            }
            glyph(ui.painter(), pos2(r.min.x + 10.0, r.center().y), e.level);
            let when = ago(e.time_ms);
            let right = ui.painter().text(pos2(r.max.x - 6.0, r.center().y), Align2::RIGHT_CENTER, &when, Tokens::ui(11.0), t.text_faint);
            let shown_msg = message_ja(&e.message);
            let msg = if e.count > 1 { format!("{shown_msg}  (×{})", e.count) } else { shown_msg.into_owned() };
            let clip = Rect::from_min_max(pos2(r.min.x + 22.0, r.min.y), pos2(right.min.x - 8.0, r.max.y));
            ui.painter().with_clip_rect(clip).text(pos2(clip.min.x, r.center().y), Align2::LEFT_CENTER, &msg, Tokens::ui(12.0), t.text);
            elems.push((format!("events.row.{}", e.id), r, if e.count > 1 { format!("{}  (×{})", e.message, e.count) } else { e.message.clone() }));
            if resp.clicked() {
                select = Some(if selected { None } else { Some(e.id) });
            }
        }
    });
    if let Some(sel) = st.selected.and_then(|id| entries.iter().find(|e| e.id == id)) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), details_h - 4.0), Sense::hover());
        ui.painter().rect_filled(r, 3.0, t.field_bg);
        let level = match sel.level {
            Level::Info => crate::i18n::t("Info", "情報"),
            Level::Warning => crate::i18n::t("Warning", "警告"),
            Level::Error => crate::i18n::t("Error", "エラー"),
        };
        let text = format!("{level} · {} · {}\n{}", sel.source, ago(sel.time_ms), message_ja(&sel.message));
        let g = ui.painter().layout(text, Tokens::ui(11.5), t.text, r.width() - 12.0);
        ui.painter().with_clip_rect(r).galley(r.min + vec2(6.0, 4.0), g, t.text);
        elems.push(("events.details".into(), r, sel.message.clone()));
    }
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    if let Some(l) = level {
        app.ui.panels.events.level = l.label().to_ascii_lowercase();
    }
    if let Some(s) = select {
        app.ui.panels.events.selected = s;
    }
    app.ui.panels.events.seen = app.session.log.entries.back().map(|e| e.id).unwrap_or(0);
    if clear {
        app.ui.panels.events.selected = None;
        if let Err(e) = app.session.execute("events.clear", json!({})) {
            app.ui.status = e.to_string();
        }
    }
}

pub fn progress(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let show_finished = app.ui.panels.progress.show_finished;
    let jobs: Vec<serde_json::Value> = app.session.jobs.iter().rev().map(|j| j.to_json()).collect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 6.0))));
    let ui = &mut child;
    let mut elems: Vec<(String, Rect, String)> = Vec::new();
    let mut cancel = None;
    let mut finished = show_finished;
    let r = ui.checkbox(&mut finished, crate::i18n::t("Show finished jobs", "完了したジョブを表示"));
    elems.push(("progress.showFinished".into(), r.rect, "Show finished jobs".into()));
    ui.add_space(4.0);
    let active = jobs.iter().filter(|j| j["finished"] != json!(true)).count();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut shown = 0;
        for j in &jobs {
            let done = j["finished"] == json!(true);
            if done && !show_finished {
                continue;
            }
            shown += 1;
            let id = j["id"].as_u64().unwrap_or(0);
            let label = j["label"].as_str().unwrap_or("Job");
            let frac = j["progress"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
            ui.painter().rect_filled(r.shrink(1.0), 3.0, t.row_alt);
            ui.painter().text(r.min + vec2(8.0, 6.0), Align2::LEFT_TOP, job_label_ja(label), Tokens::semibold(12.0), t.text);
            let err = j["result"]["error"].as_str();
            let status = match (done, err) {
                (true, Some(e)) if e.to_ascii_lowercase().contains("cancel") => crate::i18n::t("Cancelled", "キャンセルしました").to_string(),
                (true, Some(e)) => crate::tf!("Failed: {e}", "失敗: {e}"),
                (true, None) => crate::i18n::t("Done", "完了").to_string(),
                (false, _) => {
                    let s = j["status"].as_str().unwrap_or("");
                    let n = format!("{} / {}", j["done"].as_u64().unwrap_or(0), j["total"].as_u64().unwrap_or(0));
                    if s.is_empty() { format!("{:.0} %  ({n})", frac * 100.0) } else { format!("{:.0} %  {}", frac * 100.0, status_ja(s)) }
                }
            };
            let bar = Rect::from_min_max(pos2(r.min.x + 8.0, r.min.y + 24.0), pos2(r.max.x - 36.0, r.min.y + 30.0));
            ui.painter().rect_filled(bar, 3.0, t.field_bg);
            let fill = if err.is_some() && done { t.danger } else { t.accent };
            let f = if done && err.is_none() { 1.0 } else { frac };
            ui.painter().rect_filled(Rect::from_min_max(bar.min, pos2(bar.min.x + bar.width() * f, bar.max.y)), 3.0, fill);
            ui.painter().text(
                pos2(r.min.x + 8.0, r.max.y - 4.0),
                Align2::LEFT_BOTTOM,
                &status,
                Tokens::ui(10.5),
                if err.is_some() { t.danger } else { t.text_dim },
            );
            elems.push((format!("progress.job.{id}"), r, format!("{label}: {status}")));
            if !done {
                let cr = Rect::from_center_size(pos2(r.max.x - 18.0, bar.center().y), vec2(20.0, 20.0));
                let resp = ui.interact(cr, egui::Id::new(("progress-cancel", id)), Sense::click());
                let col = if resp.hovered() { t.danger } else { t.icon };
                let s = Stroke::new(1.5, col);
                ui.painter().line_segment([cr.center() + vec2(-5.0, -5.0), cr.center() + vec2(5.0, 5.0)], s);
                ui.painter().line_segment([cr.center() + vec2(5.0, -5.0), cr.center() + vec2(-5.0, 5.0)], s);
                elems.push((format!("progress.cancel.{id}"), cr, format!("Cancel {label}")));
                if resp.on_hover_text(crate::i18n::t("Cancel", "キャンセル")).clicked() {
                    cancel = Some(id);
                }
            }
        }
        if shown == 0 {
            ui.label(
                RichText::new(if active == 0 { crate::i18n::t("No background jobs.", "バックグラウンドジョブはありません。") } else { "" }).color(t.text_faint),
            );
        }
    });
    for (id, r, l) in elems {
        app.auto.add(&id, r, &l);
    }
    app.ui.panels.progress.show_finished = finished;
    if let Some(id) = cancel
        && let Err(e) = app.session.execute("jobs.cancel", json!({"job": id}))
    {
        app.ui.status = e.to_string();
    }
    if active > 0 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's English messages and job names are shown in Japanese (the pure translators;
    /// the UI language itself is process-wide and not switched here).
    #[test]
    fn engine_messages_and_jobs_in_japanese() {
        assert_eq!(message_in_japanese("Started: Export Main Edit.mp4"), "開始: 書き出し: Main Edit.mp4");
        assert_eq!(message_in_japanese("Failed: Create Proxies (3 clips): disk full"), "失敗: プロキシを作成 (3 クリップ): disk full");
        assert_eq!(message_in_japanese("Finished: Rendering 12 preview segments"), "完了: プレビューをレンダリング (12 セグメント)");
        assert_eq!(message_in_japanese("Cancelled: Track Mask 1 (backward)"), "キャンセル: マスクをトラック: Mask 1 (後方)");
        assert_eq!(message_in_japanese("Export queue finished (2 failed)"), "書き出しキューが完了しました (2 件失敗)");
        let hevc = message_in_japanese(
            "Hardware decoding unavailable for hevc: Windows has no HEVC decoder; install the HEVC Video Extensions from the Microsoft Store. Playback and export decode it in software, which is slower.",
        );
        assert!(hevc.starts_with("HEVC のハードウェアデコードを使えません") && hevc.contains("HEVC ビデオ拡張機能"), "{hevc}");
        // anything else is shown as it is, never half translated
        assert_eq!(message_in_japanese("invalid parameters for `x`"), "invalid parameters for `x`");
        assert_eq!(status_in_japanese("Exporting 125 frames (H.264)"), "125 フレームを書き出し中 (H.264)");
        assert_eq!(status_in_japanese("Done in 3.2s"), "完了 (3.2s)");
    }
}
