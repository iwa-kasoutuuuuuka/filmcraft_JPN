//! Decoding Whisper token ids to text from the model's `tokenizer.json` (byte-level BPE: each
//! vocabulary entry is a string of "printable" stand-in characters, one per byte, using the
//! GPT-2 byte↔unicode table; special tokens are listed separately as added tokens).
//!
//! Only decoding is needed (Whisper produces tokens; we never encode text), so the BPE merges are
//! not used.

use std::collections::HashMap;

use crate::SpeechError;

pub struct Tokenizer {
    /// Bytes of each regular token, by id.
    bytes: Vec<Vec<u8>>,
    /// Special (added) tokens by content and by id.
    special: HashMap<String, u32>,
    special_by_id: HashMap<u32, String>,
    pub eot: u32,
    pub sot: u32,
    pub transcribe: u32,
    pub translate: Option<u32>,
    pub no_timestamps: u32,
    pub no_speech: Option<u32>,
    /// `<|0.00|>`; every later id is a timestamp in 0.02 s steps.
    pub timestamp_begin: u32,
    /// Language tokens (`"en"` → id), multilingual models only.
    pub languages: Vec<(String, u32)>,
}

/// GPT-2's byte → printable character table, inverted.
fn unicode_to_byte() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    // (every code is below 512: always a character)
    bs.iter().zip(&cs).filter_map(|(&b, &c)| char::from_u32(c).map(|ch| (ch, b as u8))).collect()
}

impl Tokenizer {
    pub fn from_json(json: &str) -> Result<Self, SpeechError> {
        let bad = |m: &str| SpeechError::Model(format!("tokenizer.json: {m}"));
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| bad(&e.to_string()))?;
        let vocab = v["model"]["vocab"].as_object().ok_or_else(|| bad("no model.vocab"))?;
        let table = unicode_to_byte();
        let max = vocab.values().filter_map(|x| x.as_u64()).max().unwrap_or(0) as usize;
        let mut bytes = vec![Vec::new(); max + 1];
        for (tok, id) in vocab {
            let id = id.as_u64().ok_or_else(|| bad("vocab id"))? as usize;
            bytes[id] = tok.chars().filter_map(|c| table.get(&c).copied()).collect();
        }
        let mut special = HashMap::new();
        let mut special_by_id = HashMap::new();
        for a in v["added_tokens"].as_array().ok_or_else(|| bad("no added_tokens"))? {
            let (Some(id), Some(c)) = (a["id"].as_u64(), a["content"].as_str()) else { continue };
            special.insert(c.to_string(), id as u32);
            special_by_id.insert(id as u32, c.to_string());
        }
        let get = |k: &str| special.get(k).copied();
        let need = |k: &str| get(k).ok_or_else(|| bad(&format!("no {k}")));
        let sot = need("<|startoftranscript|>")?;
        let translate = get("<|translate|>");
        let mut languages: Vec<(String, u32)> = special
            .iter()
            .filter(|(k, id)| **id > sot && translate.is_none_or(|t| **id < t) && k.starts_with("<|") && k.ends_with("|>"))
            .map(|(k, id)| (k[2..k.len() - 2].to_string(), *id))
            .collect();
        languages.sort_by_key(|x| x.1);
        Ok(Self {
            eot: need("<|endoftext|>")?,
            sot,
            transcribe: need("<|transcribe|>")?,
            translate,
            no_timestamps: need("<|notimestamps|>")?,
            no_speech: get("<|nospeech|>").or_else(|| get("<|nocaptions|>")),
            timestamp_begin: need("<|0.00|>")?,
            languages,
            bytes,
            special,
            special_by_id,
        })
    }

    pub fn multilingual(&self) -> bool {
        !self.languages.is_empty()
    }

    pub fn language_token(&self, code: &str) -> Option<u32> {
        self.languages.iter().find(|(c, _)| c == code).map(|x| x.1)
    }

    pub fn is_timestamp(&self, id: u32) -> bool {
        id >= self.timestamp_begin
    }

    /// Bytes of a regular token (empty for special tokens).
    pub fn token_bytes(&self, id: u32) -> &[u8] {
        if self.special_by_id.contains_key(&id) {
            return &[];
        }
        self.bytes.get(id as usize).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        let b: Vec<u8> = ids.iter().flat_map(|&i| self.token_bytes(i).iter().copied()).collect();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// Ids of all special tokens that must never be sampled as text (everything special except
    /// end-of-text and the timestamps).
    pub fn non_text_specials(&self) -> Vec<u32> {
        self.special.values().copied().filter(|&id| id != self.eot && id < self.timestamp_begin).collect()
    }
}

/// Kana, kanji and half-width kana: scripts written without spaces between words.
fn is_unspaced_script(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F | 0x20000..=0x323AF)
}

