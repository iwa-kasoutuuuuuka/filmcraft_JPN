//! Narration credits in the video: the voices a project's narration uses, as a rolling credit
//! graphic and as text for the video description.
//!
//! | command | does |
//! |---|---|
//! | `tts.credits {sequence?, scope?}` | the distinct credits of the narration used in the sequence (or the whole project), with `creditText` |
//! | `tts.addCreditRoll {seconds?, title?, extra?, at?}` | adds (or updates) a rolling "Narration Credits" graphic, a new one at the sequence end; one undo step, none when nothing changes |
//! | `tts.exportCredits {path?}` | writes the credits (and AivisSpeech model licences) to a UTF-8 text file for the description |
//!
//! **Where the voices are remembered.** `tts.synthesize` writes two metadata fields on every
//! narration WAV item it imports (`ProjectItem::metadata`, saved with the project like any other
//! metadata, so no format change):
//! - `Narration Credit`: the credit line (`VOICEVOX:ずんだもん`). Editing it in the Metadata panel
//!   changes what the roll and the description text say; clearing it removes the credit.
//! - `Narration Voice`: the credit as JSON (engine, speaker, uuid, model, licence, URLs), the
//!   same object `tts.synthesize` returns in `credits`.
//!
//! Credits are collected from the items **still used in the sequence** (clips on its tracks,
//! nested sequences included, each nested sequence walked once): narration you deleted from the
//! timeline needs no credit. `scope: "project"` collects from every narration item instead.
//!
//! **The roll** reuses the graphics machinery: a graphic clip with one centred text layer
//! (heading + one credit per line + extra lines) and Roll set to start and end off screen. Its
//! text layer carries the parameter [`ROLL_TAG`] (not shown in any panel; it survives renaming,
//! copy and paste) holding what was generated: title, extra lines, text, font and the automatic
//! duration. Commands update the tagged clip instead of adding a second one (a clip named
//! [`ROLL_NAME`] without the tag, made by an earlier version, is adopted). An automatic update
//! after Generate Narration keeps what the user changed: edited text is left alone (`skipped:
//! "edited"`), a font the user picked stays, a roll the user moved stays where it is, and a
//! roll on a locked track is not touched (`skipped: "locked"`).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_geom::Vec2;
use filmcraft_project::graphic_design::{GraphicMeta, Roll, RollMode};
use filmcraft_project::{ClipId, ItemId, ItemKind, Param, ParamValue, Project, TrackItem};
use filmcraft_time::{FrameRate, Tick, TimeRange};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, str_p, time_p};
use crate::{EngineError, Result, Session};

/// Metadata field with the credit line of a narration item.
pub const CREDIT_KEY: &str = "Narration Credit";
/// Metadata field with the credit as JSON.
pub const VOICE_KEY: &str = "Narration Voice";
/// Name of the credit-roll graphic clip these commands make.
pub const ROLL_NAME: &str = "Narration Credits";
/// Text-layer parameter that marks the credit roll and holds what was generated (JSON).
pub const ROLL_TAG: &str = "narrationCredits";
/// Undo label of `tts.addCreditRoll`.
pub const ROLL_UNDO: &str = "Narration Credit Roll";
/// Deepest nesting of sequences followed when collecting credits.
const MAX_NEST: usize = 16;
/// Most distinct narration items remembered per sequence while collecting.
const MAX_ENTRIES: usize = 4096;
/// Most credits collected.
const MAX_CREDITS: usize = 500;
/// Most extra lines / characters a roll takes.
const MAX_EXTRA: usize = 50;
const MAX_LINE_CHARS: usize = 500;
/// Latest `at` accepted: this long after the media end.
const MAX_AT_PAST_END_SECONDS: f64 = 3600.0;

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
    // the project before this synthesis (the snapshot its first edit pushed), for "was the roll
    // at the end"; only while the history has not been trimmed since
    let before = (s.history.undo.len() < s.history.limit).then(|| s.history.undo.get(n0).map(|(_, p)| p.clone())).flatten();
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
                mark_item(pr, *item, c);
            }
            Ok(())
        })?;
    }
    // the roll: added when asked (or Settings ▸ Narration ▸ credit roll), kept up to date when
    // one exists, unless `creditRoll: false`
    let want = bool_p(p, "creditRoll");
    let add = want.unwrap_or(s.prefs.narration.credit_roll);
    let exists = !find_rolls(s).is_empty();
    if add || (exists && want != Some(false)) {
        let selection = s.state.selection.clone();
        let old_end = match (&before, s.state.active_sequence) {
            (Some(pr), Some(seq)) => {
                let roll = find_rolls(s).first().copied();
                Some(media_end_in(pr, seq, roll))
            }
            _ => None,
        };
        match credit_roll(s, &json!({}), Mode::Auto { old_end }) {
            Ok(v) => result["creditRoll"] = v,
            Err(e) => result["creditRollError"] = json!(e.to_string()),
        }
        s.state.selection = selection;
    }
    crate::clip_ops::collapse_history(s, n0, crate::narration::UNDO_LABEL);
    Ok(())
}

