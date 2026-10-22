//! Distort and Transform effects (geometry).

use filmcraft_project::EffectInstance;
use rayon::prelude::*;

use super::*;
use crate::effects::warp_opt;

/// 3×3 projective matrix, row-major, mapping (x, y, 1).
pub(crate) type H3 = [f64; 9];

/// Heckbert's square-to-quad: maps the unit square's (0,0),(1,0),(1,1),(0,1) to `q`'s points.
pub(crate) fn square_to_quad(q: [Vec2; 4]) -> H3 {
    let [p0, p1, p2, p3] = q;
    let sx = p0.x - p1.x + p2.x - p3.x;
    let sy = p0.y - p1.y + p2.y - p3.y;
    if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
        return [p1.x - p0.x, p2.x - p1.x, p0.x, p1.y - p0.y, p2.y - p1.y, p0.y, 0.0, 0.0, 1.0];
    }
    let (dx1, dx2, dy1, dy2) = (p1.x - p2.x, p3.x - p2.x, p1.y - p2.y, p3.y - p2.y);
    let det = dx1 * dy2 - dx2 * dy1;
    let g = (sx * dy2 - dx2 * sy) / det;
    let h = (dx1 * sy - sx * dy1) / det;
    [p1.x - p0.x + g * p1.x, p3.x - p0.x + h * p3.x, p0.x, p1.y - p0.y + g * p1.y, p3.y - p0.y + h * p3.y, p0.y, g, h, 1.0]
}

pub(crate) fn h_inverse(m: &H3) -> Option<H3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let co =
        [e * i - f * h, -(d * i - f * g), d * h - e * g, -(b * i - c * h), a * i - c * g, -(a * h - b * g), b * f - c * e, -(a * f - c * d), a * e - b * d];
    let det = a * co[0] + b * co[1] + c * co[2];
    if det.abs() < 1e-18 {
        return None;
    }
    Some([co[0] / det, co[3] / det, co[6] / det, co[1] / det, co[4] / det, co[7] / det, co[2] / det, co[5] / det, co[8] / det])
}

#[inline]
pub(crate) fn h_apply(m: &H3, x: f64, y: f64) -> Option<(f64, f64)> {
    let w = m[6] * x + m[7] * y + m[8];
    if w.abs() < 1e-12 {
        return None;
    }
    Some(((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w))
}

/// 3D Rotate: rotates the layer about its centre in 3-D (X, then Y, then Z) with perspective.
pub fn rotate_3d(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (ax, ay, az) = ((fv(e, "rot_x", cx) as f64).to_radians(), (fv(e, "rot_y", cx) as f64).to_radians(), (fv(e, "rot_z", cx) as f64).to_radians());
    let z = fv(e, "z", cx) as f64 * cx.px_scale as f64;
    if ax.abs() < 1e-9 && ay.abs() < 1e-9 && az.abs() < 1e-9 && z.abs() < 1e-9 {
        return;
    }
    let persp = fv(e, "perspective", cx).clamp(0.0, 100.0) as f64;
    let (w, h) = (img.w as f64, img.h as f64);
    let f = if persp <= 0.0 { 1e7 } else { w.max(h) * (0.6 + (100.0 - persp) / 100.0 * 4.0) };
    let hide = bv(e, "hide_back");
    let (sx, cxr) = ax.sin_cos();
    let (sy, cy) = ay.sin_cos();
    let (sz, cz) = az.sin_cos();
    // R = Rz · Ry · Rx (screen y down; positive Z is clockwise like Rotation)
    let rx = [[1.0, 0.0, 0.0], [0.0, cxr, -sx], [0.0, sx, cxr]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
        let mut o = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    let r = mul(rz, mul(ry, rx));
    let u = [r[0][0], r[1][0], r[2][0]];
    let v = [r[0][1], r[1][1], r[2][1]];
    let n = [r[0][2], r[1][2], r[2][2]];
    let back = n[2] < 0.0;
    if hide && back {
        img.px.fill(0.0);
        return;
    }
    let c = [0.0, 0.0, f + z];
    let nc = n[0] * c[0] + n[1] * c[1] + n[2] * c[2];
    warp_opt(img, |x, y| {
        let d = [x - w / 2.0, y - h / 2.0, f];
        let den = n[0] * d[0] + n[1] * d[1] + n[2] * d[2];
        if den.abs() < 1e-12 {
            return None;
        }
        let t = nc / den;
        if t <= 0.0 {
            return None;
        }
        let l = [d[0] * t - c[0], d[1] * t - c[1], d[2] * t - c[2]];
        let su = l[0] * u[0] + l[1] * u[1] + l[2] * u[2];
        let sv = l[0] * v[0] + l[1] * v[1] + l[2] * v[2];
        Some((su + w / 2.0, sv + h / 2.0))
    });
}

/// Grow / Shrink: scale animated across the clip from Scale From to Scale To.
pub fn grow(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let p = ease(chv(e, "easing"), clip_progress(cx));
    let (a, b) = (fv(e, "from", cx) as f64, fv(e, "to", cx) as f64);
    let s = (a + (b - a) * p) / 100.0;
    let c = pv(e, "center", cx, img);
    place(img, c, Vec2::ZERO, s.max(1e-4), 0.0);
}

/// Accumulate `n` renders of `f(img, k)` for k in −0.5..0.5 (simple motion blur).
fn motion_blurred(img: &mut Image, n: usize, f: impl Fn(&mut Image, f64) + Sync) {
    if n <= 1 {
        f(img, 0.0);
        return;
    }
    let src = img.clone();
    let parts: Vec<Image> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut c = src.clone();
            f(&mut c, i as f64 / (n - 1) as f64 - 0.5);
            c
        })
        .collect();
    let inv = 1.0 / n as f32;
    img.px.par_iter_mut().enumerate().for_each(|(i, v)| *v = parts.iter().map(|p| p.px[i]).sum::<f32>() * inv);
}

