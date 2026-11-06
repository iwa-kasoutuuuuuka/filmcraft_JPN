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
/// recording, one of the synthesizer's preset `speaker`s (`Ono_Anna`, …). With `engine` set to a
/// local app (`aivisspeech`, `voicevox`), `speaker` is that app's `"Speaker/Style"` instead.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceRef {
    /// What speaks this voice: `qwen3` (Qwen3-TTS: cloning or a preset speaker), `aivisspeech` or
    /// `voicevox` (a local app; [`LOCAL_ENGINES`]). Missing or unknown: the engine the command
    /// is given (`qwen3` unless it says otherwise), so older scripts speak as they did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ref_audio: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// A standing direction for a preset speaker (`"落ち着いた声で"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruct: Option<String>,
}

/// The local text-to-speech apps a voice's `engine` can name (their parameter ids).
pub const LOCAL_ENGINES: [&str; 2] = ["aivisspeech", "voicevox"];

/// An engine name folded for comparing: lower case, without spaces, `-` and `_`.
fn engine_key(s: &str) -> String {
    s.trim().chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect::<String>().to_ascii_lowercase()
}

impl VoiceRef {
    /// The local app `engine` names (`aivisspeech`, also `aivis` / `AivisSpeech`; `voicevox`),
    /// if it names one.
    pub fn local_engine(&self) -> Option<&'static str> {
        match engine_key(self.engine.as_deref()?).as_str() {
            "aivisspeech" | "aivis" => Some("aivisspeech"),
            "voicevox" => Some("voicevox"),
            _ => None,
        }
    }

    /// `engine` when it is set but names nothing this build knows (`qwen3` or a local app).
    pub fn unknown_engine(&self) -> Option<&str> {
        let e = self.engine.as_deref().map(str::trim).filter(|e| !e.is_empty())?;
        let known = self.local_engine().is_some() || matches!(engine_key(e).as_str(), "qwen3" | "qwen" | "qwen3tts");
        (!known).then_some(e)
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
    /// Written by a tool to bridge scenes (not in the source story): a different voice or a
    /// rewrite may replace it freely.
    #[serde(skip_serializing_if = "is_false")]
    pub generated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ScriptLine {
    /// Whether the line is to be spoken.
    pub fn is_spoken(&self) -> bool {
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
    pub lines: Vec<ScriptLine>,
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
                let no_speaker = v.speaker.as_deref().is_none_or(|s| s.trim().is_empty());
                if let Some(e) = v.unknown_engine() {
                    warn(
                        &mut out,
                        None,
                        None,
                        format!("character {id}: unknown engine `{e}` (qwen3, aivisspeech or voicevox): the command's engine speaks it"),
                    );
                }
                match v.local_engine() {
                    Some(e) => {
                        if no_speaker {
                            warn(
                                &mut out,
                                None,
                                None,
                                format!(
                                    "character {id}: the {e} voice has no speaker (\"Speaker/Style\"): the default voice of Settings ▸ Narration, else the engine's first voice, speaks it"
                                ),
                            );
                        }
                        if !v.ref_audio.trim().is_empty() {
                            warn(&mut out, None, None, format!("character {id}: {e} does not clone voices; refAudio is not used"));
                        }
                    }
                    None => {
                        if v.ref_audio.trim().is_empty() && no_speaker {
                            err(&mut out, None, None, format!("character {id}: a voice needs a refAudio or a speaker"));
                        }
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

    #[test]
    fn a_local_engine_voice_warns_instead_of_failing() {
        let warnings = |s: &Script| s.validate().into_iter().filter(|p| p.severity == Severity::Warning).map(|p| p.message).collect::<Vec<_>>();
        let mut s = script();
        // no speaker: the engine's default voice speaks; a warning, not an error
        let v = VoiceRef { engine: Some("AivisSpeech".into()), ..Default::default() };
        assert_eq!(v.local_engine(), Some("aivisspeech"));
        s.characters.insert("a".into(), Character { voice: Some(v), ..Default::default() });
        assert!(errors(&s).is_empty(), "{:?}", s.validate());
        assert!(warnings(&s).iter().any(|m| m.contains("aivisspeech voice has no speaker")), "{:?}", s.validate());
        // a speaker: quiet; written and read back with its engine
        let v = VoiceRef { engine: Some("voicevox".into()), speaker: Some("ずんだもん/ノーマル".into()), ..Default::default() };
        s.characters.insert("a".into(), Character { voice: Some(v), ..Default::default() });
        assert!(!warnings(&s).iter().any(|m| m.contains("character a")), "{:?}", s.validate());
        let j = s.to_json();
        assert!(j.contains("\"engine\": \"voicevox\""), "{j}");
        assert_eq!(Script::from_json(&j).unwrap(), s);
        // an unknown engine is the command's engine: a warning, and today's rules for the voice
        let v = VoiceRef { engine: Some("mystery".into()), ..Default::default() };
        assert_eq!((v.local_engine(), v.unknown_engine()), (None, Some("mystery")));
        s.characters.insert("a".into(), Character { voice: Some(v), ..Default::default() });
        assert!(warnings(&s).iter().any(|m| m.contains("unknown engine `mystery`")));
        assert!(errors(&s).iter().any(|m| m.contains("needs a refAudio or a speaker")));
        // older scripts (no engine) read as before
        let old: Script = Script::from_json(r#"{"version":1,"characters":{"m":{"voice":{"refAudio":"m.wav"}}}}"#).unwrap();
        let v = old.characters["m"].voice.clone().unwrap();
        assert_eq!((v.engine, v.ref_audio.as_str()), (None, "m.wav"));
        assert_eq!(VoiceRef { engine: Some(" Qwen3-TTS ".into()), ..Default::default() }.unknown_engine(), None);
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
    fn size_caps_stop_a_hostile_script() {
        let s = Script { scenes: (0..MAX_SCENES + 1).map(|i| Scene { id: i.to_string(), ..Default::default() }).collect(), ..Default::default() };
        assert!(errors(&s).iter().any(|m| m.contains("more than 10000 scenes")));
        assert!(matches!(Script::from_json(&" ".repeat(MAX_SCRIPT_BYTES + 1)), Err(ScriptError::TooLarge)));
    }
}
