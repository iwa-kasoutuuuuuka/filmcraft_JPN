//! Mouth shapes per Japanese vowel, from the reading of a line and the stretches where the
//! voice is loud.
//!
//! The text says which vowels are spoken, in order; the voice says when the mouth is open. The
//! vowels are spread evenly over the open time, which is how a hand-timed anime mouth is done
//! (one picture per beat, not per phoneme). It is a fit, not an alignment: the mora of a line are
//! taken to be equally long, so a long drawn-out sound gets the same share as a short one.
//!
//! Pure functions; times are seconds.

/// Most mora taken from one line (a longer text is cut: the mouth runs on with the last vowel).
pub const MAX_MORA: usize = 4_096;

/// One beat of speech.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mora {
    /// `a`, `i`, `u`, `e` or `o`.
    Vowel(char),
    /// A sound with the mouth shut or nearly so (ん, っ).
    Closed,
    /// A character whose reading is not known (kanji, letters): the mouth is open, shape unknown.
    Unknown,
}

const ROWS: [&str; 14] = [
    "あいうえお",
    "かきくけこ",
    "がぎぐげご",
    "さしすせそ",
    "ざじずぜぞ",
    "たちつてと",
    "だぢづでど",
    "なにぬねの",
    "はひふへほ",
    "ばびぶべぼ",
    "ぱぴぷぺぽ",
    "まみむめも",
    "らりるれろ",
    "ぁぃぅぇぉ",
];

fn to_hiragana(c: char) -> char {
    // katakana ァ..ヶ sit 0x60 above hiragana ぁ..ゖ
    match c as u32 {
        0x30A1..=0x30F6 => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn vowel_of(c: char) -> Option<char> {
    for r in ROWS {
        if let Some(k) = r.chars().position(|x| x == c) {
            return "aiueo".chars().nth(k);
        }
    }
    match c {
        'や' | 'わ' => Some('a'),
        'ゆ' | 'ゔ' => Some('u'),
        'よ' | 'を' => Some('o'),
        'ゐ' => Some('i'),
        'ゑ' => Some('e'),
        _ => None,
    }
}

fn is_kanji(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3005 | 0x3007)
}

/// The mora of `text` (a reading: kana, with whatever else is left counted as unknown). Small
/// ゃゅょ and ぁぃぅぇぉ after a kana change that kana's vowel; ー repeats it; punctuation and
/// spaces are skipped. A kanji counts as two unknown mora.
pub fn moras(text: &str) -> Vec<Mora> {
    let mut out: Vec<Mora> = Vec::new();
    for raw in text.chars() {
        if out.len() >= MAX_MORA {
            break;
        }
        let c = to_hiragana(raw);
        match c {
            'っ' | 'ん' => out.push(Mora::Closed),
            'ー' | '〜' | '～' => {
                if let Some(last) = out.last().copied() {
                    out.push(last);
                }
            }
            'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' => {
                let v = match c {
                    'ゃ' | 'ゎ' | 'ぁ' => 'a',
                    'ゅ' | 'ぅ' => 'u',
                    'ょ' | 'ぉ' => 'o',
                    'ぃ' => 'i',
                    _ => 'e',
                };
                match out.last_mut() {
                    Some(m @ Mora::Vowel(_)) => *m = Mora::Vowel(v),
                    _ => out.push(Mora::Vowel(v)),
                }
            }
            _ => {
                if let Some(v) = vowel_of(c) {
                    out.push(Mora::Vowel(v));
                } else if is_kanji(c) {
                    out.push(Mora::Unknown);
                    out.push(Mora::Unknown);
                } else if c.is_alphanumeric() {
                    out.push(Mora::Unknown);
                }
            }
        }
    }
    out.truncate(MAX_MORA);
    out
}

/// A stretch of one open-mouth shape: `vowel` is `None` for the generic open mouth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VowelSegment {
    pub start: f64,
    pub end: f64,
    pub vowel: Option<char>,
}

fn push_joined(out: &mut Vec<VowelSegment>, start: f64, end: f64, vowel: Option<char>) {
    if end <= start {
        return;
    }
    match out.last_mut() {
        Some(l) if l.vowel == vowel && (start - l.end).abs() < 1e-9 => l.end = end,
        _ => out.push(VowelSegment { start, end, vowel }),
    }
}

