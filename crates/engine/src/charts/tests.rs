use serde_json::json;

use super::*;
use crate::Session;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn chart_of(s: &Session, clip: u64) -> ChartSpec {
    clip_chart(s, ClipId(clip)).expect("the clip holds a chart")
}

#[test]
fn new_chart_places_an_eight_second_clip_with_a_sample_or_the_given_table() {
    let mut s = demo();
    let t = s.playhead();
    let r = s.execute("graphics.newChart", json!({"kind": "column"})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    let q = s.active_sequence().unwrap();
    let (_, it) = q.find_item(ClipId(clip)).unwrap();
    assert_eq!(it.start, t);
    assert_eq!(it.duration, q.settings.frame_rate.snap_nearest(Tick::from_seconds_f64(8.0)));
    let c = chart_of(&s, clip);
    assert_eq!(c.kind, ChartKind::Column);
    assert!(!c.categories.is_empty(), "a sample to start from");
    assert_eq!(s.state.selection, vec![ClipId(clip)]);

    let r = s
        .execute(
            "graphics.newChart",
            json!({"table": "\t1990\t2000\nChina\t1\t2\nIndia\t3\t4", "title": "Population", "theme": "light", "palette": "ocean", "seconds": 4}),
        )
        .unwrap();
    let c = chart_of(&s, r["clip"].as_u64().unwrap());
    assert_eq!(
        (c.categories.len(), c.steps.clone(), c.title.as_str(), c.theme),
        (2, vec!["1990".to_string(), "2000".to_string()], "Population", ChartTheme::Light)
    );
    assert_eq!(c.palette, PALETTES[1].1.to_vec());
    assert_eq!(c.values, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
}

#[test]
fn set_chart_edits_in_one_undo_step_and_reads_back() {
    let mut s = demo();
    let clip = s.execute("graphics.newChart", json!({})).unwrap()["clip"].as_u64().unwrap();
    s.execute("graphics.setChart", json!({"clip": clip, "kind": "donut", "title": "Share"})).unwrap();
    let c = chart_of(&s, clip);
    assert_eq!((c.kind, c.title.as_str()), (ChartKind::Donut, "Share"));
    // a whole chart replaces the old one
    let whole = ChartSpec { kind: ChartKind::Line, categories: vec!["A".into()], steps: vec!["x".into()], values: vec![vec![5.0]], ..ChartSpec::default() };
    s.execute("graphics.setChart", json!({"clip": clip, "chart": whole})).unwrap();
    assert_eq!(chart_of(&s, clip), whole.sanitized());
    let back = s.execute("graphics.chart", json!({"clip": clip})).unwrap();
    assert_eq!(back["chart"]["kind"], "line");
    assert_eq!(back["table"], "\tx\nA\t5");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(chart_of(&s, clip).kind, ChartKind::Donut);
}

#[test]
fn bad_chart_params_are_refused_and_change_nothing() {
    let mut s = demo();
    let clip = s.execute("graphics.newChart", json!({})).unwrap()["clip"].as_u64().unwrap();
    let before = chart_of(&s, clip);
    for p in [
        json!({"clip": clip, "kind": "radar"}),
        json!({"clip": clip, "theme": "neon"}),
        json!({"clip": clip, "palette": "nope"}),
        json!({"clip": clip, "table": "just text"}),
        json!({"clip": clip, "chart": {"kind": 5}}),
    ] {
        assert!(s.execute("graphics.setChart", p.clone()).is_err(), "{p}");
    }
    assert_eq!(chart_of(&s, clip), before);
    // a text graphic has no chart to edit
    let text = s.execute("graphics.newText", json!({"text": "Hi"})).unwrap()["clip"].as_u64().unwrap();
    assert!(s.execute("graphics.setChart", json!({"clip": text, "kind": "pie"})).is_err());
    assert!(s.execute("graphics.newChart", json!({"seconds": f64::MAX})).is_ok(), "the length is capped");
}

#[test]
fn chart_shows_in_the_program_and_survives_save_and_open() {
    let mut s = demo();
    let r = s.execute("graphics.newChart", json!({"kind": "column", "palette": "sunset", "seconds": 6})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    // mid-clip, past the intro
    let start = s.active_sequence().unwrap().find_item(ClipId(clip)).unwrap().1.start;
    s.execute("playhead.set", json!({"time": (start + Tick::from_seconds_f64(3.0)).0})).unwrap();
    let img = s.render_program(0.25).unwrap();
    let [r0, g0, b0] = PALETTES[2].1[0].map(|v| f32::from(v) / 255.0);
    let lin = |c: f32| filmcraft_color::srgb_to_linear(c);
    let hits = img.px.chunks(4).filter(|p| (p[0] - lin(r0)).abs() < 0.02 && (p[1] - lin(g0)).abs() < 0.02 && (p[2] - lin(b0)).abs() < 0.02).count();
    assert!(hits > 50, "the first column's colour is in the program ({hits})");

    let dir = std::env::temp_dir().join(format!("fc-chart-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c.fcproj");
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(chart_of(&s2, clip), chart_of(&s, clip));
    let _ = std::fs::remove_dir_all(&dir);
}