/// Move: the layer slides from Offset From to Offset To across the clip.
pub fn move_fx(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let p = clip_progress(cx);
    let kind = chv(e, "easing");
    let (a, b) = (offv(e, "from", cx), offv(e, "to", cx));
    let blur = fv(e, "motion_blur", cx) as f64 / 100.0;
    let span = cx.env.map_or(0.0, |env| 1.0 / (env.frame_rate() * env.clip_seconds()).max(1.0));
    let c = Vec2::new(img.w as f64 / 2.0, img.h as f64 / 2.0);
    let n = if blur > 0.0 && span > 0.0 { 5 } else { 1 };
    motion_blurred(img, n, |im, k| {
        let q = ease(kind, p + k * blur * span);
        place(im, c, a + (b - a) * q, 1.0, 0.0);
    });
}

/// Spin: rotates by Rotation Amount across the clip.
pub fn spin(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let p = ease(chv(e, "easing"), clip_progress(cx));
    let ang = fv(e, "amount", cx) as f64 * p;
    let s = fv(e, "scale", cx) as f64 / 100.0;
    let c = pv(e, "center", cx, img);
    place(img, c, Vec2::ZERO, s.max(1e-4), ang);
}

/// Wiggle: smooth random position / rotation / scale jitter.
pub fn wiggle(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let t = cx.seconds * fv(e, "frequency", cx) as f64;
    let seed = fv(e, "seed", cx).max(0.0) as u64;
    let amt = (fv(e, "amount", cx) * cx.px_scale) as f64;
    let dims = chv(e, "dimensions");
    let ox = if dims == 2 { 0.0 } else { noise1(t, 0, seed) as f64 * amt * 1.6 };
    let oy = if dims == 1 { 0.0 } else { noise1(t, 1, seed) as f64 * amt * 1.6 };
    let rot = noise1(t, 2, seed) as f64 * fv(e, "rotation", cx) as f64 * 1.6;
    let sc = 1.0 + noise1(t, 3, seed) as f64 * fv(e, "scale", cx) as f64 / 100.0 * 1.6;
    let c = Vec2::new(img.w as f64 / 2.0, img.h as f64 / 2.0);
    place(img, c, Vec2::new(ox, oy), sc.max(1e-3), rot);
}

