//! More effects with a GPU implementation (GPU4): Lens Flare, RGB Split, Lighting Effects, Color
//! Emboss, Rounded Crop, Gradient, Light Leaks, Turbulent Displace, Median, Channel Blur, Alpha
//! Glow, Edge Glow and Roughen Edges (Reduce Interlace Flicker is a plain `FxOp::Gaussian`).
//!
//! As for [`crate::gpufx_ext`], [`MoreOp::eval`] reads an effect's parameters into plain numbers
//! and [`MoreOp::apply`] is the CPU reference: the code these effects always ran on the CPU (in
//! `vfx` and `effects`), moved here with its arithmetic unchanged, so the CPU render is the same
//! as before. The Gaussian blurs inside them run on the box radii `effects::gaussian` would use
//! (evaluated once, here), which the GPU runs as the same box passes. `filmcraft_gpu` reproduces
//! `apply` in WGSL; [`MoreOp::gpu_ok`] keeps what the shader cannot reproduce (non-finite
//! numbers, positions or noise coordinates beyond what f32 / i32 resolve there) on the CPU.

use filmcraft_project::EffectInstance;
use filmcraft_project::effect::LIGHT_IDS;
use rayon::prelude::*;

use crate::effects::{FxCtx, gaussian_boxes};
use crate::image::Image;
use crate::vfx::{add_light, alpha_of, blend_simple, bv, chv, cv, dec, enc, fbm, fv, hash01, lerp3, lin, luma, offv, over, pv, smoothstep};

/// Box radii per axis of one Gaussian (`effects::gaussian_boxes`; empty: that axis untouched).
pub type Boxes = (Vec<u32>, Vec<u32>);

/// Lens Flare, evaluated (geometry in f64 as the CPU always had it).
#[derive(Clone, Debug, PartialEq)]
pub struct LensFlare {
    pub cx: f64,
    pub cy: f64,
    /// Brightness × (1 − Blend).
    pub k: f32,
    pub lens: u32,
    /// Linear flare colour.
    pub tint: [f32; 3],
    /// Normalising distance: the larger image side × Size.
    pub scale: f64,
    pub core_k: f64,
    /// Ghosts: centre (x, y), radius (in `scale` units) and colour.
    pub ghosts: Vec<(f64, f64, f64, [f32; 3])>,
}

/// One light of Lighting Effects.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// 1 directional, 2 omni, 3 spot.
    pub kind: u32,
    pub color: [f32; 3],
    pub cx: f64,
    pub cy: f64,
    pub major: f64,
    pub minor: f64,
    /// (sin, cos) of the angle.
    pub rot: (f64, f64),
    pub intensity: f32,
    pub focus: f32,
}

/// Lighting Effects, evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    pub lights: Vec<Light>,
    pub amb: [f32; 3],
    pub gloss: f32,
    pub material: f32,
    pub exposure: f32,
    pub bump: f32,
    pub white_high: bool,
}

/// One evaluated effect of this module.
#[derive(Clone, Debug, PartialEq)]
pub enum MoreOp {
    LensFlare(Box<LensFlare>),
    /// RGB Split: red and blue sampled at ∓ the offset (radial: the offset scales with the
    /// distance from (cx, cy) over `maxd`).
    RgbSplit {
        radial: bool,
        ox: f32,
        oy: f32,
        cx: f32,
        cy: f32,
        maxd: f32,
        amt: f32,
        blend: f32,
    },
    Lighting(Box<Lighting>),
    ColorEmboss {
        dx: f32,
        dy: f32,
        contrast: f32,
        blend: f32,
    },
    /// Rounded Crop: the rectangle l..r × t..b (working pixels) with corner radius, feather and a
    /// border band in linear colour `bc`.
    RoundedCrop {
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        rad: f32,
        fe: f32,
        border: f32,
        bc: [f32; 3],
    },
    /// Gradient (26.2): sRGB colours with alpha along start → start + (vx, vy).
    Gradient {
        sx: f32,
        sy: f32,
        vx: f32,
        vy: f32,
        sc: [f32; 4],
        ec: [f32; 4],
        shape: u32,
        scatter: f32,
        gamma: f32,
        blend: f32,
    },
    LightLeaks {
        c1: [f32; 3],
        c2: [f32; 3],
        inten: f32,
        scale: f32,
        dx: f32,
        dy: f32,
        t: f32,
        seed: u64,
        mode: u32,
    },
    /// Turbulent Displace: fractal-noise offsets (noise coordinates `x / size + o`, evolution
    /// offsets `e`) × `amount`, pinned at the edges.
    Turbulent {
        kind: u32,
        amount: f32,
        size: f32,
        ox: f32,
        oy: f32,
        cplx: f32,
        seed: u64,
        ex: f32,
        ey: f32,
        ez: f32,
        pin: u32,
    },
    /// Median of the (2r + 1)² edge-clamped neighbourhood per channel (r 1..=4).
    Median {
        r: u32,
    },
    /// Channel Blur: a Gaussian per channel (R, G, B, A).
    ChannelBlur {
        boxes: [Boxes; 4],
        repeat: bool,
    },
    /// Alpha Glow (Legacy): a glow behind the layer from its blurred alpha.
    AlphaGlow {
        boxes: Boxes,
        bright: f32,
        sc: [f32; 3],
        ec: [f32; 3],
        use_end: bool,
        fade: bool,
    },
    /// Edge Glow: edge mask (optionally widened), its halo, added as light.
    EdgeGlow {
        th: f32,
        width: Option<Boxes>,
        halo: Boxes,
        inten: f32,
        col: [f32; 3],
        only: bool,
    },
    /// Roughen Edges: fractal noise against the blurred alpha (`depth`).
    Roughen {
        kind: u32,
        depth: Boxes,
        sharp: f32,
        infl: f32,
        scale: f32,
        stretch: f32,
        offx: f32,
        offy: f32,
        cplx: f32,
        evo: f32,
        seed: u64,
        ec: [f32; 3],
    },
    /// Nothing to do (no amount, no radius…).
    Nop,
}

