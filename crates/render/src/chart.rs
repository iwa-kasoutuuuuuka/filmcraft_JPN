//! Animated charts ([`filmcraft_project::chart`]) drawn as vectors: bars, columns, lines, areas
//! and pie / donut wedges are filled paths, labels are laid out by the text engine, everything is
//! anti-aliased and composited straight into the output (no resampling). The chart is designed on
//! the graphic canvas (the sequence frame); `m` maps canvas pixels to output pixels, so Motion
//! scales it crisply.

use std::f32::consts::{FRAC_PI_2, TAU};

use filmcraft_geom::Affine;
use filmcraft_project::chart::{ChartKind, ChartSpec, ChartState, ChartTheme, format_value, nice_step, state_at};
use filmcraft_text::raster::fill_into;
use filmcraft_text::{Mask, ParagraphStyle, Path, TextStyle, layout, measure, render};

use crate::graphic_clip::xform;
use crate::graphics::{paint_mask, premul_linear};
use crate::image::Image;

/// Largest area one shape or label may cover in output pixels (a hostile Motion scale must not
/// allocate without bound).
const MAX_MASK_PIXELS: usize = 64 << 20;
/// Points kept per line before joins are left out (round joins on every vertex of a long line
/// cost more than they show).
const MAX_JOINED_POINTS: usize = 400;

type Rgba = [f32; 4];

fn rgb(c: [u8; 3], a: f32) -> Rgba {
    [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, a]
}

struct Colors {
    card: Rgba,
    text: Rgba,
    dim: Rgba,
    grid: Rgba,
    axis: Rgba,
    big: Rgba,
    on_color: Rgba,
}

fn colors(theme: ChartTheme) -> Colors {
    match theme {
        ChartTheme::Dark => Colors {
            card: [0.106, 0.114, 0.137, 1.0],
            text: [0.93, 0.94, 0.96, 1.0],
            dim: [0.66, 0.69, 0.74, 1.0],
            grid: [1.0, 1.0, 1.0, 0.09],
            axis: [1.0, 1.0, 1.0, 0.32],
            big: [1.0, 1.0, 1.0, 0.3],
            on_color: [1.0, 1.0, 1.0, 1.0],
        },
        ChartTheme::Light => Colors {
            card: [1.0, 1.0, 1.0, 1.0],
            text: [0.12, 0.13, 0.16, 1.0],
            dim: [0.38, 0.40, 0.45, 1.0],
            grid: [0.0, 0.0, 0.0, 0.08],
            axis: [0.0, 0.0, 0.0, 0.32],
            big: [0.0, 0.0, 0.0, 0.24],
            on_color: [1.0, 1.0, 1.0, 1.0],
        },
    }
}

/// Horizontal text anchor.
#[derive(Clone, Copy)]
enum H {
    Left,
    Center,
    Right,
}

/// Draws chart shapes in canvas pixels onto the output.
struct Painter<'a> {
    out: &'a mut Image,
    m: Affine,
    family: String,
}

