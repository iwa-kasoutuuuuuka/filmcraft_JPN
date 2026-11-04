//! Animated chart commands: `graphics.newChart` places a graphic clip holding a chart,
//! `graphics.setChart` changes one, `graphics.chart` reads one back (see
//! `filmcraft_project::chart`).
//!
//! A chart is given as `chart` (the whole [`ChartSpec`] as JSON), and / or piecewise: `kind`
//! (`barRace`, `bar`, `column`, `line`, `area`, `pie`, `donut`), `table` (a spreadsheet paste or
//! CSV: a corner cell and the step names, then one row per category), `title`, `theme`
//! (`dark` / `light`) and `palette` (a built-in palette's name). The pieces apply over `chart`, or
//! over the clip's current chart for `graphics.setChart`.

use filmcraft_project::ClipId;
use filmcraft_project::chart::{ChartKind, ChartSpec, ChartTheme, PALETTES};
use filmcraft_project::graphic_design::GraphicMeta;
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, f64_p, has_seq, str_p};
use crate::graphics::{graphic_source, place_video_clip, target_clip};
use crate::{EngineError, Result, Session};

/// `spec` with the piecewise parameters of `p` applied.
fn apply_params(cmd: &str, mut spec: ChartSpec, p: &Value) -> Result<ChartSpec> {
    if let Some(v) = p.get("chart").filter(|v| !v.is_null()) {
        spec = serde_json::from_value(v.clone()).map_err(|e| bad(cmd, format!("chart: {e}")))?;
    }
    if let Some(k) = str_p(p, "kind") {
        spec.kind = ChartKind::from_id(k).ok_or_else(|| bad(cmd, format!("unknown chart kind `{k}`")))?;
    }
    if let Some(t) = str_p(p, "table") {
        let (categories, steps, values) = ChartSpec::parse_table(t).map_err(|e| bad(cmd, format!("table: {e}")))?;
        spec.categories = categories;
        spec.steps = steps;
        spec.values = values;
    }
    if let Some(t) = str_p(p, "title") {
        spec.title = t.to_string();
    }
    if let Some(t) = str_p(p, "theme") {
        spec.theme = match t.to_ascii_lowercase().as_str() {
            "dark" => ChartTheme::Dark,
            "light" => ChartTheme::Light,
            _ => return Err(bad(cmd, format!("unknown theme `{t}` (dark or light)"))),
        };
    }
    if let Some(name) = str_p(p, "palette") {
        let (_, colors) = PALETTES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).ok_or_else(|| bad(cmd, format!("unknown palette `{name}`")))?;
        spec.palette = colors.to_vec();
    }
    Ok(spec.sanitized())
}

/// The chart of graphic clip `clip` in the active sequence.
pub fn clip_chart(s: &Session, clip: ClipId) -> Option<ChartSpec> {
    let (_, it) = s.active_sequence()?.find_item(clip)?;
    it.graphic.as_deref()?.chart.as_deref().cloned()
}

/// Whether the target clip has a chart (for `enabled`).
fn has_chart(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    target_clip(s, &Value::Null).and_then(|c| clip_chart(s, c)).map(|_| ()).ok_or_else(|| "select a chart clip".into())
}

/// The chart clip a command acts on: `clip`, else the selected / topmost graphic clip holding a
/// chart.
fn chart_target(s: &Session, p: &Value, cmd: &str) -> Result<ClipId> {
    let c = target_clip(s, p).ok_or_else(|| bad(cmd, "select a chart clip"))?;
    clip_chart(s, c).map(|_| c).ok_or_else(|| bad(cmd, "the clip has no chart"))
}

fn new_chart(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "graphics.newChart";
    let kind = str_p(p, "kind").and_then(ChartKind::from_id).unwrap_or_default();
    let spec = apply_params(cmd, ChartSpec::sample(kind), p)?;
    let seconds = f64_p(p, "seconds").filter(|v| v.is_finite()).unwrap_or(8.0).clamp(0.1, 3600.0);
    let name = if spec.title.trim().is_empty() { "Chart".to_string() } else { spec.title.chars().take(60).collect() };
    let meta = GraphicMeta { chart: Some(Box::new(spec)), ..GraphicMeta::default() };
    let clip = place_video_clip(s, &name, p, "New Chart", Vec::new(), Some(Box::new(meta)), move |pr, (w, h, rate)| {
        let src = graphic_source(pr, w, h, rate);
        (src, rate.snap_nearest(Tick::from_seconds_f64(seconds)).max(rate.frame_duration()))
    })?;
    Ok(json!({ "clip": clip.0 }))
}

fn set_chart(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "graphics.setChart";
    let clip = chart_target(s, p, cmd)?;
    let base = clip_chart(s, clip).unwrap_or_default();
    let spec = apply_params(cmd, base, p)?;
    let out = serde_json::to_value(&spec).unwrap_or(Value::Null);
    s.edit_sequence("Edit Chart", move |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
        it.graphic.get_or_insert_with(Default::default).chart = Some(Box::new(spec));
        Ok::<(), EngineError>(())
    })?;
    Ok(json!({ "clip": clip.0, "chart": out }))
}

fn get_chart(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "graphics.chart";
    let clip = chart_target(s, p, cmd)?;
    let spec = clip_chart(s, clip).unwrap_or_default();
    Ok(json!({ "clip": clip.0, "chart": serde_json::to_value(&spec).unwrap_or(Value::Null), "table": spec.table_text() }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "graphics.newChart",
            label: "Animated Chart…",
            menu: &["Graphics and Titles", "New Layer"],
            shortcut: None,
            params: r#"{"kind":"barRace|bar|column|line|area|pie|donut"?,"chart":object?,"table":str?,"title":str?,"theme":"dark|light"?,"palette":str?,"seconds":f64=8,"track":index?,"time":ticks?}"#,
            enabled: has_seq,
            run: new_chart,
            journal: true,
        },
        CommandSpec {
            id: "graphics.setChart",
            label: "Edit Chart…",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?,"kind":"barRace|bar|column|line|area|pie|donut"?,"chart":object?,"table":str?,"title":str?,"theme":"dark|light"?,"palette":str?}"#,
            enabled: has_chart,
            run: set_chart,
            journal: true,
        },
        CommandSpec {
            id: "graphics.chart",
            label: "Chart",
            menu: &[],
            shortcut: None,
            params: r#"{"clip":id?}"#,
            enabled: always,
            run: get_chart,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