/// The widest working image [`MoreOp::gpu_ok`] allows for (beyond any GPU texture).
pub const MAX_DIM: f32 = 65536.0;

/// Effect ids [`MoreOp::eval`] understands.
pub const MORE_EFFECTS: &[&str] = &[
    "lens_flare",
    "rgb_split",
    "lighting_effects",
    "color_emboss",
    "rounded_crop",
    "gradient",
    "light_leaks",
    "turbulent_displace",
    "median",
    "channel_blur",
    "alpha_glow",
    "edge_glow",
    "roughen_edges",
];

impl MoreOp {
    /// Evaluate `e` at `cx.t` for a `w`×`h` working image (None: not one of [`MORE_EFFECTS`]).
    pub fn eval(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<MoreOp> {
        let img = Image { w, h, px: Vec::new() };
        Some(match e.effect.as_str() {
            "lens_flare" => {
                let c = pv(e, "center", cx, &img);
                let br = fv(e, "brightness", cx) / 100.0;
                let blend = fv(e, "blend", cx) / 100.0;
                let lens = chv(e, "lens_type");
                let size = (fv(e, "size", cx) / 100.0).max(0.05) as f64;
                let ghosts = fv(e, "ghosts", cx).clamp(0.0, 8.0) as usize;
                let (fw, fh) = (w as f64, h as f64);
                let (mx, my) = (fw / 2.0, fh / 2.0);
                let ghosts = (0..ghosts)
                    .map(|i| {
                        let t = 0.35 + i as f64 * 0.42 + hash01(i as i64, lens as i64, 0, 7) as f64 * 0.2;
                        let r = 0.02 + hash01(i as i64, lens as i64, 1, 7) as f64 * 0.07;
                        let hue = [[0.4, 0.6, 1.0], [1.0, 0.5, 0.3], [0.4, 1.0, 0.5], [0.9, 0.4, 1.0]][i % 4];
                        (c.x + (mx - c.x) * 2.0 * t, c.y + (my - c.y) * 2.0 * t, r, hue)
                    })
                    .collect();
                MoreOp::LensFlare(Box::new(LensFlare {
                    cx: c.x,
                    cy: c.y,
                    k: br * (1.0 - blend),
                    lens,
                    tint: lin(cv(e, "color", cx)),
                    scale: fw.max(fh) * size,
                    core_k: [18.0, 26.0, 14.0, 22.0][lens.min(3) as usize],
                    ghosts,
                }))
            }
            "rgb_split" => {
                let amt = fv(e, "amount", cx) * cx.px_scale;
                if amt.abs() < 1e-3 {
                    return Some(MoreOp::Nop);
                }
                let ang = fv(e, "angle", cx).to_radians();
                let c = pv(e, "center", cx, &img);
                MoreOp::RgbSplit {
                    radial: chv(e, "mode") == 1,
                    ox: ang.cos() * amt,
                    oy: ang.sin() * amt,
                    cx: c.x as f32,
                    cy: c.y as f32,
                    maxd: ((w as f64).hypot(h as f64) / 2.0) as f32,
                    amt,
                    blend: fv(e, "blend", cx) / 100.0,
                }
            }
            "lighting_effects" => {
                let diag = (w as f64).hypot(h as f64);
                let lights = LIGHT_IDS
                    .iter()
                    .map(|ids| {
                        let c = pv(e, ids[2], cx, &img);
                        Light {
                            kind: chv(e, ids[0]),
                            color: lin(cv(e, ids[1], cx)),
                            cx: c.x,
                            cy: c.y,
                            major: (fv(e, ids[3], cx) as f64 / 100.0 * diag).max(1.0),
                            minor: (fv(e, ids[4], cx) as f64 / 100.0 * diag).max(1.0),
                            rot: (fv(e, ids[5], cx) as f64).to_radians().sin_cos(),
                            intensity: fv(e, ids[6], cx) / 20.0,
                            focus: fv(e, ids[7], cx) / 100.0,
                        }
                    })
                    .filter(|l| l.kind != 0)
                    .collect();
                MoreOp::Lighting(Box::new(Lighting {
                    lights,
                    amb: lin(cv(e, "ambient_color", cx)).map(|v| v * fv(e, "ambient", cx) / 100.0),
                    gloss: fv(e, "gloss", cx) / 100.0,
                    material: (fv(e, "material", cx) / 100.0 + 1.0) / 2.0,
                    exposure: 2f32.powf(fv(e, "exposure", cx) / 100.0 * 2.0),
                    bump: fv(e, "bump_height", cx) / 100.0 * 4.0,
                    white_high: bv(e, "white_high"),
                }))
            }
            "color_emboss" => {
                let dir = (fv(e, "direction", cx) as f64).to_radians();
                let relief = fv(e, "relief", cx) * cx.px_scale.max(0.25);
                MoreOp::ColorEmboss {
                    dx: (dir.cos() as f32) * relief,
                    dy: (-dir.sin() as f32) * relief,
                    contrast: fv(e, "contrast", cx) / 100.0,
                    blend: fv(e, "blend", cx) / 100.0,
                }
            }
            "rounded_crop" => {
                let (fw, fh) = (w as f32, h as f32);
                let (l, t, r, b) = (
                    fv(e, "left", cx) / 100.0 * fw,
                    fv(e, "top", cx) / 100.0 * fh,
                    fw - fv(e, "right", cx) / 100.0 * fw,
                    fh - fv(e, "bottom", cx) / 100.0 * fh,
                );
                MoreOp::RoundedCrop {
                    l,
                    t,
                    r,
                    b,
                    rad: (fv(e, "radius", cx) * cx.px_scale).min(((r - l).min(b - t) / 2.0).max(0.0)),
                    fe: fv(e, "feather", cx) * cx.px_scale,
                    border: fv(e, "border", cx) * cx.px_scale,
                    bc: lin(cv(e, "border_color", cx)),
                }
            }
            "gradient" => {
                let s = pv(e, "start", cx, &img);
                let en = pv(e, "end", cx, &img);
                let mid = (fv(e, "midpoint", cx) / 100.0).clamp(0.01, 0.99);
                MoreOp::Gradient {
                    sx: s.x as f32,
                    sy: s.y as f32,
                    vx: (en.x - s.x) as f32,
                    vy: (en.y - s.y) as f32,
                    sc: cv(e, "start_color", cx),
                    ec: cv(e, "end_color", cx),
                    shape: chv(e, "shape"),
                    scatter: fv(e, "scatter", cx) / 100.0,
                    gamma: (0.5f32).ln() / mid.ln(),
                    blend: fv(e, "blend", cx) / 100.0,
                }
            }
            "light_leaks" => {
                let dir = fv(e, "direction", cx).to_radians();
                MoreOp::LightLeaks {
                    c1: lin(cv(e, "c1", cx)),
                    c2: lin(cv(e, "c2", cx)),
                    inten: fv(e, "intensity", cx) / 100.0,
                    scale: (fv(e, "scale", cx) / 100.0).max(0.05) * w.max(h) as f32 * 0.6,
                    dx: dir.cos(),
                    dy: dir.sin(),
                    t: (cx.seconds * fv(e, "speed", cx) as f64) as f32,
                    seed: fv(e, "seed", cx).max(0.0) as u64,
                    mode: chv(e, "mode"),
                }
            }
            "turbulent_displace" => {
                let kind = chv(e, "displacement");
                let amount = fv(e, "amount", cx) * cx.px_scale;
                if amount.abs() < 1e-4 {
                    return Some(MoreOp::Nop);
                }
                let size = (fv(e, "size", cx) * cx.px_scale).max(0.5);
                let off = pv(e, "offset", cx, &img);
                let (ox, oy) = (((off.x - w as f64 / 2.0) / size as f64) as f32, ((off.y - h as f64 / 2.0) / size as f64) as f32);
                let mut cplx = fv(e, "complexity", cx).clamp(1.0, 10.0);
                if (3..=5).contains(&kind) {
                    cplx = (cplx * 0.5).max(1.0);
                }
                let evo = fv(e, "evolution", cx) / 360.0;
                let (ex, ey, ez) = if bv(e, "cycle") {
                    let revs = fv(e, "cycle_revs", cx).max(1.0);
                    let th = (evo.rem_euclid(revs) / revs) * std::f32::consts::TAU;
                    (th.cos() * 0.8, th.sin() * 0.8, 0.0)
                } else {
                    (0.0, 0.0, evo)
                };
                MoreOp::Turbulent { kind, amount, size, ox, oy, cplx, seed: fv(e, "seed", cx).max(0.0) as u64, ex, ey, ez, pin: chv(e, "pinning") }
            }
            "median" => {
                let r = (fv(e, "radius", cx) * cx.px_scale).round().clamp(0.0, 4.0) as u32;
                if r == 0 {
                    return Some(MoreOp::Nop);
                }
                MoreOp::Median { r }
            }
            "channel_blur" => {
                let dims = chv(e, "dimensions");
                let boxes = ["red", "green", "blue", "alpha"].map(|id| {
                    let s = fv(e, id, cx) * cx.px_scale * 0.5;
                    gaussian_boxes(w, h, if dims == 2 { 0.0 } else { s }, if dims == 1 { 0.0 } else { s })
                });
                MoreOp::ChannelBlur { boxes, repeat: bv(e, "repeat_edge") }
            }
            "alpha_glow" => {
                let r = fv(e, "glow", cx) * cx.px_scale * 0.5;
                if r <= 0.3 {
                    return Some(MoreOp::Nop);
                }
                MoreOp::AlphaGlow {
                    boxes: gaussian_boxes(w, h, r, r),
                    bright: fv(e, "brightness", cx) / 255.0,
                    sc: lin(cv(e, "start_color", cx)),
                    ec: lin(cv(e, "end_color", cx)),
                    use_end: bv(e, "use_end"),
                    fade: bv(e, "fade_out"),
                }
            }
            "edge_glow" => {
                let width = fv(e, "width", cx) * cx.px_scale;
                let r = fv(e, "radius", cx) * cx.px_scale * 0.5;
                MoreOp::EdgeGlow {
                    th: fv(e, "threshold", cx) / 100.0,
                    width: (width > 1.0).then(|| gaussian_boxes(w, h, width * 0.5, width * 0.5)),
                    halo: gaussian_boxes(w, h, r, r),
                    inten: fv(e, "intensity", cx) / 100.0,
                    col: lin(cv(e, "color", cx)),
                    only: bv(e, "only"),
                }
            }
            "roughen_edges" => {
                let border = fv(e, "border", cx) * cx.px_scale;
                if border < 0.5 {
                    return Some(MoreOp::Nop);
                }
                let off = offv(e, "offset", cx);
                MoreOp::Roughen {
                    kind: chv(e, "edge_type"),
                    depth: gaussian_boxes(w, h, border * 0.5, border * 0.5),
                    sharp: fv(e, "sharpness", cx).max(0.0),
                    infl: fv(e, "influence", cx).clamp(0.0, 1.0),
                    scale: (fv(e, "scale", cx) / 100.0 * 30.0 * cx.px_scale).max(0.5),
                    stretch: 2f32.powf(fv(e, "stretch", cx)),
                    offx: off.x as f32,
                    offy: off.y as f32,
                    cplx: fv(e, "complexity", cx).clamp(1.0, 10.0),
                    evo: fv(e, "evolution", cx) / 360.0,
                    seed: fv(e, "seed", cx).max(0.0) as u64,
                    ec: lin(cv(e, "edge_color", cx)),
                }
            }
            _ => return None,
        })
    }

    /// Whether the GPU shader reproduces [`apply`](Self::apply): every number finite, positions
    /// within what f32 resolves on the GPU (|v| < 1e7), and noise coordinates (on any working
    /// image up to [`MAX_DIM`] pixels) small enough that their lattice cells fit the shader's
    /// 32-bit integers.
    pub fn gpu_ok(&self) -> bool {
        let fin = |v: &[f32]| v.iter().all(|x| x.is_finite());
        let pos = |v: &[f64]| v.iter().all(|x| x.is_finite() && x.abs() < 1e7);
        let pos32 = |v: &[f32]| v.iter().all(|x| x.is_finite() && x.abs() < 1e7);
        // the largest noise coordinate (before the octaves' ×2⁹) the shader takes
        const NOISE_MAX: f32 = 1e6;
        let (fw, fh) = (MAX_DIM, MAX_DIM);
        match self {
            MoreOp::Nop | MoreOp::ChannelBlur { .. } => true,
            MoreOp::LensFlare(l) => {
                pos(&[l.cx, l.cy, l.scale, l.core_k])
                    && l.scale > 0.0
                    && fin(&[l.k])
                    && fin(&l.tint)
                    && l.ghosts.iter().all(|g| pos(&[g.0, g.1, g.2]) && fin(&g.3))
            }
            MoreOp::RgbSplit { ox, oy, cx, cy, maxd, amt, blend, .. } => pos32(&[*ox, *oy, *cx, *cy, *amt]) && fin(&[*blend]) && *maxd > 0.0,
            MoreOp::Lighting(l) => {
                l.lights.len() <= 5
                    && l.lights
                        .iter()
                        .all(|li| pos(&[li.cx, li.cy, li.major, li.minor, li.rot.0, li.rot.1]) && fin(&li.color) && fin(&[li.intensity, li.focus]))
                    && fin(&l.amb)
                    && fin(&[l.gloss, l.material, l.exposure, l.bump])
            }
            MoreOp::ColorEmboss { dx, dy, contrast, blend } => pos32(&[*dx, *dy]) && fin(&[*contrast, *blend]),
            MoreOp::RoundedCrop { l, t, r, b, rad, fe, border, bc } => pos32(&[*l, *t, *r, *b, *rad, *fe, *border]) && fin(bc),
            MoreOp::Gradient { sx, sy, vx, vy, sc, ec, scatter, gamma, blend, .. } => {
                pos32(&[*sx, *sy, *vx, *vy]) && fin(sc) && fin(ec) && fin(&[*scatter, *gamma, *blend])
            }
            MoreOp::LightLeaks { c1, c2, inten, scale, dx, dy, t, .. } => {
                fin(c1)
                    && fin(c2)
                    && fin(&[*inten, *dx, *dy])
                    && scale.is_finite()
                    && *scale > 0.0
                    && t.abs() < 1e5
                    && fw / scale < NOISE_MAX
                    && fh / scale < NOISE_MAX
            }
            MoreOp::Turbulent { amount, size, ox, oy, cplx, ex, ey, ez, .. } => {
                pos32(&[*amount])
                    && fin(&[*cplx, *ex, *ey])
                    && size.is_finite()
                    && *size >= 0.5
                    && ox.abs() < 1e5
                    && oy.abs() < 1e5
                    && ez.abs() < 1e5
                    && fw / size < NOISE_MAX
                    && fh / size < NOISE_MAX
            }
            MoreOp::Median { r } => (1..=4).contains(r),
            MoreOp::AlphaGlow { bright, sc, ec, .. } => bright.is_finite() && fin(sc) && fin(ec),
            MoreOp::EdgeGlow { th, inten, col, .. } => fin(&[*th, *inten]) && fin(col),
            MoreOp::Roughen { sharp, infl, scale, stretch, offx, offy, cplx, evo, ec, .. } => {
                let sx = scale * stretch;
                let sy = scale / stretch;
                fin(&[*sharp, *infl, *cplx])
                    && fin(ec)
                    && sx.is_finite()
                    && sx > 0.0
                    && sy.is_finite()
                    && sy > 0.0
                    && offx.abs() < 1e5
                    && offy.abs() < 1e5
                    && evo.abs() < 1e5
                    && (fw + offx.abs()) / sx < NOISE_MAX
                    && (fh + offy.abs()) / sy < NOISE_MAX
            }
        }
    }

    /// The CPU reference (the effects' CPU implementation).
    pub fn apply(&self, img: &mut Image) {
        if img.w == 0 || img.h == 0 {
            return;
        }
        match self {
            MoreOp::Nop => {}
            MoreOp::LensFlare(l) => lens_flare(img, l),
            MoreOp::RgbSplit { radial, ox, oy, cx, cy, maxd, amt, blend } => rgb_split(img, *radial, (*ox, *oy), (*cx, *cy), *maxd, *amt, *blend),
            MoreOp::Lighting(l) => lighting(img, l),
            MoreOp::ColorEmboss { dx, dy, contrast, blend } => color_emboss(img, *dx, *dy, *contrast, *blend),
            MoreOp::RoundedCrop { l, t, r, b, rad, fe, border, bc } => rounded_crop(img, [*l, *t, *r, *b], *rad, *fe, *border, *bc),
            MoreOp::Gradient { .. } => gradient(img, self),
            MoreOp::LightLeaks { .. } => light_leaks(img, self),
            MoreOp::Turbulent { .. } => turbulent_displace(img, self),
            MoreOp::Median { r } => crate::effects::median(img, *r as isize),
            MoreOp::ChannelBlur { boxes, repeat } => {
                for (ch, b) in boxes.iter().enumerate() {
                    blur_channel(img, ch, b, *repeat);
                }
            }
            MoreOp::AlphaGlow { boxes, bright, sc, ec, use_end, fade } => alpha_glow(img, boxes, *bright, *sc, *ec, *use_end, *fade),
            MoreOp::EdgeGlow { th, width, halo, inten, col, only } => edge_glow(img, *th, width.as_ref(), halo, *inten, *col, *only),
            MoreOp::Roughen { .. } => roughen_edges(img, self),
        }
    }
}

/// `vfx::blur_plane` on box radii: a single-channel buffer blurred (edges not repeated).
fn blur_plane_boxes(plane: &[f32], w: usize, h: usize, b: &Boxes) -> Vec<f32> {
    if b.0.is_empty() && b.1.is_empty() {
        return plane.to_vec();
    }
    let mut im = Image { w, h, px: plane.iter().flat_map(|&v| [v, 0.0, 0.0, 0.0]).collect() };
    crate::effects::box_blur(&mut im, &b.0, &b.1, false);
    im.px.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
}

/// Blur one channel of an image (0..3) on these box radii.
fn blur_channel(img: &mut Image, ch: usize, b: &Boxes, repeat: bool) {
    if b.0.is_empty() && b.1.is_empty() {
        return;
    }
    let mut plane = Image { w: img.w, h: img.h, px: img.px.as_chunks::<4>().0.iter().flat_map(|p| [p[ch], 0.0, 0.0, 0.0]).collect() };
    crate::effects::box_blur(&mut plane, &b.0, &b.1, repeat);
    img.px.par_chunks_mut(4).zip(plane.px.par_chunks(4)).for_each(|(p, q)| p[ch] = q[0]);
}

/// Lens Flare (26.5 rebuild): core glow, halo, ghosts along the line through the frame centre and
/// a lens-type-dependent streak; tinted by Flare Color, scaled by Size.
fn lens_flare(img: &mut Image, l: &LensFlare) {
    let (c, lens, scale, core_k, tintc, k) = ((l.cx, l.cy), l.lens, l.scale, l.core_k, l.tint, l.k);
    let wi = img.w;
    img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        for x in 0..wi {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let d = (px - c.0).hypot(py - c.1) / scale;
            let core = 0.35 * (-d * core_k).exp() + 0.08 * (-d * 3.0).exp();
            let mut add = [core as f32; 3];
            if lens == 1 || lens == 2 {
                let ring = (1.0 - ((d - 0.12) / 0.012).abs()).max(0.0) * 0.08;
                for (k, v) in add.iter_mut().enumerate() {
                    *v += (ring * [0.6, 0.8, 1.0][k]) as f32;
                }
            }
            if lens == 3 {
                // anamorphic: long horizontal streak
                let s = (-((py - c.1).abs() / scale) * 160.0).exp() * (-((px - c.0).abs() / scale) * 2.0).exp() * 0.5;
                add[0] += s as f32 * 0.5;
                add[1] += s as f32 * 0.7;
                add[2] += s as f32;
            }
            for (gx, gy, rad, hue) in &l.ghosts {
                let dd = (px - gx).hypot(py - gy) / scale;
                let g = ((1.0 - ((dd - rad) / 0.01).abs()).max(0.0) * 0.12 + if dd < *rad { 0.05 } else { 0.0 }) as f32;
                for k in 0..3 {
                    add[k] += hue[k] * g;
                }
            }
            let p = &mut row[x * 4..x * 4 + 4];
            add_light(p, [add[0] * tintc[0] * k, add[1] * tintc[1] * k, add[2] * tintc[2] * k]);
        }
    });
}

