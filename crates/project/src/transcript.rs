//! Transcripts of media clips (Text panel ▸ Transcript).
//!
//! A [`Transcript`] belongs to one media item (`Project::transcripts`, keyed by the item id) and
//! lists the spoken [`Word`]s with their **media-time** bounds, so it stays valid however the clip
//! is trimmed, moved or reused: the sequence transcript is derived from the clip transcripts by
//! mapping each word through the track items that show it (`filmcraft_edit::transcript`).
//!
//! Speakers are numbered per transcript (`Word::speaker` indexes [`Transcript::speakers`]); the
//! Text panel shows and renames them by name, so two clips whose speakers share a name read as one
//! speaker in the sequence transcript.

use filmcraft_time::{Tick, TimeRange};
use serde::{Deserialize, Serialize};

/// One transcribed word.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// The word as written, with attached punctuation (`"Hello,"`). No surrounding spaces.
    pub text: String,
    /// Media time of the word's first sample.
    pub start: Tick,
    /// Media time just after the word (exclusive).
    pub end: Tick,
    /// Index into [`Transcript::speakers`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<u32>,
    /// Recogniser confidence 0..1 (1 for hand-made or corrected words).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub confidence: f32,
}

fn one() -> f32 {
    1.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}

impl Word {
    pub fn new(text: impl Into<String>, start: Tick, end: Tick) -> Self {
        Self { text: text.into(), start, end, speaker: None, confidence: 1.0 }
    }
    pub fn range(&self) -> TimeRange {
        TimeRange::from_bounds(self.start, self.end.max(self.start))
    }
    /// Lower-case text without leading/trailing punctuation (`"Um,"` → `"um"`), for search and
    /// filler-word matching.
    pub fn normalized(&self) -> String {
        normalize_word(&self.text)
    }
}

/// Lower-case `s` and trim punctuation and symbols from both ends (apostrophes inside a word stay:
/// `"Don't!"` → `"don't"`).
pub fn normalize_word(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// A speaker label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
}

/// The transcript of one media item.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transcript {
    /// ISO 639-1 code of the spoken language (`"en"`).
    pub language: String,
    /// What produced it: a model id (`"whisper-base"`), `"imported"` or `"manual"`.
    pub source: String,
    pub speakers: Vec<Speaker>,
    /// Words in time order, non-overlapping.
    pub words: Vec<Word>,
}

/// Most speakers one transcript may number; [`Transcript::normalize`] drops higher indices.
pub const MAX_SPEAKERS: u32 = 256;

impl Transcript {
    /// The speaker name of `word` (`"Speaker 2"` when the label list is short; `None` when the word
    /// has no speaker).
    pub fn speaker_name(&self, word: &Word) -> Option<String> {
        let i = word.speaker? as usize;
        Some(self.speakers.get(i).map(|s| s.name.clone()).unwrap_or_else(|| format!("Speaker {}", i + 1)))
    }

    /// The text of all words, with spaces between them where the script has them
    /// ([`needs_space`]: none inside Japanese or Chinese text).
    pub fn text(&self) -> String {
        let mut out = String::new();
        for w in &self.words {
            if needs_space(&out, &w.text) {
                out.push(' ');
            }
            out.push_str(&w.text);
        }
        out
    }

    /// Indices of the words overlapping the media range.
    pub fn words_in(&self, r: TimeRange) -> std::ops::Range<usize> {
        let a = self.words.partition_point(|w| w.end <= r.start);
        let b = self.words.partition_point(|w| w.start < r.end());
        a..b.max(a)
    }

    /// Sort words by time and make the transcript well formed: empty words dropped, `end >= start`,
    /// no overlaps (a word ends at the next one's start at the latest), speaker indices in range
    /// (missing labels are added as "Speaker N").
    pub fn normalize(&mut self) {
        self.words.retain(|w| !w.text.trim().is_empty());
        for w in &mut self.words {
            w.text = w.text.trim().to_string();
            if w.end < w.start {
                w.end = w.start;
            }
            w.confidence = w.confidence.clamp(0.0, 1.0);
        }
        self.words.sort_by_key(|w| (w.start, w.end));
        for i in 1..self.words.len() {
            let s = self.words[i].start;
            if self.words[i - 1].end > s {
                self.words[i - 1].end = s.max(self.words[i - 1].start);
            }
        }
        // A hostile speaker index would grow the label list without bound: drop it instead.
        for w in &mut self.words {
            if w.speaker.is_some_and(|s| s >= MAX_SPEAKERS) {
                w.speaker = None;
            }
        }
        let max = self.words.iter().filter_map(|w| w.speaker).max();
        if let Some(m) = max {
            while self.speakers.len() <= m as usize {
                let n = self.speakers.len() + 1;
                self.speakers.push(Speaker { name: format!("Speaker {n}") });
            }
        }
    }

