use super::*;

fn race() -> ChartSpec {
    ChartSpec {
        kind: ChartKind::BarRace,
        categories: vec!["A".into(), "B".into()],
        steps: vec!["1990".into(), "2000".into()],
        values: vec![vec![10.0, 10.0], vec![0.0, 30.0]],
        intro_seconds: 1.0,
        hold_seconds: 1.0,
        ..ChartSpec::default()
    }
}

#[test]
fn intro_grows_then_steps_play_then_hold() {
    let s = race();
    // 6 s clip: intro 0–1, steps 1–5, hold 5–6
    let a = state_at(&s, 0.0, 6.0);
    assert_eq!((a.grow, a.step_pos, a.step), (0.0, 0.0, 0));
    assert!(a.values.iter().all(|&v| v == 0.0));
    let b = state_at(&s, 1.0, 6.0);
    assert_eq!((b.grow, b.step_pos), (1.0, 0.0));
    assert_eq!(b.values, vec![10.0, 0.0]);
    let mid = state_at(&s, 3.0, 6.0);
    assert!((mid.step_pos - 0.5).abs() < 1e-12);
    assert_eq!(mid.values, vec![10.0, 15.0]);
    let end = state_at(&s, 5.5, 6.0);
    assert_eq!((end.step_pos, end.step), (1.0, 1));
    assert_eq!(end.values, vec![10.0, 30.0]);
}

#[test]
fn race_ranks_glide_between_steps() {
    let s = race();
    assert_eq!(state_at(&s, 1.0, 6.0).ranks, vec![0.0, 1.0]);
    assert_eq!(state_at(&s, 5.0, 6.0).ranks, vec![1.0, 0.0]);
    let r = state_at(&s, 3.0, 6.0).ranks;
    assert!((r[0] - 0.5).abs() < 1e-12 && (r[1] - 0.5).abs() < 1e-12, "{r:?}");
    // a bar chart keeps table order
    let bar = ChartSpec { kind: ChartKind::Bar, ..race() };
    assert_eq!(state_at(&bar, 5.0, 6.0).ranks, vec![0.0, 1.0]);
}

#[test]
fn hostile_timing_and_values_stay_finite() {
    let mut s = race();
    s.values[0][1] = f64::NAN;
    s.values[1] = vec![f64::INFINITY];
    s.intro_seconds = f64::NAN;
    s.hold_seconds = -5.0;
    for (u, d) in [(0.0, 0.0), (f64::NAN, 1.0), (1e300, 1e300), (-1.0, 2.0), (2.0, f64::INFINITY)] {
        let st = state_at(&s, u, d);
        assert!(st.step_pos.is_finite() && st.grow.is_finite(), "{u} {d}");
        assert!(st.values.iter().chain(&st.ranks).all(|v| v.is_finite()), "{u} {d} {st:?}");
        assert!(st.step < s.steps.len());
    }
    // no steps, no categories
    let empty = ChartSpec::default();
    let st = state_at(&empty, 1.0, 2.0);
    assert!(st.values.is_empty() && st.step == 0);
}

#[test]
fn sanitized_pads_rows_and_clamps_settings() {
    let s = ChartSpec {
        categories: vec!["A".into(), "B".into(), "C".into()],
        steps: vec!["x".into(), "y".into()],
        values: vec![vec![1.0], vec![f64::NAN, 2.0, 3.0]],
        max_bars: 0,
        decimals: 99,
        intro_seconds: f64::INFINITY,
        title: "t".repeat(1000),
        ..ChartSpec::default()
    }
    .sanitized();
    assert_eq!(s.values, vec![vec![1.0, 0.0], vec![0.0, 2.0], vec![0.0, 0.0]]);
    assert_eq!((s.max_bars, s.decimals, s.intro_seconds), (1, 6, 0.0));
    assert_eq!(s.title.chars().count(), MAX_LABEL_CHARS);
    assert_eq!(ChartSpec::default().sanitized().steps.len(), 1);
}

#[test]
fn parses_spreadsheet_and_csv_tables() {
    let (c, s, v) = ChartSpec::parse_table("\t1990\t2000\nChina\t1,000\t2,000\nIndia\t5\t\n").unwrap();
    assert_eq!(c, vec!["China", "India"]);
    assert_eq!(s, vec!["1990", "2000"]);
    assert_eq!(v, vec![vec![1000.0, 2000.0], vec![5.0, 0.0]]);
    let (c, s, v) = ChartSpec::parse_table("Country,1990,2000\r\n\"Korea, South\",\"1,234\",12%\r\n").unwrap();
    assert_eq!((c, s, v), (vec!["Korea, South".to_string()], vec!["1990".to_string(), "2000".to_string()], vec![vec![1234.0, 12.0]]));
    let (_, _, v) = ChartSpec::parse_table("国;２０２０\n日本;１２３．５\n").unwrap();
    assert_eq!(v, vec![vec![123.5]]);
    assert!(ChartSpec::parse_table("").is_err());
    assert!(ChartSpec::parse_table("only one cell").is_err());
    assert!(ChartSpec::parse_table("a,b\n").is_err());
}

#[test]
fn table_text_round_trips() {
    let s = ChartSpec::sample(ChartKind::Line);
    let (c, st, v) = ChartSpec::parse_table(&s.table_text()).unwrap();
    assert_eq!((c, st, v), (s.categories.clone(), s.steps.clone(), s.values.clone()));
    let q = ChartSpec { categories: vec!["a\t\"b\"".into()], steps: vec!["x".into()], values: vec![vec![1.5]], ..ChartSpec::default() };
    let (c, _, v) = ChartSpec::parse_table(&q.table_text()).unwrap();
    assert_eq!((c, v), (q.categories, q.values));
}