/// RGB Split: red and blue sampled from opposite offsets (linear or radial from a centre).
fn rgb_split(img: &mut Image, radial: bool, off: (f32, f32), c: (f32, f32), maxd: f32, amt: f32, blend: f32) {
    let src = img.clone();
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (ox, oy) = if radial {
                let (dx, dy) = (px - c.0, py - c.1);
                (dx / maxd * amt, dy / maxd * amt)
            } else {
                off
            };
            let r = src.sample_bilinear(px - ox, py - oy);
            let g = src.get(x, y);
            let b = src.sample_bilinear(px + ox, py + oy);
            let o = [r[0], g[1], b[2], r[3].max(g[3]).max(b[3])];
            for k in 0..4 {
                row[x * 4 + k] = o[k] + (g[k] - o[k]) * blend;
            }
        }
    });
}

/// Lighting Effects: up to five directional / omni / spot lights, ambience, gloss and a bump map
/// from the layer's own luminance.
fn lighting(img: &mut Image, lt: &Lighting) {
    let (amb, gloss, material, exposure, bump, white_high) = (lt.amb, lt.gloss, lt.material, lt.exposure, lt.bump, lt.white_high);
    let src = img.clone();
    let lum = |x: isize, y: isize| {
        let p = src.get_clamped(x, y);
        let l = luma(Image::unpremul(p));
        if white_high { l } else { 1.0 - l }
    };
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let p = &mut row[x * 4..x * 4 + 4];
            if p[3] <= 1e-6 {
                continue;
            }
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let (gx, gy) = if bump != 0.0 {
                (
                    (lum(x as isize + 1, y as isize) - lum(x as isize - 1, y as isize)) * bump,
                    (lum(x as isize, y as isize + 1) - lum(x as isize, y as isize - 1)) * bump,
                )
            } else {
                (0.0, 0.0)
            };
            let mut light = amb;
            let mut spec = [0.0f32; 3];
            for l in &lt.lights {
                let (dx, dy) = (px - l.cx, py - l.cy);
                let (s, c) = l.rot;
                let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
                let fall = match l.kind {
                    1 => 1.0,
                    2 => {
                        let d = ((dx * dx + dy * dy).sqrt() / l.major) as f32;
                        (1.0 - d).max(0.0).powi(2)
                    }
                    _ => {
                        let d = ((u / l.major).powi(2) + (v / l.minor).powi(2)).sqrt() as f32;
                        1.0 - smoothstep(l.focus * 0.95, 1.0, d)
                    }
                };
                // bump shading: light direction in the image plane (towards the light), tilted 45°
                let (lx, ly) = if l.kind == 1 { (l.rot.1 as f32, l.rot.0 as f32) } else { (-dx as f32, -dy as f32) };
                let ln = (lx * lx + ly * ly).sqrt().max(1e-6);
                let shade = (1.0 - (gx * lx + gy * ly) / ln).max(0.0);
                for k in 0..3 {
                    light[k] += l.color[k] * l.intensity * fall * shade;
                    spec[k] += l.color[k] * gloss.max(0.0) * fall.powi(8) * l.intensity.max(0.0);
                }
            }
            let c = Image::unpremul([p[0], p[1], p[2], p[3]]);
            for k in 0..3 {
                let s = spec[k] * (1.0 - material) + spec[k] * c[k] * material;
                let v = ((c[k] * light[k] + s) * exposure).max(0.0);
                p[k] = v * p[3];
            }
        }
    });
}