impl Painter<'_> {
    fn fill(&mut self, path: &Path, c: Rgba) {
        if path.is_empty() || c[3] <= 0.0 {
            return;
        }
        let p = path.transformed(&xform(&self.m));
        let Some(b) = p.bounds() else { return };
        self.fill_device(&p, [b.0, b.1, b.2, b.3], c, |_| {});
    }

    /// Fill the device-space region `b` with coverage from `p` (and anything `extra` adds).
    fn fill_device(&mut self, p: &Path, b: [f32; 4], c: Rgba, extra: impl FnOnce(&mut (Mask, i32, i32))) {
        if !b.iter().all(|v| v.is_finite()) {
            return;
        }
        let x0 = (b[0].floor() as i64).max(0);
        let y0 = (b[1].floor() as i64).max(0);
        let x1 = (b[2].ceil() as i64 + 1).min(self.out.w as i64);
        let y1 = (b[3].ceil() as i64 + 1).min(self.out.h as i64);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        if w.saturating_mul(h) > MAX_MASK_PIXELS {
            return;
        }
        let mut mk = (Mask::new(w, h), x0 as i32, y0 as i32);
        if !p.is_empty() {
            fill_into(p, &mut mk.0, -(x0 as f32), -(y0 as f32));
        }
        extra(&mut mk);
        paint_mask(self.out, &mk.0, mk.1, mk.2, premul_linear(c), 1.0);
    }

    fn style(&self, size: f32, bold: bool) -> TextStyle {
        TextStyle { family: self.family.clone(), style: if bold { "Bold".into() } else { "Regular".into() }, size: size.max(0.5), ..TextStyle::default() }
    }

    /// Width of `s` at `size` in canvas pixels.
    fn measure(&self, s: &str, size: f32, bold: bool) -> f32 {
        if s.is_empty() { 0.0 } else { measure(s, &self.style(size, bold)) }
    }

    /// Draw `s` with its line box anchored at `x` (by `h`) and vertically centred on `y`.
    fn text(&mut self, s: &str, size: f32, bold: bool, c: Rgba, x: f32, y: f32, h: H) {
        if s.is_empty() || c[3] <= 0.0 {
            return;
        }
        let l = layout(s, &self.style(size, bold), &ParagraphStyle::default());
        let b = l.bounds;
        let w = b[2] - b[0];
        let ax = match h {
            H::Left => b[0],
            H::Center => b[0] + w / 2.0,
            H::Right => b[2],
        };
        let (dx, dy) = (x - ax, y - (b[1] + b[3]) / 2.0);
        let m = self.m.then_apply(&Affine::translate(f64::from(dx), f64::from(dy)));
        let xf = xform(&m);
        let pad = size * 0.4;
        let corners = [(b[0] - pad, b[1] - pad), (b[2] + pad, b[1] - pad), (b[0] - pad, b[3] + pad), (b[2] + pad, b[3] + pad)];
        let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (px, py) in corners {
            let (qx, qy) = filmcraft_text::raster::apply(&xf, px, py);
            bb = [bb[0].min(qx), bb[1].min(qy), bb[2].max(qx), bb[3].max(qy)];
        }
        self.fill_device(&Path::new(), bb, c, |mk| render::draw(&l, &xf, &mut mk.0, (mk.1, mk.2)));
    }
}

/// A thick open polyline as a filled outline: one quad per segment plus round joins, all wound
/// the same way so the non-zero fill unions them.
fn stroke_polyline(pts: &[(f32, f32)], width: f32) -> Path {
    let mut out = Path::new();
    let r = width / 2.0;
    if r <= 0.0 {
        return out;
    }
    let reference = Path::ellipse(0.0, 0.0, 1.0, 1.0).signed_area();
    let joins = pts.len() <= MAX_JOINED_POINTS;
    for w in pts.windows(2) {
        let (Some(&(ax, ay)), Some(&(bx, by))) = (w.first(), w.get(1)) else { continue };
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let (nx, ny) = (-dy / len * r, dx / len * r);
        let mut quad = vec![(ax + nx, ay + ny), (bx + nx, by + ny), (bx - nx, by - ny), (ax - nx, ay - ny)];
        if (Path::polygon(&quad).signed_area() > 0.0) != (reference > 0.0) {
            quad.reverse();
        }
        out.extend(&Path::polygon(&quad));
    }
    if joins {
        for &(x, y) in pts {
            out.extend(&Path::ellipse(x, y, r, r));
        }
    }
    out
}

/// A pie wedge (or a ring segment when `inner` > 0) from angle `a0` to `a1` (radians, clockwise
/// from 3 o'clock on screen).
fn wedge(cx: f32, cy: f32, outer: f32, inner: f32, a0: f32, a1: f32) -> Path {
    let sweep = a1 - a0;
    if sweep <= 1e-5 || outer <= 0.0 {
        return Path::new();
    }
    let n = ((sweep / (TAU / 180.0)).ceil() as usize).clamp(2, 360);
    let arc = |r: f32| (0..=n).map(move |i| a0 + sweep * i as f32 / n as f32).map(move |a| (cx + r * a.cos(), cy + r * a.sin()));
    let mut pts: Vec<(f32, f32)> = arc(outer).collect();
    if inner > 0.0 {
        let back: Vec<(f32, f32)> = arc(inner).collect();
        pts.extend(back.into_iter().rev());
    } else {
        pts.push((cx, cy));
    }
    Path::polygon(&pts)
}

