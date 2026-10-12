//! Ruby (furigana): small reading text set beside the characters it annotates.
//!
//! The markup is the Aozora Bunko notation, the common plain-text convention for Japanese:
//!
//! - `漢字《かんじ》`: the ruby annotates the run of kanji right before `《`;
//! - `｜東京スカイツリー《とうきょうスカイツリー》`: a bar (full-width `｜` or `|`) marks where the
//!   annotated text starts, for bases that are not just kanji.
//!
//! [`parse`] splits marked-up text into the plain text and its rubies; markup that is not well
//! formed (no closing `》`, nothing to annotate, an empty reading) stays in the text as typed.
//! [`layout`] lays the plain text out with room for the readings, sets each reading at half size
//! centred over its base (to the right of it in vertical text), and reports every position in
//! byte offsets of the *marked-up* text, so carets and selections keep working on what was typed.
//!
//! It is on when [`ParagraphStyle::ruby`] is set.

use std::ops::Range;

use crate::fonts;
use crate::layout::{Align, Layout, ParagraphStyle, StyleRun, TextStyle, layout_rich_uncached, layout_uncached};

/// Ruby size relative to the base text.
const SCALE: f32 = 0.5;
/// Most characters in one reading, and most readings in one text (more stay as typed).
const MAX_READING: usize = 64;
const MAX_RUBIES: usize = 4096;

/// One reading: `text` annotates `base`, a byte range of [`Parsed::plain`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ruby {
    pub base: Range<usize>,
    pub text: String,
}

/// Marked-up text taken apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// The text without the markup.
    pub plain: String,
    pub rubies: Vec<Ruby>,
    /// For every byte offset of `plain` (and one past its end), the offset in the marked-up text.
    to_raw: Vec<usize>,
    raw_len: usize,
}

impl Parsed {
    /// Offset in the marked-up text of a byte offset of `plain`.
    pub fn raw_offset(&self, plain: usize) -> usize {
        self.to_raw.get(plain).copied().unwrap_or(self.raw_len)
    }

    /// Offset in `plain` of a byte offset of the marked-up text (offsets inside markup give the
    /// position right after it).
    pub fn plain_offset(&self, raw: usize) -> usize {
        self.to_raw.partition_point(|r| *r < raw).min(self.plain.len())
    }
}

/// Whether `c` belongs to a kanji run (what a reading annotates when no bar says otherwise).
pub fn is_kanji(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x323AF) || matches!(c, '々' | '〆' | 'ヶ' | '〇')
}

fn is_bar(c: char) -> bool {
    c == '｜' || c == '|'
}

/// The reading that starts at `rest` (right after a `《`): its text and its length in bytes
/// including the closing `》`. None when it is not closed on its line, empty or too long.
fn reading(rest: &str) -> Option<(&str, usize)> {
    let end = rest.find(['》', '\n'])?;
    let text = rest.get(..end)?;
    if !rest.get(end..)?.starts_with('》') || text.is_empty() || text.contains('《') || text.chars().count() > MAX_READING {
        return None;
    }
    Some((text, end + '》'.len_utf8()))
}

/// Take marked-up text apart (see the module documentation).
pub fn parse(raw: &str) -> Parsed {
    let mut out = Parsed { raw_len: raw.len(), ..Default::default() };
    // where the base of the next ruby starts, when a bar said so
    let mut bar: Option<usize> = None;
    // where the previous ruby's base ended: a kanji run does not reach back into it
    let mut annotated = 0usize;
    let mut i = 0;
    while let Some(c) = raw.get(i..).and_then(|r| r.chars().next()) {
        let next = i + c.len_utf8();
        let rest = raw.get(next..).unwrap_or_default();
        if is_bar(c) {
            // a bar counts only when a reading follows on the line, with text in between
            let stop = rest.find(|x: char| x == '《' || x == '\n' || is_bar(x));
            let opens = stop.is_some_and(|s| s > 0 && rest.get(s..).is_some_and(|r| r.starts_with('《')));
            if opens && stop.and_then(|s| rest.get(s + '《'.len_utf8()..)).and_then(reading).is_some() && out.rubies.len() < MAX_RUBIES {
                bar = Some(out.plain.len());
                i = next;
                continue;
            }
        } else if c == '《'
            && out.rubies.len() < MAX_RUBIES
            && let Some((text, len)) = reading(rest)
        {
            let line_start = out.plain.rfind('\n').map_or(0, |n| n + 1).max(annotated);
            let start = bar.take().unwrap_or_else(|| {
                let line = out.plain.get(line_start..).unwrap_or_default();
                out.plain.len() - line.chars().rev().take_while(|k| is_kanji(*k)).map(char::len_utf8).sum::<usize>()
            });
            if start < out.plain.len() {
                out.rubies.push(Ruby { base: start..out.plain.len(), text: text.to_string() });
                annotated = out.plain.len();
                i = next + len;
                continue;
            }
        }
        if c == '\n' {
            bar = None;
        }
        out.to_raw.extend(std::iter::repeat_n(i, c.len_utf8()));
        // (each byte of a character maps to the character's start plus its own index)
        let n = out.to_raw.len();
        for (k, slot) in out.to_raw.iter_mut().skip(n - c.len_utf8()).enumerate() {
            *slot = i + k;
        }
        out.plain.push(c);
        i = next;
    }
    out.to_raw.push(raw.len());
    out
}