/// Color Emboss: emboss relief that keeps the picture's colours.
fn color_emboss(img: &mut Image, dx: f32, dy: f32, contrast: f32, blend: f32) {
    let src = img.clone();
    img.map_rgb(|c, x, y| {
        let a = src.sample_bilinear_clamped(x as f32 + 0.5 + dx, y as f32 + 0.5 + dy);
        let b = src.sample_bilinear_clamped(x as f32 + 0.5 - dx, y as f32 + 0.5 - dy);
        let d = luma(Image::unpremul(a)) - luma(Image::unpremul(b));
        let v = enc(c);
        let o = v.map(|q| (q + d * contrast * 2.0).clamp(0.0, 1.0));
        dec(lerp3(o, v, blend))
    });
}

/// Rounded Crop: crop edges with rounded corners, feather and an optional border.
fn rounded_crop(img: &mut Image, [l, t, r, b]: [f32; 4], rad: f32, fe: f32, border: f32, bc: [f32; 3]) {
    use crate::vfx::distort::round_rect_sdf;
    let wi = img.w;
    img.px.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        for x in 0..wi {
            let d = round_rect_sdf(x as f32 + 0.5, y as f32 + 0.5, l, t, r, b, rad);
            let p = &mut row[x * 4..x * 4 + 4];
            if border > 0.0 {
                // coverage of the band −border < d < 0 just inside the edge
                let k = (0.5 - d).clamp(0.0, 1.0) - (0.5 - d - border).clamp(0.0, 1.0);
                if k > 0.0 {
                    over(p, [bc[0] * k, bc[1] * k, bc[2] * k, k]);
                }
            }
            let a = if fe > 0.0 { (-d / fe).clamp(0.0, 1.0) } else { (0.5 - d).clamp(0.0, 1.0) };
            if a < 1.0 {
                for v in p.iter_mut() {
                    *v *= a;
                }
            }
        }
    });
}