/// Remember on item `id` the voice credit `c` (a [`filmcraft_speech::tts::credit`]): the
/// `Narration Credit` line and the whole credit as `Narration Voice`. Every route that makes
/// narration through a local engine (`tts.synthesize`, `speech.synthesize`, `anime.build` from
/// `script.synthesize`'s lines) records it the same way, so the credit commands find it.
pub(crate) fn mark_item(pr: &mut Project, id: ItemId, c: &Value) {
    if let Some(it) = pr.item_mut(id) {
        if let Some(line) = c["credit"].as_str() {
            it.metadata.insert(CREDIT_KEY.into(), line.to_string());
        }
        it.metadata.insert(VOICE_KEY.into(), c.to_string());
    }
}

/// The distinct credits among `credits` (one per voice, first seen first).
pub(crate) fn distinct<'a>(credits: impl IntoIterator<Item = &'a Value>) -> Vec<Value> {
    let mut seen = HashSet::new();
    credits.into_iter().filter(|c| c.is_object() && seen.insert(dedup_key(c))).take(MAX_CREDITS).cloned().collect()
}

// ------------------------------------------------------------------------------------- collect

/// The credit an item carries (its metadata), if it is narration. `tts.synthesize` always
/// writes both fields; the `Narration Credit` line decides: cleared (the Metadata panel removes an
/// emptied field), the item is not credited, whatever `Narration Voice` says.
fn item_credit(p: &Project, id: ItemId) -> Option<Value> {
    let it = p.item(id)?;
    let json = it.metadata.get(VOICE_KEY).and_then(|s| serde_json::from_str::<Value>(s).ok()).filter(Value::is_object);
    match it.metadata.get(CREDIT_KEY).map(|s| s.trim()) {
        Some("") => None,
        Some(l) => Some(match json {
            Some(mut j) => {
                j["credit"] = json!(l);
                j
            }
            None => json!({"credit": l}),
        }),
        None => None,
    }
}

/// Narration items of a sequence with the earliest time each is used at (relative to the
/// sequence start).
type Offsets = HashMap<ItemId, Tick>;

/// Walks sequences once each (nested sequences memoised), so a sequence nesting 1000 clips of a
/// sequence nesting 1000 clips costs 2000 clip visits, not a million.
struct Walk<'a> {
    p: &'a Project,
    memo: HashMap<ItemId, Offsets>,
    narration: HashMap<ItemId, bool>,
}

impl<'a> Walk<'a> {
    fn new(p: &'a Project) -> Self {
        Walk { p, memo: HashMap::new(), narration: HashMap::new() }
    }

    fn is_narration(&mut self, id: ItemId) -> bool {
        let p = self.p;
        *self.narration.entry(id).or_insert_with(|| item_credit(p, id).is_some())
    }

    fn add(out: &mut Offsets, id: ItemId, t: Tick) {
        if let Some(x) = out.get_mut(&id) {
            if t < *x {
                *x = t;
            }
        } else if out.len() < MAX_ENTRIES {
            out.insert(id, t);
        }
    }