/// Spreads `moras` evenly over the open `spans` (seconds, in order, apart). Closed mora leave a
/// gap; neighbours of the same shape join. With no mora the spans come back as generic open
/// mouths. Spans that are empty, reversed or not finite are ignored.
pub fn fit(spans: &[(f64, f64)], moras: &[Mora]) -> Vec<VowelSegment> {
    let spans: Vec<(f64, f64)> = spans.iter().copied().filter(|(a, b)| a.is_finite() && b.is_finite() && b > a).collect();
    let total: f64 = spans.iter().map(|(a, b)| b - a).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<VowelSegment> = Vec::new();
    if moras.is_empty() {
        for &(start, end) in &spans {
            push_joined(&mut out, start, end, None);
        }
        return out;
    }
    let n = moras.len() as f64;
    for (k, m) in moras.iter().enumerate() {
        let vowel = match m {
            Mora::Closed => continue,
            Mora::Vowel(v) => Some(*v),
            Mora::Unknown => None,
        };
        // this beat's share of the open time, cut where it crosses a gap between open stretches
        let (x0, x1) = (total * k as f64 / n, total * (k + 1) as f64 / n);
        let mut cursor = 0.0;
        for &(a, b) in &spans {
            let (lo, hi) = (x0.max(cursor), x1.min(cursor + (b - a)));
            if hi > lo {
                push_joined(&mut out, a + (lo - cursor), a + (hi - cursor), vowel);
            }
            cursor += b - a;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Mora::*;

    #[test]
    fn kana_give_their_vowels() {
        assert_eq!(moras("あいうえお"), vec![Vowel('a'), Vowel('i'), Vowel('u'), Vowel('e'), Vowel('o')]);
        assert_eq!(moras("かさたなは"), vec![Vowel('a'); 5]);
        assert_eq!(moras("コンニチハ"), vec![Vowel('o'), Closed, Vowel('i'), Vowel('i'), Vowel('a')]);
        assert_eq!(moras("ぱぴぷぺぽ"), vec![Vowel('a'), Vowel('i'), Vowel('u'), Vowel('e'), Vowel('o')]);
        assert_eq!(moras("やゆよわを"), vec![Vowel('a'), Vowel('u'), Vowel('o'), Vowel('a'), Vowel('o')]);
    }

    #[test]
    fn small_kana_long_marks_and_stops() {
        // きゃ is one beat with the vowel of ゃ; ふぁ likewise
        assert_eq!(moras("きゃ"), vec![Vowel('a')]);
        assert_eq!(moras("しゅっぱつ"), vec![Vowel('u'), Closed, Vowel('a'), Vowel('u')]);
        assert_eq!(moras("ふぁ"), vec![Vowel('a')]);
        assert_eq!(moras("おかあさーん"), vec![Vowel('o'), Vowel('a'), Vowel('a'), Vowel('a'), Vowel('a'), Closed]);
        assert!(moras("ー").is_empty(), "nothing to repeat");
        assert_eq!(moras("ゃ"), vec![Vowel('a')], "a stray small kana is a beat of its own");
    }

    #[test]
    fn punctuation_is_skipped_and_other_text_is_unknown() {
        assert_eq!(moras("あ、い。 う！？…"), vec![Vowel('a'), Vowel('i'), Vowel('u')]);
        assert_eq!(moras("漢"), vec![Unknown, Unknown]);
        assert_eq!(moras("Aあ"), vec![Unknown, Vowel('a')]);
        assert!(moras("").is_empty());
        let long = "あ".repeat(MAX_MORA * 2);
        assert_eq!(moras(&long).len(), MAX_MORA);
    }

    #[test]
    fn mora_are_spread_over_the_open_time() {
        let v = fit(&[(0.0, 1.0)], &moras("あいうえ"));
        let got: Vec<(f64, f64, Option<char>)> = v.iter().map(|s| (s.start, s.end, s.vowel)).collect();
        assert_eq!(got, vec![(0.0, 0.25, Some('a')), (0.25, 0.5, Some('i')), (0.5, 0.75, Some('u')), (0.75, 1.0, Some('e'))]);
    }

    #[test]
    fn a_gap_between_open_stretches_takes_no_time_and_cuts_a_beat() {
        // two open stretches of 0.5 s with a long gap between; three beats of 1/3 s each
        let v = fit(&[(0.0, 0.5), (2.0, 2.5)], &moras("あいう"));
        assert_eq!(v.first().map(|s| (s.start, s.vowel)), Some((0.0, Some('a'))));
        assert!(v.windows(2).all(|w| w[0].end <= w[1].start + 1e-9), "{v:?}");
        assert!(v.iter().all(|s| (s.start >= 0.0 && s.end <= 0.5 + 1e-9) || (s.start >= 2.0 - 1e-9 && s.end <= 2.5 + 1e-9)), "{v:?}");
        // the middle beat straddles the gap: it appears on both sides
        assert_eq!(v.iter().filter(|s| s.vowel == Some('i')).count(), 2, "{v:?}");
    }

    #[test]
    fn closed_beats_leave_a_gap_and_equal_neighbours_join() {
        let v = fit(&[(0.0, 0.9)], &moras("あんい"));
        assert_eq!(v.len(), 2);
        assert!((v[0].end - 0.3).abs() < 1e-9 && (v[1].start - 0.6).abs() < 1e-9, "{v:?}");
        let joined = fit(&[(0.0, 1.0)], &moras("ああ"));
        assert_eq!(joined, vec![VowelSegment { start: 0.0, end: 1.0, vowel: Some('a') }]);
    }

    #[test]
    fn no_reading_gives_the_generic_open_mouth_and_bad_spans_are_ignored() {
        let v = fit(&[(0.0, 1.0), (2.0, 3.0)], &[]);
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|s| s.vowel.is_none()));
        assert!(fit(&[], &moras("あ")).is_empty());
        assert!(fit(&[(1.0, 1.0), (f64::NAN, 2.0), (3.0, 2.0)], &moras("あ")).is_empty());
        assert!(fit(&[(0.0, 1.0)], &[Closed, Closed]).is_empty());
    }
}