/// Draw `spec` as it looks `u` seconds into a clip `d` seconds long, designed on a `canvas`-sized
/// frame, through `m` (canvas → output pixels).
pub fn draw_chart(spec: &ChartSpec, u: f64, d: f64, canvas: (u32, u32), m: &Affine, out: &mut Image) {
    let spec = spec.sanitized();
    let st = state_at(&spec, u, d);
    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
    let s = cw.min(ch);
    let family = if spec.font.trim().is_empty() { filmcraft_text::fonts::DEFAULT_FAMILY.to_string() } else { spec.font.clone() };
    let mut p = Painter { out, m: *m, family };
    let col = colors(spec.theme);
    let margin = 0.035 * s;
    let card = [margin, margin, cw - margin, ch - margin];
    if spec.background {
        p.fill(&Path::round_rect(card[0], card[1], card[2], card[3], 0.035 * s), col.card);
    }
    let pad = 0.045 * s;
    let mut area = [card[0] + pad, card[1] + pad * 0.8, card[2] - pad, card[3] - pad * 0.8];
    if !spec.title.trim().is_empty() {
        let size = 0.055 * s;
        p.text(&spec.title, size, false, col.text, area[0], area[1] + size * 0.55, H::Left);
        area[1] += size * 1.6;
    }
    if area[2] - area[0] < 4.0 || area[3] - area[1] < 4.0 || spec.categories.is_empty() {
        return;
    }
    let fs = 0.03 * s;
    match spec.kind {
        ChartKind::BarRace | ChartKind::Bar => bars(&mut p, &spec, &st, &col, area, fs, s),
        ChartKind::Column => columns(&mut p, &spec, &st, &col, area, fs, s),
        ChartKind::Line | ChartKind::Area => lines(&mut p, &spec, &st, &col, area, fs, s),
        ChartKind::Pie | ChartKind::Donut => pie(&mut p, &spec, &st, &col, area, fs, s),
    }
}

/// Axis tick values from 0 to `max` (inclusive when it lands on one) at a nice step.
fn ticks(max: f64, n: u32) -> (f64, Vec<f64>) {
    let step = nice_step(max, n);
    let count = ((max / step).floor() as usize).min(50);
    (step, (0..=count).map(|i| i as f64 * step).collect())
}

/// Decimals for axis labels at `step`.
fn tick_decimals(step: f64) -> u32 {
    if step >= 1.0 { 0 } else { ((-step.log10()).ceil() as u32).min(6) }
}

