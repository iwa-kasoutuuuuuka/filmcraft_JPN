//! More effects with a GPU implementation (GPU3): Noise, Drop Shadow, the warps (Corner Pin,
//! Lens Distortion, Twirl, Wave Warp, Spherize, Basic 3D), Magnify, Find Edges, Emboss, Bevel
//! Alpha, Replicate, Strobe Light and the Generate effects (Ramp, 4-Color Gradient, Circle, Grid).
//!
//! As for the rest of [`crate::gpufx`], [`ExtOp::eval`] reads an effect's parameters into plain
//! numbers and [`ExtOp::apply`] is the CPU reference — the code these effects always ran on the
//! CPU, moved here unchanged (geometry stays in f64 where it was), so the CPU render is the same
//! as before. `filmcraft_gpu` reproduces `apply` in WGSL; [`ExtOp::gpu_ok`] keeps the cases the
//! shader does not reproduce exactly (non-finite numbers, Noise grains that are not whole pixels)
//! on the CPU.

use filmcraft_color::{luma709, srgb_to_linear};
use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::EffectInstance;
use rayon::prelude::*;

use crate::effects::{FxCtx, b, choice, color, dec, enc, f, gaussian_boxes, point, warp_opt};
use crate::image::Image;
use crate::vfx::{bv, chv, fv, pv};

/// A geometric warp: every output pixel samples the image (bilinear, transparent outside) at a
/// mapped position.
#[derive(Clone, Debug, PartialEq)]
pub enum Warp {
    /// Corner Pin: output pixel → unit square through `inv`, then × the image size (outside the
    /// square: transparent).
    CornerPin { inv: [f64; 9], w: f64, h: f64 },
    /// Lens Distortion: radial `1 − k r²` around `(cx, cy)`, radius normalised by `norm`.
    Lens { k: f64, cx: f64, cy: f64, norm: f64 },
    /// Twirl: rotation by `ang (1 − d / rad)²` inside `rad`.
    Twirl { cx: f64, cy: f64, ang: f64, rad: f64 },
    /// Wave Warp: displacement `hgt sin(along / wid · 2π − phase)` across the direction (ux, uy).
    Wave { hgt: f64, wid: f64, ux: f64, uy: f64, phase: f64 },
    /// Spherize: radius `r` around `(cx, cy)`.
    Spherize { cx: f64, cy: f64, r: f64 },
    /// Basic 3D: the ray through each pixel meets the plane (normal `n`, axes `u` / `v`) at `z0`.
    Basic3d { n: [f64; 3], u: [f64; 3], v: [f64; 3], z0: f64, focal: f64, w: f64, h: f64 },
}

impl Warp {
    /// The source position for the output pixel centre `(x, y)` (None: transparent).
    pub fn map(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        match *self {
            Warp::CornerPin { ref inv, w, h } => {
                let (u, v) = crate::vfx::distort::h_apply(inv, x, y)?;
                (u >= -1e-3 && v >= -1e-3 && u <= 1.0 + 1e-3 && v <= 1.0 + 1e-3).then_some((u * w, v * h))
            }
            Warp::Lens { k, cx, cy, norm } => {
                let (dx, dy) = ((x - cx) / norm, (y - cy) / norm);
                let r2 = dx * dx + dy * dy;
                let s = 1.0 - k * r2;
                Some((cx + dx * s * norm, cy + dy * s * norm))
            }
            Warp::Twirl { cx, cy, ang, rad } => {
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if d >= rad {
                    return Some((x, y));
                }
                let k = 1.0 - d / rad;
                let a = ang * k * k;
                let (s, co) = a.sin_cos();
                Some((cx + dx * co - dy * s, cy + dx * s + dy * co))
            }
            Warp::Wave { hgt, wid, ux, uy, phase } => {
                let along = x * ux + y * uy;
                let off = hgt * (along / wid * std::f64::consts::TAU - phase).sin();
                Some((x - uy * off, y + ux * off))
            }
            Warp::Spherize { cx, cy, r } => {
                let (dx, dy) = (x - cx, y - cy);
                let d = dx.hypot(dy);
                if d >= r || d < 1e-9 {
                    return Some((x, y));
                }
                let n = d / r;
                let k = 1.0 - 0.5 * (1.0 - n * n).sqrt();
                Some((cx + dx * k, cy + dy * k))
            }
            Warp::Basic3d { n, u, v, z0, focal, w, h } => {
                let (px, py) = (x - w / 2.0, y - h / 2.0);
                let dir = [px, py, focal];
                let denom = n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2];
                if denom.abs() < 1e-9 {
                    return None;
                }
                let tt = (n[2] * z0) / denom;
                if tt <= 0.0 {
                    return None;
                }
                let hit = [dir[0] * tt, dir[1] * tt, dir[2] * tt - z0];
                let su = hit[0] * u[0] + hit[1] * u[1] + hit[2] * u[2];
                let sv = hit[0] * v[0] + hit[1] * v[1] + hit[2] * v[2];
                Some((su + w / 2.0, sv + h / 2.0))
            }
        }
    }

    fn numbers(&self) -> Vec<f64> {
        match self {
            Warp::CornerPin { inv, w, h } => inv.iter().copied().chain([*w, *h]).collect(),
            Warp::Lens { k, cx, cy, norm } => vec![*k, *cx, *cy, *norm],
            Warp::Twirl { cx, cy, ang, rad } => vec![*cx, *cy, *ang, *rad],
            // (the phase is reduced to cycles in f64 before the GPU sees it: `gpu_ok` bounds it apart)
            Warp::Wave { hgt, wid, ux, uy, .. } => vec![*hgt, *wid, *ux, *uy],
            Warp::Spherize { cx, cy, r } => vec![*cx, *cy, *r],
            Warp::Basic3d { n, u, v, z0, focal, w, h } => n.iter().chain(u).chain(v).copied().chain([*z0, *focal, *w, *h]).collect(),
        }
    }
}

