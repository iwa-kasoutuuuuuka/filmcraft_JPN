//! The script of a story: the one upstream document of the novel / game-to-anime pipeline
//! (`docs/script.md`).
//!
//! A [`Script`] is a **standalone JSON file** (`*.fcscript.json`), not part of the `.fcproj`:
//! outside tools (an LLM turning a novel into lines, a converter for a game's scenario files)
//! write it, FilmCraft reads it. It holds the [`Character`]s (voices, sprites), the [`Scene`]s
//! (background, music, point of view, story time, prerequisites) and their [`ScriptLine`]s, plus
//! an optional `route`: the order in which the scenes are shown, which is how a multi-viewpoint
//! game becomes one linear story.
//!
//! [`Script::validate`] checks everything that can be checked without the files: ids, speakers,
//! sizes, prerequisites against the route, story time running backwards. Every number and string
//! in a script is hostile input: counts and lengths are capped before anything is built.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// The format version this build reads and writes.
pub const SCRIPT_VERSION: u32 = 1;
/// Largest script file read, bytes.
pub const MAX_SCRIPT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CHARACTERS: usize = 500;
pub const MAX_SCENES: usize = 10_000;
pub const MAX_LINES: usize = 200_000;
/// Longest text of one line, characters.
pub const MAX_TEXT_CHARS: usize = 4_000;
/// Longest id, characters.
pub const MAX_ID_CHARS: usize = 128;
/// Longest pause after a line, seconds.
pub const MAX_PAUSE: f64 = 60.0;

/// What a line is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    /// Spoken by a character.
    #[default]
    Dialogue,
    /// The narrator's voice.
    Narration,
    /// A character's thoughts, heard as a voice-over.
    Monologue,
    /// A direction with no speech: sound effects, a cut, a pause.
    Direction,
}

/// A character's voice: a reference recording and what it says (voice cloning), or, without a
/// recording, one of the synthesizer's preset `speaker`s (`Ono_Anna`, …).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceRef {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ref_audio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// A standing direction for a preset speaker (`"落ち着いた声で"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruct: Option<String>,
    /// Other voices of the same character by label (`"怒り"`, `"静か"`): a line whose `instruct`
    /// starts with a label (`怒り（強）`, `怒り: 低く`, or just `怒り`) is spoken from that voice
    /// instead, so an angry line is cloned from an angry recording. One level only: a style's own
    /// `styles` are ignored (and reported by [`Script::validate`]).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub styles: BTreeMap<String, VoiceRef>,
}

/// Most styles one voice may have.
pub const MAX_STYLES: usize = 64;

/// Characters that may follow a style label at the start of an `instruct`.
const LABEL_ENDS: &[char] = &['（', '(', '［', '[', '【', ' ', '\u{3000}', ':', '：', '、', ',', '，', '・', '/', '／'];

impl VoiceRef {
    /// True when this voice can speak: a reference recording or a preset speaker.
    pub fn is_usable(&self) -> bool {
        !self.ref_audio.trim().is_empty() || self.speaker.as_deref().is_some_and(|s| !s.trim().is_empty())
    }

    /// The label of the style `instruct` asks for, if this voice has it: the longest label that the
    /// instruct starts with, followed by nothing or by a bracket, space, colon or comma (so `怒り`
    /// does not match `怒りっぽく`). A style that cannot speak is not chosen.
    pub fn style_label(&self, instruct: Option<&str>) -> Option<&str> {
        let ins = instruct?.trim_start();
        self.styles
            .iter()
            .filter(|(k, v)| {
                let k = k.trim();
                !k.is_empty() && v.is_usable() && ins.strip_prefix(k).is_some_and(|rest| rest.is_empty() || rest.starts_with(LABEL_ENDS))
            })
            .max_by_key(|(k, _)| k.trim().chars().count())
            .map(|(k, _)| k.as_str())
    }

    /// The voice a line with this `instruct` is spoken from: its style, else this voice.
    pub fn for_instruct(&self, instruct: Option<&str>) -> &VoiceRef {
        self.style_label(instruct).and_then(|k| self.styles.get(k)).unwrap_or(self)
    }
}