    /// Validate the invariants [`Transcript::normalize`] establishes.
    pub fn check(&self) -> Result<(), String> {
        for (i, w) in self.words.iter().enumerate() {
            if w.end < w.start {
                return Err(format!("word {i} ({:?}) ends before it starts", w.text));
            }
            if i > 0 && self.words[i - 1].end > w.start {
                return Err(format!("word {i} ({:?}) overlaps the word before it", w.text));
            }
            if let Some(s) = w.speaker
                && s as usize >= self.speakers.len()
            {
                return Err(format!("word {i} has speaker {s}, but there are {} speakers", self.speakers.len()));
            }
        }
        Ok(())
    }
}

/// Kana, kanji, CJK punctuation and full-width forms: text set without spaces between words,
/// in characters twice as wide as Latin ones.
pub fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x323AF)
}

/// Whether a space goes between `before` and the word `next` when words are joined into text:
/// not at the start, and not where either side is Japanese or Chinese text (今日 + は → 今日は,
/// 東京 + Tower → 東京Tower; hello + world → hello world).
pub fn needs_space(before: &str, next: &str) -> bool {
    match (before.chars().next_back(), next.chars().next()) {
        (Some(a), Some(b)) => !a.is_whitespace() && !is_wide(a) && !is_wide(b),
        _ => false,
    }
}

/// Width of text in caption columns: wide characters count two, the others one.
pub fn display_width(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, a: i64, b: i64) -> Word {
        Word::new(t, Tick(a), Tick(b))
    }

    #[test]
    fn text_has_spaces_only_where_the_script_does() {
        let t = |words: &[&str]| Transcript { words: words.iter().map(|x| w(x, 0, 1)).collect(), ..Default::default() }.text();
        assert_eq!(t(&["Hello,", "world!"]), "Hello, world!");
        assert_eq!(t(&["こんにちは。", "今日", "は", "東京", "の", "天気", "です。"]), "こんにちは。今日は東京の天気です。");
        assert_eq!(t(&["東京", "Tower", "is", "tall"]), "東京Tower is tall");
        assert_eq!(t(&["I", "love", "東京", "very", "much"]), "I love東京very much");
        assert_eq!(t(&[]), "");
        assert!(needs_space("a", "b") && !needs_space("", "b") && !needs_space("a", "") && !needs_space("a ", "b"));
        assert!(!needs_space("今日", "は") && !needs_space("go", "へ") && !needs_space("。", "next"));
        assert_eq!((display_width("abc"), display_width("日本語"), display_width("日本go。"), display_width("")), (3, 6, 8, 0));
        assert!(is_wide('あ') && is_wide('漢') && is_wide('。') && is_wide('Ａ') && !is_wide('a') && !is_wide('é') && !is_wide('ｱ'));
    }

    #[test]
    fn normalize_sorts_clamps_and_labels() {
        let mut t = Transcript { words: vec![w("b", 10, 30), w(" a ", 0, 15), w("", 40, 50), w("c", 35, 20)], ..Default::default() };
        t.words[0].speaker = Some(1);
        t.normalize();
        assert_eq!(t.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(t.words[0].end, Tick(10));
        assert_eq!(t.words[2].end, Tick(35));
        assert_eq!(t.speakers.len(), 2);
        assert_eq!(t.speaker_name(&t.words[1]).as_deref(), Some("Speaker 2"));
        t.check().unwrap();
    }

    #[test]
    fn normalize_drops_hostile_speaker_indices() {
        let mut t = Transcript { words: vec![w("a", 0, 10), w("b", 10, 20)], ..Default::default() };
        t.words[0].speaker = Some(u32::MAX);
        t.words[1].speaker = Some(MAX_SPEAKERS - 1);
        t.normalize();
        assert_eq!(t.words[0].speaker, None);
        assert_eq!(t.speakers.len(), MAX_SPEAKERS as usize);
        t.check().unwrap();
    }

    #[test]
    fn words_in_range_and_normalized() {
        let t = Transcript { words: vec![w("Um,", 0, 10), w("Don't!", 10, 20), w("go", 25, 30)], ..Default::default() };
        assert_eq!(t.words_in(TimeRange::from_bounds(Tick(5), Tick(24))), 0..2);
        assert_eq!(t.words_in(TimeRange::from_bounds(Tick(20), Tick(25))), 2..2);
        assert_eq!(t.words[0].normalized(), "um");
        assert_eq!(t.words[1].normalized(), "don't");
        assert_eq!(t.text(), "Um, Don't! go");
    }
}