#[test]
fn value_labels_and_axis_steps() {
    assert_eq!(format_value(300_165_618.0, 0), "300,165,618");
    assert_eq!(format_value(1234.5, 1), "1,234.5");
    assert_eq!(format_value(-1234.0, 0), "-1,234");
    assert_eq!(format_value(-0.0001, 2), "0.00");
    assert_eq!(format_value(f64::NAN, 0), "0");
    assert_eq!(format_value(999.0, 0), "999");
    assert_eq!(nice_step(350_000_000.0, 5), 100_000_000.0);
    assert_eq!(nice_step(95.0, 4), 50.0);
    assert_eq!(nice_step(0.0, 4), 1.0);
    assert_eq!(nice_step(f64::NAN, 4), 1.0);
}

#[test]
fn kinds_round_trip_by_id_and_serde() {
    for k in ChartKind::ALL {
        assert_eq!(ChartKind::from_id(k.id()), Some(k));
    }
    let s = ChartSpec::sample(ChartKind::Donut);
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"kind\":\"donut\""), "{json}");
    let back: ChartSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    // missing fields take the defaults
    let partial: ChartSpec = serde_json::from_str(r#"{"kind":"pie"}"#).unwrap();
    assert_eq!((partial.kind, partial.max_bars, partial.show_values), (ChartKind::Pie, 10, true));
}

#[test]
fn colours_repeat_the_palette() {
    let s = ChartSpec { palette: vec![[1, 2, 3], [4, 5, 6]], ..ChartSpec::default() };
    assert_eq!((s.color(0), s.color(1), s.color(2)), ([1, 2, 3], [4, 5, 6], [1, 2, 3]));
    assert_eq!(ChartSpec::default().color(0), PALETTES[0].1[0]);
}

#[test]
fn images_round_trip_and_old_charts_have_none() {
    // a chart saved before pictures existed reads with none, and writes no `images` key
    let old: ChartSpec = serde_json::from_str(r#"{"kind":"bar","categories":["A"],"steps":["x"],"values":[[1]]}"#).unwrap();
    assert!(old.images.is_empty() && old.image_shape == ImageShape::Circle);
    let v = serde_json::to_value(&old).unwrap();
    assert!(v.get("images").is_none() && v.get("imageShape").is_none(), "{v}");
    let mut s = race();
    s.set_image(1, Some(ChartImage { item: Some(ItemId(7)), path: "C:/flags/b.png".into() }));
    s.image_shape = ImageShape::Rounded;
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains(r#""images":[null,{"item":7,"path":"C:/flags/b.png"}]"#), "{json}");
    assert!(json.contains(r#""imageShape":"rounded""#), "{json}");
    let back: ChartSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.image(1).and_then(|i| i.item), Some(ItemId(7)));
    assert_eq!(back.image(0), None);
    assert_eq!(back.image(99), None);
}

#[test]
fn sanitize_aligns_images_with_categories() {
    let mut s = race();
    // too many, an empty one, and a hostile path
    s.images = vec![Some(ChartImage::default()), Some(ChartImage::of_item(ItemId(3))), Some(ChartImage::of_item(ItemId(4)))];
    s.images.push(Some(ChartImage { item: None, path: "x".repeat(MAX_PATH_CHARS * 2) }));
    let c = s.sanitized();
    assert_eq!(c.images, vec![None, Some(ChartImage::of_item(ItemId(3)))]);
    // too few: padded
    s.images = vec![Some(ChartImage { item: None, path: "y".repeat(MAX_PATH_CHARS + 5) })];
    let c = s.sanitized();
    assert_eq!(c.images.len(), 2);
    assert_eq!(c.image(0).map(|i| i.path.chars().count()), Some(MAX_PATH_CHARS));
    // only empty entries: dropped so the JSON stays as before
    s.images = vec![None, Some(ChartImage::default())];
    assert!(s.sanitized().images.is_empty());
    // set_image past the end does nothing
    let mut t = race();
    t.set_image(5, Some(ChartImage::of_item(ItemId(1))));
    assert!(t.images.is_empty());
}

#[test]
fn moving_removing_and_reloading_categories_keeps_pictures_aligned() {
    let mut s = ChartSpec {
        categories: vec!["A".into(), "B".into(), "C".into()],
        steps: vec!["1".into()],
        values: vec![vec![1.0], vec![2.0], vec![3.0]],
        ..ChartSpec::default()
    };
    s.set_image(0, Some(ChartImage::of_item(ItemId(10))));
    s.set_image(2, Some(ChartImage::of_item(ItemId(30))));
    s.move_category(2, 0);
    assert_eq!(s.categories, ["C", "A", "B"]);
    assert_eq!(s.values, vec![vec![3.0], vec![1.0], vec![2.0]]);
    assert_eq!(s.images, vec![Some(ChartImage::of_item(ItemId(30))), Some(ChartImage::of_item(ItemId(10))), None]);
    s.move_category(0, 9);
    s.remove_category(9);
    assert_eq!(s.categories.len(), 3);
    s.remove_category(1);
    assert_eq!(s.categories, ["C", "B"]);
    assert_eq!(s.images, vec![Some(ChartImage::of_item(ItemId(30))), None]);
    // a pasted table keeps pictures by category name
    s.set_table(vec!["B".into(), "C".into(), "D".into()], vec!["1".into()], vec![vec![0.0]; 3]);
    assert_eq!(s.images, vec![None, Some(ChartImage::of_item(ItemId(30))), None]);
    s.set_table(vec!["Z".into()], vec!["1".into()], vec![vec![0.0]]);
    assert!(s.images.is_empty());
}