/// Drop Shadow: the layer's alpha moved by (dx, dy), coloured, scaled by the opacity, blurred,
/// under the layer (or alone).
#[derive(Clone, Debug, PartialEq)]
pub struct DropShadow {
    pub dx: f64,
    pub dy: f64,
    /// Linear shadow colour.
    pub color: [f32; 3],
    pub opacity: f32,
    /// Box radii of the (non-repeating) softness blur per axis (empty: no blur).
    pub boxes: (Vec<u32>, Vec<u32>),
    pub only: bool,
    /// The pixels the moved layer covers (x0, x1, y0, y1, as `Image::transformed` writes them).
    pub rect: [u32; 4],
}

/// The Generate effects: a colour computed per pixel centre, filled over the image.
#[derive(Clone, Debug, PartialEq)]
pub enum Generator {
    /// Ramp (Gradient Ramp): sRGB start / end colours along (vx, vy) from `s`, or radially.
    Ramp { sx: f64, sy: f64, vx: f64, vy: f64, sc: [f32; 3], ec: [f32; 3], radial: bool, blend: f32 },
    /// 4-Color Gradient: inverse-distance weights `1 / d^exp` of four colours at the quarter points.
    FourColor { colors: [[f32; 3]; 4], opacity: f32, exp: f32 },
    /// Circle: a filled antialiased disc (linear colour).
    Circle { cx: f64, cy: f64, r: f32, color: [f32; 3], opacity: f32 },
    /// Grid: lines `border` px wide every `size` px from the image centre (linear colour).
    Grid { size: f32, border: f32, color: [f32; 3], opacity: f32 },
}

/// One evaluated effect of this module.
#[derive(Clone, Debug, PartialEq)]
pub enum ExtOp {
    /// Noise / Noise (Legacy): hashed per grain cell and frame, added to display-encoded colour.
    Noise {
        amount: f32,
        colored: bool,
        clip: bool,
        grain: f32,
        frame: u64,
    },
    Warp(Warp),
    /// Magnify: a loupe of radius `r` (feather `fe`, opacity `op`) magnifying by `m` around (cx, cy).
    Magnify {
        cx: f64,
        cy: f64,
        m: f64,
        r: f32,
        fe: f32,
        op: f32,
        square: bool,
        mode: u32,
    },
    FindEdges {
        invert: bool,
        blend: f32,
    },
    Emboss {
        dx: f32,
        dy: f32,
        contrast: f32,
        blend: f32,
    },
    BevelAlpha {
        th: f32,
        lx: f32,
        ly: f32,
        intensity: f32,
    },
    Replicate {
        n: usize,
    },
    /// Strobe Light while it flashes: linear colour mixed with the image by `blend`.
    Strobe {
        color: [f32; 3],
        blend: f32,
    },
    DropShadow(Box<DropShadow>),
    Generate(Generator),
    /// Nothing to do (Corner Pin at its corners, Spherize of no radius, Strobe between flashes…).
    Nop,
}