fn bars(p: &mut Painter, spec: &ChartSpec, st: &ChartState, col: &Colors, area: [f32; 4], fs: f32, s: f32) {
    let n = spec.categories.len();
    let visible = (spec.max_bars as usize).min(n).max(1);
    let race = spec.kind == ChartKind::BarRace;
    // the bars on screen: by rank in a race, the first `visible` rows in a bar chart
    let shown: Vec<usize> = (0..n).filter(|&c| st.ranks.get(c).is_some_and(|&r| r < visible as f64)).collect();
    let cur_max = shown.iter().filter_map(|&c| st.values.get(c)).fold(0.0f64, |a, &v| a.max(v));
    let all_max = (0..n).flat_map(|c| (0..spec.steps.len()).map(move |j| (c, j))).map(|(c, j)| spec.value(c, j)).fold(0.0f64, f64::max);
    // a race scales to the current leader, as it is before the intro growth (so the axis holds
    // still while the bars grow in); a bar chart to the largest value of the table
    let unscaled_max = if st.grow > 1e-3 { cur_max / st.grow } else { shown.iter().map(|&c| spec.value(c, st.step)).fold(0.0f64, f64::max) };
    let amax = if race { unscaled_max } else { all_max }.max(1e-9);
    let label_w = shown.iter().filter_map(|&c| spec.categories.get(c)).map(|t| p.measure(t, fs, false)).fold(0.0f32, f32::max).min((area[2] - area[0]) * 0.3);
    let value_w = if spec.show_values {
        let top = if race { unscaled_max } else { all_max };
        p.measure(&format_value(top, spec.decimals), fs, false) + fs * 0.8
    } else {
        fs * 0.5
    };
    let axis_h = if spec.show_grid { fs * 1.9 } else { 0.0 };
    let plot = [area[0] + label_w + fs * 0.6, area[1], area[2] - value_w, area[3] - axis_h];
    if plot[2] - plot[0] < 4.0 || plot[3] - plot[1] < 4.0 {
        return;
    }
    let pw = plot[2] - plot[0];
    if spec.show_grid {
        let (step, tv) = ticks(amax, 4);
        let dec = tick_decimals(step);
        for v in tv {
            let x = plot[0] + (v / amax) as f32 * pw;
            if x > plot[2] + 0.5 {
                continue;
            }
            p.fill(&Path::rect(x - 0.6, plot[1], x + 0.6, plot[3]), col.grid);
            p.text(&format_value(v, dec), fs * 0.85, false, col.dim, x, plot[3] + fs * 1.0, H::Center);
        }
        p.fill(&Path::rect(plot[0] - 0.8, plot[1], plot[0] + 0.8, plot[3]), col.axis);
    }
    let slot = (plot[3] - plot[1]) / visible as f32;
    let thick = slot * 0.78;
    let radius = (thick * 0.12).min(0.006 * s);
    for &c in &shown {
        let (Some(&r), Some(&v)) = (st.ranks.get(c), st.values.get(c)) else { continue };
        // a bar leaving the top places fades as it slides below the last one
        let fade = ((visible as f64 - r) as f32).clamp(0.0, 1.0);
        let y = plot[1] + r as f32 * slot + (slot - thick) / 2.0;
        let len = ((v.max(0.0) / amax) as f32 * pw).clamp(0.0, pw);
        if len > 0.5 {
            p.fill(&Path::round_rect(plot[0], y, plot[0] + len, y + thick, radius.min(len / 2.0)), rgb(spec.color(c), fade));
        }
        let mid = y + thick / 2.0;
        if let Some(name) = spec.categories.get(c) {
            let mut t = col.text;
            t[3] *= fade;
            p.text(name, fs, false, t, plot[0] - fs * 0.45, mid, H::Right);
        }
        if spec.show_values {
            let mut t = col.text;
            t[3] *= fade;
            p.text(&format_value(v, spec.decimals), fs, false, t, plot[0] + len + fs * 0.35, mid, H::Left);
        }
    }
    if spec.show_step && spec.steps.len() > 1 {
        let size = 0.2 * s;
        if let Some(label) = spec.steps.get(st.step) {
            p.text(label, size, false, col.big, plot[2], plot[3] - size * 0.42, H::Right);
        }
    }
}

