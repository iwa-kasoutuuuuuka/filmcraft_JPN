//! Tests of [`crate::text_style`]: text style presets and ruby on text layers of the demo project.

use serde_json::json;

use crate::Session;
use crate::text_style::add_ruby;
use filmcraft_project::graphic::{LayerContent, LayerSpec, eval_layer, layer_indices};
use filmcraft_project::{ClipId, ParamValue};
use filmcraft_time::Tick;

/// The demo project with a new text layer holding `text`, selected.
fn session(text: &str) -> (Session, ClipId) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let r = s.execute("graphics.newText", json!({"text": text, "position": [960, 540], "size": 120})).unwrap();
    (s, ClipId(r["clip"].as_u64().unwrap()))
}

fn layer(s: &Session, clip: ClipId) -> LayerSpec {
    let q = s.active_sequence().unwrap();
    let (_, it) = q.find_item(clip).unwrap();
    let i = layer_indices(&it.effects)[0];
    eval_layer(&it.effects[i], Tick::ZERO, (q.settings.width, q.settings.height)).unwrap()
}

fn text(l: &LayerSpec) -> &filmcraft_project::graphic::TextProps {
    match &l.content {
        LayerContent::Text(t) => t,
        _ => panic!("not text"),
    }
}

fn rgb(hex: &str) -> [u8; 3] {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap();
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

fn close(c: [f32; 4], hex: &str) -> bool {
    let want = rgb(hex);
    (0..3).all(|k| (c[k] * 255.0 - want[k] as f32).abs() < 1.5)
}

#[test]
fn every_style_sets_the_whole_appearance_in_one_undo_step() {
    let (mut s, clip) = session("テロップ Title");
    let plain = layer(&s, clip);
    let list = s.execute("graphics.textStyles", json!({})).unwrap();
    let styles = list["styles"].as_array().unwrap();
    assert_eq!(styles.len(), 8);
    let mut seen = Vec::new();
    for st in styles {
        let id = st["id"].as_str().unwrap();
        let steps = s.history.undo.len();
        let r = s.execute("graphics.textStyle", json!({"style": id})).unwrap();
        assert_eq!((r["style"].as_str(), r["clip"].as_u64()), (Some(id), Some(clip.0)));
        assert_eq!(s.history.undo.len(), steps + 1, "{id}: one undo step");
        assert_eq!(s.history.undo.last().unwrap().0, "Apply Text Style");
        let l = layer(&s, clip);
        // what the style says, and nothing left of the style before it
        let p = &st["props"];
        assert!(close(l.appearance.fill.unwrap(), p["fill_color"].as_str().unwrap()), "{id}: fill");
        let outlines = [p["stroke"].as_bool().unwrap(), p["stroke2"].as_bool().unwrap()].iter().filter(|b| **b).count();
        assert_eq!(l.appearance.strokes.len(), outlines, "{id}: outlines");
        assert_eq!(l.appearance.background.is_some(), p["background"].as_bool().unwrap(), "{id}: background");
        assert_eq!(l.appearance.shadow.is_some(), p["shadow"].as_bool().unwrap(), "{id}: shadow");
        // the text, its font, size and place are the user's
        let (a, b) = (text(&plain), text(&l));
        assert_eq!((&a.text, &a.font, a.size), (&b.text, &b.font, b.size), "{id}");
        assert_eq!(plain.transform.position, l.transform.position, "{id}");
        s.active_sequence().unwrap().check().unwrap();
        assert!(!seen.contains(&l.appearance), "{id} looks different from the other styles");
        seen.push(l.appearance.clone());
    }
    // a double outline: the coloured outline inside, the wider white one outside
    s.execute("graphics.textStyle", json!({"style": "fukuro-red"})).unwrap();
    let l = layer(&s, clip);
    assert!(close(l.appearance.strokes[0].0, "#e60012") && close(l.appearance.strokes[1].0, "#ffffff"));
    assert!(l.appearance.strokes[1].1 > l.appearance.strokes[0].1, "the outer outline is the wider one");
    // by name too; undo restores the previous style
    s.execute("graphics.textStyle", json!({"style": "Soft Shadow"})).unwrap();
    assert!(layer(&s, clip).appearance.strokes.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, clip).appearance.strokes.len(), 2);
}

