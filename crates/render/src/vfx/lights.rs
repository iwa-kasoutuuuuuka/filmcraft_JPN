//! Lights & Glows on the CPU only (Lens Flare, RGB Split, Light Leaks, Edge Glow and Alpha Glow
//! are in `gpufx_more`, with their GPU twin).

use filmcraft_project::EffectInstance;
use rayon::prelude::*;

use super::*;
use crate::effects::gaussian;

fn blurred(src: &Image, sigma: f32) -> Image {
    let mut g = src.clone();
    gaussian(&mut g, sigma, sigma, false);
    g
}

/// Add `light` (premultiplied linear) to `img`: Add or Screen.
fn add_image(img: &mut Image, light: &Image, gain: [f32; 3], screen: bool) {
    img.px.par_chunks_mut(4).zip(light.px.par_chunks(4)).for_each(|(p, l)| {
        let add = [l[0] * gain[0], l[1] * gain[1], l[2] * gain[2]];
        if screen {
            let a = [add[0].min(1.0), add[1].min(1.0), add[2].min(1.0)];
            for k in 0..3 {
                p[k] = p[k] + a[k] - p[k].min(1.0) * a[k];
            }
            p[3] = p[3] + (1.0 - p[3]) * a[0].max(a[1]).max(a[2]);
        } else {
            add_light(p, add);
        }
    });
}

fn saturate_image(img: &mut Image, sat: f32) {
    if (sat - 1.0).abs() < 1e-4 {
        return;
    }
    img.px.par_chunks_mut(4).for_each(|p| {
        let l = luma([p[0], p[1], p[2]]);
        for v in &mut p[..3] {
            *v = (l + (*v - l) * sat).max(0.0);
        }
    });
}

/// Replace the hue of a light buffer with `c` (keeping its luminance), by `k`.
fn tint(img: &mut Image, c: [f32; 3], k: f32) {
    if k <= 0.0 {
        return;
    }
    img.px.par_chunks_mut(4).for_each(|p| {
        let l = luma([p[0], p[1], p[2]]);
        for i in 0..3 {
            p[i] += (l * c[i] * 1.5 - p[i]) * k;
        }
    });
}

/// Echo Glow: glow repeated in widening echoes that fade by Decay.
pub fn echo_glow(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let hi = highlights(img, fv(e, "threshold", cx) / 100.0, 0.1);
    let n = fv(e, "echoes", cx).clamp(1.0, 12.0) as usize;
    let r = fv(e, "radius", cx) * cx.px_scale * 0.5;
    let spread = fv(e, "spread", cx).max(1.0);
    let inten = fv(e, "intensity", cx) / 100.0;
    let decay = 1.0 - fv(e, "decay", cx) / 100.0;
    let col = lin(cv(e, "color", cx));
    let layers: Vec<(Image, f32)> = (0..n).into_par_iter().map(|i| (blurred(&hi, r * spread.powi(i as i32)), decay.powi(i as i32))).collect();
    let mut acc = Image::new(img.w, img.h);
    for (l, wgt) in &layers {
        acc.px.par_iter_mut().zip(l.px.par_iter()).for_each(|(a, b)| *a += b * wgt);
    }
    let norm = inten / layers.iter().map(|l| l.1).sum::<f32>().max(1e-6) * 2.0;
    tint(&mut acc, col, 1.0);
    add_image(img, &acc, [norm; 3], false);
}

/// Sum of `a^k · src(p − k·d)` for k = 0…len (an exponentially decaying streak), by doubling.
fn streak(src: &Image, dir: (f32, f32), len: f32) -> Image {
    let a = (-1.0 / len.max(1.0)).exp();
    let mut s = src.clone();
    let mut n = 1.0f32;
    while n < len * 3.0 {
        let prev = s.clone();
        let k = a.powf(n);
        let (dx, dy) = (dir.0 * n, dir.1 * n);
        let w = s.w;
        s.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for x in 0..w {
                let q = prev.sample_bilinear(x as f32 + 0.5 - dx, y as f32 + 0.5 - dy);
                for c in 0..4 {
                    row[x * 4 + c] += q[c] * k;
                }
            }
        });
        n *= 2.0;
    }
    let norm = 1.0 - a;
    s.px.par_iter_mut().for_each(|v| *v *= norm);
    s
}