fn columns(p: &mut Painter, spec: &ChartSpec, st: &ChartState, col: &Colors, area: [f32; 4], fs: f32, s: f32) {
    let n = spec.categories.len().min(spec.max_bars as usize).max(1);
    let all_max = (0..n).flat_map(|c| (0..spec.steps.len()).map(move |j| (c, j))).map(|(c, j)| spec.value(c, j)).fold(0.0f64, f64::max).max(1e-9);
    let (step, tv) = ticks(all_max, 4);
    let amax = (all_max / step).ceil().max(1.0) * step;
    let dec = tick_decimals(step);
    let axis_w = if spec.show_grid { p.measure(&format_value(amax, dec), fs * 0.85, false) + fs * 0.6 } else { 0.0 };
    let top = if spec.show_values { fs * 1.5 } else { fs * 0.3 };
    let plot = [area[0] + axis_w, area[1] + top, area[2], area[3] - fs * 2.0];
    if plot[2] - plot[0] < 4.0 || plot[3] - plot[1] < 4.0 {
        return;
    }
    let ph = plot[3] - plot[1];
    if spec.show_grid {
        let mut tv = tv;
        if tv.last().is_some_and(|&v| v < amax - step * 0.5) {
            tv.push(amax);
        }
        for v in tv {
            let y = plot[3] - (v / amax) as f32 * ph;
            p.fill(&Path::rect(plot[0], y - 0.6, plot[2], y + 0.6), col.grid);
            p.text(&format_value(v, dec), fs * 0.85, false, col.dim, plot[0] - fs * 0.4, y, H::Right);
        }
    }
    p.fill(&Path::rect(plot[0], plot[3] - 0.8, plot[2], plot[3] + 0.8), col.axis);
    let slot = (plot[2] - plot[0]) / n as f32;
    let thick = slot * 0.62;
    let radius = (thick * 0.1).min(0.006 * s);
    for c in 0..n {
        let v = st.values.get(c).copied().unwrap_or(0.0);
        let x = plot[0] + c as f32 * slot + (slot - thick) / 2.0;
        let hgt = ((v.max(0.0) / amax) as f32 * ph).clamp(0.0, ph);
        if hgt > 0.5 {
            p.fill(&Path::round_rect(x, plot[3] - hgt, x + thick, plot[3], radius.min(hgt / 2.0)), rgb(spec.color(c), 1.0));
        }
        let cx = x + thick / 2.0;
        if let Some(name) = spec.categories.get(c) {
            p.text(name, fs * 0.9, false, col.text, cx, plot[3] + fs * 1.0, H::Center);
        }
        if spec.show_values {
            p.text(&format_value(v, spec.decimals), fs * 0.85, false, col.text, cx, plot[3] - hgt - fs * 0.7, H::Center);
        }
    }
    if spec.show_step && spec.steps.len() > 1 {
        let size = 0.1 * s;
        if let Some(label) = spec.steps.get(st.step) {
            p.text(label, size, false, col.big, plot[2], plot[1] + size * 0.5, H::Right);
        }
    }
}

/// Legend rows (colour chip + text) down the right of `area`; returns the width it took.
fn legend(p: &mut Painter, spec: &ChartSpec, col: &Colors, area: [f32; 4], fs: f32, rows: &[(usize, String)]) -> f32 {
    let w = rows.iter().map(|(_, t)| p.measure(t, fs * 0.9, false)).fold(0.0f32, f32::max).min((area[2] - area[0]) * 0.35);
    let chip = fs * 0.75;
    let total = w + chip + fs * 1.4;
    let x = area[2] - w - chip - fs * 0.4;
    let line = fs * 1.5;
    let fit = ((area[3] - area[1]) / line).floor().max(0.0) as usize;
    for (k, (c, t)) in rows.iter().take(fit).enumerate() {
        let y = area[1] + line * (k as f32 + 0.5);
        p.fill(&Path::round_rect(x, y - chip / 2.0, x + chip, y + chip / 2.0, chip * 0.2), rgb(spec.color(*c), 1.0));
        p.text(t, fs * 0.9, false, col.text, x + chip + fs * 0.4, y, H::Left);
    }
    total
}