/// Effect ids [`ExtOp::eval`] understands.
pub const EXT_EFFECTS: &[&str] = &[
    "noise",
    "noise_legacy",
    "drop_shadow",
    "corner_pin",
    "lens_distortion",
    "twirl",
    "twirl_legacy",
    "wave_warp",
    "spherize",
    "basic_3d",
    "magnify",
    "magnify_legacy",
    "find_edges",
    "emboss",
    "bevel_alpha",
    "replicate",
    "strobe",
    "ramp",
    "four_color_gradient",
    "circle",
    "grid",
];

impl ExtOp {
    /// Evaluate `e` at `cx.t` for a `w`×`h` working image (None: not one of [`EXT_EFFECTS`]).
    pub fn eval(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<ExtOp> {
        let img = Image { w, h, px: Vec::new() };
        let (fw, fh) = (w as f64, h as f64);
        Some(match e.effect.as_str() {
            "noise" | "noise_legacy" => {
                let amount = fv(e, "amount", cx) / 100.0;
                if amount <= 0.0 || amount.is_nan() {
                    return Some(ExtOp::Nop);
                }
                ExtOp::Noise {
                    amount,
                    colored: bv(e, "color"),
                    clip: bv(e, "clip"),
                    grain: (fv(e, "grain_size", cx).max(1.0) * cx.px_scale).max(1.0),
                    frame: (cx.seconds * 30.0) as u64,
                }
            }
            "drop_shadow" => {
                let c = color(e, "color", cx);
                let dir = (f(e, "direction", cx) as f64).to_radians();
                let dist = f(e, "distance", cx) as f64 * cx.px_scale as f64;
                let soft = f(e, "softness", cx) * cx.px_scale * 0.5;
                // (`effects::gaussian` below 0.3 is no blur, as it was)
                let boxes = if soft > 0.3 { gaussian_boxes(w, h, soft, soft) } else { (Vec::new(), Vec::new()) };
                let (dx, dy) = (dir.sin() * dist, -dir.cos() * dist);
                ExtOp::DropShadow(Box::new(DropShadow {
                    dx,
                    dy,
                    rect: crate::gpufx::resample(w, h, &Affine::translate(dx, dy), 1.0).rect,
                    color: dec([c[0], c[1], c[2]]),
                    opacity: (f(e, "opacity", cx) / 100.0).clamp(0.0, 1.0),
                    boxes,
                    only: b(e, "only"),
                }))
            }
            "corner_pin" => {
                let q = [pv(e, "upper_left", cx, &img), pv(e, "upper_right", cx, &img), pv(e, "lower_right", cx, &img), pv(e, "lower_left", cx, &img)];
                let ident = [Vec2::new(0.0, 0.0), Vec2::new(fw, 0.0), Vec2::new(fw, fh), Vec2::new(0.0, fh)];
                if q.iter().zip(&ident).all(|(a, b)| (*a - *b).length() < 1e-6) {
                    return Some(ExtOp::Nop);
                }
                let Some(inv) = crate::vfx::distort::h_inverse(&crate::vfx::distort::square_to_quad(q)) else { return Some(ExtOp::Nop) };
                ExtOp::Warp(Warp::CornerPin { inv, w: fw, h: fh })
            }
            "lens_distortion" => {
                let k = f(e, "curvature", cx) as f64 / 100.0;
                let (cxp, cyp) =
                    (fw / 2.0 + f(e, "h_decentering", cx) as f64 / 100.0 * fw / 2.0, fh / 2.0 + f(e, "v_decentering", cx) as f64 / 100.0 * fh / 2.0);
                ExtOp::Warp(Warp::Lens { k, cx: cxp, cy: cyp, norm: (fw / 2.0).hypot(fh / 2.0) })
            }
            "twirl" | "twirl_legacy" => {
                let c = point(e, "center", cx, &img);
                let ang = (f(e, "angle", cx) as f64).to_radians();
                let rad = f(e, "radius", cx) as f64 / 100.0 * (w.min(h) as f64);
                ExtOp::Warp(Warp::Twirl { cx: c.x, cy: c.y, ang, rad })
            }
            "wave_warp" => {
                let hgt = f(e, "height", cx) as f64 * cx.px_scale as f64;
                let wid = (f(e, "width", cx) as f64 * cx.px_scale as f64).max(1.0);
                let dir = (f(e, "direction", cx) as f64).to_radians();
                let phase = cx.seconds * f(e, "speed", cx) as f64 * std::f64::consts::TAU;
                ExtOp::Warp(Warp::Wave { hgt, wid, ux: dir.sin(), uy: -dir.cos(), phase })
            }
            "spherize" => {
                let r = (fv(e, "radius", cx) * cx.px_scale) as f64;
                // (NaN: no radius, as before)
                if r.is_nan() || r < 0.5 {
                    return Some(ExtOp::Nop);
                }
                let c = pv(e, "center", cx, &img);
                ExtOp::Warp(Warp::Spherize { cx: c.x, cy: c.y, r })
            }
            "basic_3d" => {
                let sw = (f(e, "swivel", cx) as f64).to_radians();
                let tl = (f(e, "tilt", cx) as f64).to_radians();
                let dist = f(e, "distance", cx) as f64;
                let (ss, cs) = sw.sin_cos();
                let (st, ct) = tl.sin_cos();
                // plane normal after rotation (Ry(swivel) * Rx(tilt)) of (0,0,1)
                let focal = fw.max(fh) * 1.2;
                ExtOp::Warp(Warp::Basic3d {
                    n: [ss * ct, -st, cs * ct],
                    u: [cs, 0.0, -ss],
                    v: [ss * st, ct, cs * st],
                    z0: focal + dist * 10.0,
                    focal,
                    w: fw,
                    h: fh,
                })
            }
            "magnify" | "magnify_legacy" => {
                let c = pv(e, "center", cx, &img);
                ExtOp::Magnify {
                    cx: c.x,
                    cy: c.y,
                    m: (fv(e, "magnification", cx) / 100.0).max(0.01) as f64,
                    r: fv(e, "size", cx) * cx.px_scale,
                    fe: fv(e, "feather", cx) * cx.px_scale,
                    op: fv(e, "opacity", cx) / 100.0,
                    square: chv(e, "shape") == 1,
                    mode: chv(e, "mode"),
                }
            }
            "find_edges" => ExtOp::FindEdges { invert: b(e, "invert"), blend: f(e, "blend", cx) / 100.0 },
            "emboss" => {
                let dir = (f(e, "direction", cx) as f64).to_radians();
                let relief = f(e, "relief", cx) * cx.px_scale.max(0.25);
                ExtOp::Emboss {
                    dx: (dir.cos() as f32) * relief,
                    dy: (-dir.sin() as f32) * relief,
                    contrast: f(e, "contrast", cx) / 100.0,
                    blend: f(e, "blend", cx) / 100.0,
                }
            }
            "bevel_alpha" => {
                let ang = (f(e, "angle", cx) as f64).to_radians();
                ExtOp::BevelAlpha {
                    th: f(e, "thickness", cx) * cx.px_scale,
                    lx: ang.cos() as f32,
                    ly: -ang.sin() as f32,
                    intensity: f(e, "intensity", cx) / 100.0,
                }
            }
            "replicate" => ExtOp::Replicate { n: f(e, "count", cx).clamp(1.0, 16.0) as usize },
            "strobe" => {
                let period = f(e, "period", cx).max(0.001) as f64;
                let dur = f(e, "duration", cx) as f64;
                if (cx.seconds % period) < dur {
                    let c = color(e, "color", cx);
                    ExtOp::Strobe { color: dec([c[0], c[1], c[2]]), blend: f(e, "blend", cx) / 100.0 }
                } else {
                    ExtOp::Nop
                }
            }
            "ramp" => {
                let s = point(e, "start", cx, &img);
                let en = point(e, "end", cx, &img);
                let (sc, ec) = (color(e, "start_color", cx), color(e, "end_color", cx));
                ExtOp::Generate(Generator::Ramp {
                    sx: s.x,
                    sy: s.y,
                    vx: en.x - s.x,
                    vy: en.y - s.y,
                    sc: [sc[0], sc[1], sc[2]],
                    ec: [ec[0], ec[1], ec[2]],
                    radial: choice(e, "shape") == 1,
                    blend: f(e, "blend", cx) / 100.0,
                })
            }
            "four_color_gradient" => {
                let cs = [color(e, "c1", cx), color(e, "c2", cx), color(e, "c3", cx), color(e, "c4", cx)];
                ExtOp::Generate(Generator::FourColor {
                    colors: cs.map(|c| [c[0], c[1], c[2]]),
                    opacity: f(e, "opacity", cx) / 100.0,
                    exp: 1.0 + f(e, "blend", cx) / 100.0,
                })
            }
            "circle" => {
                let c = point(e, "center", cx, &img);
                let col = color(e, "color", cx);
                ExtOp::Generate(Generator::Circle {
                    cx: c.x,
                    cy: c.y,
                    r: f(e, "radius", cx) * cx.px_scale,
                    color: dec([col[0], col[1], col[2]]),
                    opacity: f(e, "opacity", cx) / 100.0,
                })
            }
            "grid" => {
                let col = color(e, "color", cx);
                ExtOp::Generate(Generator::Grid {
                    size: (f(e, "size", cx) * cx.px_scale).max(1.0),
                    border: f(e, "border", cx) * cx.px_scale,
                    color: dec([col[0], col[1], col[2]]),
                    opacity: f(e, "opacity", cx) / 100.0,
                })
            }
            _ => return None,
        })
    }

    /// Whether the GPU shader reproduces [`apply`](Self::apply): every number finite and within
    /// what f32 positions on the GPU resolve (|v| < 1e7), Noise grains of whole pixels (the
    /// grain cell is an integer division there), at most 16 copies for Replicate.
    pub fn gpu_ok(&self) -> bool {
        let fin = |v: &[f32]| v.iter().all(|x| x.is_finite());
        let pos = |v: &[f64]| v.iter().all(|x| x.is_finite() && x.abs() < 1e7);
        match self {
            ExtOp::Nop => true,
            ExtOp::Noise { amount, grain, .. } => amount.is_finite() && *grain >= 1.0 && *grain < 4096.0 && grain.fract() == 0.0,
            // (Wave Warp: phases beyond 1e9 rad stay on the CPU, whose f64 `along · 2π − phase` loses the
            // position's low bits there)
            ExtOp::Warp(wp) => pos(&wp.numbers()) && if let Warp::Wave { phase, .. } = wp { phase.abs() < 1e9 } else { true },
            ExtOp::Magnify { cx, cy, m, r, fe, op, .. } => pos(&[*cx, *cy, *m]) && fin(&[*r, *fe, *op]),
            ExtOp::FindEdges { blend, .. } => blend.is_finite(),
            ExtOp::Emboss { dx, dy, contrast, blend } => fin(&[*dx, *dy, *contrast, *blend]) && dx.abs() < 1e7 && dy.abs() < 1e7,
            ExtOp::BevelAlpha { th, lx, ly, intensity } => fin(&[*th, *lx, *ly, *intensity]) && th.abs() < 1e7,
            ExtOp::Replicate { n } => (1..=16).contains(n),
            ExtOp::Strobe { color, blend } => fin(color) && blend.is_finite(),
            ExtOp::DropShadow(d) => pos(&[d.dx, d.dy]) && fin(&d.color) && d.opacity.is_finite(),
            ExtOp::Generate(g) => match g {
                Generator::Ramp { sx, sy, vx, vy, sc, ec, blend, .. } => pos(&[*sx, *sy, *vx, *vy]) && fin(sc) && fin(ec) && blend.is_finite(),
                Generator::FourColor { colors, opacity, exp } => colors.iter().all(|c| fin(c)) && fin(&[*opacity, *exp]),
                Generator::Circle { cx, cy, r, color, opacity } => pos(&[*cx, *cy]) && fin(&[*r, *opacity]) && fin(color),
                Generator::Grid { size, border, color, opacity } => fin(&[*size, *border, *opacity]) && fin(color) && *size >= 1.0 && *size < 1e7,
            },
        }
    }

    /// The CPU reference (the effects' CPU implementation).
    pub fn apply(&self, img: &mut Image) {
        if img.w == 0 || img.h == 0 {
            return;
        }
        match self {
            ExtOp::Nop => {}
            ExtOp::Noise { amount, colored, clip, grain, frame } => noise(img, *amount, *colored, *clip, *grain, *frame),
            ExtOp::Warp(wp) => warp_opt(img, |x, y| wp.map(x, y)),
            ExtOp::Magnify { cx, cy, m, r, fe, op, square, mode } => magnify(img, Vec2::new(*cx, *cy), *m, *r, *fe, *op, *square, *mode),
            ExtOp::FindEdges { invert, blend } => find_edges(img, *invert, *blend),
            ExtOp::Emboss { dx, dy, contrast, blend } => emboss(img, *dx, *dy, *contrast, *blend),
            ExtOp::BevelAlpha { th, lx, ly, intensity } => bevel_alpha(img, *th, *lx, *ly, *intensity),
            ExtOp::Replicate { n } => replicate(img, *n),
            ExtOp::Strobe { color: lc, blend } => {
                let blend = *blend;
                img.map_rgb(|o, _, _| [lc[0] + (o[0] - lc[0]) * blend, lc[1] + (o[1] - lc[1]) * blend, lc[2] + (o[2] - lc[2]) * blend]);
            }
            ExtOp::DropShadow(d) => drop_shadow(img, d),
            ExtOp::Generate(g) => generate(img, g),
        }
    }
}

fn noise(img: &mut Image, amt: f32, colored: bool, clip: bool, grain: f32, frame: u64) {
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let p = &mut row[x * 4..x * 4 + 4];
            let a = p[3];
            if a <= 0.0 {
                continue;
            }
            let (gx, gy) = ((x as f32 / grain) as usize, (y as f32 / grain) as usize);
            let h = |z: u64| crate::effects::hash3(gx, gy, z) - 0.5;
            let n0 = h(frame);
            let ns = if colored { [n0, h(frame.wrapping_add(7777)), h(frame.wrapping_add(99_991))] } else { [n0; 3] };
            let c = enc([p[0] / a, p[1] / a, p[2] / a]);
            let mut o = [c[0] + ns[0] * amt, c[1] + ns[1] * amt, c[2] + ns[2] * amt];
            if clip {
                o = o.map(|v| v.clamp(0.0, 1.0));
            }
            let o = dec(o);
            p[0] = o[0] * a;
            p[1] = o[1] * a;
            p[2] = o[2] * a;
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn magnify(img: &mut Image, c: Vec2, m: f64, r: f32, fe: f32, op: f32, square: bool, mode: u32) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (dx, dy) = (x as f64 + 0.5 - c.x, y as f64 + 0.5 - c.y);
            let d = if square { dx.abs().max(dy.abs()) } else { dx.hypot(dy) } as f32;
            let k = if fe > 0.0 { ((r - d) / fe).clamp(0.0, 1.0) } else { (r - d + 0.5).clamp(0.0, 1.0) } * op;
            if k <= 0.0 {
                continue;
            }
            let s = src.sample_bilinear((c.x + dx / m) as f32, (c.y + dy / m) as f32);
            let p = &mut row[x * 4..x * 4 + 4];
            let base = Image::unpremul([p[0], p[1], p[2], p[3]]);
            let sc = Image::unpremul(s);
            let mixed = crate::vfx::blend_simple(mode, base, sc);
            let a = s[3];
            let out = [mixed[0] * a, mixed[1] * a, mixed[2] * a, a];
            for i in 0..4 {
                p[i] += (out[i] - p[i]) * k;
            }
        }
    });
}