/// Gradient (26.2): linear / radial / reflected / diamond ramp with midpoint and scatter.
fn gradient(img: &mut Image, op: &MoreOp) {
    let MoreOp::Gradient { sx, sy, vx, vy, sc, ec, shape, scatter, gamma, blend } = *op else { return };
    let l2 = (vx * vx + vy * vy).max(1e-6);
    let l = l2.sqrt();
    crate::vfx::generate(img, |x, y, p| {
        let (dx, dy) = (x - sx, y - sy);
        let along = (dx * vx + dy * vy) / l2;
        let across = (-dx * vy + dy * vx) / l2;
        let mut t = match shape {
            1 => (dx * dx + dy * dy).sqrt() / l,
            2 => along.abs(),
            3 => along.abs() + across.abs(),
            _ => along,
        };
        if scatter > 0.0 {
            t += (hash01(x as i64, y as i64, 0, 13) - 0.5) * scatter * 0.1;
        }
        let t = t.clamp(0.0, 1.0).powf(gamma);
        let c = dec(lerp3([sc[0], sc[1], sc[2]], [ec[0], ec[1], ec[2]], t));
        let a = sc[3] + (ec[3] - sc[3]) * t;
        let g = [c[0] * a, c[1] * a, c[2] * a, a];
        [0, 1, 2, 3].map(|k| g[k] + (p[k] - g[k]) * blend)
    });
}