fn lines(p: &mut Painter, spec: &ChartSpec, st: &ChartState, col: &Colors, area: [f32; 4], fs: f32, s: f32) {
    let n = spec.categories.len();
    let steps = spec.steps.len().max(1);
    let all = (0..n).flat_map(|c| (0..steps).map(move |j| (c, j))).map(|(c, j)| spec.value(c, j));
    let (lo, hi) = all.fold((0.0f64, 0.0f64), |(a, b), v| (a.min(v), b.max(v)));
    let span = (hi - lo).max(1e-9);
    let (step, _) = ticks(span, 4);
    let (amin, amax) = ((lo / step).floor() * step, (hi / step).ceil().max(lo / step + 1.0) * step);
    let dec = tick_decimals(step);
    let mut area = area;
    if spec.show_legend {
        let rows: Vec<(usize, String)> = spec.categories.iter().cloned().enumerate().collect();
        let w = legend(p, spec, col, area, fs, &rows);
        area[2] -= w;
    }
    let axis_w = if spec.show_grid {
        p.measure(&format_value(amax, dec), fs * 0.85, false).max(p.measure(&format_value(amin, dec), fs * 0.85, false)) + fs * 0.6
    } else {
        0.0
    };
    let plot = [area[0] + axis_w, area[1] + fs * 0.5, area[2] - fs * 0.8, area[3] - fs * 2.0];
    if plot[2] - plot[0] < 4.0 || plot[3] - plot[1] < 4.0 {
        return;
    }
    let (pw, ph) = (plot[2] - plot[0], plot[3] - plot[1]);
    let y_of = |v: f64| plot[3] - ((v - amin) / (amax - amin).max(1e-12)) as f32 * ph;
    let x_of = |i: f64| if steps > 1 { plot[0] + (i / (steps - 1) as f64) as f32 * pw } else { plot[0] + pw / 2.0 };
    if spec.show_grid {
        let count = (((amax - amin) / step).round() as usize).min(50);
        for k in 0..=count {
            let v = amin + k as f64 * step;
            let y = y_of(v);
            p.fill(&Path::rect(plot[0], y - 0.6, plot[2], y + 0.6), col.grid);
            p.text(&format_value(v, dec), fs * 0.85, false, col.dim, plot[0] - fs * 0.4, y, H::Right);
        }
    }
    p.fill(&Path::rect(plot[0], y_of(amin.max(0.0).min(amax)) - 0.8, plot[2], y_of(amin.max(0.0).min(amax)) + 0.8), col.axis);
    // step labels under the axis, thinned to about 8
    let every = steps.div_ceil(8).max(1);
    for (j, label) in spec.steps.iter().enumerate().filter(|(j, _)| j % every == 0 || *j + 1 == steps) {
        p.text(label, fs * 0.85, false, col.dim, x_of(j as f64), plot[3] + fs * 1.0, H::Center);
    }
    let head = st.step_pos.clamp(0.0, (steps - 1) as f64);
    let lw = (0.005 * s).max(1.0);
    let base = y_of(amin.max(0.0).min(amax));
    // (x, y, value) of each line's head, for the value labels
    let mut heads: Vec<(f32, f32, f64)> = Vec::with_capacity(n);
    for c in 0..n {
        let mut pts: Vec<(f32, f32)> = (0..=head.floor() as usize).map(|j| (x_of(j as f64), y_of(spec.value(c, j) * st.grow))).collect();
        let f = head - head.floor();
        let j0 = head.floor() as usize;
        let vh = spec.value(c, j0) * (1.0 - f) + spec.value(c, (j0 + 1).min(steps - 1)) * f;
        if f > 1e-9 {
            pts.push((x_of(head), y_of(vh * st.grow)));
        }
        let color = spec.color(c);
        if spec.kind == ChartKind::Area && pts.len() > 1 {
            let mut poly = pts.clone();
            if let (Some(&(xl, _)), Some(&(xf, _))) = (pts.last(), pts.first()) {
                poly.push((xl, base));
                poly.push((xf, base));
            }
            p.fill(&Path::polygon(&poly), rgb(color, 0.3));
        }
        if pts.len() > 1 {
            p.fill(&stroke_polyline(&pts, lw), rgb(color, 1.0));
        }
        if let Some(&(x, y)) = pts.last() {
            p.fill(&Path::ellipse(x, y, lw * 1.8, lw * 1.8), rgb(color, 1.0));
            heads.push((x, y, vh * st.grow));
        }
    }
    if spec.show_values {
        // the labels beside the line heads, nudged apart so close lines do not overprint
        let gap = fs * 0.95;
        let mut order: Vec<usize> = (0..heads.len()).collect();
        order.sort_by(|&a, &b| heads[a].1.total_cmp(&heads[b].1));
        let mut ys: Vec<f32> = order.iter().map(|&i| heads[i].1).collect();
        for k in 1..ys.len() {
            ys[k] = ys[k].max(ys[k - 1] + gap);
        }
        // the stack may have run off the bottom: push it back up
        if let Some(&last) = ys.last()
            && last > plot[3]
        {
            let shift = last - plot[3];
            ys.iter_mut().for_each(|y| *y -= shift);
            for k in (0..ys.len().saturating_sub(1)).rev() {
                ys[k] = ys[k].min(ys[k + 1] - gap);
            }
        }
        for (&i, &y) in order.iter().zip(&ys) {
            let (x, _, v) = heads[i];
            p.text(&format_value(v, spec.decimals), fs * 0.85, false, col.text, x + lw * 3.0, y, H::Left);
        }
    }
}

