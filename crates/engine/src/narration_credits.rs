//! Narration credits in the video: the voices a project's narration uses, as a rolling credit
//! graphic and as text for the video description.
//!
//! | command | does |
//! |---|---|
//! | `tts.credits {sequence?, scope?}` | the distinct credits of the narration used in the sequence (or the whole project), with `creditText` |
//! | `tts.addCreditRoll {seconds?, title?, extra?, at?}` | adds (or updates) a rolling "Narration Credits" graphic, by default at the sequence end; one undo step |
//! | `tts.exportCredits {path?}` | writes the credits (and AivisSpeech model licences) to a UTF-8 text file for the description |
//!
//! **Where the voices are remembered.** `tts.synthesize` writes two metadata fields on every
//! narration WAV item it imports (`ProjectItem::metadata`, saved with the project like any other
//! metadata, so no format change):
//! - `Narration Credit`: the credit line (`VOICEVOX:ずんだもん`). Editing it in the Metadata panel
//!   changes what the roll and the description text say.
//! - `Narration Voice`: the credit as JSON (engine, speaker, uuid, model, licence, URLs), the
//!   same object `tts.synthesize` returns in `credits`.
//!
//! Credits are collected from the items **still used in the sequence** (clips on its tracks,
//! nested sequences included): narration you deleted from the timeline needs no credit.
//! `scope: "project"` collects from every narration item in the project instead.
//!
//! The roll reuses the graphics machinery: a graphic clip named [`ROLL_NAME`] with one centred
//! text layer (heading + one credit per line) and Roll set to start and end off screen. Commands
//! update that clip instead of adding a second one.

use serde_json::{Value, json};

use filmcraft_geom::Vec2;
use filmcraft_project::graphic_design::{GraphicMeta, Roll, RollMode};
use filmcraft_project::{ClipId, ItemId, ItemKind, Param, ParamValue, Project};
use filmcraft_time::Tick;

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, str_p, time_p};
use crate::{EngineError, Result, Session};

/// Metadata field with the credit line of a narration item.
pub const CREDIT_KEY: &str = "Narration Credit";
/// Metadata field with the credit as JSON.
pub const VOICE_KEY: &str = "Narration Voice";
/// Name of the credit-roll graphic clip these commands make and update.
pub const ROLL_NAME: &str = "Narration Credits";
/// Undo label of `tts.addCreditRoll`.
pub const ROLL_UNDO: &str = "Narration Credit Roll";
/// Deepest nesting of sequences followed when collecting credits.
const MAX_NEST: usize = 16;
/// Most extra lines / characters a roll takes.
const MAX_EXTRA: usize = 50;
const MAX_LINE_CHARS: usize = 500;

// ------------------------------------------------------------------------------------- record

/// The credit of one line of a `tts.synthesize` result: the `credits` entry of its voice.
fn credit_of_line<'a>(line: &Value, credits: &'a [Value]) -> Option<&'a Value> {
    let v = &line["voice"];
    let uuid = v["uuid"].as_str().unwrap_or_default();
    let speaker = v["speaker"].as_str().unwrap_or_default();
    let engine = v["engine"].as_str().unwrap_or_default();
    credits.iter().find(|c| {
        let same_engine = engine.is_empty() || c["engine"].as_str() == Some(engine);
        let same_voice = if uuid.is_empty() { c["speaker"].as_str() == Some(speaker) } else { c["uuid"].as_str() == Some(uuid) };
        same_engine && same_voice
    })
}