fn find_edges(img: &mut Image, inv: bool, blend: f32) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let l = |dx: isize, dy: isize| {
                let p = src.get_clamped(x as isize + dx, y as isize + dy);
                luma709(p[0], p[1], p[2])
            };
            let gx = l(1, -1) + 2.0 * l(1, 0) + l(1, 1) - l(-1, -1) - 2.0 * l(-1, 0) - l(-1, 1);
            let gy = l(-1, 1) + 2.0 * l(0, 1) + l(1, 1) - l(-1, -1) - 2.0 * l(0, -1) - l(1, -1);
            let mut m = (gx * gx + gy * gy).sqrt().min(1.0);
            if !inv {
                m = 1.0 - m;
            }
            let o = src.get(x, y);
            let a = o[3];
            for k in 0..3 {
                row[x * 4 + k] = m * a * (1.0 - blend) + o[k] * blend;
            }
        }
    });
}

fn emboss(img: &mut Image, dx: f32, dy: f32, contrast: f32, blend: f32) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let a = src.sample_bilinear_clamped(x as f32 + 0.5 + dx, y as f32 + 0.5 + dy);
            let bq = src.sample_bilinear_clamped(x as f32 + 0.5 - dx, y as f32 + 0.5 - dy);
            let v = srgb_to_linear((0.5 + (luma709(a[0], a[1], a[2]) - luma709(bq[0], bq[1], bq[2])) * contrast * 2.0).clamp(0.0, 1.0));
            let o = src.get(x, y);
            for k in 0..3 {
                row[x * 4 + k] = v * o[3] * (1.0 - blend) + o[k] * blend;
            }
        }
    });
}