/// Light Leaks: drifting warm coloured light washes (procedural, seeded, animated by Speed).
fn light_leaks(img: &mut Image, op: &MoreOp) {
    let MoreOp::LightLeaks { c1, c2, inten, scale, dx, dy, t, seed, mode } = *op else { return };
    let (w, h) = (img.w as f32, img.h as f32);
    crate::vfx::generate(img, |x, y, p| {
        let (u, v) = (x / scale, y / scale);
        // stronger towards the edge the leak enters from
        let along = ((x / w - 0.5) * dx + (y / h - 0.5) * dy) * 2.0;
        let bias = smoothstep(-0.2, 1.0, -along);
        let n = fbm(u + dx * t * 0.3, v + dy * t * 0.3, t * 0.15, 3.0, seed) * 0.5 + 0.5;
        let m = fbm(u * 0.7 + 5.2, v * 0.7, t * 0.1 + 9.0, 2.0, seed) * 0.5 + 0.5;
        let k = smoothstep(0.35, 0.85, n) * bias * inten;
        let light = lerp3(c1, c2, m).map(|c| c * k);
        let base = Image::unpremul(p);
        let o = match mode {
            1 => [base[0] + light[0], base[1] + light[1], base[2] + light[2]],
            2 => blend_simple(4, base, lerp3(base, light.map(|q| q + 0.5), k.min(1.0))),
            _ => blend_simple(2, base, light),
        };
        let a = p[3] + (1.0 - p[3]) * k.min(1.0);
        [o[0] * a, o[1] * a, o[2] * a, a]
    });
}