/// After `tts.synthesize` placed its lines (undo history was `n0` long before): remember each
/// item's voice and add / update the credit roll, folded into the same undo step. Failures of the
/// roll are reported in the result (`creditRollError`); the narration itself stays.
pub(crate) fn after_synthesize(s: &mut Session, p: &Value, n0: usize, result: &mut Value) -> Result<()> {
    let credits = result["credits"].as_array().cloned().unwrap_or_default();
    let mut marks: Vec<(ItemId, Value)> = Vec::new();
    for l in result["lines"].as_array().map(Vec::as_slice).unwrap_or_default() {
        if let (Some(item), Some(c)) = (l["item"].as_u64(), credit_of_line(l, &credits)) {
            marks.push((ItemId(item), c.clone()));
        }
    }
    if !marks.is_empty() {
        s.edit(crate::narration::UNDO_LABEL, |pr, _| {
            for (item, c) in &marks {
                if let Some(it) = pr.item_mut(*item) {
                    if let Some(line) = c["credit"].as_str() {
                        it.metadata.insert(CREDIT_KEY.into(), line.to_string());
                    }
                    it.metadata.insert(VOICE_KEY.into(), c.to_string());
                }
            }
            Ok(())
        })?;
    }
    // the roll: added when asked (or Settings ▸ Narration ▸ credit roll), kept up to date when
    // one exists, unless `creditRoll: false`
    let want = bool_p(p, "creditRoll");
    let add = want.unwrap_or(s.prefs.narration.credit_roll);
    let exists = find_roll(s).is_some();
    if add || (exists && want != Some(false)) {
        let selection = s.state.selection.clone();
        match credit_roll(s, &json!({})) {
            Ok(v) => result["creditRoll"] = v,
            Err(e) => result["creditRollError"] = json!(e.to_string()),
        }
        s.state.selection = selection;
    }
    crate::clip_ops::collapse_history(s, n0, crate::narration::UNDO_LABEL);
    Ok(())
}

// ------------------------------------------------------------------------------------- collect

/// The credit an item carries (its metadata), if it is narration.
fn item_credit(p: &Project, id: ItemId) -> Option<Value> {
    let it = p.item(id)?;
    let line = it.metadata.get(CREDIT_KEY).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let json = it.metadata.get(VOICE_KEY).and_then(|s| serde_json::from_str::<Value>(s).ok()).filter(Value::is_object);
    match (json, line) {
        (Some(mut j), Some(l)) => {
            j["credit"] = json!(l);
            Some(j)
        }
        (Some(j), None) => j["credit"].as_str().is_some_and(|c| !c.trim().is_empty()).then_some(j),
        (None, Some(l)) => Some(json!({"credit": l})),
        (None, None) => None,
    }
}

/// (start, item) of every clip in sequence `seq`, nested sequences followed (`base` = where the
/// nest starts on the outer timeline).
fn used(p: &Project, seq: ItemId, base: Tick, depth: usize, out: &mut Vec<(Tick, ItemId)>, stack: &mut Vec<ItemId>) {
    if depth > MAX_NEST || stack.contains(&seq) {
        return;
    }
    let Some(q) = p.sequence(seq) else { return };
    stack.push(seq);
    for t in q.all_tracks() {
        for it in t.items.iter().filter(|i| i.enabled) {
            let at = Tick(base.0.saturating_add(it.start.0));
            match p.item(it.item).map(|x| &x.kind) {
                Some(ItemKind::Sequence(_)) => used(p, it.item, at, depth + 1, out, stack),
                Some(ItemKind::Subclip { parent, .. }) => out.push((at, *parent)),
                _ => out.push((at, it.item)),
            }
        }
    }
    stack.pop();
}

fn dedup_key(c: &Value) -> String {
    let engine = c["engine"].as_str().unwrap_or_default();
    match c["uuid"].as_str().filter(|u| !u.is_empty()) {
        Some(u) => format!("{engine}\u{1}{u}"),
        None => format!("\u{2}{}", c["credit"].as_str().unwrap_or_default()),
    }
}