fn bevel_alpha(img: &mut Image, th: f32, lx: f32, ly: f32, inten: f32) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let a = |dx: f32, dy: f32| src.sample_bilinear_clamped(x as f32 + 0.5 + dx, y as f32 + 0.5 + dy)[3];
            let gx = a(th, 0.0) - a(-th, 0.0);
            let gy = a(0.0, th) - a(0.0, -th);
            let shade = -(gx * lx + gy * ly) * inten;
            let p = &mut row[x * 4..x * 4 + 4];
            // an earlier effect can leave alpha negative or NaN; clamp's bounds must not cross
            let al = p[3].max(0.0);
            for k in 0..3 {
                p[k] = (p[k] + shade * al).clamp(0.0, al);
            }
        }
    });
}

fn replicate(img: &mut Image, n: usize) {
    let src = img.clone();
    let (w, h) = (img.w, img.h);
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let u = (x * n) % w;
            let v = (y * n) % h;
            let s = src.sample_bilinear_clamped((u as f32 + 0.5) + (n as f32 - 1.0) * 0.5 / n as f32, v as f32 + 0.5);
            row[x * 4..x * 4 + 4].copy_from_slice(&s);
        }
    });
}

fn drop_shadow(img: &mut Image, d: &DropShadow) {
    let lc = d.color;
    let op = d.opacity;
    let mut sh = img.transformed(img.w, img.h, &Affine::translate(d.dx, d.dy));
    sh.px.par_chunks_mut(4).for_each(|p| {
        let a = p[3] * op;
        p[0] = lc[0] * a;
        p[1] = lc[1] * a;
        p[2] = lc[2] * a;
        p[3] = a;
    });
    crate::effects::box_blur(&mut sh, &d.boxes.0, &d.boxes.1, false);
    if !d.only {
        crate::blend::composite(&mut sh, img, 1.0, crate::blend::Blend::Normal);
    }
    *img = sh;
}

