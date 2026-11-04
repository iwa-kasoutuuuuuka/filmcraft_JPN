use super::*;
use filmcraft_project::chart::ChartSpec;

const CANVAS: (u32, u32) = (1920, 1080);

fn render(spec: &ChartSpec, u: f64) -> Image {
    let mut img = Image::new(480, 270);
    draw_chart(spec, u, 8.0, CANVAS, &Affine::scale(0.25, 0.25), &mut img);
    img
}

/// Pixels whose colour is close to `c` (sRGB bytes) and opaque.
fn count_color(img: &Image, c: [u8; 3]) -> usize {
    let t = premul_linear(rgb(c, 1.0));
    img.px.as_chunks::<4>().0.iter().filter(|p| p[3] > 0.99 && (0..3).all(|i| (p[i] - t[i]).abs() < 0.01)).count()
}

#[test]
fn every_kind_draws_its_card_and_colours() {
    for kind in ChartKind::ALL {
        let spec = ChartSpec::sample(kind);
        let img = render(&spec, 4.0);
        let opaque = img.px.as_chunks::<4>().0.iter().filter(|p| p[3] > 0.99).count();
        assert!(opaque > 480 * 270 / 2, "{kind:?}: the card covers most of the frame ({opaque})");
        assert!(count_color(&img, spec.color(0)) > 20, "{kind:?}: the first category's colour shows");
    }
}

#[test]
fn intro_starts_empty_and_no_background_is_transparent() {
    let spec = ChartSpec { background: false, title: String::new(), show_grid: false, show_step: false, ..ChartSpec::sample(ChartKind::Column) };
    // at 0 s every bar is still at zero: only the axis line and labels are drawn
    let start = render(&spec, 0.0);
    assert_eq!(count_color(&start, spec.color(0)), 0);
    let later = render(&spec, 4.0);
    assert!(count_color(&later, spec.color(0)) > 50);
    let corner = later.get(2, 2);
    assert_eq!(corner[3], 0.0, "no card: the corner stays transparent");
}

#[test]
fn race_puts_the_leader_on_top() {
    let spec = ChartSpec {
        kind: ChartKind::BarRace,
        title: String::new(),
        categories: vec!["A".into(), "B".into()],
        steps: vec!["1".into(), "2".into()],
        values: vec![vec![100.0, 10.0], vec![10.0, 100.0]],
        palette: vec![[255, 0, 0], [0, 0, 255]],
        show_step: false,
        ..ChartSpec::default()
    };
    // how much of the upper half of the frame is colour `c`
    let top = |img: &Image, c: [u8; 3]| {
        let t = premul_linear(rgb(c, 1.0));
        (0..img.h / 2).flat_map(|y| (0..img.w).map(move |x| (x, y))).filter(|&(x, y)| (0..3).all(|i| (img.get(x, y)[i] - t[i]).abs() < 0.01)).count()
    };
    let a = render(&spec, 1.0);
    assert!(top(&a, [255, 0, 0]) > top(&a, [0, 0, 255]), "A leads at the first step");
    let b = render(&spec, 7.5);
    assert!(top(&b, [0, 0, 255]) > top(&b, [255, 0, 0]), "B leads at the last step");
}

#[test]
fn hostile_specs_draw_without_panicking() {
    let mut s = ChartSpec::sample(ChartKind::Line);
    s.values[0] = vec![f64::NAN, f64::INFINITY, -1e308, 1e308];
    s.max_bars = u32::MAX;
    s.decimals = u32::MAX;
    s.font = "No Such Font".into();
    for kind in ChartKind::ALL {
        s.kind = kind;
        for u in [0.0, 4.0, f64::NAN, 1e300] {
            let _ = render(&s, u);
        }
        let empty = ChartSpec { kind, ..ChartSpec::default() };
        let _ = render(&empty, 4.0);
        // a huge Motion scale stays bounded
        let mut img = Image::new(64, 64);
        draw_chart(&s, 4.0, 8.0, CANVAS, &Affine::scale(1e6, 1e6), &mut img);
        draw_chart(&s, 4.0, 8.0, (0, 0), &Affine::scale(f64::NAN, 1.0), &mut img);
    }
}

#[test]
fn rgba_preview_has_the_requested_size() {
    let v = chart_rgba(&ChartSpec::sample(ChartKind::Pie), 4.0, 8.0, CANVAS, 320, 180);
    assert_eq!(v.len(), 320 * 180 * 4);
    assert!(v.chunks(4).any(|p| p[3] == 255));
}

#[test]
fn polyline_stroke_covers_its_segments() {
    let p = stroke_polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 2.0);
    let m = filmcraft_text::raster::fill(&p, 16, 16, 2.0, 2.0);
    // on the first segment, on the corner and on the second segment
    for (x, y) in [(7, 2), (12, 2), (12, 9)] {
        assert!(m.get(x, y) > 0.9, "({x}, {y}) = {}", m.get(x, y));
    }
    assert!(m.get(7, 9) < 0.01, "inside the bend stays empty");
}
