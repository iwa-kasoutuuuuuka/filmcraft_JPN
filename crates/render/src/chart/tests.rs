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

mod pictures {
    use super::*;
    use filmcraft_media::SharedSource;
    use filmcraft_media::still::StillSource;
    use filmcraft_project::ItemId;
    use filmcraft_project::chart::{ChartImage, ImageShape};
    use std::collections::HashMap;

    const MAGENTA: [u8; 3] = [255, 0, 255];

    /// A provider with item 1 a solid magenta 300×200 still and item 2 a 0×0 (undecodable) one.
    struct Media(HashMap<ItemId, SharedSource>);

    impl SourceProvider for Media {
        fn source(&self, item: ItemId) -> Option<SharedSource> {
            self.0.get(&item).cloned()
        }
    }

    fn media() -> Media {
        let rgba: Vec<u8> = (0..300 * 200).flat_map(|_| [255, 0, 255, 255]).collect();
        let mut m = HashMap::new();
        m.insert(ItemId(1), Arc::new(StillSource::from_rgba("flag.png", 300, 200, rgba, false, 0)) as SharedSource);
        m.insert(ItemId(2), Arc::new(StillSource::from_rgba("broken.png", 0, 0, Vec::new(), false, 0)) as SharedSource);
        Media(m)
    }

    fn with_pictures(spec: &ChartSpec, u: f64, sources: &dyn SourceProvider) -> Image {
        let mut img = Image::new(480, 270);
        let images = chart_images(spec, sources);
        draw_chart_with(spec, u, 8.0, CANVAS, &Affine::scale(0.25, 0.25), &images, &mut img);
        img
    }

    /// Magenta-ish pixels (anti-aliased edges included) and their mean y.
    fn magenta(img: &Image) -> (usize, f64) {
        let t = premul_linear(rgb(MAGENTA, 1.0));
        let hits: Vec<usize> = (0..img.h)
            .flat_map(|y| (0..img.w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let p = img.get(x, y);
                p[3] > 0.99 && (p[0] - t[0]).abs() < 0.02 && (p[1] - t[1]).abs() < 0.02 && (p[2] - t[2]).abs() < 0.02
            })
            .map(|(_, y)| y)
            .collect();
        let mean = if hits.is_empty() { 0.0 } else { hits.iter().sum::<usize>() as f64 / hits.len() as f64 };
        (hits.len(), mean)
    }

    fn race() -> ChartSpec {
        let mut s = ChartSpec {
            kind: ChartKind::BarRace,
            categories: vec!["A".into(), "B".into()],
            steps: vec!["1".into(), "2".into()],
            values: vec![vec![100.0, 10.0], vec![10.0, 100.0]],
            palette: vec![[255, 0, 0], [0, 0, 255]],
            show_step: false,
            ..ChartSpec::default()
        };
        s.set_image(0, Some(ChartImage::of_item(ItemId(1))));
        s
    }

    #[test]
    fn every_kind_draws_the_category_picture() {
        let m = media();
        for kind in ChartKind::ALL {
            let mut spec = ChartSpec::sample(kind);
            assert_eq!(magenta(&with_pictures(&spec, 4.0, &m)).0, 0, "{kind:?}: no picture yet");
            spec.set_image(0, Some(ChartImage::of_item(ItemId(1))));
            let (n, _) = magenta(&with_pictures(&spec, 4.0, &m));
            assert!(n > 20, "{kind:?}: the picture shows ({n} px)");
            // without the media (the plain draw) nothing is drawn and nothing panics
            assert_eq!(magenta(&render(&spec, 4.0)).0, 0, "{kind:?}");
        }
    }

    #[test]
    fn picture_is_cropped_to_a_circle_or_rounded_square() {
        let m = media();
        let mut spec = ChartSpec { kind: ChartKind::Column, max_bars: 1, show_values: false, ..ChartSpec::sample(ChartKind::Column) };
        spec.set_image(0, Some(ChartImage::of_item(ItemId(1))));
        let (circle, _) = magenta(&with_pictures(&spec, 4.0, &m));
        spec.image_shape = ImageShape::Rounded;
        let (rounded, _) = magenta(&with_pictures(&spec, 4.0, &m));
        // a circle covers π/4 of its square, a rounded square more
        assert!(circle > 20 && rounded > circle, "circle {circle}, rounded {rounded}");
    }

    #[test]
    fn race_picture_follows_its_bar_when_the_order_changes() {
        let m = media();
        let spec = race();
        let (n0, y0) = magenta(&with_pictures(&spec, 1.0, &m));
        let (n1, y1) = magenta(&with_pictures(&spec, 7.5, &m));
        assert!(n0 > 20 && n1 > 20, "{n0} {n1}");
        assert!(y1 > y0 + 20.0, "A drops from first to second place: picture y {y0} → {y1}");
    }

    #[test]
    fn missing_or_undecodable_pictures_are_left_out() {
        let m = media();
        let mut spec = race();
        spec.set_image(0, Some(ChartImage::of_item(ItemId(2))));
        spec.set_image(1, Some(ChartImage::of_item(ItemId(99))));
        let a = with_pictures(&spec, 4.0, &m);
        assert_eq!(magenta(&a).0, 0);
        assert!(chart_images(&spec, &m).iter().all(Option::is_none));
        // a hostile Motion scale with a picture stays bounded
        spec.set_image(0, Some(ChartImage::of_item(ItemId(1))));
        let images = chart_images(&spec, &m);
        let mut img = Image::new(64, 64);
        draw_chart_with(&spec, 4.0, 8.0, CANVAS, &Affine::scale(1e6, 1e6), &images, &mut img);
        draw_chart_with(&spec, 4.0, 8.0, CANVAS, &Affine::scale(0.0, 0.0), &images, &mut img);
        draw_chart_with(&spec, 4.0, 8.0, CANVAS, &Affine::scale(f64::NAN, 1.0), &images, &mut img);
    }

    #[test]
    fn thumbnails_are_small_and_cached() {
        let m = media();
        let a = item_thumbnail(ItemId(1), &m).unwrap();
        assert!(a.w.max(a.h) <= MAX_THUMB_SIDE && a.w > a.h);
        let b = item_thumbnail(ItemId(1), &m).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(item_thumbnail(ItemId(2), &m).is_none());
    }
}
