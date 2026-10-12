//! Text styles for titles and subtitles, and ruby (furigana).
//!
//! **Text styles.** `graphics.textStyle {style, clip?, layer?}` gives a text layer one of the
//! built-in looks in one undo step ("Apply Text Style"): fill, up to two outlines, background and
//! shadow are all set (switching styles leaves nothing of the previous one behind); the text, its
//! font, size and position are not touched. `graphics.textStyles` lists them. The looks are our
//! own: plain outlined subtitles, double-outlined ("fukuro-moji") titles as used on Japanese
//! television, a band, a soft shadow and a glow.
//!
//! **Ruby.** A text layer with the `ruby` property on sets the Aozora Bunko notation as ruby
//! (`漢字《かんじ》`, `｜base《reading》`; `filmcraft_text::ruby`). `graphics.ruby {base, reading,
//! all=false, clip?, layer?}` writes that markup around the first (or every) occurrence of `base`
//! in the layer's text that has no reading yet and turns the property on; `{enabled: bool}`
//! switches the property alone. The markup stays in the text, so readings can also be typed.

use serde_json::{Map, Value, json};

use crate::commands::{CommandSpec, bad, bool_p, has_seq, str_p};
use crate::graphics::{has_graphic, layer_effect_index, set_props, target_clip};
use crate::{EngineError, Result, Session};