    /// Fill `memo[seq]`. A sequence being walked already (a cycle) or nested too deep adds nothing.
    fn visit(&mut self, seq: ItemId, depth: usize) {
        if depth > MAX_NEST || self.memo.contains_key(&seq) {
            return;
        }
        self.memo.insert(seq, Offsets::new());
        let p = self.p;
        let Some(q) = p.sequence(seq) else { return };
        let mut out = Offsets::new();
        for t in q.all_tracks() {
            for it in t.items.iter().filter(|i| i.enabled) {
                match p.item(it.item).map(|x| &x.kind) {
                    Some(ItemKind::Sequence(_)) => {
                        self.visit(it.item, depth + 1);
                        if let Some(inner) = self.memo.get(&it.item) {
                            for (id, off) in inner {
                                Self::add(&mut out, *id, Tick(it.start.0.saturating_add(off.0)));
                            }
                        }
                    }
                    Some(ItemKind::Subclip { parent, .. }) => {
                        let parent = *parent;
                        if self.is_narration(parent) {
                            Self::add(&mut out, parent, it.start);
                        }
                    }
                    _ => {
                        if self.is_narration(it.item) {
                            Self::add(&mut out, it.item, it.start);
                        }
                    }
                }
            }
        }
        self.memo.insert(seq, out);
    }
}

fn dedup_key(c: &Value) -> String {
    let engine = c["engine"].as_str().unwrap_or_default();
    match c["uuid"].as_str().filter(|u| !u.is_empty()) {
        Some(u) => format!("{engine}\u{1}{u}"),
        None => format!("\u{2}{}", c["credit"].as_str().unwrap_or_default()),
    }
}

/// The distinct credits of the narration in sequence `seq` (in timeline order), or of every
/// narration item in the project when `seq` is None (in import order). At most [`MAX_CREDITS`].
pub fn collect(p: &Project, seq: Option<ItemId>) -> Vec<Value> {
    let items: Vec<ItemId> = match seq {
        Some(q) => {
            let mut w = Walk::new(p);
            w.visit(q, 0);
            let mut v: Vec<(Tick, ItemId)> = w.memo.remove(&q).unwrap_or_default().into_iter().map(|(i, t)| (t, i)).collect();
            v.sort();
            v.into_iter().map(|(_, i)| i).collect()
        }
        None => {
            let mut v: Vec<(u64, ItemId)> = p.items.values().map(|i| (i.created, i.id)).collect();
            v.sort();
            v.into_iter().map(|(_, i)| i).collect()
        }
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in items {
        if out.len() >= MAX_CREDITS {
            break;
        }
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

// ------------------------------------------------------------------------------------- language

/// Whether the engine writes the heading in Japanese: when the UI says the interface is Japanese
/// ([`Session::japanese_text`]), else from Settings ▸ Appearance ▸ Language like the UI reads it
/// (`ja` / `en`; `auto` and anything else follow [`Session::system_language`], Japanese when the
/// system does not say), so the CLI and MCP agree with the app.
pub fn japanese(s: &Session) -> bool {
    if s.japanese_text {
        return true;
    }
    match s.prefs.appearance.language.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => false,
        "ja" | "japanese" => true,
        _ => match s.system_language.as_deref().map(|t| t.trim().to_ascii_lowercase()) {
            Some(t) => t == "ja" || t.starts_with("ja-") || t.starts_with("ja_"),
            None => true,
        },
    }
}

/// The heading of the roll and of the description text, in the interface language.
fn heading(s: &Session) -> &'static str {
    if japanese(s) { "音声" } else { "Voices" }
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3005}'..='\u{3007}' // 々 〆 〇
        | '\u{3040}'..='\u{30ff}' // kana
        | '\u{3400}'..='\u{4dbf}' // CJK extension A
        | '\u{4e00}'..='\u{9fff}' // CJK ideographs
        | '\u{f900}'..='\u{faff}' // compatibility ideographs
        | '\u{ff00}'..='\u{ffef}') // full-width / half-width forms
}

fn has_cjk(t: &str) -> bool {
    t.chars().any(is_cjk)
}

/// Whether `family` (when installed) has glyphs for the Japanese characters of `text`. A family
/// that is not installed here counts as covering (it is the user's choice for another machine).
fn covers_cjk(family: &str, text: &str) -> bool {
    let r = filmcraft_text::fonts::resolve(family, "Regular");
    if r.missing {
        return true;
    }
    let face = filmcraft_text::fonts::face(r.face);
    text.chars().filter(|c| is_cjk(*c)).all(|c| face.has_char(c))
}

/// The font of the roll: the default text font; the Japanese face when the text has Japanese in
/// it (voice names usually do) and the default font is Automatic or has no glyphs for it, so it
/// renders whatever the interface language.
fn roll_font(s: &Session, text: &str) -> String {
    if has_cjk(text) {
        let chosen = s.prefs.graphics.default_font.trim();
        if chosen.is_empty() || !covers_cjk(chosen, text) {
            return filmcraft_text::fonts::JAPANESE_FAMILY.to_string();
        }
    }
    s.default_text_font()
}