/// Turbulent Displace: fractal-noise displacement (turbulent, bulge, twist, directional), with
/// evolution, cycling and edge pinning.
fn turbulent_displace(img: &mut Image, op: &MoreOp) {
    let MoreOp::Turbulent { kind, amount, size, ox, oy, cplx, seed, ex, ey, ez, pin } = *op else { return };
    let (w, h) = (img.w as f32, img.h as f32);
    let n = |x: f32, y: f32, ch: f32| fbm(x + ex + ch * 31.7, y + ey, ez + ch * 3.1, cplx, seed);
    crate::effects::warp(img, |x, y| {
        let (u, v) = (x as f32 / size + ox, y as f32 / size + oy);
        let (dx, dy) = match kind {
            0 | 3 => (n(u, v, 0.0), n(u, v, 1.0)),
            1 | 4 | 2 | 5 => {
                let eps = 0.05;
                let gx = (n(u + eps, v, 0.0) - n(u - eps, v, 0.0)) / (2.0 * eps);
                let gy = (n(u, v + eps, 0.0) - n(u, v - eps, 0.0)) / (2.0 * eps);
                if kind == 1 || kind == 4 { (gx * 0.3, gy * 0.3) } else { (-gy * 0.3, gx * 0.3) }
            }
            6 => (0.0, n(u, v, 0.0)),
            7 => (n(u, v, 0.0), 0.0),
            _ => {
                let q = n(u, v, 0.0);
                (q, q)
            }
        };
        let (px, py) = (x as f32, y as f32);
        let edge = |d: f32, len: f32| smoothstep(1.0, (len * 0.1).max(2.0), d);
        let (kx, ky) = match pin {
            1 => {
                let k = edge(px.min(w - px), w).min(edge(py.min(h - py), h));
                (k, k)
            }
            2 => {
                let k = edge(px.min(w - px), w);
                (k, k)
            }
            3 => {
                let k = edge(py.min(h - py), h);
                (k, k)
            }
            _ => (1.0, 1.0),
        };
        (x - (dx * amount * kx) as f64, y - (dy * amount * ky) as f64)
    });
}

/// Alpha Glow (Legacy): glow outward from the alpha edges, Start Color fading to End Color.
fn alpha_glow(img: &mut Image, boxes: &Boxes, bright: f32, sc: [f32; 3], ec: [f32; 3], use_end: bool, fade: bool) {
    let a = alpha_of(img);
    let g = blur_plane_boxes(&a, img.w, img.h, boxes);
    img.px.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let gv = (g[i] * 2.0).min(1.0);
        let k = if fade {
            gv * bright
        } else if gv > 0.02 {
            bright
        } else {
            0.0
        };
        if k <= 0.0 || p[3] >= 1.0 {
            return;
        }
        let col = if use_end { lerp3(ec, sc, gv) } else { sc };
        // glow behind the layer
        let under = [col[0] * k, col[1] * k, col[2] * k, k];
        let front = [p[0], p[1], p[2], p[3]];
        let mut o = under;
        over(&mut o, front);
        p.copy_from_slice(&o);
    });
}

/// Sobel edge strength of display luma.
fn edges(img: &Image) -> Vec<f32> {
    let w = img.w;
    let mut out = vec![0.0f32; img.w * img.h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let l = |dx: isize, dy: isize| {
                let p = img.get_clamped(x as isize + dx, y as isize + dy);
                filmcraft_color::linear_to_srgb(luma([p[0], p[1], p[2]]).clamp(0.0, 1.0)) * p[3].min(1.0) + 0.0
            };
            let gx = l(1, -1) + 2.0 * l(1, 0) + l(1, 1) - l(-1, -1) - 2.0 * l(-1, 0) - l(-1, 1);
            let gy = l(-1, 1) + 2.0 * l(0, 1) + l(1, 1) - l(-1, -1) - 2.0 * l(0, -1) - l(1, -1);
            *o = (gx * gx + gy * gy).sqrt() / 4.0;
        }
    });
    out
}

/// Edge Glow: coloured glow along the picture's edges.
fn edge_glow(img: &mut Image, th: f32, width: Option<&Boxes>, halo: &Boxes, inten: f32, col: [f32; 3], only: bool) {
    let mut m: Vec<f32> = edges(img).into_iter().map(|v| smoothstep(th, th + 0.1, v)).collect();
    if let Some(wb) = width {
        m = blur_plane_boxes(&m, img.w, img.h, wb).into_iter().map(|v| (v * 3.0).min(1.0)).collect();
    }
    let core = m.clone();
    let halo = blur_plane_boxes(&m, img.w, img.h, halo);
    if only {
        *img = Image::new(img.w, img.h);
    }
    // (`add_image` with unit gain: the light's own alpha is not used)
    img.px.par_chunks_mut(4).enumerate().for_each(|(i, p)| {
        let v = (core[i] * 0.6 + halo[i] * 1.6) * inten;
        add_light(p, [col[0] * v, col[1] * v, col[2] * v]);
    });
}