/// One picture of a character: mouth closed, and optionally mouth open (for lip sync).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sprite {
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_image: Option<String>,
    /// Open-mouth pictures per vowel (`"a"`, `"i"`, `"u"`, `"e"`, `"o"`), used instead of
    /// `openImage` while that vowel is spoken. A vowel without one falls back to `openImage`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vowel_images: BTreeMap<String, String>,
    /// The same picture with the eyes closed (for blinking). Without it the expression
    /// `<name>_blink`, else one called `blink`, is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blink: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Character {
    /// Display name (the key in [`Script::characters`] is the id lines refer to).
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceRef>,
    /// Pictures by expression name (`"normal"`, `"smile"`, …).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub expressions: BTreeMap<String, Sprite>,
    /// The expression used when a line names none (else the first one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_expression: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScriptLine {
    pub id: String,
    pub kind: LineKind,
    /// The character id (required for dialogue and monologue).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// The words, as they should be spoken and subtitled (ruby in Aozora notation allowed).
    pub text: String,
    /// The reading to speak when it differs from `text` (kana), for names and odd readings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    /// An existing recording of this line (the game's own voice file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_file: Option<String>,
    /// A direction for how the line is spoken by a preset speaker (`"怒って"`, `"ささやき声で"`);
    /// a cloned voice follows its recording instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruct: Option<String>,
    /// A sound effect to play with the line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sfx: Option<String>,
    /// Silence after the line, seconds (else the build's default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_after: Option<f64>,
    /// A movie shown full screen for its own length (a direction line only): it covers the
    /// background, and its sound plays with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    /// A picture shown over the characters at its own pixel size (an inset), from this line until
    /// the next `subCg`, a `video` or the end of the scene. An empty string takes it away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_cg: Option<String>,
    /// Where the centre of the `subCg` goes, as fractions of the frame (`[0.5, 0.5]`: the middle).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_cg_anchor: Option<[f64; 2]>,
    /// Where the speaker stands: 1 left, 2 centre (one person on screen), 3 right. A character
    /// at the left or right stays there while others speak, until the scene ends, a `video`
    /// plays, someone else takes the place or a line puts a character in the centre. Without it:
    /// where the speaker already stands, else the centre.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
    /// Written by a tool to bridge scenes (not in the source story): a different voice or a
    /// rewrite may replace it freely.
    #[serde(skip_serializing_if = "is_false")]
    pub generated: bool,
    /// `false`: the line is subtitled but not voiced, as a sound word (`ガタッ`, `プルルルル`)
    /// that is a noise and not speech. Its sound, if any, is the line's `sfx`; nothing is
    /// synthesized and a `voiceFile` is not played. Unset or `true`: spoken as usual.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spoken: Option<bool>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ScriptLine {
    /// Whether the line is to be spoken (and so needs a recording or a synthesized voice).
    pub fn is_spoken(&self) -> bool {
        self.shows_text() && self.spoken != Some(false)
    }

    /// Whether the line's text is subtitled: every line but a direction, spoken or not.
    pub fn shows_text(&self) -> bool {
        self.kind != LineKind::Direction && !self.text.trim().is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scene {
    pub id: String,
    pub title: String,
    /// Whose viewpoint (a character id), for multi-viewpoint stories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pov: Option<String>,
    /// Story time (`"1998-04-12T10:30"`; any text that sorts in time order).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Scenes that must be shown before this one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bgm: Option<String>,
    /// How the scene begins: `"cut"` (the default) or `"fade"` (in from black).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition: Option<String>,
    /// No character pictures in this scene (an event picture as the background, a flashback):
    /// the voices and subtitles stay, sprites, mouths and blinks are not placed.
    #[serde(skip_serializing_if = "is_false")]
    pub hide_characters: bool,
    pub lines: Vec<ScriptLine>,
}

/// The scene transitions a build knows.
pub const TRANSITIONS: [&str; 2] = ["cut", "fade"];

impl Scene {
    /// Whether the scene fades in (an unknown transition is a cut; `validate` reports it).
    pub fn fades_in(&self) -> bool {
        self.transition.as_deref().is_some_and(|t| t.trim().eq_ignore_ascii_case("fade"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Script {
    pub version: u32,
    pub title: String,
    /// The model's name for the language (`"Japanese"`).
    pub language: String,
    pub characters: BTreeMap<String, Character>,
    pub scenes: Vec<Scene>,
    /// Scene ids in the order they are shown; empty: the scenes in file order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<String>,
}

impl Default for Script {
    fn default() -> Self {
        Self { version: SCRIPT_VERSION, title: String::new(), language: "Japanese".into(), characters: BTreeMap::new(), scenes: Vec::new(), route: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
}

/// One finding of [`Script::validate`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    #[error("the script is larger than {} MB", MAX_SCRIPT_BYTES / 1024 / 1024)]
    TooLarge,
    #[error("not a script: {0}")]
    Parse(String),
    #[error("script version {0} is not supported (this build reads version {SCRIPT_VERSION})")]
    Version(u32),
}

impl Script {
    /// Parse a script file's text.
    pub fn from_json(text: &str) -> Result<Script, ScriptError> {
        if text.len() > MAX_SCRIPT_BYTES {
            return Err(ScriptError::TooLarge);
        }
        if !text.trim_start().starts_with('{') {
            return Err(ScriptError::Parse("a script is a JSON object".into()));
        }
        let s: Script = serde_json::from_str(text).map_err(|e| ScriptError::Parse(e.to_string()))?;
        if s.version == 0 || s.version > SCRIPT_VERSION {
            return Err(ScriptError::Version(s.version));
        }
        Ok(s)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The scene with `id`.
    pub fn scene(&self, id: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id == id)
    }

    /// The scenes in the order they are shown: `route` (ids that name no scene are skipped), else
    /// file order.
    pub fn scenes_in_route(&self) -> Vec<&Scene> {
        if self.route.is_empty() {
            return self.scenes.iter().collect();
        }
        let by_id: HashMap<&str, &Scene> = self.scenes.iter().map(|s| (s.id.as_str(), s)).collect();
        self.route.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect()
    }

    /// Every line in route order with its scene.
    pub fn lines_in_route(&self) -> impl Iterator<Item = (&Scene, &ScriptLine)> {
        self.scenes_in_route().into_iter().flat_map(|s| s.lines.iter().map(move |l| (s, l)))
    }

    /// The expression a line shows: its own, else the speaker's default, else `normal`, else the
    /// speaker's first by name.
    pub fn expression_of<'a>(&'a self, line: &'a ScriptLine) -> Option<(&'a str, &'a Sprite)> {
        let c = self.characters.get(line.speaker.as_deref()?)?;
        if let Some(e) = line.expression.as_deref()
            && let Some(sp) = c.expressions.get(e)
        {
            return Some((e, sp));
        }
        if let Some(e) = c.default_expression.as_deref()
            && let Some(sp) = c.expressions.get(e)
        {
            return Some((e, sp));
        }
        // an expression called `normal` is the natural face; else the first by name
        if let Some((k, v)) = c.expressions.get_key_value("normal") {
            return Some((k.as_str(), v));
        }
        c.expressions.iter().next().map(|(k, v)| (k.as_str(), v))
    }

    /// Check the script. Errors stop a build; warnings are worth reading.
    pub fn validate(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        let err = |out: &mut Vec<Problem>, scene: Option<&str>, line: Option<&str>, m: String| {
            out.push(Problem { severity: Severity::Error, scene: scene.map(str::to_string), line: line.map(str::to_string), message: m });
        };
        let warn = |out: &mut Vec<Problem>, scene: Option<&str>, line: Option<&str>, m: String| {
            out.push(Problem { severity: Severity::Warning, scene: scene.map(str::to_string), line: line.map(str::to_string), message: m });
        };
        if self.version == 0 || self.version > SCRIPT_VERSION {
            err(&mut out, None, None, format!("version {} is not supported", self.version));
        }
        if self.characters.len() > MAX_CHARACTERS {
            err(&mut out, None, None, format!("more than {MAX_CHARACTERS} characters"));
        }
        if self.scenes.len() > MAX_SCENES {
            err(&mut out, None, None, format!("more than {MAX_SCENES} scenes"));
            return out;
        }
        let total: usize = self.scenes.iter().map(|s| s.lines.len()).sum();
        if total > MAX_LINES {
            err(&mut out, None, None, format!("more than {MAX_LINES} lines"));
            return out;
        }
        for (id, c) in &self.characters {
            if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
                err(&mut out, None, None, format!("character id `{id}` is empty or longer than {MAX_ID_CHARS} characters"));
            }
            if let Some(d) = &c.default_expression
                && !c.expressions.contains_key(d)
            {
                warn(&mut out, None, None, format!("character {id}: default expression `{d}` has no picture"));
            }
            if let Some(v) = &c.voice {
                if !v.is_usable() {
                    err(&mut out, None, None, format!("character {id}: a voice needs a refAudio or a speaker"));
                }
                if v.styles.len() > MAX_STYLES {
                    err(&mut out, None, None, format!("character {id}: more than {MAX_STYLES} voice styles"));
                }
                for (label, st) in v.styles.iter().take(MAX_STYLES) {
                    let n = label.trim().chars().count();
                    if n == 0 || n > MAX_ID_CHARS {
                        err(&mut out, None, None, format!("character {id}: voice style label `{label}` is empty or longer than {MAX_ID_CHARS} characters"));
                    } else if !st.is_usable() {
                        err(&mut out, None, None, format!("character {id}: voice style `{label}` needs a refAudio or a speaker"));
                    }
                    if !st.styles.is_empty() {
                        warn(&mut out, None, None, format!("character {id}: voice style `{label}` has styles of its own, which are ignored"));
                    }
                }
            }
        }
        let mut scene_ids: HashSet<&str> = HashSet::new();
        let mut line_ids: HashSet<&str> = HashSet::new();
        for sc in &self.scenes {
            let sid = Some(sc.id.as_str());
            if sc.id.is_empty() || sc.id.chars().count() > MAX_ID_CHARS {
                err(&mut out, sid, None, format!("scene id `{}` is empty or longer than {MAX_ID_CHARS} characters", sc.id));
            } else if !scene_ids.insert(sc.id.as_str()) {
                err(&mut out, sid, None, "duplicate scene id".into());
            }
            if let Some(p) = &sc.pov
                && !self.characters.contains_key(p)
            {
                warn(&mut out, sid, None, format!("pov `{p}` is not a character"));
            }
            if let Some(t) = &sc.transition
                && !TRANSITIONS.iter().any(|k| t.trim().eq_ignore_ascii_case(k))
            {
                err(&mut out, sid, None, format!("transition `{t}` is not one of {}", TRANSITIONS.join(", ")));
            }
            for l in &sc.lines {
                let lid = Some(l.id.as_str());
                if l.id.is_empty() || l.id.chars().count() > MAX_ID_CHARS {
                    err(&mut out, sid, lid, format!("line id `{}` is empty or longer than {MAX_ID_CHARS} characters", l.id));
                } else if !line_ids.insert(l.id.as_str()) {
                    err(&mut out, sid, lid, "duplicate line id (ids name the audio files, so they are unique across the script)".into());
                }
                for (name, v) in [("text", Some(l.text.as_str())), ("reading", l.reading.as_deref()), ("instruct", l.instruct.as_deref())] {
                    if v.is_some_and(|v| v.chars().count() > MAX_TEXT_CHARS) {
                        err(&mut out, sid, lid, format!("{name} longer than {MAX_TEXT_CHARS} characters"));
                    }
                }
                match l.kind {
                    LineKind::Dialogue | LineKind::Monologue => match l.speaker.as_deref() {
                        None => err(&mut out, sid, lid, "dialogue needs a speaker".into()),
                        Some(sp) if !self.characters.contains_key(sp) => err(&mut out, sid, lid, format!("speaker `{sp}` is not a character")),
                        _ => {}
                    },
                    LineKind::Narration => {
                        if let Some(sp) = l.speaker.as_deref()
                            && !self.characters.contains_key(sp)
                        {
                            err(&mut out, sid, lid, format!("speaker `{sp}` is not a character"));
                        }
                    }
                    LineKind::Direction => {}
                }
                if l.kind != LineKind::Direction && l.text.trim().is_empty() {
                    err(&mut out, sid, lid, "empty text".into());
                }
                if let (Some(e), Some(sp)) = (l.expression.as_deref(), l.speaker.as_deref())
                    && let Some(c) = self.characters.get(sp)
                    && !c.expressions.is_empty()
                    && !c.expressions.contains_key(e)
                {
                    warn(&mut out, sid, lid, format!("{sp} has no expression `{e}`"));
                }
                if let Some(p) = l.pause_after
                    && !(p.is_finite() && (0.0..=MAX_PAUSE).contains(&p))
                {
                    err(&mut out, sid, lid, format!("pauseAfter must be 0..{MAX_PAUSE} seconds"));
                }
                if let Some(s) = l.slot
                    && !(1..=3).contains(&s)
                {
                    err(&mut out, sid, lid, format!("slot must be 1 (left), 2 (centre) or 3 (right), got {s}"));
                }
                if let Some(v) = &l.video {
                    if v.trim().is_empty() {
                        err(&mut out, sid, lid, "empty video".into());
                    } else if l.kind != LineKind::Direction {
                        warn(&mut out, sid, lid, "a video is only shown on a direction line; this one is ignored".into());
                    }
                }
                if let Some(a) = l.sub_cg_anchor
                    && !a.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                {
                    err(&mut out, sid, lid, "subCgAnchor must be two fractions of the frame, 0..1".into());
                }
                if l.spoken == Some(false) {
                    if l.kind == LineKind::Direction {
                        warn(&mut out, sid, lid, "`spoken: false` on a direction changes nothing (a direction is never spoken)".into());
                    } else if l.voice_file.is_some() {
                        warn(&mut out, sid, lid, "`spoken: false`: the voiceFile is not played (a sound for the line goes in `sfx`)".into());
                    }
                }
                if l.is_spoken() && l.voice_file.is_none() {
                    let has_voice = l.speaker.as_deref().and_then(|s| self.characters.get(s)).is_some_and(|c| c.voice.is_some());
                    if !has_voice && l.kind != LineKind::Narration {
                        warn(
                            &mut out,
                            sid,
                            lid,
                            "no recording and the speaker has no voice: the synthesizer's preset voice for the language would speak it".into(),
                        );
                    }
                }
            }
        }
        for sc in &self.scenes {
            for r in &sc.requires {
                if r == &sc.id {
                    err(&mut out, Some(&sc.id), None, "requires itself".into());
                } else if !scene_ids.contains(r.as_str()) {
                    err(&mut out, Some(&sc.id), None, format!("requires `{r}`, which is not a scene"));
                }
            }
        }
        // route: known ids, no repeats, prerequisites shown first, story time
        let mut pos: HashMap<&str, usize> = HashMap::new();
        for (i, r) in self.route.iter().enumerate() {
            if !scene_ids.contains(r.as_str()) {
                err(&mut out, None, None, format!("route names `{r}`, which is not a scene"));
            } else if pos.insert(r.as_str(), i).is_some() {
                err(&mut out, Some(r), None, "appears twice in the route".into());
            }
        }
        let order: Vec<&Scene> = self.scenes_in_route();
        if !self.route.is_empty() {
            for sc in &self.scenes {
                if !pos.contains_key(sc.id.as_str()) {
                    warn(&mut out, Some(&sc.id), None, "is not in the route (it will not be shown)".into());
                }
            }
        }
        let mut seen: HashSet<&str> = HashSet::new();
        let mut last_when: Option<(&str, &str)> = None;
        for sc in &order {
            for r in &sc.requires {
                if scene_ids.contains(r.as_str()) && !seen.contains(r.as_str()) {
                    err(&mut out, Some(&sc.id), None, format!("requires `{r}`, which is not shown before it"));
                }
            }
            if let Some(w) = sc.when.as_deref() {
                if let Some((pw, pid)) = last_when
                    && w < pw
                {
                    warn(
                        &mut out,
                        Some(&sc.id),
                        None,
                        format!("story time {w} is earlier than {pw} of the scene before ({pid}); fine for a flashback, else reorder"),
                    );
                }
                last_when = Some((w, sc.id.as_str()));
            }
            seen.insert(sc.id.as_str());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> Script {
        let mut s = Script::default();
        s.characters.insert(
            "marina".into(),
            Character {
                name: "まりな".into(),
                expressions: [("normal".to_string(), Sprite { image: "m.png".into(), open_image: Some("mo.png".into()), ..Default::default() })].into(),
                ..Default::default()
            },
        );
        let line = |id: &str, text: &str| ScriptLine { id: id.into(), speaker: Some("marina".into()), text: text.into(), ..Default::default() };
        s.scenes.push(Scene {
            id: "a".into(),
            when: Some("1998-04-12T10:00".into()),
            lines: vec![line("a1", "待って。"), line("a2", "どうして？")],
            ..Default::default()
        });
        s.scenes.push(Scene {
            id: "b".into(),
            when: Some("1998-04-12T12:00".into()),
            requires: vec!["a".into()],
            lines: vec![line("b1", "ここは。")],
            ..Default::default()
        });
        s
    }

    fn errors(s: &Script) -> Vec<String> {
        s.validate().into_iter().filter(|p| p.severity == Severity::Error).map(|p| p.message).collect()
    }

    #[test]
    fn a_good_script_has_no_errors_and_round_trips() {
        let s = script();
        assert!(errors(&s).is_empty(), "{:?}", s.validate());
        let back = Script::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(s.lines_in_route().count(), 3);
    }

    #[test]
    fn parses_a_minimal_file_with_defaults() {
        let s = Script::from_json(r#"{"scenes":[{"id":"s","lines":[{"id":"l","kind":"narration","text":"夜だった。"}]}]}"#).unwrap();
        assert_eq!(s.version, SCRIPT_VERSION);
        assert_eq!(s.language, "Japanese");
        assert!(errors(&s).is_empty(), "{:?}", s.validate());
        assert!(matches!(Script::from_json("[]"), Err(ScriptError::Parse(_))));
        assert!(matches!(Script::from_json(r#"{"version": 9}"#), Err(ScriptError::Version(9))));
        assert!(matches!(Script::from_json(r#"{"version": 0}"#), Err(ScriptError::Version(0))));
    }

    #[test]
    fn ids_speakers_and_text_are_checked() {
        let mut s = script();
        s.scenes[0].lines[1].id = "a1".into();
        s.scenes[1].lines[0].speaker = Some("ghost".into());
        s.scenes[1].lines.push(ScriptLine { id: "b2".into(), text: "  ".into(), speaker: Some("marina".into()), ..Default::default() });
        s.scenes[1].lines.push(ScriptLine { id: "b3".into(), kind: LineKind::Dialogue, text: "x".into(), ..Default::default() });
        s.scenes[1].lines.push(ScriptLine { id: "b4".into(), kind: LineKind::Direction, sfx: Some("door".into()), ..Default::default() });
        s.scenes[1].lines.push(ScriptLine {
            id: "b5".into(),
            speaker: Some("marina".into()),
            text: "x".into(),
            pause_after: Some(f64::NAN),
            ..Default::default()
        });
        let e = errors(&s);
        assert!(e.iter().any(|m| m.contains("duplicate line id")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("`ghost` is not a character")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("empty text")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("needs a speaker")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("pauseAfter")), "{e:?}");
        // a direction needs no text and no speaker
        assert!(!e.iter().any(|m| m.contains("b4")));
        let mut s = script();
        s.scenes[0].lines[0].text = "あ".repeat(MAX_TEXT_CHARS + 1);
        s.scenes[0].id = String::new();
        let e = errors(&s);
        assert!(e.iter().any(|m| m.contains("longer than 4000")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("scene id")), "{e:?}");
    }

    #[test]
    fn the_route_is_checked_against_prerequisites_and_story_time() {
        let mut s = script();
        s.route = vec!["b".into(), "a".into()];
        let e = errors(&s);
        assert!(e.iter().any(|m| m.contains("requires `a`, which is not shown before it")), "{e:?}");
        let w: Vec<_> = s.validate().into_iter().filter(|p| p.severity == Severity::Warning).collect();
        assert!(w.iter().any(|p| p.message.contains("earlier than")), "{w:?}");
        s.route = vec!["a".into(), "b".into(), "b".into(), "zzz".into()];
        let e = errors(&s);
        assert!(e.iter().any(|m| m.contains("appears twice")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("`zzz`")), "{e:?}");
        s.route = vec!["a".into()];
        let w = s.validate();
        assert!(w.iter().any(|p| p.severity == Severity::Warning && p.message.contains("not in the route")), "{w:?}");
        assert_eq!(s.scenes_in_route().len(), 1);
        // a prerequisite cycle can never be shown
        let mut s = script();
        s.scenes[0].requires = vec!["b".into()];
        assert!(!errors(&s).is_empty());
        s.scenes[0].requires = vec!["a".into()];
        assert!(errors(&s).iter().any(|m| m.contains("requires itself")));
    }

    #[test]
    fn a_voice_needs_a_recording_or_a_preset_speaker() {
        let mut s = script();
        s.characters.insert("a".into(), Character { voice: Some(VoiceRef::default()), ..Default::default() });
        assert!(errors(&s).iter().any(|m| m.contains("needs a refAudio or a speaker")), "{:?}", s.validate());
        s.characters.insert("a".into(), Character { voice: Some(VoiceRef { speaker: Some("Ono_Anna".into()), ..Default::default() }), ..Default::default() });
        assert!(errors(&s).is_empty(), "{:?}", s.validate());
        let j = s.to_json();
        assert!(j.contains("Ono_Anna") && !j.contains("refAudio"), "{j}");
        assert_eq!(Script::from_json(&j).unwrap(), s);
    }

    fn styled() -> VoiceRef {
        let r = |a: &str| VoiceRef { ref_audio: a.into(), ref_text: Some(format!("{a} の文")), ..Default::default() };
        let mut v = r("calm.wav");
        v.styles.insert("怒り".into(), r("angry.wav"));
        v.styles.insert("怒りっぽい".into(), r("grumpy.wav"));
        v.styles.insert("静か".into(), r("quiet.wav"));
        v.styles.insert("無音".into(), VoiceRef::default()); // cannot speak: never chosen
        v
    }

    #[test]
    fn a_line_label_picks_the_voice_style() {
        let v = styled();
        for (ins, want) in [
            (Some("怒り（強）"), "angry.wav"),
            (Some("怒り"), "angry.wav"),
            (Some("  怒り: 低く"), "angry.wav"),
            (Some("怒りっぽい（中）"), "grumpy.wav"), // the longest label wins
            (Some("怒りっぽく"), "calm.wav"),         // not a label followed by a break
            (Some("静か（弱）"), "quiet.wav"),
            (Some("ささやき声で"), "calm.wav"),
            (Some("無音"), "calm.wav"),
            (Some(""), "calm.wav"),
            (None, "calm.wav"),
        ] {
            assert_eq!(v.for_instruct(ins).ref_audio, want, "{ins:?}");
        }
        assert_eq!(v.style_label(Some("静か（弱）")), Some("静か"));
        assert_eq!(VoiceRef::default().for_instruct(Some("怒り")), &VoiceRef::default());
    }

    #[test]
    fn voice_styles_round_trip_and_are_checked() {
        let mut s = script();
        let mut v = styled();
        v.styles.remove("無音");
        s.characters.insert("a".into(), Character { voice: Some(v.clone()), ..Default::default() });
        assert!(errors(&s).is_empty(), "{:?}", s.validate());
        let j = s.to_json();
        assert!(j.contains("\"styles\"") && j.contains("angry.wav"), "{j}");
        assert_eq!(Script::from_json(&j).unwrap(), s);
        // a voice without styles writes no `styles` key (older readers see the same file)
        assert!(!script().to_json().contains("styles"));
        // a style that cannot speak, an empty label, too many styles, nested styles
        v.styles.insert("無音".into(), VoiceRef::default());
        v.styles.insert(" ".into(), VoiceRef { speaker: Some("x".into()), ..Default::default() });
        s.characters.insert("a".into(), Character { voice: Some(v.clone()), ..Default::default() });
        let e = errors(&s);
        assert!(e.iter().any(|m| m.contains("style `無音` needs a refAudio")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("label ` ` is empty")), "{e:?}");
        let mut many = VoiceRef { speaker: Some("x".into()), ..Default::default() };
        for k in 0..=MAX_STYLES {
            many.styles.insert(format!("s{k}"), VoiceRef { speaker: Some("x".into()), ..Default::default() });
        }
        many.styles.get_mut("s0").map(|st| st.styles.insert("deeper".into(), VoiceRef::default()));
        s.characters.insert("a".into(), Character { voice: Some(many), ..Default::default() });
        assert!(errors(&s).iter().any(|m| m.contains("more than 64 voice styles")));
        assert!(s.validate().iter().any(|p| p.severity == Severity::Warning && p.message.contains("styles of its own")));
    }

    #[test]
    fn expressions_fall_back_to_the_speakers_default() {
        let mut s = script();
        let l = s.scenes[0].lines[0].clone();
        assert_eq!(s.expression_of(&l).map(|(n, _)| n), Some("normal"));
        s.scenes[0].lines[0].expression = Some("angry".into());
        let l = s.scenes[0].lines[0].clone();
        assert_eq!(s.expression_of(&l).map(|(n, _)| n), Some("normal"));
        assert!(s.validate().iter().any(|p| p.severity == Severity::Warning && p.message.contains("no expression `angry`")));
        // no default set: `normal`, even though `angry` sorts first
        s.characters.get_mut("marina").unwrap().expressions.insert("angry".into(), Sprite { image: "a.png".into(), open_image: None, ..Default::default() });
        let l = s.scenes[0].lines[1].clone();
        assert_eq!(s.expression_of(&l).map(|(n, _)| n), Some("normal"));
        let narr = ScriptLine { id: "n".into(), kind: LineKind::Narration, text: "x".into(), ..Default::default() };
        assert!(s.expression_of(&narr).is_none());
    }

    #[test]
    fn a_long_reading_or_direction_is_an_error_not_a_failed_run() {
        let mut s = script();
        s.scenes[0].lines[0].reading = Some("あ".repeat(MAX_TEXT_CHARS + 1));
        s.scenes[0].lines[1].instruct = Some("あ".repeat(MAX_TEXT_CHARS + 1));
        let e = errors(&s);
        assert!(e.iter().any(|m| m.contains("reading longer")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("instruct longer")), "{e:?}");
    }

    #[test]
    fn movies_insets_places_and_transitions_are_checked() {
        let s = Script::from_json(
            r#"{"characters": {"m": {"name": "m"}}, "scenes": [{"id": "s", "transition": "fade", "lines": [
                {"id": "v", "kind": "direction", "video": "mv/op.mp4"},
                {"id": "c", "kind": "narration", "text": "x", "subCg": "cg/a.png", "subCgAnchor": [0.5, 0.25]},
                {"id": "l", "speaker": "m", "text": "x", "slot": 1}]}]}"#,
        )
        .unwrap();
        assert!(errors(&s).is_empty(), "{:?}", s.validate());
        assert!(s.scenes[0].fades_in());
        assert_eq!(s.scenes[0].lines[1].sub_cg_anchor, Some([0.5, 0.25]));
        assert_eq!(Script::from_json(&s.to_json()).unwrap(), s);
        let mut bad = s.clone();
        bad.scenes[0].transition = Some("wipe".into());
        bad.scenes[0].lines[2].slot = Some(4);
        bad.scenes[0].lines[1].sub_cg_anchor = Some([f64::NAN, 2.0]);
        bad.scenes[0].lines[0].video = Some(" ".into());
        let e = errors(&bad);
        for want in ["transition `wipe`", "slot must be", "subCgAnchor", "empty video"] {
            assert!(e.iter().any(|m| m.contains(want)), "{want}: {e:?}");
        }
        assert!(!bad.scenes[0].fades_in());
        // a video on a spoken line is a warning: it is not shown there
        let mut w = s.clone();
        w.scenes[0].lines[2].video = Some("mv/op.mp4".into());
        assert!(w.validate().iter().any(|p| p.severity == Severity::Warning && p.message.contains("direction line")));
        // a slot that does not fit the type is a parse error, not a crash
        assert!(Script::from_json(r#"{"scenes": [{"id": "s", "lines": [{"id": "l", "kind": "narration", "text": "x", "slot": -1}]}]}"#).is_err());
    }

    #[test]
    fn a_line_that_is_not_spoken_is_still_subtitled() {
        let s = Script::from_json(
            r#"{"characters": {"m": {"name": "m"}}, "scenes": [{"id": "s", "lines": [
                {"id": "a", "kind": "narration", "text": "ガタッ。", "spoken": false, "sfx": "se/door.wav"},
                {"id": "b", "speaker": "m", "text": "待って。"},
                {"id": "c", "speaker": "m", "text": "ぐぅ～。", "spoken": false, "voiceFile": "v/c.wav"},
                {"id": "d", "kind": "direction", "spoken": false},
                {"id": "e", "kind": "narration", "text": " ", "spoken": false}]}]}"#,
        )
        .unwrap();
        let l = &s.scenes[0].lines;
        assert!(!l[0].is_spoken() && l[0].shows_text());
        assert!(l[1].is_spoken() && l[1].shows_text() && l[1].spoken.is_none());
        assert!(!l[3].is_spoken() && !l[3].shows_text());
        // written back only where it was given
        let j = s.to_json();
        assert_eq!(j.matches("\"spoken\"").count(), 4, "{j}");
        assert_eq!(Script::from_json(&j).unwrap(), s);
        let w: Vec<_> = s.validate().into_iter().filter(|p| p.severity == Severity::Warning).map(|p| (p.line.unwrap_or_default(), p.message)).collect();
        assert!(w.iter().any(|(l, m)| l == "c" && m.contains("voiceFile is not played")), "{w:?}");
        assert!(w.iter().any(|(l, m)| l == "d" && m.contains("changes nothing")), "{w:?}");
        // an unspoken line still needs its text (or it is a direction)
        assert!(errors(&s).iter().any(|m| m.contains("empty text")), "{:?}", s.validate());
        assert!(Script::from_json(r#"{"scenes": [{"id": "s", "lines": [{"id": "l", "text": "x", "spoken": "no"}]}]}"#).is_err());
    }

    #[test]
    fn size_caps_stop_a_hostile_script() {
        let s = Script { scenes: (0..MAX_SCENES + 1).map(|i| Scene { id: i.to_string(), ..Default::default() }).collect(), ..Default::default() };
        assert!(errors(&s).iter().any(|m| m.contains("more than 10000 scenes")));
        assert!(matches!(Script::from_json(&" ".repeat(MAX_SCRIPT_BYTES + 1)), Err(ScriptError::TooLarge)));
    }
}