/// Camera Shake: layered smooth noise on position, rotation and zoom, with optional motion blur.
pub fn camera_shake(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let freq = fv(e, "frequency", cx) as f64;
    let seed = fv(e, "seed", cx).max(0.0) as u64;
    let amt = (fv(e, "amount", cx) * cx.px_scale) as f64;
    let rot = fv(e, "rotation", cx) as f64;
    let zoom = fv(e, "zoom", cx) as f64 / 100.0;
    let octaves = fv(e, "complexity", cx).clamp(1.0, 6.0) as usize;
    let blur = fv(e, "motion_blur", cx) as f64 / 100.0;
    let fps = cx.env.map_or(30.0, |env| env.frame_rate());
    let shake = |t: f64, ch: u64| -> f64 {
        let (mut s, mut a, mut f, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for o in 0..octaves {
            s += noise1(t * freq * f, ch * 10 + o as u64, seed) as f64 * a;
            norm += a;
            a *= 0.5;
            f *= 2.0;
        }
        s / norm * 1.6
    };
    let c = Vec2::new(img.w as f64 / 2.0, img.h as f64 / 2.0);
    let t0 = cx.seconds;
    let n = if blur > 0.0 { 5 } else { 1 };
    motion_blurred(img, n, |im, k| {
        let t = t0 + k * blur / fps;
        let off = Vec2::new(shake(t, 0) * amt, shake(t, 1) * amt);
        // zoom in a little with the shake so edges stay covered
        let s = 1.0 + zoom * (0.5 + 0.5 * shake(t, 3));
        place(im, c, off, s, shake(t, 2) * rot);
    });
}

/// Spacer: insets the layer inside margins (scaled to fit), with rounded corners and an optional
/// background fill.
pub fn spacer(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let k = cx.px_scale as f64;
    let l = fv(e, "left", cx) as f64 * k;
    let (t, r, b) = if bv(e, "uniform") { (l, l, l) } else { (fv(e, "top", cx) as f64 * k, fv(e, "right", cx) as f64 * k, fv(e, "bottom", cx) as f64 * k) };
    let (w, h) = (img.w as f64, img.h as f64);
    let (bw, bh) = ((w - l - r).max(1.0), (h - t - b).max(1.0));
    let s = (bw / w).min(bh / h);
    let (dw, dh) = (w * s, h * s);
    let (x0, y0) = (l + (bw - dw) / 2.0, t + (bh - dh) / 2.0);
    let m = Affine::translate(x0, y0).then_apply(&Affine::scale(s, s));
    let mut out = img.transformed(img.w, img.h, &m);
    let rad = (fv(e, "radius", cx) as f64 * k).min(dw.min(dh) / 2.0);
    if rad > 0.0 {
        round_rect_alpha(&mut out, x0, y0, x0 + dw, y0 + dh, rad as f32, 0.0);
    }
    if bv(e, "fill") {
        let c = lin(cv(e, "color", cx));
        out.px.par_chunks_mut(4).for_each(|p| {
            let k = 1.0 - p[3];
            p[0] += c[0] * k;
            p[1] += c[1] * k;
            p[2] += c[2] * k;
            p[3] = 1.0;
        });
    }
    *img = out;
}

/// Signed distance from (px, py) to a rounded rectangle (negative inside).
#[inline]
pub(crate) fn round_rect_sdf(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
    let (qx, qy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - r
}

/// Multiply alpha by the coverage of a rounded rectangle (feathered inward by `feather`).
fn round_rect_alpha(img: &mut Image, x0: f64, y0: f64, x1: f64, y1: f64, r: f32, feather: f32) {
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let d = round_rect_sdf(x as f32 + 0.5, y as f32 + 0.5, x0 as f32, y0 as f32, x1 as f32, y1 as f32, r);
            let a = if feather > 0.0 { (-d / feather).clamp(0.0, 1.0) } else { (0.5 - d).clamp(0.0, 1.0) };
            if a < 1.0 {
                for v in &mut row[x * 4..x * 4 + 4] {
                    *v *= a;
                }
            }
        }
    });
}