fn generate(img: &mut Image, g: &Generator) {
    use crate::effects::fill_over;
    match g {
        Generator::Ramp { sx, sy, vx, vy, sc, ec, radial, blend } => {
            let (sx, sy, vx, vy, radial) = (*sx, *sy, *vx, *vy, *radial);
            let len2 = (vx * vx + vy * vy).max(1e-9);
            fill_over(img, 1.0 - blend, |x, y| {
                let t = if radial {
                    ((x as f64 - sx).hypot(y as f64 - sy) / len2.sqrt()) as f32
                } else {
                    (((x as f64 - sx) * vx + (y as f64 - sy) * vy) / len2) as f32
                }
                .clamp(0.0, 1.0);
                dec([sc[0] + (ec[0] - sc[0]) * t, sc[1] + (ec[1] - sc[1]) * t, sc[2] + (ec[2] - sc[2]) * t])
            });
        }
        Generator::FourColor { colors: cs, opacity, exp } => {
            let (w, h) = (img.w as f32, img.h as f32);
            let pts = [(0.25 * w, 0.25 * h), (0.75 * w, 0.25 * h), (0.25 * w, 0.75 * h), (0.75 * w, 0.75 * h)];
            let blend_exp = *exp;
            fill_over(img, *opacity, |x, y| {
                let mut acc = [0.0f32; 3];
                let mut wsum = 0.0;
                for (i, p) in pts.iter().enumerate() {
                    let d = ((x - p.0).powi(2) + (y - p.1).powi(2)).sqrt().max(1.0);
                    let wgt = 1.0 / d.powf(blend_exp);
                    for k in 0..3 {
                        acc[k] += cs[i][k] * wgt;
                    }
                    wsum += wgt;
                }
                dec(acc.map(|v| v / wsum))
            });
        }
        Generator::Circle { cx, cy, r, color: lc, opacity: op } => {
            let (c, r, op) = (Vec2::new(*cx, *cy), *r, *op);
            let w = img.w;
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let d = ((x as f64 + 0.5 - c.x).hypot(y as f64 + 0.5 - c.y)) as f32;
                    let a = (r - d + 0.5).clamp(0.0, 1.0) * op;
                    if a > 0.0 {
                        let p = &mut row[x * 4..x * 4 + 4];
                        for k in 0..3 {
                            p[k] = lc[k] * a + p[k] * (1.0 - a);
                        }
                        p[3] = a + p[3] * (1.0 - a);
                    }
                }
            });
        }
        Generator::Grid { size, border, color: lc, opacity: op } => {
            let (size, border, op) = (*size, *border, *op);
            let w = img.w;
            let (ox, oy) = (img.w as f32 / 2.0, img.h as f32 / 2.0);
            img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w {
                    let gx = ((x as f32 - ox).rem_euclid(size)).min(size - (x as f32 - ox).rem_euclid(size));
                    let gy = ((y as f32 - oy).rem_euclid(size)).min(size - (y as f32 - oy).rem_euclid(size));
                    let a = ((border * 0.5 - gx.min(gy) + 0.5).clamp(0.0, 1.0)) * op;
                    if a > 0.0 {
                        let p = &mut row[x * 4..x * 4 + 4];
                        for k in 0..3 {
                            p[k] = lc[k] * a + p[k] * (1.0 - a);
                        }
                        p[3] = a + p[3] * (1.0 - a);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{ParamValue, find_effect};
    use filmcraft_time::Tick;

    fn cx(seconds: f64, px_scale: f32) -> FxCtx<'static> {
        FxCtx { t: Tick::ZERO, px_scale, seconds, timecode: "", clip_name: "", project: None, env: None, working: filmcraft_color::WorkingSpace::Rec709 }
    }

    fn effect(id: &str, params: &[(&str, ParamValue)]) -> EffectInstance {
        let mut e = find_effect(id).unwrap().instance();
        for (k, v) in params {
            e.params.get_mut(*k).unwrap_or_else(|| panic!("{id}.{k}")).value = v.clone();
        }
        e
    }

    /// Noise at a huge clip time: the frame number saturates at `u64::MAX` and the colour-noise
    /// seeds (`frame + 7777`, `frame + 99 991`) used to overflow (a panic in debug builds).
    #[test]
    fn noise_frame_overflow_does_not_panic() {
        let e = effect("noise", &[("amount", ParamValue::Float(50.0))]);
        for s in [1e30, f64::INFINITY, -5.0, f64::NAN] {
            let op = ExtOp::eval(&e, &cx(s, 1.0), 8, 6).unwrap();
            let mut img = Image::filled(8, 6, [0.3, 0.2, 0.1, 1.0]);
            op.apply(&mut img);
            assert!(img.px.iter().all(|v| v.is_finite()), "seconds {s}");
            assert!(op.gpu_ok());
        }
    }

    /// The GPU takes Noise only with whole-pixel grains (its grain cell is an integer division).
    #[test]
    fn noise_grain_on_the_gpu_is_whole_pixels() {
        let e = effect("noise", &[("amount", ParamValue::Float(50.0)), ("grain_size", ParamValue::Float(3.0))]);
        assert!(ExtOp::eval(&e, &cx(0.0, 1.0), 64, 36).unwrap().gpu_ok());
        // ×0.5 playback resolution: 1.5-pixel grains stay on the CPU
        assert!(!ExtOp::eval(&e, &cx(0.0, 0.5), 32, 18).unwrap().gpu_ok());
    }

    /// Hostile values (NaN / infinite / huge parameters) never panic on the CPU, and the GPU
    /// declines non-finite geometry.
    #[test]
    fn hostile_parameters_are_handled() {
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300];
        for id in EXT_EFFECTS {
            let def = find_effect(id).unwrap();
            for v in bad {
                for p in &def.params {
                    let value = match p.default {
                        ParamValue::Float(_) => ParamValue::Float(v),
                        ParamValue::Vec2(_) => ParamValue::Vec2(Vec2::new(v, v)),
                        _ => continue,
                    };
                    let e = effect(id, &[(p.id, value)]);
                    for (w, h) in [(9usize, 7usize), (1, 1)] {
                        let Some(op) = ExtOp::eval(&e, &cx(0.0, 1.0), w, h) else { panic!("{id}: no op") };
                        let _ = op.gpu_ok();
                        let mut img = Image::filled(w, h, [0.3, 0.2, 0.1, 1.0]);
                        op.apply(&mut img);
                        assert_eq!(img.px.len(), w * h * 4);
                    }
                }
            }
        }
        let e = effect("twirl", &[("center", ParamValue::Vec2(Vec2::new(f64::INFINITY, 0.0)))]);
        assert!(!ExtOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap().gpu_ok());
        let e = effect("drop_shadow", &[("distance", ParamValue::Float(f64::NAN))]);
        assert!(!ExtOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap().gpu_ok());
        let e = effect("replicate", &[("count", ParamValue::Float(f64::NAN))]);
        assert!(!ExtOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap().gpu_ok());
    }

    /// Every id here is in the plan's list of GPU effects.
    #[test]
    fn ext_effects_are_gpu_effects() {
        for id in EXT_EFFECTS {
            assert!(crate::gpufx::GPU_EFFECTS.contains(id), "{id}");
            assert!(find_effect(id).is_some(), "{id}");
        }
    }
}