/// The distinct credits of the narration in sequence `seq` (in timeline order), or of every
/// narration item in the project when `seq` is None (in import order).
pub fn collect(p: &Project, seq: Option<ItemId>) -> Vec<Value> {
    let items: Vec<ItemId> = match seq {
        Some(q) => {
            let mut v = Vec::new();
            used(p, q, Tick::ZERO, 0, &mut v, &mut Vec::new());
            v.sort_by_key(|(t, _)| *t);
            v.into_iter().map(|(_, i)| i).collect()
        }
        None => {
            let mut v: Vec<(u64, ItemId)> = p.items.values().map(|i| (i.created, i.id)).collect();
            v.sort();
            v.into_iter().map(|(_, i)| i).collect()
        }
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in items {
        if let Some(c) = item_credit(p, id)
            && seen.insert(dedup_key(&c))
        {
            out.push(c);
        }
    }
    out
}

/// The credit lines, one per voice.
pub fn credit_text(credits: &[Value]) -> String {
    credits.iter().filter_map(|c| c["credit"].as_str()).collect::<Vec<_>>().join("\n")
}

fn credits_p(s: &Session, p: &Value, cmd: &str) -> Result<Vec<Value>> {
    match str_p(p, "scope") {
        Some("project") => Ok(collect(&s.project, None)),
        None | Some("sequence") => {
            let seq = crate::export_tools::sequence_param(s, p, cmd)?;
            Ok(collect(&s.project, Some(seq)))
        }
        Some(o) => Err(bad(cmd, format!("unknown scope `{o}` (sequence or project)"))),
    }
}

fn list_credits(s: &mut Session, p: &Value) -> Result<Value> {
    let c = credits_p(s, p, "tts.credits")?;
    Ok(json!({"creditText": credit_text(&c), "credits": c}))
}

// ------------------------------------------------------------------------------------- roll

/// The heading of the roll and of the description text, in the interface language.
fn heading(s: &Session) -> &'static str {
    if s.japanese_text { "音声" } else { "Voices" }
}

fn has_cjk(t: &str) -> bool {
    t.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{ff00}'..='\u{ffef}').contains(&c))
}

/// The font of the roll: the default text font; on Automatic, the Japanese face when the text
/// has Japanese in it (voice names usually do), so it renders whatever the interface language.
fn roll_font(s: &Session, text: &str) -> String {
    if s.prefs.graphics.default_font.trim().is_empty() && has_cjk(text) {
        return filmcraft_text::fonts::JAPANESE_FAMILY.to_string();
    }
    s.default_text_font()
}

/// The credit-roll clip of the active sequence: a graphic clip named [`ROLL_NAME`].
fn find_roll(s: &Session) -> Option<ClipId> {
    let q = s.active_sequence()?;
    q.video_tracks.iter().flat_map(|t| &t.items).find(|i| i.name == ROLL_NAME && crate::graphics::is_graphic(s, q, i.id)).map(|i| i.id)
}

/// Where the media of the active sequence ends, leaving out `skip`.
fn media_end(s: &Session, skip: Option<ClipId>) -> Tick {
    s.active_sequence()
        .map(|q| q.all_tracks().flat_map(|t| &t.items).filter(|i| Some(i.id) != skip).map(|i| i.end()).max().unwrap_or(Tick::ZERO))
        .unwrap_or(Tick::ZERO)
}

fn roll_text(s: &Session, p: &Value, credits: &[Value]) -> Result<String> {
    const CMD: &str = "tts.addCreditRoll";
    let clip = |t: &str| t.chars().take(MAX_LINE_CHARS).collect::<String>();
    let title = str_p(p, "title").map(|t| clip(t.trim())).unwrap_or_else(|| heading(s).to_string());
    let mut lines: Vec<String> = Vec::new();
    if !title.is_empty() {
        lines.push(title);
        lines.push(String::new());
    }
    lines.extend(credits.iter().filter_map(|c| c["credit"].as_str()).map(clip));
    match p.get("extra") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            if a.len() > MAX_EXTRA {
                return Err(bad(CMD, format!("at most {MAX_EXTRA} extra lines")));
            }
            let extra: Vec<String> = a.iter().map(|v| v.as_str().map(clip).ok_or_else(|| bad(CMD, "`extra` holds strings"))).collect::<Result<_>>()?;
            if !extra.is_empty() {
                lines.push(String::new());
                lines.extend(extra);
            }
        }
        Some(Value::String(t)) => {
            lines.push(String::new());
            lines.extend(t.lines().take(MAX_EXTRA).map(clip));
        }
        Some(_) => return Err(bad(CMD, "`extra` is a list of lines")),
    }
    Ok(lines.join("\n"))
}