/// Clone: the layer repeated in a grid of tiles (optionally mirrored alternately).
pub fn clone_fx(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let cols = fv(e, "columns", cx).clamp(1.0, 16.0) as usize;
    let rows = fv(e, "rows", cx).clamp(1.0, 16.0) as usize;
    if cols == 1 && rows == 1 {
        return;
    }
    let gap = (fv(e, "gap", cx) * cx.px_scale) as f64;
    let mirror = bv(e, "mirror");
    let (w, h) = (img.w as f64, img.h as f64);
    let tw = ((w - gap * (cols - 1) as f64) / cols as f64).max(1.0);
    let th = ((h - gap * (rows - 1) as f64) / rows as f64).max(1.0);
    let s = (tw / w).min(th / h);
    let src = img.clone();
    let wi = img.w;
    img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        for x in 0..wi {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let (ci, ri) = ((px / (tw + gap)).floor() as usize, (py / (th + gap)).floor() as usize);
            let (ci, ri) = (ci.min(cols - 1), ri.min(rows - 1));
            let (ox, oy) = (ci as f64 * (tw + gap) + (tw - w * s) / 2.0, ri as f64 * (th + gap) + (th - h * s) / 2.0);
            let mut u = (px - ox) / s;
            let v = (py - oy) / s;
            if mirror && (ci + ri) % 2 == 1 {
                u = w - u;
            }
            let p = if u < 0.0 || v < 0.0 || u > w || v > h { [0.0; 4] } else { src.sample_bilinear(u as f32, v as f32) };
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
}

/// Auto Align: moves the layer's visible content (alpha bounding box) to the chosen frame edge
/// or centre.
pub fn auto_align(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (hz, vt) = (chv(e, "horizontal"), chv(e, "vertical"));
    if hz == 0 && vt == 0 {
        return;
    }
    let w = img.w;
    let bounds = img
        .px
        .par_chunks(w * 4)
        .enumerate()
        .filter_map(|(y, row)| {
            let xs: Vec<usize> = (0..w).filter(|&x| row[x * 4 + 3] > 0.01).collect();
            Some((xs.first().copied()?, xs.last().copied()?, y))
        })
        .fold(|| (usize::MAX, 0usize, usize::MAX, 0usize), |a, (x0, x1, y)| (a.0.min(x0), a.1.max(x1), a.2.min(y), a.3.max(y)))
        .reduce(|| (usize::MAX, 0usize, usize::MAX, 0usize), |a, b| (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3)));
    if bounds.0 == usize::MAX {
        return;
    }
    let (x0, x1, y0, y1) = (bounds.0 as f64, bounds.1 as f64 + 1.0, bounds.2 as f64, bounds.3 as f64 + 1.0);
    let (mx, my) = ((fv(e, "margin_x", cx) * cx.px_scale) as f64, (fv(e, "margin_y", cx) * cx.px_scale) as f64);
    let (fw, fh) = (img.w as f64, img.h as f64);
    let dx = match hz {
        1 => mx - x0,
        2 => (fw - (x1 - x0)) / 2.0 - x0,
        3 => fw - mx - x1,
        _ => 0.0,
    };
    let dy = match vt {
        1 => my - y0,
        2 => (fh - (y1 - y0)) / 2.0 - y0,
        3 => fh - my - y1,
        _ => 0.0,
    };
    affine_warp(img, &Affine::translate(dx.round(), dy.round()));
}

/// Mosaic (and Mosaic (Legacy)): solid blocks; Softness blends towards a smooth interpolation of
/// the block colours.
pub fn mosaic(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let bx = fv(e, "horizontal", cx).max(1.0) as usize;
    let by = fv(e, "vertical", cx).max(1.0) as usize;
    let soft = (fv(e, "softness", cx) / 100.0).clamp(0.0, 1.0);
    let (w, h) = (img.w, img.h);
    let bw = (w as f32 / bx as f32).max(1.0);
    let bh = (h as f32 / by as f32).max(1.0);
    let (nx, ny) = ((w as f32 / bw).ceil() as usize, (h as f32 / bh).ceil() as usize);
    let mut small = Image::new(nx.max(1), ny.max(1));
    for j in 0..small.h {
        for i in 0..small.w {
            let p = img.sample_bilinear_clamped((i as f32 + 0.5) * bw, (j as f32 + 0.5) * bh);
            let k = (j * small.w + i) * 4;
            small.px[k..k + 4].copy_from_slice(&p);
        }
    }
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / bw, (y as f32 + 0.5) / bh);
            // tile index as in the original Mosaic (pixel x belongs to tile floor(x / bw))
            let (ti, tj) = ((x as f32 / bw) as usize, (y as f32 / bh) as usize);
            let sharp = small.get(ti.min(small.w - 1), tj.min(small.h - 1));
            let p = if soft > 0.0 {
                let sm = small.sample_bilinear_clamped(u, v);
                [0, 1, 2, 3].map(|k| sharp[k] + (sm[k] - sharp[k]) * soft)
            } else {
                sharp
            };
            row[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
}