#[test]
fn styles_change_the_picture() {
    let (mut s, _) = session("STYLE");
    let before = s.render_program(0.25).unwrap();
    s.execute("graphics.textStyle", json!({"style": "fukuro-pop"})).unwrap();
    let after = s.render_program(0.25).unwrap();
    let changed = before.px.iter().zip(&after.px).filter(|(a, b)| (**a - **b).abs() > 0.02).count();
    assert!(changed > 500, "the outlines and the pink fill are drawn: {changed} samples differ");
}

#[test]
fn ruby_command_marks_the_text_and_turns_ruby_on() {
    let (mut s, clip) = session("今日は東京へ行く。東京は晴れ");
    assert!(!text(&layer(&s, clip)).ruby);
    let r = s.execute("graphics.ruby", json!({"base": "東京", "reading": "とうきょう"})).unwrap();
    assert_eq!((r["marked"].as_u64(), r["enabled"].as_bool()), (Some(1), Some(true)), "{r}");
    let l = layer(&s, clip);
    assert_eq!(text(&l).text, "今日は東京《とうきょう》へ行く。東京は晴れ", "the first one, no bar needed after kana");
    assert!(text(&l).ruby);
    assert_eq!(s.history.undo.last().unwrap().0, "Ruby");
    // the layout: the reading at half size, the text without the markup
    let lay = filmcraft_render::graphic_clip::text_layout(text(&l));
    let (big, small): (Vec<&filmcraft_text::Glyph>, Vec<&filmcraft_text::Glyph>) = lay.glyphs.iter().partition(|g| g.size > 100.0);
    assert_eq!((big.len(), small.len()), (14, 5), "14 characters and a 5-kana reading");
    assert!(small.iter().all(|g| (g.size - 60.0).abs() < 1e-3));
    // every occurrence that has no reading yet
    let r = s.execute("graphics.ruby", json!({"base": "東京", "reading": "とうきょう", "all": true})).unwrap();
    assert_eq!(r["marked"], 1, "the second one; the first already has a reading");
    assert_eq!(text(&layer(&s, clip)).text, "今日は東京《とうきょう》へ行く。東京《とうきょう》は晴れ");
    assert!(s.execute("graphics.ruby", json!({"base": "東京", "reading": "とうきょう", "all": true})).is_err(), "nothing left to mark");
    // a base that is not a kanji run gets a bar
    s.execute("graphics.ruby", json!({"base": "晴れ", "reading": "はれ"})).unwrap();
    assert!(text(&layer(&s, clip)).text.ends_with("は｜晴れ《はれ》"));
    // off: the markup is drawn as typed; on again; undo
    let r = s.execute("graphics.ruby", json!({"enabled": false})).unwrap();
    assert_eq!((r["marked"].as_u64(), r["enabled"].as_bool()), (Some(0), Some(false)));
    let l = layer(&s, clip);
    let raw_chars = text(&l).text.chars().count();
    assert_eq!(filmcraft_render::graphic_clip::text_layout(text(&l)).glyphs.len(), raw_chars);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(text(&layer(&s, clip)).ruby);
}

#[test]
fn ruby_works_on_a_layer_saved_before_the_property_existed() {
    let (mut s, clip) = session("漢字");
    // as an older project stores it: no `ruby` parameter on the layer
    let mut p = (*s.project).clone();
    let seq = s.state.active_sequence.unwrap();
    let it = p.sequence_mut(seq).unwrap().find_item_mut(clip).unwrap().1;
    let i = layer_indices(&it.effects)[0];
    assert!(it.effects[i].params.remove("ruby").is_some());
    s.project = std::sync::Arc::new(p);
    assert!(!text(&layer(&s, clip)).ruby, "missing means off");
    s.execute("graphics.ruby", json!({"base": "漢字", "reading": "かんじ"})).unwrap();
    let q = s.active_sequence().unwrap();
    let e = &q.find_item(clip).unwrap().1.effects[i];
    assert_eq!(e.param("ruby").map(|p| p.value.clone()), Some(ParamValue::Bool(true)));
    assert!(text(&layer(&s, clip)).ruby);
}