/// Add or update the credit roll (see [`ROLL_NAME`]); one undo step.
fn credit_roll(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.addCreditRoll";
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let credits = collect(&s.project, Some(seq));
    if credits.is_empty() {
        return Err(bad(CMD, "this sequence has no narration made with Generate Narration (tts.synthesize), so there are no voices to credit"));
    }
    let text = roll_text(s, p, &credits)?;
    let font = roll_font(s, &text);
    let (w, h, rate) = s.active_sequence().map(|q| (q.settings.width, q.settings.height, q.settings.frame_rate)).ok_or(EngineError::NoSequence)?;
    let lines = text.lines().count();
    let seconds = match f64_p(p, "seconds") {
        Some(x) if x.is_finite() && x > 0.0 => x.min(600.0),
        Some(_) => return Err(bad(CMD, "`seconds` must be more than 0")),
        None => (4.0 + 0.5 * lines as f64).clamp(5.0, 30.0),
    };
    let dur = rate.snap_nearest(Tick::from_seconds_f64(seconds)).max(rate.frame_duration());
    let existing = find_roll(s);
    let at_end = matches!(p.get("at"), None | Some(Value::Null)) || str_p(p, "at") == Some("end");
    let at = if at_end {
        frame_ceil(rate, media_end(s, existing))
    } else {
        let t = match p.get("at") {
            Some(Value::Number(n)) => n.as_i64().map(Tick),
            Some(v @ Value::Object(_)) => time_p(s, v, ""),
            Some(Value::String(x)) => x.parse::<f64>().ok().filter(|x| x.is_finite()).map(Tick::from_seconds_f64),
            _ => None,
        };
        let t = t.ok_or_else(|| bad(CMD, "`at` is \"end\", ticks, seconds as a string, or {seconds|frame|timecode}"))?;
        rate.snap(t.clamp(Tick::ZERO, Tick::MAX))
    };
    if let Some(clip) = existing {
        // update the text (and font); move it to the new end / `at`, and resize when asked
        let new_dur = f64_p(p, "seconds").map(|_| dur);
        let moved = s.edit_sequence(ROLL_UNDO, |q, _, st| {
            let (tid, it) = q.find_item(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
            let d = new_dur.unwrap_or(it.duration);
            let range = filmcraft_time::TimeRange::new(at, d);
            let free = q.track(tid).is_some_and(|t| !t.items.iter().any(|i| i.id != clip && i.range().overlaps(&range)));
            let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
            let layer = filmcraft_project::graphic::layer_indices(&it.effects).first().copied();
            let e = layer.and_then(|l| it.effects.get_mut(l)).ok_or_else(|| bad(CMD, "the credit roll has no text layer"))?;
            e.params.insert("text".into(), Param::new(ParamValue::Text(text.clone())));
            e.params.insert("font".into(), Param::new(ParamValue::Text(font.clone())));
            let moved = free && (it.start != at || it.duration != d);
            if moved {
                it.start = at;
                it.duration = d;
            }
            if let Some(t) = q.track_mut(tid) {
                t.sort();
            }
            q.check().map_err(EngineError::Other)?;
            st.selection = vec![clip];
            Ok(moved)
        })?;
        let it = s.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, i)| (i.start, i.duration)).unwrap_or((at, dur));
        return Ok(json!({"clip": clip.0, "updated": true, "moved": moved, "start": it.0.0, "duration": it.1.0, "text": text, "credits": credits}));
    }
    let size = (h as f64 / 18.0).clamp(8.0, 400.0);
    let mut layer = filmcraft_project::graphic::new_text_layer(&text, Vec2::new(w as f64 / 2.0, h as f64 / 2.0), size);
    layer.params.insert("font".into(), Param::new(ParamValue::Text(font)));
    // Alignment: Center
    layer.params.insert("align".into(), Param::new(ParamValue::Choice(1)));
    layer.params.insert("name".into(), Param::new(ParamValue::Text(ROLL_NAME.into())));
    let meta = GraphicMeta { roll: Roll { mode: RollMode::Roll, start_off_screen: true, end_off_screen: true, ..Default::default() }, ..Default::default() };
    let place = json!({"time": at.0});
    let clip = crate::graphics::place_video_clip(s, ROLL_NAME, &place, ROLL_UNDO, vec![layer], Some(Box::new(meta)), move |pr, (w, h, rate)| {
        (crate::graphics::graphic_source(pr, w, h, rate), dur)
    })?;
    Ok(json!({"clip": clip.0, "updated": false, "moved": false, "start": at.0, "duration": dur.0, "text": text, "credits": credits}))
}