/// A built-in look: (id, name, properties).
struct TextStyle {
    id: &'static str,
    name: &'static str,
    /// Fill colour.
    fill: &'static str,
    /// Inner outline: (colour, width).
    stroke: Option<(&'static str, f64)>,
    /// Outer outline: (colour, width).
    stroke2: Option<(&'static str, f64)>,
    /// Background band: (colour, opacity %, padding, corner radius).
    background: Option<(&'static str, f64, f64, f64)>,
    /// Shadow: (colour, opacity %, distance, blur, size).
    shadow: Option<(&'static str, f64, f64, f64, f64)>,
    bold: bool,
}

const SOFT_SHADOW: Option<(&str, f64, f64, f64, f64)> = Some(("#000000", 60.0, 6.0, 12.0, 0.0));

const STYLES: &[TextStyle] = &[
    TextStyle {
        id: "subtitle-white",
        name: "Subtitle: White, Black Outline",
        fill: "#ffffff",
        stroke: Some(("#000000", 10.0)),
        stroke2: None,
        background: None,
        shadow: SOFT_SHADOW,
        bold: true,
    },
    TextStyle {
        id: "subtitle-yellow",
        name: "Subtitle: Yellow, Black Outline",
        fill: "#ffe100",
        stroke: Some(("#000000", 10.0)),
        stroke2: None,
        background: None,
        shadow: SOFT_SHADOW,
        bold: true,
    },
    TextStyle {
        id: "fukuro-red",
        name: "Double Outline: White on Red",
        fill: "#ffffff",
        stroke: Some(("#e60012", 8.0)),
        stroke2: Some(("#ffffff", 16.0)),
        background: None,
        shadow: SOFT_SHADOW,
        bold: true,
    },
    TextStyle {
        id: "fukuro-blue",
        name: "Double Outline: White on Blue",
        fill: "#ffffff",
        stroke: Some(("#0b5ed7", 8.0)),
        stroke2: Some(("#ffffff", 16.0)),
        background: None,
        shadow: SOFT_SHADOW,
        bold: true,
    },
    TextStyle {
        id: "fukuro-pop",
        name: "Double Outline: Pink, White, Black",
        fill: "#ff3b6b",
        stroke: Some(("#ffffff", 8.0)),
        stroke2: Some(("#1a1a1a", 14.0)),
        background: None,
        shadow: None,
        bold: true,
    },
    TextStyle {
        id: "band-navy",
        name: "Band: White on Navy",
        fill: "#ffffff",
        stroke: None,
        stroke2: None,
        background: Some(("#0b3d91", 90.0, 18.0, 4.0)),
        shadow: None,
        bold: true,
    },
    TextStyle {
        id: "soft-shadow",
        name: "Soft Shadow",
        fill: "#ffffff",
        stroke: None,
        stroke2: None,
        background: None,
        shadow: Some(("#000000", 85.0, 4.0, 24.0, 0.0)),
        bold: false,
    },
    TextStyle {
        id: "glow-cyan",
        name: "Glow: Cyan",
        fill: "#ffffff",
        stroke: Some(("#00e5ff", 4.0)),
        stroke2: None,
        background: None,
        shadow: Some(("#00e5ff", 90.0, 0.0, 60.0, 6.0)),
        bold: true,
    },
];

impl TextStyle {
    /// Every appearance property, on or off, so one style fully replaces another.
    fn props(&self) -> Map<String, Value> {
        let mut m = Map::new();
        let mut set = |k: &str, v: Value| {
            m.insert(k.to_string(), v);
        };
        set("fill", json!(true));
        set("fill_color", json!(self.fill));
        set("faux_bold", json!(self.bold));
        set("stroke", json!(self.stroke.is_some()));
        if let Some((c, w)) = self.stroke {
            set("stroke_color", json!(c));
            set("stroke_width", json!(w));
            set("stroke_type", json!("outer"));
        }
        set("stroke2", json!(self.stroke2.is_some()));
        if let Some((c, w)) = self.stroke2 {
            set("stroke2_color", json!(c));
            set("stroke2_width", json!(w));
            set("stroke2_type", json!("outer"));
        }
        set("background", json!(self.background.is_some()));
        if let Some((c, o, size, radius)) = self.background {
            set("background_color", json!(c));
            set("background_opacity", json!(o));
            set("background_size", json!(size));
            set("background_radius", json!(radius));
        }
        set("shadow", json!(self.shadow.is_some()));
        if let Some((c, o, distance, blur, size)) = self.shadow {
            set("shadow_color", json!(c));
            set("shadow_opacity", json!(o));
            set("shadow_distance", json!(distance));
            set("shadow_blur", json!(blur));
            set("shadow_size", json!(size));
        }
        m
    }
}

/// The built-in text styles as (id, name), in menu order.
pub fn style_names() -> Vec<(&'static str, &'static str)> {
    STYLES.iter().map(|t| (t.id, t.name)).collect()
}

/// (clip, effect index) of the text layer a command works on.
fn text_layer(s: &Session, p: &Value, cmd: &str) -> Result<(filmcraft_project::ClipId, usize)> {
    let clip = target_clip(s, p).ok_or_else(|| bad(cmd, "select a graphic clip, or pass `clip`"))?;
    let (_, ei) = layer_effect_index(s, clip, p)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let is_text = q.find_item(clip).and_then(|(_, it)| it.effects.get(ei)).is_some_and(|e| e.effect == filmcraft_project::graphic::TEXT_LAYER);
    if is_text { Ok((clip, ei)) } else { Err(bad(cmd, "the layer is not a text layer")) }
}

fn list(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"styles": STYLES.iter().map(|t| json!({"id": t.id, "name": t.name, "props": t.props()})).collect::<Vec<_>>()}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "graphics.textStyle";
    let id = str_p(p, "style").ok_or_else(|| bad(CMD, "`style` is required (see graphics.textStyles)"))?;
    let style = STYLES.iter().find(|t| t.id == id || t.name.eq_ignore_ascii_case(id)).ok_or_else(|| bad(CMD, format!("no text style `{id}`")))?;
    let (clip, ei) = text_layer(s, p, CMD)?;
    let ph = s.playhead();
    set_props(s, clip, ei, &style.props(), ph, "Apply Text Style")?;
    Ok(json!({"clip": clip.0, "style": style.id, "name": style.name}))
}

/// `text` with `｜base《reading》` around occurrences of `base` that have no reading yet (the
/// first one, or all), and how many were marked.
pub fn add_ruby(text: &str, base: &str, reading: &str, all: bool) -> (String, usize) {
    let mut out = String::with_capacity(text.len() + reading.len() + 8);
    let (mut rest, mut n) = (text, 0usize);
    while let Some(at) = rest.find(base) {
        let (before, tail) = rest.split_at(at);
        let after = tail.get(base.len()..).unwrap_or_default();
        out.push_str(before);
        if after.starts_with('《') || (n > 0 && !all) {
            out.push_str(base);
        } else {
            // the bar goes in unless the kanji-run rule finds exactly this base: all kanji, and
            // no kanji right before it
            let needs_bar = !base.chars().all(filmcraft_text::ruby::is_kanji) || out.chars().next_back().is_some_and(filmcraft_text::ruby::is_kanji);
            if needs_bar {
                out.push('｜');
            }
            out.push_str(base);
            out.push('《');
            out.push_str(reading);
            out.push('》');
            n += 1;
        }
        rest = after;
    }
    out.push_str(rest);
    (out, n)
}

fn ruby(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "graphics.ruby";
    let (clip, ei) = text_layer(s, p, CMD)?;
    let ph = s.playhead();
    let mut props = Map::new();
    let mut marked = 0;
    match (str_p(p, "base"), str_p(p, "reading")) {
        (Some(base), Some(reading)) => {
            let clean = |t: &str| !t.is_empty() && t.chars().count() <= 64 && !t.contains(['《', '》', '｜', '|', '\n', '\r']);
            if !clean(base) || !clean(reading) {
                return Err(bad(CMD, "`base` and `reading` must be 1 to 64 characters without 《 》 ｜ | or line breaks"));
            }
            let text = {
                let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                let (_, it) = q.find_item(clip).ok_or_else(|| bad(CMD, "no such clip"))?;
                let mt = it.source_time_at(ph.clamp(it.start, it.end() - filmcraft_time::Tick(1)));
                match it.effects.get(ei).and_then(|e| e.param("text")).map(|t| t.value_at(mt)) {
                    Some(filmcraft_project::ParamValue::Text(t)) => t,
                    _ => String::new(),
                }
            };
            let (next, n) = add_ruby(&text, base, reading, bool_p(p, "all").unwrap_or(false));
            if n == 0 {
                return Err(bad(CMD, format!("`{base}` is not in the text, or already has a reading")));
            }
            marked = n;
            props.insert("text".into(), json!(next));
            props.insert("ruby".into(), json!(bool_p(p, "enabled").unwrap_or(true)));
        }
        (None, None) => {
            let on = bool_p(p, "enabled").ok_or_else(|| bad(CMD, "pass `base` and `reading`, or `enabled`"))?;
            props.insert("ruby".into(), json!(on));
        }
        _ => return Err(bad(CMD, "`base` and `reading` go together")),
    }
    let enabled = props.get("ruby").and_then(Value::as_bool).unwrap_or(false);
    set_props(s, clip, ei, &props, ph, "Ruby")?;
    Ok(json!({"clip": clip.0, "marked": marked, "enabled": enabled}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "graphics.textStyle",
            label: "Apply Text Style",
            menu: &[],
            shortcut: None,
            params: r#"{"style":"subtitle-white|subtitle-yellow|fukuro-red|fukuro-blue|fukuro-pop|band-navy|soft-shadow|glow-cyan","clip":id?,"layer":n|name?}"#,
            enabled: has_graphic,
            run: apply,
            journal: true,
        },
        CommandSpec {
            id: "graphics.textStyles",
            label: "List Text Styles",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: has_seq,
            run: list,
            journal: false,
        },
        CommandSpec {
            id: "graphics.ruby",
            label: "Ruby (Furigana)",
            menu: &[],
            shortcut: None,
            params: r#"{"base":str,"reading":str,"all":bool=false,"clip":id?,"layer":n|name?}|{"enabled":bool}"#,
            enabled: has_graphic,
            run: ruby,
            journal: true,
        },
    ]
}