/// Roughen Edges: fractal-eroded alpha edges (Roughen, Cut, Spiky, Rusty, Photocopy…).
fn roughen_edges(img: &mut Image, op: &MoreOp) {
    let MoreOp::Roughen { kind, ref depth, sharp, infl, scale, stretch, offx, offy, cplx, evo, seed, ec } = *op else { return };
    let colored = matches!(kind, 1 | 5 | 7);
    let a = alpha_of(img);
    let depth = blur_plane_boxes(&a, img.w, img.h, depth);
    let w = img.w;
    img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let i = y * w + x;
            let p = &mut row[x * 4..x * 4 + 4];
            if p[3] <= 1e-6 {
                continue;
            }
            let (u, v) = ((x as f32 + offx) / (scale * stretch), (y as f32 + offy) * stretch / scale);
            let mut n = fbm(u, v, evo, cplx, seed);
            if kind == 3 {
                n = 1.0 - 2.0 * n.abs();
            }
            let d = depth[i];
            let mut k = ((d - 0.5 + n * infl * 0.5) * (1.0 + sharp * 4.0) + 0.5).clamp(0.0, 1.0);
            if kind == 2 {
                k = if k > 0.5 { 1.0 } else { 0.0 };
            }
            if matches!(kind, 4..=7) {
                // rust / photocopy: holes eaten into the border region (a saturated seed wraps)
                let hole = fbm(u * 2.3 + 11.0, v * 2.3, evo, cplx, seed.wrapping_add(1));
                if d < 0.97 && hole > 0.25 {
                    k *= 1.0 - smoothstep(0.25, 0.4, hole) * (1.0 - d);
                }
            }
            if colored && d < 0.97 {
                let t = (1.0 - d) * 1.5;
                let c = Image::unpremul([p[0], p[1], p[2], p[3]]);
                let col = lerp3(c, ec, t.min(1.0));
                for c in 0..3 {
                    p[c] = col[c] * p[3];
                }
            }
            if matches!(kind, 6 | 7) {
                let c = Image::unpremul([p[0], p[1], p[2], p[3]]);
                let l = if luma(enc(c)) > 0.5 { 1.0 } else { 0.0 };
                if kind == 6 {
                    for c in 0..3 {
                        p[c] = l * p[3];
                    }
                }
            }
            for v in p.iter_mut() {
                *v *= k;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_geom::Vec2;
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

    /// Roughen Edges' rust / photocopy holes use the seed + 1: a saturated seed (a huge Random
    /// Seed value) used to overflow there (a panic in debug builds).
    #[test]
    fn roughen_edges_saturated_seed_does_not_panic() {
        for kind in [4u32, 5, 6, 7] {
            let e = effect("roughen_edges", &[("seed", ParamValue::Float(1e30)), ("edge_type", ParamValue::Choice(kind)), ("border", ParamValue::Float(4.0))]);
            let op = MoreOp::eval(&e, &cx(0.0, 1.0), 12, 9).unwrap();
            let mut img = Image::filled(12, 9, [0.3, 0.2, 0.1, 1.0]);
            for x in 0..4 {
                img.px[x * 4 + 3] = 0.0;
            }
            op.apply(&mut img);
            assert!(img.px.iter().all(|v| v.is_finite()), "kind {kind}");
        }
    }

    /// Noise far out (a hostile Turbulent Displace offset or evolution): the lattice cell of a
    /// huge coordinate saturates at `i64::MAX`, and its neighbour cell used to overflow (a panic
    /// in debug builds).
    #[test]
    fn noise_far_out_does_not_overflow() {
        for (x, y, z) in [(f32::MAX, 0.5, 0.5), (-f32::MAX, f32::MAX, 1e30), (1e30, 1e30, -1e30), (f32::INFINITY, f32::NAN, 0.0)] {
            let _ = crate::vfx::noise3(x, y, z, u64::MAX);
            let _ = fbm(x, y, z, 10.0, u64::MAX);
        }
        let e = effect("turbulent_displace", &[("offset", ParamValue::Vec2(Vec2::new(1e300, -1e300)))]);
        let op = MoreOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap();
        assert!(!op.gpu_ok());
        let mut img = Image::filled(9, 7, [0.3, 0.2, 0.1, 1.0]);
        op.apply(&mut img);
    }

    /// Hostile values (NaN / infinite / huge parameters) never panic on the CPU, and the GPU
    /// declines non-finite numbers.
    #[test]
    fn hostile_parameters_are_handled() {
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300];
        for id in MORE_EFFECTS {
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
                        let Some(op) = MoreOp::eval(&e, &cx(0.0, 1.0), w, h) else { panic!("{id}: no op") };
                        let _ = op.gpu_ok();
                        let mut img = Image::filled(w, h, [0.3, 0.2, 0.1, 1.0]);
                        op.apply(&mut img);
                        assert_eq!(img.px.len(), w * h * 4);
                    }
                }
            }
        }
        let e = effect("lens_flare", &[("center", ParamValue::Vec2(Vec2::new(f64::INFINITY, 0.0)))]);
        assert!(!MoreOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap().gpu_ok());
        let e = effect("turbulent_displace", &[("evolution", ParamValue::Float(1e12))]);
        assert!(!MoreOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap().gpu_ok());
        let e = effect("light_leaks", &[("speed", ParamValue::Float(1e12))]);
        assert!(!MoreOp::eval(&e, &cx(1.0, 1.0), 9, 7).unwrap().gpu_ok());
        let e = effect("roughen_edges", &[("stretch", ParamValue::Float(200.0))]);
        assert!(!MoreOp::eval(&e, &cx(0.0, 1.0), 9, 7).unwrap().gpu_ok());
    }

    /// Every id here is in the plan's list of GPU effects.
    #[test]
    fn more_effects_are_gpu_effects() {
        for id in MORE_EFFECTS {
            assert!(crate::gpufx::GPU_EFFECTS.contains(id), "{id}");
            assert!(find_effect(id).is_some(), "{id}");
        }
        assert!(crate::gpufx::GPU_EFFECTS.contains(&"reduce_interlace_flicker"));
    }
}