/// Glint: star streaks from the brightest points.
pub fn glint(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let hi = highlights(img, fv(e, "threshold", cx) / 100.0, 0.08);
    let rays = [2usize, 4, 6, 8][chv(e, "rays").min(3) as usize];
    let len = fv(e, "length", cx) * cx.px_scale;
    let rot = fv(e, "rotation", cx).to_radians();
    let inten = fv(e, "intensity", cx) / 100.0 * 3.0;
    let col = lin(cv(e, "color", cx));
    let colorize = fv(e, "colorize", cx) / 100.0;
    let parts: Vec<Image> = (0..rays)
        .into_par_iter()
        .map(|i| {
            let a = rot + std::f32::consts::TAU * i as f32 / rays as f32;
            streak(&hi, (a.cos(), a.sin()), len * 0.35)
        })
        .collect();
    let mut acc = Image::new(img.w, img.h);
    for p in &parts {
        acc.px.par_iter_mut().zip(p.px.par_iter()).for_each(|(a, b)| *a += b);
    }
    tint(&mut acc, col, colorize);
    add_image(img, &acc, [inten * col[0].max(0.2), inten * col[1].max(0.2), inten * col[2].max(0.2)], false);
}

/// Volumetric Rays: light shafts radiating from a source point (radial blur of the highlights).
pub fn volumetric_rays(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let c = pv(e, "center", cx, img);
    let len = fv(e, "length", cx) / 100.0;
    let inten = fv(e, "intensity", cx) / 100.0;
    let col = lin(cv(e, "color", cx));
    let only = bv(e, "only");
    let mut hi = highlights(img, fv(e, "threshold", cx) / 100.0, 0.1);
    let (fw, fh) = (img.w, img.h);
    let mut down = 1.0;
    while hi.w > 960 {
        hi = hi.downsample2();
        down *= 2.0;
    }
    let (ccx, ccy) = ((c.x / down) as f32, (c.y / down) as f32);
    const N: usize = 48;
    let src = hi.clone();
    let w = hi.w;
    hi.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0;
            for i in 0..N {
                let t = i as f32 / N as f32 * len;
                let s = src.sample_bilinear(px + (ccx - px) * t, py + (ccy - py) * t);
                let wgt = 1.0 - i as f32 / N as f32 * 0.6;
                for k in 0..4 {
                    acc[k] += s[k] * wgt;
                }
                wsum += wgt;
            }
            for k in 0..4 {
                row[x * 4 + k] = acc[k] / wsum;
            }
        }
    });
    let rays = if down > 1.0 { hi.transformed(fw, fh, &Affine::scale(fw as f64 / hi.w as f64, fh as f64 / hi.h as f64)) } else { hi };
    if only {
        *img = Image::new(fw, fh);
    }
    add_image(img, &rays, col.map(|v| v * inten * 2.5), false);
}

/// Wonder Glow: a rich multi-scale glow (three radii) with saturation and colour controls.
pub fn wonder_glow(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let mut hi = highlights(img, fv(e, "threshold", cx) / 100.0, 0.15);
    saturate_image(&mut hi, fv(e, "saturation", cx) / 100.0);
    if bv(e, "use_color") {
        tint(&mut hi, lin(cv(e, "color", cx)), 1.0);
    }
    let r = fv(e, "radius", cx) * cx.px_scale * 0.5;
    let inten = fv(e, "intensity", cx) / 100.0;
    let parts: Vec<(Image, f32)> = [(0.25f32, 0.45f32), (1.0, 0.35), (3.0, 0.2)].par_iter().map(|&(k, wgt)| (blurred(&hi, r * k), wgt)).collect();
    let mut acc = Image::new(img.w, img.h);
    for (p, wgt) in &parts {
        acc.px.par_iter_mut().zip(p.px.par_iter()).for_each(|(a, b)| *a += b * wgt);
    }
    add_image(img, &acc, [inten * 1.5; 3], chv(e, "mode") == 1);
}