// ------------------------------------------------------------------------------------- roll

/// What the roll was generated from and as, kept in its text layer's [`ROLL_TAG`] parameter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RollState {
    /// The heading line (empty: none); fixed when the roll is made, so re-runs keep its language.
    title: String,
    extra: Vec<String>,
    /// The text last written by these commands; anything else is the user's edit.
    text: String,
    /// The font last chosen by these commands; anything else is the user's choice.
    font: String,
    /// The automatic duration (ticks) last given; 0 when `seconds` set it.
    auto_duration: i64,
}

fn text_of(e: &filmcraft_project::EffectInstance, key: &str) -> Option<String> {
    match e.params.get(key).map(|p| &p.value) {
        Some(ParamValue::Text(t)) => Some(t.clone()),
        _ => None,
    }
}

/// The tagged text layer of a clip and its state.
fn tag_layer(it: &TrackItem) -> Option<(usize, RollState)> {
    filmcraft_project::graphic::layer_indices(&it.effects).into_iter().find_map(|l| {
        let raw = text_of(it.effects.get(l)?, ROLL_TAG)?;
        Some((l, serde_json::from_str(&raw).unwrap_or_default()))
    })
}

/// The credit-roll clips of the active sequence: graphic clips tagged [`ROLL_TAG`], else (made
/// by an earlier version) graphic clips named [`ROLL_NAME`].
fn find_rolls(s: &Session) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let graphics = || q.video_tracks.iter().flat_map(|t| &t.items).filter(|i| crate::graphics::is_graphic(s, q, i.id));
    let tagged: Vec<ClipId> = graphics().filter(|i| tag_layer(i).is_some()).map(|i| i.id).collect();
    if !tagged.is_empty() {
        return tagged;
    }
    graphics().filter(|i| i.name == ROLL_NAME).map(|i| i.id).collect()
}

/// The credit roll of the active sequence the commands would update, if there is one.
pub fn credit_roll_clip(s: &Session) -> Option<ClipId> {
    find_rolls(s).first().copied()
}

/// Where the media of sequence `seq` ends, leaving out `skip`.
fn media_end_in(p: &Project, seq: ItemId, skip: Option<ClipId>) -> Tick {
    p.sequence(seq).and_then(|q| q.all_tracks().flat_map(|t| &t.items).filter(|i| Some(i.id) != skip).map(|i| i.end()).max()).unwrap_or(Tick::ZERO)
}

fn media_end(s: &Session, skip: Option<ClipId>) -> Tick {
    s.state.active_sequence.map(|q| media_end_in(&s.project, q, skip)).unwrap_or(Tick::ZERO)
}

fn clip_line(t: &str) -> String {
    t.chars().take(MAX_LINE_CHARS).collect()
}

/// The `extra` parameter: None when not given.
fn extra_p(p: &Value) -> Result<Option<Vec<String>>> {
    const CMD: &str = "tts.addCreditRoll";
    match p.get("extra") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => {
            if a.len() > MAX_EXTRA {
                return Err(bad(CMD, format!("at most {MAX_EXTRA} extra lines")));
            }
            let v = a.iter().map(|v| v.as_str().map(clip_line).ok_or_else(|| bad(CMD, "`extra` holds strings"))).collect::<Result<_>>()?;
            Ok(Some(v))
        }
        Some(Value::String(t)) => Ok(Some(t.lines().take(MAX_EXTRA).map(clip_line).collect())),
        Some(_) => Err(bad(CMD, "`extra` is a list of lines")),
    }
}

fn build_text(title: &str, credits: &[Value], extra: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !title.is_empty() {
        lines.push(title.to_string());
        lines.push(String::new());
    }
    lines.extend(credits.iter().filter_map(|c| c["credit"].as_str()).map(clip_line));
    if !extra.is_empty() {
        lines.push(String::new());
        lines.extend(extra.iter().cloned());
    }
    lines.join("\n")
}

/// The automatic length of a roll of `lines` lines.
fn auto_duration(rate: FrameRate, lines: usize) -> Tick {
    let seconds = (4.0 + 0.5 * lines as f64).clamp(5.0, 30.0);
    rate.snap_nearest(Tick::from_seconds_f64(seconds)).max(rate.frame_duration())
}