fn pie(p: &mut Painter, spec: &ChartSpec, st: &ChartState, col: &Colors, area: [f32; 4], fs: f32, s: f32) {
    let vals: Vec<f64> = st.values.iter().map(|v| v.max(0.0)).collect();
    let total: f64 = vals.iter().sum();
    let mut area = area;
    if spec.show_legend {
        let rows: Vec<(usize, String)> = spec
            .categories
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let pct = if total > 0.0 { vals.get(i).copied().unwrap_or(0.0) / total * 100.0 } else { 0.0 };
                (i, format!("{name}  {}%", format_value(pct, 1)))
            })
            .collect();
        let w = legend(p, spec, col, area, fs, &rows);
        area[2] -= w + fs;
    }
    let (cx, cy) = ((area[0] + area[2]) / 2.0, (area[1] + area[3]) / 2.0);
    let outer = 0.46 * (area[2] - area[0]).min(area[3] - area[1]);
    if outer < 2.0 {
        return;
    }
    let inner = if spec.kind == ChartKind::Donut { outer * 0.58 } else { 0.0 };
    if total > 0.0 {
        let sweep = TAU * st.grow as f32;
        let mut a = -FRAC_PI_2;
        for (i, &v) in vals.iter().enumerate() {
            let da = (v / total) as f32 * sweep;
            if da > 1e-5 {
                p.fill(&wedge(cx, cy, outer, inner, a, a + da), rgb(spec.color(i), 1.0));
                // a thin gap in the card colour between slices
                let gap = Path::rect(-0.0, -0.6, outer + 1.0, 0.6);
                let rot = Affine::translate(f64::from(cx), f64::from(cy)).then_apply(&Affine::rotate_deg(f64::from(a.to_degrees())));
                p.fill(&gap.transformed(&xform(&rot)), if spec.background { col.card } else { [0.0; 4] });
                if spec.show_values && v / total >= 0.04 {
                    let mid = a + da / 2.0;
                    let r = if inner > 0.0 { (outer + inner) / 2.0 } else { outer * 0.66 };
                    p.text(
                        &format!("{}%", format_value(v / total * 100.0, 0)),
                        fs * 0.9,
                        true,
                        col.on_color,
                        cx + r * mid.cos(),
                        cy + r * mid.sin(),
                        H::Center,
                    );
                }
            }
            a += da;
        }
    }
    if spec.show_step
        && spec.steps.len() > 1
        && let Some(label) = spec.steps.get(st.step)
    {
        if inner > 0.0 {
            p.text(label, inner * 0.5, false, col.text, cx, cy, H::Center);
        } else {
            let size = 0.08 * s;
            p.text(label, size, false, col.big, area[2], area[1] + size * 0.5, H::Right);
        }
    }
}

/// The chart alone as straight-alpha sRGB RGBA8 at `w`×`h` (previews, thumbnails).
pub fn chart_rgba(spec: &ChartSpec, u: f64, d: f64, canvas: (u32, u32), w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w.clamp(1, 4096), h.clamp(1, 4096));
    let mut img = Image::new(w as usize, h as usize);
    let k = (f64::from(w) / f64::from(canvas.0.max(1))).min(f64::from(h) / f64::from(canvas.1.max(1)));
    draw_chart(spec, u, d, canvas, &Affine::scale(k, k), &mut img);
    img.to_rgba8()
}

#[cfg(test)]
mod tests;