/// Lay marked-up text out with its rubies. None when the text has none (the caller lays it out
/// as plain text).
pub(crate) fn layout(raw: &str, style: &TextStyle, runs: &[StyleRun], para: &ParagraphStyle) -> Option<Layout> {
    if !raw.contains('《') {
        return None;
    }
    let p = parse(raw);
    if p.rubies.is_empty() {
        return None;
    }
    let small = TextStyle { size: (style.size * SCALE).max(0.1), tracking: 0.0, baseline_shift: 0.0, underline: false, ..style.clone() };
    let sm = fonts::face(fonts::resolve(&small.family, &small.style).face).metrics(small.size);
    let room = sm.ascent + sm.descent;
    let runs: Vec<StyleRun> = runs
        .iter()
        .map(|r| StyleRun { range: p.plain_offset(r.range.start)..p.plain_offset(r.range.end), style: r.style.clone() })
        .filter(|r| r.range.start < r.range.end)
        .collect();
    // the readings need room between the lines (between the columns in vertical text)
    let inner = ParagraphStyle { ruby: false, leading: para.leading + room, ..para.clone() };
    let mut lay = layout_rich_uncached(&p.plain, style, &runs, &inner);
    let gap = style.size * 0.04;
    let mut extra = Vec::new();
    let (mut top, mut right) = (lay.bounds[1], lay.bounds[2]);
    for r in &p.rubies {
        let rl = layout_uncached(&r.text, &small, &ParagraphStyle { align: Align::Center, vertical: para.vertical, ..Default::default() });
        if rl.glyphs.is_empty() {
            continue;
        }
        // the style run of the base's first glyph: the reading is painted like its base
        let run = lay.glyphs.iter().find(|g| r.base.contains(&g.cluster)).map_or(0, |g| g.run);
        let (dx, dy) = if para.vertical {
            // one line per character: the cells of the base, top to bottom
            let mut cells = lay.lines.iter().filter(|l| l.range.start >= r.base.start && l.range.start < r.base.end);
            let Some(first) = cells.next() else { continue };
            let last = cells.next_back().unwrap_or(first);
            let centre = (first.baseline - first.ascent + last.baseline + last.descent) / 2.0;
            let dx = first.x + first.width / 2.0 + style.size * 0.5 + small.size * 0.5 + gap;
            right = right.max(dx + small.size * 0.5);
            (dx, centre - (rl.bounds[1] + rl.bounds[3]) / 2.0)
        } else {
            let Some(line) = lay.lines.get(lay.line_of(r.base.start)) else { continue };
            let stop = |at: usize, or: f32| line.carets.iter().find(|c| c.0 == at).map_or(or, |c| c.1);
            let (a, b) = (stop(r.base.start, line.x), stop(r.base.end.min(line.range.end), line.x + line.width));
            let dy = line.baseline - line.ascent - sm.descent - gap;
            top = top.min(dy - sm.ascent);
            ((a + b) / 2.0 - (rl.bounds[0] + rl.bounds[2]) / 2.0, dy)
        };
        extra.extend(rl.glyphs.iter().map(|g| {
            let mut g = *g;
            g.x += dx;
            g.y += dy;
            g.cluster = r.base.start;
            g.run = run;
            g
        }));
    }
    lay.glyphs.extend(extra);
    lay.bounds[1] = top;
    lay.bounds[2] = right;
    // positions are reported in the text as typed
    for g in &mut lay.glyphs {
        g.cluster = p.raw_offset(g.cluster);
    }
    for l in &mut lay.lines {
        l.range = p.raw_offset(l.range.start)..p.raw_offset(l.range.end);
        for c in &mut l.carets {
            c.0 = p.raw_offset(c.0);
        }
    }
    lay.text_len = raw.len();
    Some(lay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rubies(raw: &str) -> (String, Vec<(String, String)>) {
        let p = parse(raw);
        let list = p.rubies.iter().map(|r| (p.plain[r.base.clone()].to_string(), r.text.clone())).collect();
        (p.plain, list)
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    #[test]
    fn aozora_notation() {
        // the kanji run before 《 is the base
        assert_eq!(rubies("今日は東京《とうきょう》へ行く"), ("今日は東京へ行く".into(), vec![pair("東京", "とうきょう")]));
        // a bar starts a base that is not just kanji; the ASCII bar works too
        assert_eq!(rubies("｜東京スカイツリー《とうきょうスカイツリー》だ").1, vec![pair("東京スカイツリー", "とうきょうスカイツリー")]);
        assert_eq!(rubies("the |Rust《ラスト》 book"), ("the Rust book".into(), vec![pair("Rust", "ラスト")]));
        // several, on several lines; 々 belongs to the kanji run
        let (plain, r) = rubies("時々《ときどき》晴《は》れ\n｜A《エー》と漢字《かんじ》");
        assert_eq!(plain, "時々晴れ\nAと漢字");
        assert_eq!(r, vec![pair("時々", "ときどき"), pair("晴", "は"), pair("A", "エー"), pair("漢字", "かんじ")]);
    }

    #[test]
    fn markup_that_is_not_ruby_stays_as_typed() {
        for raw in [
            "かな《よみ》",   // nothing to annotate: kana before 《 and no bar
            "《よみ》",       // at the start of the text
            "漢字《よみ",     // not closed
            "漢字《》",       // empty reading
            "漢字《よ\nみ》", // not closed on its line
            "a | b | c",      // bars that open nothing
            "｜《よみ》",     // a bar with nothing after it
            "｜漢字",         // a bar without a reading
            "漢字》",
        ] {
            let p = parse(raw);
            assert_eq!((p.plain.as_str(), p.rubies.len()), (raw, 0), "{raw:?}");
        }
        // a reading longer than the limit stays as typed
        let long = format!("漢字《{}》", "あ".repeat(MAX_READING + 1));
        assert_eq!(parse(&long).plain, long);
    }

    #[test]
    fn offsets_map_between_the_typed_and_the_plain_text() {
        let raw = "A｜東京《とうきょう》B";
        let p = parse(raw);
        assert_eq!(p.plain, "A東京B");
        // plain → typed: A, 東 (after the bar), 京, B (after the reading), end
        assert_eq!(
            (0..=p.plain.len()).filter(|i| p.plain.is_char_boundary(*i)).map(|i| p.raw_offset(i)).collect::<Vec<_>>(),
            [0, 4, 7, raw.len() - 1, raw.len()]
        );
        // typed → plain: offsets inside the markup land right after it
        assert_eq!(p.plain_offset(0), 0);
        assert_eq!(p.plain_offset(1), 1, "the bar is before 東");
        assert_eq!(p.plain_offset(4), 1);
        assert_eq!(p.plain_offset(10), 7, "inside the reading: after 京");
        assert_eq!(p.plain_offset(raw.len()), p.plain.len());
        assert_eq!(p.plain_offset(usize::MAX), p.plain.len());
        assert_eq!(p.raw_offset(usize::MAX), raw.len());
    }

    #[test]
    fn readings_sit_over_their_bases_at_half_size() {
        let style = TextStyle { size: 100.0, ..Default::default() };
        let on = ParagraphStyle { ruby: true, ..Default::default() };
        let raw = "東京《とうきょう》タワー";
        let plain = layout_uncached("東京タワー", &style, &ParagraphStyle::default());
        let l = layout_uncached(raw, &style, &on);
        assert_eq!(l.text_len, raw.len());
        let base: Vec<_> = l.glyphs.iter().filter(|g| g.size == 100.0).collect();
        let small: Vec<_> = l.glyphs.iter().filter(|g| g.size == 50.0).collect();
        assert_eq!((base.len(), small.len()), (5, 5), "five characters and a five-kana reading");
        assert!(l.glyphs.iter().all(|g| g.id != 0), "no missing glyphs");
        // the base text is where it would be without the reading
        for (a, b) in base.iter().zip(&plain.glyphs) {
            assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
        }
        // the reading is above the line and centred over 東京 (the first two characters)
        let line = &l.lines[0];
        assert!(small.iter().all(|g| g.y < line.baseline - line.ascent + 1.0), "above the base's ascent");
        let (x0, x1) = (small[0].x, small[4].x + 50.0);
        let centre = (base[0].x + base[2].x) / 2.0;
        assert!(((x0 + x1) / 2.0 - centre).abs() < 6.0, "reading {x0}..{x1}, base centre {centre}");
        assert!(l.bounds[1] < plain.bounds[1] - 30.0, "the bounds include the reading");
        // carets are offsets of the typed text and stay on character boundaries of it
        assert!(line.carets.iter().all(|c| raw.is_char_boundary(c.0)));
        assert_eq!(line.carets.first().map(|c| c.0), Some(0));
        assert_eq!(line.carets.last().map(|c| c.0), Some(raw.len()));
        // lines with readings are further apart than lines without
        let two = layout_uncached("漢字《かんじ》\n漢字《かんじ》", &style, &on);
        let two_plain = layout_uncached("漢字\n漢字", &style, &ParagraphStyle::default());
        assert!(two.lines[1].baseline - two.lines[0].baseline > two_plain.lines[1].baseline - two_plain.lines[0].baseline + 40.0);
    }

    #[test]
    fn off_by_default_and_without_markup_nothing_changes() {
        let style = TextStyle { size: 80.0, ..Default::default() };
        let raw = "東京《とうきょう》";
        let off = layout_uncached(raw, &style, &ParagraphStyle::default());
        assert!(off.glyphs.iter().all(|g| g.size == 80.0) && off.glyphs.len() == 9, "the markup is drawn as typed");
        let on = ParagraphStyle { ruby: true, ..Default::default() };
        assert_eq!(layout_uncached("ふつうの文 plain", &style, &on), layout_uncached("ふつうの文 plain", &style, &ParagraphStyle::default()));
    }

    #[test]
    fn vertical_text_puts_readings_to_the_right() {
        let style = TextStyle { size: 100.0, ..Default::default() };
        let l = layout_uncached("東京《とうきょう》都", &style, &ParagraphStyle { ruby: true, vertical: true, ..Default::default() });
        let base: Vec<_> = l.glyphs.iter().filter(|g| g.size == 100.0).collect();
        let small: Vec<_> = l.glyphs.iter().filter(|g| g.size == 50.0).collect();
        assert_eq!((base.len(), small.len()), (3, 5));
        assert!(small.iter().all(|g| g.x > base[0].x + 60.0), "right of the column");
        // stacked top to bottom beside 東京, not beside 都
        assert!(small.windows(2).all(|w| w[1].y > w[0].y));
        let (top, bottom) = (l.lines[0].baseline - l.lines[0].ascent, l.lines[1].baseline + l.lines[1].descent);
        let mid = (small[0].y + small[4].y) / 2.0;
        assert!(mid > top - 30.0 && mid < bottom + 30.0, "reading centre {mid}, base {top}..{bottom}");
    }

    #[test]
    fn hostile_text_never_panics() {
        let style = TextStyle { size: 40.0, ..Default::default() };
        let on = ParagraphStyle { ruby: true, width: Some(120.0), ..Default::default() };
        let mut seed = 0x9E37_79B9u32;
        let alphabet = ['｜', '|', '《', '》', '漢', '字', 'あ', 'a', '\n', ' ', '々', '\u{202e}', '🎬'];
        for n in 0..400 {
            let mut s = String::new();
            for _ in 0..(n % 40) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                s.push(alphabet[seed as usize % alphabet.len()]);
            }
            let p = parse(&s);
            assert!(p.rubies.iter().all(|r| r.base.start < r.base.end && p.plain.get(r.base.clone()).is_some()), "{s:?}");
            for vertical in [false, true] {
                let l = layout_uncached(&s, &style, &ParagraphStyle { vertical, ..on.clone() });
                assert!(l.glyphs.iter().all(|g| g.x.is_finite() && g.y.is_finite() && g.cluster <= s.len()), "{s:?}");
                assert!(l.lines.iter().all(|line| line.carets.iter().all(|c| c.0 <= s.len())), "{s:?}");
            }
        }
        // many rubies and a huge text stay bounded
        let many = "漢《か》".repeat(MAX_RUBIES + 50);
        assert_eq!(parse(&many).rubies.len(), MAX_RUBIES);
    }
}