/// The first frame boundary at or after `t`.
fn frame_ceil(rate: FrameRate, t: Tick) -> Tick {
    let f = rate.snap(t);
    if f < t { Tick(f.0.saturating_add(rate.frame_duration().0)) } else { f }
}

/// The `at` parameter: None when not given; `end` is the media end (without `skip`). Clamped to
/// 0 ..= media end + [`MAX_AT_PAST_END_SECONDS`].
fn at_p(s: &Session, p: &Value, rate: FrameRate, skip: Option<ClipId>) -> Result<Option<Tick>> {
    const CMD: &str = "tts.addCreditRoll";
    let end = media_end(s, skip);
    let t = match p.get("at") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(x)) if x.trim() == "end" => return Ok(Some(frame_ceil(rate, end))),
        Some(Value::Number(n)) => n.as_i64().map(Tick),
        Some(v @ Value::Object(_)) => time_p(s, v, ""),
        Some(Value::String(x)) => x.trim().parse::<f64>().ok().filter(|x| x.is_finite()).map(Tick::from_seconds_f64),
        _ => None,
    };
    let t = t.ok_or_else(|| bad(CMD, "`at` is \"end\", ticks, seconds as a string, or {seconds|frame|timecode}"))?;
    let latest = Tick(end.0.saturating_add(Tick::from_seconds_f64(MAX_AT_PAST_END_SECONDS).0)).min(Tick::MAX);
    Ok(Some(rate.snap(t.clamp(Tick::ZERO, latest))))
}

/// Who asks for the roll.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// `tts.addCreditRoll`: regenerates the text, moves the roll only on `at`.
    Explicit,
    /// After `tts.synthesize`: keeps the user's edits; moves the roll only when it sat at the
    /// media end before the synthesis (`old_end`, when known) and the new narration runs past it.
    Auto { old_end: Option<Tick> },
}

/// Add or update the credit roll; one undo step, none when nothing changes.
fn credit_roll(s: &mut Session, p: &Value, mode: Mode) -> Result<Value> {
    const CMD: &str = "tts.addCreditRoll";
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let credits = collect(&s.project, Some(seq));
    if credits.is_empty() {
        return Err(bad(CMD, "this sequence has no narration made with Generate Narration (tts.synthesize), so there are no voices to credit"));
    }
    let (w, h, rate) = s.active_sequence().map(|q| (q.settings.width, q.settings.height, q.settings.frame_rate)).ok_or(EngineError::NoSequence)?;
    let title_p = str_p(p, "title").map(|t| clip_line(t.trim()));
    let extra_given = extra_p(p)?;
    let seconds = match f64_p(p, "seconds") {
        Some(x) if x.is_finite() && x > 0.0 => Some(rate.snap_nearest(Tick::from_seconds_f64(x.min(600.0))).max(rate.frame_duration())),
        Some(_) => return Err(bad(CMD, "`seconds` must be more than 0")),
        None => None,
    };
    let rolls = find_rolls(s);
    let at = at_p(s, p, rate, rolls.first().copied())?;
    if let Some(&clip) = rolls.first() {
        return update_roll(s, mode, UpdateArgs { clip, count: rolls.len(), credits, rate, title_p, extra_given, seconds, at });
    }

    // a new roll
    let title = title_p.unwrap_or_else(|| heading(s).to_string());
    let extra = extra_given.unwrap_or_default();
    let text = build_text(&title, &credits, &extra);
    let font = roll_font(s, &text);
    let auto = auto_duration(rate, text.lines().count());
    let dur = seconds.unwrap_or(auto);
    let at = match at {
        Some(t) => t,
        None => frame_ceil(rate, media_end(s, None)),
    };
    let state = RollState { title, extra, text: text.clone(), font: font.clone(), auto_duration: if seconds.is_some() { 0 } else { auto.0 } };
    let size = (h as f64 / 18.0).clamp(8.0, 400.0);
    let mut layer = filmcraft_project::graphic::new_text_layer(&text, Vec2::new(w as f64 / 2.0, h as f64 / 2.0), size);
    layer.params.insert("font".into(), Param::new(ParamValue::Text(font)));
    // Alignment: Center
    layer.params.insert("align".into(), Param::new(ParamValue::Choice(1)));
    // credits read better with air between the lines (leading is extra pixels per line)
    layer.params.insert("leading".into(), Param::new(ParamValue::Float(size * 0.5)));
    layer.params.insert("name".into(), Param::new(ParamValue::Text(ROLL_NAME.into())));
    layer.params.insert(ROLL_TAG.into(), Param::new(ParamValue::Text(serde_json::to_string(&state).map_err(|e| EngineError::Other(e.to_string()))?)));
    let meta = GraphicMeta { roll: Roll { mode: RollMode::Roll, start_off_screen: true, end_off_screen: true, ..Default::default() }, ..Default::default() };
    let place = json!({"time": at.0});
    let clip = crate::graphics::place_video_clip(s, ROLL_NAME, &place, ROLL_UNDO, vec![layer], Some(Box::new(meta)), move |pr, (w, h, rate)| {
        (crate::graphics::graphic_source(pr, w, h, rate), dur)
    })?;
    let (start, dur) = s.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, i)| (i.start, i.duration)).unwrap_or((at, dur));
    Ok(json!({"clip": clip.0, "updated": false, "changed": true, "moved": false, "start": start.0, "duration": dur.0, "text": text, "credits": credits}))
}