/// The first frame boundary at or after `t`.
fn frame_ceil(rate: filmcraft_time::FrameRate, t: Tick) -> Tick {
    let f = rate.snap(t);
    if f < t { Tick(f.0.saturating_add(rate.frame_duration().0)) } else { f }
}

// ------------------------------------------------------------------------------------- export

/// The description text: the credit lines, each AivisSpeech model's name / licence / page
/// indented under its voice.
pub fn description_text(credits: &[Value], japanese: bool) -> String {
    let mut out = String::new();
    out.push_str(if japanese { "音声:\n" } else { "Voices:\n" });
    for c in credits {
        let Some(line) = c["credit"].as_str() else { continue };
        out.push_str(line);
        out.push('\n');
        let model = c["model"].as_str().filter(|m| !m.is_empty());
        let license = c["license"].as_str().and_then(|l| l.lines().next()).map(str::trim).filter(|l| !l.is_empty());
        let url = c["modelUrl"].as_str().filter(|u| !u.is_empty());
        let mut parts = Vec::new();
        if let Some(m) = model {
            parts.push(if japanese { format!("モデル: {m}") } else { format!("Model: {m}") });
        }
        if let Some(l) = license {
            parts.push(if japanese { format!("ライセンス: {l}") } else { format!("Licence: {l}") });
        }
        if let Some(u) = url {
            parts.push(u.to_string());
        }
        if !parts.is_empty() {
            out.push_str("  ");
            out.push_str(&parts.join(" / "));
            out.push('\n');
        }
    }
    out
}

/// Default file of `tts.exportCredits`: `<project name>_credits.txt` next to the project, else
/// `credits.txt` in the narration folder.
fn default_path(s: &Session) -> String {
    if let Some(path) = s.path.as_deref().map(std::path::Path::new)
        && let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
    {
        let stem = path.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "Project".into());
        return dir.join(format!("{stem}_credits.txt")).to_string_lossy().into_owned();
    }
    std::path::Path::new(&crate::narration::narration_dir(s, &Value::Null)).join("credits.txt").to_string_lossy().into_owned()
}

fn export_credits(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.exportCredits";
    let credits = credits_p(s, p, CMD)?;
    if credits.is_empty() {
        return Err(bad(CMD, "no narration made with Generate Narration (tts.synthesize) is used, so there are no voices to credit"));
    }
    let path = match str_p(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => x.to_string(),
        None => default_path(s),
    };
    let text = description_text(&credits, s.japanese_text);
    if !cfg!(target_arch = "wasm32")
        && let Some(dir) = std::path::Path::new(&path).parent().filter(|d| !d.as_os_str().is_empty())
    {
        let _ = std::fs::create_dir_all(dir);
    }
    s.services.write_file(&path, text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"path": path, "text": text, "creditText": credit_text(&credits), "credits": credits}))
}

// ------------------------------------------------------------------------------------- commands

fn add_roll(s: &mut Session, p: &Value) -> Result<Value> {
    credit_roll(s, p)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "tts.credits",
            label: "Narration Credits",
            menu: &[],
            shortcut: None,
            params: r#"{"sequence":id?,"scope":"sequence|project"="sequence"}"#,
            enabled: crate::commands::always,
            run: list_credits,
            journal: false,
        },
        CommandSpec {
            id: "tts.addCreditRoll",
            label: "Add Narration Credit Roll",
            menu: &["Graphics and Titles"],
            shortcut: None,
            params: r#"{"seconds":f64?,"title":str?,"extra":[str]?,"at":"end"|ticks|{seconds|frame|timecode}="end"}"#,
            enabled: has_seq,
            run: add_roll,
            journal: true,
        },
        CommandSpec {
            id: "tts.exportCredits",
            label: "Export Narration Credits",
            menu: &["Graphics and Titles"],
            shortcut: None,
            params: r#"{"path":str?,"sequence":id?,"scope":"sequence|project"="sequence"}"#,
            enabled: has_seq,
            run: export_credits,
            journal: false,
        },
    ]
}