#[test]
fn add_ruby_text_rules() {
    assert_eq!(add_ruby("漢字と漢字", "漢字", "かんじ", false), ("漢字《かんじ》と漢字".into(), 1));
    assert_eq!(add_ruby("漢字と漢字", "漢字", "かんじ", true), ("漢字《かんじ》と漢字《かんじ》".into(), 2));
    // part of a longer kanji run, kana or Latin: the bar says where the base starts
    assert_eq!(add_ruby("東京都", "京都", "きょうと", false).0, "東｜京都《きょうと》");
    assert_eq!(add_ruby("Rust入門", "Rust", "ラスト", false).0, "｜Rust《ラスト》入門");
    assert_eq!(add_ruby("ここは晴れ", "晴れ", "はれ", false).0, "ここは｜晴れ《はれ》");
    // already annotated, not there, nothing to do
    assert_eq!(add_ruby("漢字《かんじ》", "漢字", "x", true), ("漢字《かんじ》".into(), 0));
    assert_eq!(add_ruby("かな", "漢字", "x", true).1, 0);
    assert_eq!(add_ruby("", "漢字", "x", true), (String::new(), 0));
    // what it writes parses back to exactly that base and reading
    for (t, base, reading) in [("東京都知事", "都知", "とち"), ("abc def", "c d", "x"), ("時々晴れ", "時々", "ときどき"), ("AあA", "A", "エー")]
    {
        let (marked, n) = add_ruby(t, base, reading, true);
        let p = filmcraft_text::ruby::parse(&marked);
        assert_eq!(p.plain, t, "{marked}");
        assert_eq!(p.rubies.len(), n, "{marked}");
        assert!(p.rubies.iter().all(|r| &p.plain[r.base.clone()] == base && r.text == reading), "{marked}");
    }
}

#[test]
fn hostile_parameters_and_wrong_targets() {
    let (mut s, clip) = session("漢字");
    let steps = s.history.undo.len();
    let long = "あ".repeat(65);
    for (cmd, p) in [
        ("graphics.textStyle", json!({})),
        ("graphics.textStyle", json!({"style": "no-such-style"})),
        ("graphics.textStyle", json!({"style": 7})),
        ("graphics.textStyle", json!({"style": "fukuro-red", "layer": 99})),
        ("graphics.textStyle", json!({"style": "fukuro-red", "clip": 999_999_999u64})),
        ("graphics.ruby", json!({})),
        ("graphics.ruby", json!({"base": "漢字"})),
        ("graphics.ruby", json!({"reading": "かんじ"})),
        ("graphics.ruby", json!({"base": "", "reading": "x"})),
        ("graphics.ruby", json!({"base": "漢字", "reading": ""})),
        ("graphics.ruby", json!({"base": "漢字", "reading": "か《ん》じ"})),
        ("graphics.ruby", json!({"base": "漢｜字", "reading": "かんじ"})),
        ("graphics.ruby", json!({"base": "漢字", "reading": "か\nんじ"})),
        ("graphics.ruby", json!({"base": "漢字", "reading": long})),
        ("graphics.ruby", json!({"base": "ない", "reading": "x"})),
    ] {
        assert!(s.execute(cmd, p.clone()).is_err(), "{cmd} {p}");
    }
    assert_eq!(s.history.undo.len(), steps, "refused calls leave no undo step");
    assert_eq!(text(&layer(&s, clip)).text, "漢字");
    // a shape layer is not a text layer
    let r = s.execute("graphics.newRectangle", json!({"newClip": true})).unwrap();
    let shape = r["clip"].as_u64().unwrap();
    assert!(s.execute("graphics.textStyle", json!({"style": "fukuro-red", "clip": shape})).is_err());
    assert!(s.execute("graphics.ruby", json!({"enabled": true, "clip": shape})).is_err());
}