struct UpdateArgs {
    clip: ClipId,
    count: usize,
    credits: Vec<Value>,
    rate: FrameRate,
    title_p: Option<String>,
    extra_given: Option<Vec<String>>,
    seconds: Option<Tick>,
    at: Option<Tick>,
}

fn update_roll(s: &mut Session, mode: Mode, a: UpdateArgs) -> Result<Value> {
    const CMD: &str = "tts.addCreditRoll";
    let UpdateArgs { clip, count, credits, rate, title_p, extra_given, seconds, at } = a;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (tid, it) = q.find_item(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
    let locked = q.track(tid).is_some_and(|t| t.locked);
    let mut out =
        json!({"clip": clip.0, "updated": false, "changed": false, "moved": false, "start": it.start.0, "duration": it.duration.0, "credits": credits});
    if count > 1 {
        out["rolls"] = json!(count);
    }
    if locked {
        out["skipped"] = json!("locked");
        return Ok(out);
    }
    let (layer, state) = match tag_layer(it) {
        Some((l, st)) => (l, Some(st)),
        None => (filmcraft_project::graphic::layer_indices(&it.effects).first().copied().ok_or_else(|| bad(CMD, "the credit roll has no text layer"))?, None),
    };
    let e = it.effects.get(layer).ok_or_else(|| bad(CMD, "the credit roll has no text layer"))?;
    let cur_text = text_of(e, "text").unwrap_or_default();
    let cur_font = text_of(e, "font").unwrap_or_default();
    // a roll of an earlier version (no state): the text and font were ours
    let edited = state.as_ref().is_some_and(|st| st.text != cur_text);
    if edited && matches!(mode, Mode::Auto { .. }) {
        out["skipped"] = json!("edited");
        out["text"] = json!(cur_text);
        return Ok(out);
    }
    let title = title_p.or_else(|| state.as_ref().map(|st| st.title.clone())).unwrap_or_else(|| heading(s).to_string());
    let extra = extra_given.or_else(|| state.as_ref().map(|st| st.extra.clone())).unwrap_or_default();
    let text = build_text(&title, &credits, &extra);
    let ours_font = state.as_ref().is_none_or(|st| st.font == cur_font);
    let font = if ours_font { roll_font(s, &text) } else { cur_font.clone() };
    // duration: `seconds`, else grow with the text while it is still the automatic one
    let was_auto = match &state {
        Some(st) => st.auto_duration == it.duration.0,
        None => auto_duration(rate, cur_text.lines().count()) == it.duration,
    };
    let auto = auto_duration(rate, text.lines().count());
    let dur = match seconds {
        Some(d) => d,
        None if was_auto => it.duration.max(auto),
        None => it.duration,
    };
    // position: `at`; automatically, the new end when the roll sat at the old end and the new
    // narration runs past it; else where it is
    let new_end = media_end(s, Some(clip));
    let start = match (at, mode) {
        (Some(t), _) => t,
        (None, Mode::Auto { old_end: Some(old) }) if (it.start == old || it.start == frame_ceil(rate, old)) && new_end > it.start => frame_ceil(rate, new_end),
        _ => it.start,
    };
    let range = TimeRange::new(start, dur);
    let free = q.track(tid).is_some_and(|t| !t.items.iter().any(|i| i.id != clip && i.range().overlaps(&range)));
    let (start, dur) = if free { (start, dur) } else { (it.start, it.duration) };
    let new_state = RollState { title, extra, text: text.clone(), font: font.clone(), auto_duration: if seconds.is_none() && was_auto { dur.0 } else { 0 } };
    let moved = start != it.start;
    let resized = dur != it.duration;
    let changed = moved || resized || cur_text != text || cur_font != font || state.as_ref() != Some(&new_state);
    out["updated"] = json!(true);
    out["text"] = json!(text);
    if !changed {
        return Ok(out);
    }
    let tag = serde_json::to_string(&new_state).map_err(|e| EngineError::Other(e.to_string()))?;
    s.edit_sequence(ROLL_UNDO, |q, _, st| {
        let (tid, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        let e = it.effects.get_mut(layer).ok_or_else(|| bad(CMD, "the credit roll has no text layer"))?;
        e.params.insert("text".into(), Param::new(ParamValue::Text(text.clone())));
        e.params.insert("font".into(), Param::new(ParamValue::Text(font.clone())));
        e.params.insert(ROLL_TAG.into(), Param::new(ParamValue::Text(tag)));
        it.start = start;
        it.duration = dur;
        if let Some(t) = q.track_mut(tid) {
            t.sort();
        }
        st.selection = vec![clip];
        Ok(())
    })?;
    out["changed"] = json!(true);
    out["moved"] = json!(moved);
    out["start"] = json!(start.0);
    out["duration"] = json!(dur.0);
    Ok(out)
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

/// The folder of the saved project, else the narration folder.
fn base_dir(s: &Session) -> std::path::PathBuf {
    if let Some(dir) = s.path.as_deref().map(std::path::Path::new).and_then(|p| p.parent()).filter(|d| !d.as_os_str().is_empty()) {
        return dir.to_path_buf();
    }
    std::path::PathBuf::from(crate::narration::narration_dir(s, &Value::Null))
}

/// Default file of `tts.exportCredits`: `<project name>_credits.txt` next to the project, else
/// `credits.txt` in the narration folder.
fn default_path(s: &Session) -> std::path::PathBuf {
    let stem = s.path.as_deref().map(std::path::Path::new).and_then(|p| p.file_stem()).map(|x| x.to_string_lossy().into_owned());
    match stem {
        Some(stem) if s.path.as_deref().map(std::path::Path::new).and_then(|p| p.parent()).is_some_and(|d| !d.as_os_str().is_empty()) => {
            base_dir(s).join(format!("{stem}_credits.txt"))
        }
        _ => base_dir(s).join("credits.txt"),
    }
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// The file `tts.exportCredits` writes: `path` (relative to the project folder, or the narration
/// folder while the project is unsaved; `.txt` added when it has no extension), else
/// [`default_path`]. Never the open project, never an existing file that is not `.txt`.
fn export_path(s: &Session, p: &Value) -> Result<String> {
    const CMD: &str = "tts.exportCredits";
    let mut path = match str_p(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(x) => {
            let x = std::path::PathBuf::from(x);
            if x.is_relative() { base_dir(s).join(x) } else { x }
        }
        None => default_path(s),
    };
    if path.extension().is_none_or(|e| e.is_empty()) {
        path.set_extension("txt");
    }
    if let Some(project) = s.path.as_deref()
        && same_file(&path, std::path::Path::new(project))
    {
        return Err(bad(CMD, "that is the open project file; choose a .txt file"));
    }
    let txt = path.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("txt"));
    if !cfg!(target_arch = "wasm32") && path.exists() && (!txt || path.is_dir()) {
        return Err(bad(CMD, format!("{} exists and is not a .txt file; it is not overwritten", path.display())));
    }
    Ok(path.to_string_lossy().into_owned())
}

fn export_credits(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tts.exportCredits";
    let credits = credits_p(s, p, CMD)?;
    if credits.is_empty() {
        return Err(bad(CMD, "no narration made with Generate Narration (tts.synthesize) is used, so there are no voices to credit"));
    }
    let path = export_path(s, p)?;
    let text = description_text(&credits, japanese(s));
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
    credit_roll(s, p, Mode::Explicit)
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
            params: r#"{"seconds":f64?,"title":str?,"extra":[str]?,"at":"end"|ticks|{seconds|frame|timecode}?}"#,
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
            enabled: crate::commands::always,
            run: export_credits,
            journal: false,
        },
    ]
}