/// Group text tokens into words: a token whose text starts with a space starts a new word;
/// punctuation-only tokens join the word before them (opening quotes/brackets the word after).
/// Japanese and Chinese have no spaces: there every token is a word of its own (the model's
/// tokens are word-sized pieces: 今日 / は / 東京 / の / 天気), so each gets its own time, and a
/// character split over two tokens stays in one word. Returns (word text, token index range).
pub fn group_words(tok: &Tokenizer, ids: &[u32]) -> Vec<(String, std::ops::Range<usize>)> {
    let mut groups: Vec<(Vec<u8>, std::ops::Range<usize>)> = Vec::new();
    let mut pending_open: Option<(Vec<u8>, usize)> = None;
    for (i, &id) in ids.iter().enumerate() {
        let b = tok.token_bytes(id);
        if b.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(b);
        let trimmed = text.trim();
        let starts_space = b[0] == b' ';
        let punct = !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_punctuation() || "“”‘’«»¿¡…。，！？：、".contains(c));
        let opening = punct && trimmed.chars().all(|c| "\"'“‘¿¡([{«-".contains(c)) && starts_space;
        if opening {
            let start = pending_open.as_ref().map(|p| p.1).unwrap_or(i);
            let mut acc = pending_open.take().map(|p| p.0).unwrap_or_default();
            acc.extend_from_slice(b);
            pending_open = Some((acc, start));
            continue;
        }
        if let Some((mut pre, start)) = pending_open.take() {
            pre.extend_from_slice(b);
            groups.push((pre, start..i + 1));
            continue;
        }
        // text without spaces: a new word at every token, once the word before is whole characters
        let unspaced = !starts_space
            && !punct
            && groups.last().is_some_and(|g| {
                std::str::from_utf8(&g.0)
                    .is_ok_and(|prev| text.chars().next().is_some_and(is_unspaced_script) || prev.chars().next_back().is_some_and(is_unspaced_script))
            });
        if unspaced {
            groups.push((b.to_vec(), i..i + 1));
            continue;
        }
        match groups.last_mut() {
            Some(g) if !starts_space => {
                g.0.extend_from_slice(b);
                g.1.end = i + 1;
            }
            Some(g) if punct => {
                // " ," style punctuation with a space still belongs to the previous word
                g.0.extend_from_slice(b);
                g.1.end = i + 1;
            }
            _ => groups.push((b.to_vec(), i..i + 1)),
        }
    }
    if let Some((pre, start)) = pending_open {
        groups.push((pre, start..ids.len()));
    }
    groups.into_iter().map(|(b, r)| (String::from_utf8_lossy(&b).trim().to_string(), r)).filter(|(t, _)| !t.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tokenizer whose regular tokens are `tokens` (id = index).
    fn tokenizer(tokens: &[&[u8]]) -> Tokenizer {
        let n = tokens.len() as u32;
        Tokenizer {
            bytes: tokens.iter().map(|t| t.to_vec()).collect(),
            special: HashMap::new(),
            special_by_id: HashMap::new(),
            eot: n,
            sot: n + 1,
            transcribe: n + 2,
            translate: None,
            no_timestamps: n + 3,
            no_speech: None,
            timestamp_begin: n + 4,
            languages: Vec::new(),
        }
    }

    fn words(tokens: &[&[u8]]) -> Vec<(String, std::ops::Range<usize>)> {
        let ids: Vec<u32> = (0..tokens.len() as u32).collect();
        group_words(&tokenizer(tokens), &ids)
    }

    fn texts(tokens: &[&[u8]]) -> Vec<String> {
        words(tokens).into_iter().map(|w| w.0).collect()
    }

    #[test]
    fn spaced_languages_group_by_the_leading_space() {
        assert_eq!(texts(&[b" Hel", b"lo", b",", b" wor", b"ld", b"!"]), ["Hello,", "world!"]);
        assert_eq!(texts(&[b" The", b" \"", b"quick", b"\"", b" fox"]), ["The", "\"quick\"", "fox"]);
        // a first token without a space still starts the first word
        assert_eq!(texts(&[b"To", b"kyo", b" Tower"]), ["Tokyo", "Tower"]);
    }

    /// Japanese has no spaces: every token is a word with its own time, punctuation stays with
    /// the word before it. (Before: the whole sentence was one word spanning all of the audio.)
    #[test]
    fn japanese_gets_a_word_per_token() {
        let t: Vec<&[u8]> = ["こんにちは", "。", "今日", "は", "東京", "の", "天気", "です", "。"].iter().map(|s| s.as_bytes()).collect();
        let w = words(&t);
        assert_eq!(w.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["こんにちは。", "今日", "は", "東京", "の", "天気", "です。"]);
        assert_eq!(w[0].1, 0..2, "the full stop is part of the first word's token range");
        assert_eq!(w[1].1, 2..3);
        // every token is in exactly one word, in order
        assert_eq!(w.iter().map(|x| x.1.len()).sum::<usize>(), t.len());
        assert!(w.windows(2).all(|p| p[0].1.end == p[1].1.start));
    }

    #[test]
    fn mixed_scripts_and_characters_split_over_tokens() {
        // Latin right after Japanese (and the reverse) starts a new word; Latin pieces still join
        assert_eq!(texts(&["東京".as_bytes(), b"To", b"kyo", "へ".as_bytes(), "行く".as_bytes()]), ["東京", "Tokyo", "へ", "行く"]);
        assert_eq!(texts(&[b" Rust", "入門".as_bytes()]), ["Rust", "入門"]);
        // 京 (E4 BA AC) split over two tokens stays one word, between 東 and の
        assert_eq!(texts(&["東".as_bytes(), &[0xE4, 0xBA], &[0xAC], "の".as_bytes()]), ["東", "京", "の"]);
        // a character that never completes does not swallow what follows for good
        let w = texts(&["東".as_bytes(), &[0xE4, 0xBA], "の".as_bytes(), "町".as_bytes()]);
        assert_eq!(w.len(), 2, "{w:?}");
        // empty and special tokens are skipped; no tokens, no words
        assert_eq!(texts(&[b"", "雨".as_bytes(), b""]), ["雨"]);
        assert!(texts(&[]).is_empty());
    }
}
