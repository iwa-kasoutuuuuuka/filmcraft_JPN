//! The GPU effect stage: standard effects of a plan layer ([`LayerFx`]) run in WGSL (`fx.wgsl`).
//!
//! A layer with effects is first drawn from its source (YUV planes or RGBA, through the
//! compositor's own `fs` shader, so decoding is the same as for a plain layer) into a working
//! texture of the size the CPU decodes it at; each [`FxOp`] then runs as compute passes that
//! ping-pong between two working textures (one more for Unsharp Mask, which keeps the original,
//! and one more for an effect with masks, which keeps the image before it); the last one is drawn
//! into the accumulator with the layer's matrix, opacity and blend mode like any RGBA layer.
//! Working textures are `Rgba32Float` (the CPU reference's precision) and pooled per size; layers
//! without effects never touch any of this.
//!
//! Masks ([`LayerFx::masks`], [`LayerFx::opacity_masks`]) are one more pass each: the coverage of
//! the flattened polygons is evaluated per pixel (the math of `mask.wgsl` and
//! `filmcraft_render::mask`) and mixes the effect's result with the image before it, or scales
//! the layer for Opacity masks.
//!
//! Box blurs (Gaussian Blur, Camera Blur, the blur inside Unsharp / Sharpen) run per pixel for
//! radii up to [`BOX_PER_PIXEL_MAX`] and as a running sum per row / column above, so a huge
//! radius costs O(1) per pixel like on the CPU (radii are capped by `gaussian_boxes`).

use std::collections::{HashMap, HashSet};

use filmcraft_render::gpufx::{FxOp, LumetriHdr, luma2020};
use filmcraft_render::gpufx_ext::{ExtOp, Generator, Warp};
use filmcraft_render::gpufx_more::MoreOp;
use filmcraft_render::mask::FlatMask;
use filmcraft_render::plan::LayerFx;

/// Working texture format.
pub(crate) const FX_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// Box radii up to this many pixels are summed per output pixel; larger ones as a running sum.
pub const BOX_PER_PIXEL_MAX: u32 = 32;

const OP_BRIGHTNESS_CONTRAST: u32 = 1;
const OP_PROC_AMP: u32 = 2;
const OP_TINT: u32 = 3;
const OP_BLACK_WHITE: u32 = 4;
const OP_COLOR_BALANCE: u32 = 5;
const OP_LEAVE_COLOR: u32 = 6;
const OP_CHANGE_TO_COLOR: u32 = 7;
const OP_COLOR_PASS: u32 = 8;
const OP_GAMMA: u32 = 9;
const OP_LEVELS: u32 = 10;
const OP_EXTRACT: u32 = 11;
const OP_INVERT: u32 = 12;
const OP_INVERT_ALPHA: u32 = 13;
const OP_POSTERIZE: u32 = 14;
const OP_ASC_CDL: u32 = 15;
const OP_CHANNEL_MIX: u32 = 16;
const OP_COLOR_REPLACE: u32 = 17;
const OP_ALPHA_ADJUST: u32 = 18;
const OP_BOX: u32 = 20;
const OP_DIRECTIONAL: u32 = 22;
const OP_UNSHARP: u32 = 23;
const OP_CROP: u32 = 24;
const OP_RESAMPLE: u32 = 25;
const OP_HFLIP: u32 = 26;
const OP_VFLIP: u32 = 27;
const OP_MIRROR: u32 = 28;
const OP_OFFSET: u32 = 29;
const OP_LUMETRI: u32 = 30;
const OP_MOSAIC: u32 = 31;
const OP_VIGNETTE: u32 = 32;
const OP_MASK: u32 = 33;
const OP_HSL_KEY: u32 = 34;
const OP_HSL_MEDIAN: u32 = 35;
const OP_HSL_APPLY: u32 = 36;
const OP_KEY: u32 = 37;
const OP_LUMA_KEY: u32 = 38;
const OP_VIDEO_LIMITER: u32 = 39;
const OP_UKEY_MATTE: u32 = 40;
const OP_UKEY_CHOKE: u32 = 41;
const OP_UKEY_APPLY: u32 = 42;
const OP_NOISE: u32 = 43;
const OP_WARP: u32 = 44;
const OP_MAGNIFY: u32 = 45;
const OP_FIND_EDGES: u32 = 46;
const OP_EMBOSS: u32 = 47;
const OP_BEVEL_ALPHA: u32 = 48;
const OP_REPLICATE: u32 = 49;
const OP_STROBE: u32 = 50;
const OP_SHADOW: u32 = 51;
const OP_SHADOW_OVER: u32 = 52;
const OP_GENERATE: u32 = 53;
const OP_LENS_FLARE: u32 = 54;
const OP_RGB_SPLIT: u32 = 55;
const OP_LIGHTING: u32 = 56;
const OP_COLOR_EMBOSS: u32 = 57;
const OP_ROUNDED_CROP: u32 = 58;
const OP_GRADIENT: u32 = 59;
const OP_LIGHT_LEAKS: u32 = 60;
const OP_TURBULENT: u32 = 61;
const OP_MEDIAN: u32 = 62;
const OP_ALPHA_GLOW: u32 = 63;
const OP_EDGE_GLOW_EDGES: u32 = 64;
const OP_EDGE_GLOW_X3: u32 = 65;
const OP_EDGE_GLOW_ADD: u32 = 66;
const OP_ROUGHEN: u32 = 67;

/// Box-pass channel masks (`i1.w`; 0 = every channel).
const CH_R: u32 = 1;
const CH_G: u32 = 2;
const CH_A: u32 = 8;

type Target = (wgpu::Texture, wgpu::TextureView);

pub(crate) struct FxStage {
    px: wgpu::ComputePipeline,
    run: wgpu::ComputePipeline,
    /// GPU4 passes (`fx_more`) and Median (`fx_median`): entry points of their own.
    more: wgpu::ComputePipeline,
    median: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    /// Draws a layer's source into a working texture (`composite.wgsl` `fs`, no blending).
    source: wgpu::RenderPipeline,
    /// Bound as the mask buffers of the passes without masks.
    no_masks: wgpu::Buffer,
    pool: HashMap<(u32, u32), Vec<Target>>,
    used: HashSet<(u32, u32)>,
    uniform_pool: Vec<wgpu::Buffer>,
    used_uniforms: Vec<wgpu::Buffer>,
}

/// The entry point a pass runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pipe {
    Px,
    Run,
    More,
    Median,
}

/// One compute pass of a job.
struct Dispatch {
    pipe: Pipe,
    bg: wgpu::BindGroup,
    groups: (u32, u32),
}

/// The recorded work of one layer's effects (bind groups made; recorded into the frame's encoder
/// right before the layer is drawn, so layers can share pooled textures).
pub(crate) struct FxJob {
    source_bg: wgpu::BindGroup,
    source_target: wgpu::TextureView,
    dispatches: Vec<Dispatch>,
    /// The texture holding the result.
    pub(crate) result: wgpu::TextureView,
    pub(crate) result_texture: wgpu::Texture,
}

/// One pass before texture routing: entry point, op code, integer and float parameters.
struct Step {
    run: bool,
    code: u32,
    i1: [u32; 4],
    p: [f32; 16],
    /// Unsharp combine: reads the blurred image and the original.
    combine: bool,
    /// A blur pass of Unsharp Mask (the original stays untouched until the combine).
    unsharp_blur: bool,
    /// A pass between the blurs and the combine that also reads the original (as `aux`).
    aux_orig: bool,
    /// First pass of an effect with masks: the image it reads stays untouched until the mix.
    mask_begin: bool,
    /// The mask pass: these masks' coverage mixes the image with the kept one (or scales it).
    masks: Option<Vec<FlatMask>>,
    /// Tables the pass reads (Lumetri curves), bound where the mask points go.
    data: Option<Vec<f32>>,
}

fn step(code: u32, i1: [u32; 4], p: &[f32]) -> Step {
    let mut q = [0.0; 16];
    for (d, s) in q.iter_mut().zip(p) {
        *d = *s;
    }
    Step { run: false, code, i1, p: q, combine: false, unsharp_blur: false, aux_orig: false, mask_begin: false, masks: None, data: None }
}

/// The mask pass over `masks` (None without an active mask). `scale`: multiply the image by the
/// coverage (Opacity masks) instead of mixing it with the kept image.
fn mask_step(masks: &[FlatMask], scale: bool) -> Option<Step> {
    let active: Vec<FlatMask> = masks.iter().filter(|m| m.mode.index() != 0).cloned().collect();
    if active.is_empty() {
        return None;
    }
    let mut s = step(OP_MASK, [active.len() as u32, scale as u32, 0, 0], &[]);
    s.masks = Some(active);
    Some(s)
}

/// Every pass of a layer's stage, in order.
fn all_steps(fx: &LayerFx) -> Vec<Step> {
    let mut all = Vec::new();
    for (i, op) in fx.ops.iter().enumerate() {
        let mut own = steps(op);
        // (an op without passes changes nothing, masked or not)
        if let (Some(first), Some(mix)) = (own.first_mut(), fx.masks.get(i).and_then(|m| mask_step(m, false))) {
            first.mask_begin = true;
            own.push(mix);
        }
        all.append(&mut own);
    }
    all.extend(mask_step(&fx.opacity_masks, true));
    all
}

/// Box passes along x (vertical = false) or y for the radii (radius 0 is a no-op on the CPU), on
/// the channels of `mask` (bit k: channel k; 0: all; the others are copied).
fn boxes(radii: &[u32], vertical: bool, repeat: bool, mask: u32, out: &mut Vec<Step>) {
    for &r in radii.iter().filter(|r| **r > 0) {
        let mut s = step(OP_BOX, [r, repeat as u32, vertical as u32, mask], &[]);
        s.run = r > BOX_PER_PIXEL_MAX;
        out.push(s);
    }
}

/// The passes of an op (empty: no-op). Unsharp's blur passes come first, then the combine.
fn steps(op: &FxOp) -> Vec<Step> {
    let mut out = Vec::new();
    match op {
        FxOp::BrightnessContrast { br, co } => out.push(step(OP_BRIGHTNESS_CONTRAST, [0; 4], &[*br, *co])),
        FxOp::ProcAmp { br, co, hue, sat } => out.push(step(OP_PROC_AMP, [0; 4], &[*br, *co, *hue, *sat])),
        FxOp::Tint { black, white, amount } => {
            out.push(step(OP_TINT, [0; 4], &[black[0], black[1], black[2], *amount, white[0], white[1], white[2]]));
        }
        FxOp::BlackWhite => out.push(step(OP_BLACK_WHITE, [0; 4], &[])),
        FxOp::ColorBalance { sh, md, hi, preserve } => {
            out.push(step(OP_COLOR_BALANCE, [*preserve as u32, 0, 0, 0], &[sh[0], sh[1], sh[2], 0.0, md[0], md[1], md[2], 0.0, hi[0], hi[1], hi[2]]))
        }
        FxOp::LeaveColor { amount, key_hue, tol, soft } => out.push(step(OP_LEAVE_COLOR, [0; 4], &[*amount, *key_hue, *tol, *soft])),
        FxOp::ChangeToColor { from_hue, to_hue, tol, soft } => out.push(step(OP_CHANGE_TO_COLOR, [0; 4], &[*from_hue, *to_hue, *tol, *soft])),
        FxOp::ColorPass { key, sim, reverse } => out.push(step(OP_COLOR_PASS, [*reverse as u32, 0, 0, 0], &[key[0], key[1], key[2], *sim])),
        FxOp::Gamma { g } => out.push(step(OP_GAMMA, [0; 4], &[*g])),
        FxOp::Levels { ib, iw, ob, ow, g } => out.push(step(OP_LEVELS, [0; 4], &[*ib, *iw, *ob, *ow, *g])),
        FxOp::Extract { lo, hi, soft, invert } => out.push(step(OP_EXTRACT, [*invert as u32, 0, 0, 0], &[*lo, *hi, *soft])),
        FxOp::Invert { channel: 4, blend } => out.push(step(OP_INVERT_ALPHA, [0; 4], &[*blend])),
        FxOp::Invert { channel, blend } => out.push(step(OP_INVERT, [*channel, 0, 0, 0], &[*blend])),
        FxOp::Posterize { n } => out.push(step(OP_POSTERIZE, [0; 4], &[*n])),
        FxOp::Gaussian { rx, ry, repeat } => {
            boxes(rx, false, *repeat, 0, &mut out);
            boxes(ry, true, *repeat, 0, &mut out);
        }
        FxOp::DirectionalBlur { dx, dy, steps } => {
            if *steps >= 2 {
                out.push(step(OP_DIRECTIONAL, [*steps, 0, 0, 0], &[*dx, *dy]));
            }
        }
        FxOp::Unsharp { rx, ry, amount, threshold } => {
            boxes(rx, false, true, 0, &mut out);
            boxes(ry, true, true, 0, &mut out);
            for s in &mut out {
                s.unsharp_blur = true;
            }
            let mut s = step(OP_UNSHARP, [0; 4], &[*amount, *threshold]);
            s.combine = true;
            out.push(s);
        }
        FxOp::Crop { x0, x1, y0, y1, feather } => out.push(step(OP_CROP, [0; 4], &[*x0, *x1, *y0, *y1, *feather])),
        FxOp::Mosaic { bw, bh } => out.push(step(OP_MOSAIC, [0; 4], &[*bw, *bh])),
        FxOp::Vignette { amount, mid, round, feather, target } => {
            out.push(step(OP_VIGNETTE, [0; 4], &[*amount, *mid, *round, *feather, target[0], target[1], target[2]]));
        }
        FxOp::Resample(r) => {
            let op = if (r.opacity - 1.0).abs() < 1e-6 { 1.0 } else { r.opacity };
            match &r.inv {
                Some(m) => out.push(step(OP_RESAMPLE, r.rect, &[m.a as f32, m.b as f32, m.c as f32, m.d as f32, m.e as f32, m.f as f32, op, 1.0])),
                None => out.push(step(OP_RESAMPLE, [0; 4], &[0.0; 8])),
            }
        }
        FxOp::HFlip => out.push(step(OP_HFLIP, [0; 4], &[])),
        FxOp::VFlip => out.push(step(OP_VFLIP, [0; 4], &[])),
        FxOp::Mirror { cx, cy, nx, ny } => out.push(step(OP_MIRROR, [0; 4], &[*cx, *cy, *nx, *ny])),
        FxOp::Offset { dx, dy, blend } => out.push(step(OP_OFFSET, [0; 4], &[*dx, *dy, *blend])),
        FxOp::AscCdl { slope: s, offset: o, power: p, sat } => {
            out.push(step(OP_ASC_CDL, [0; 4], &[s[0], s[1], s[2], *sat, o[0], o[1], o[2], 0.0, p[0], p[1], p[2]]));
        }
        FxOp::ChannelMix { m } => {
            let p: Vec<f32> = m.iter().flatten().copied().collect();
            out.push(step(OP_CHANNEL_MIX, [0; 4], &p));
        }
        FxOp::ColorReplace { sim, solid, target: t, replace: r, replace_hsl: h } => {
            out.push(step(OP_COLOR_REPLACE, [*solid as u32, 0, 0, 0], &[t[0], t[1], t[2], *sim, r[0], r[1], r[2], 0.0, h[0], h[1], h[2]]))
        }
        FxOp::AlphaAdjust { opacity, ignore, invert, mask_only } => {
            out.push(step(OP_ALPHA_ADJUST, [*ignore as u32, *invert as u32, *mask_only as u32, 0], &[*opacity]));
        }
        FxOp::Lumetri { temp, tint, exposure, contrast, hl, sh, wh, bl, sat, vib, faded, va, vmid, vround, vfeather, aspect, extra, hdr } => {
            // split tone, Look, wheels and curves: flags and look in the integers, tables in `data`
            let (flags, look, mut data) = match extra {
                Some(x) => {
                    let (flags, data) = x.packed();
                    (flags, x.look, Some(data))
                }
                None => (0, 0, None),
            };
            // HDR: the grade spaces' codes in the integers, their parameters in the data header
            let [bm, am] = match hdr {
                Some(h) => h.pack(data.get_or_insert_with(Vec::new)),
                None => [0, 0],
            };
            let mut s = step(
                OP_LUMETRI,
                [flags, look, bm, am],
                &[*temp, *tint, *exposure, *contrast, *hl, *sh, *wh, *bl, *sat, *vib, *faded, *va, *vmid, *vround, *vfeather, *aspect],
            );
            s.data = data;
            out.push(s);
        }
        FxOp::Key(k) => out.push(step(OP_KEY, [0; 4], &[k.kycc[0], k.kycc[1], k.kycc[2], k.tol, k.soft])),
        FxOp::LumaKey { th, cut } => out.push(step(OP_LUMA_KEY, [0; 4], &[*th, *cut])),
        FxOp::VideoLimiter(v) => {
            let w = v.warning.unwrap_or_default();
            out.push(step(OP_VIDEO_LIMITER, [v.axis, v.warning.is_some() as u32, 0, 0], &[v.max, v.comp, 0.0, 0.0, w[0], w[1], w[2]]));
        }
        FxOp::HslSecondary(h) => {
            // the key (and its refinement) is made beside the image, which stays untouched until
            // the apply pass reads it back (as Unsharp keeps its original)
            // the grading signal (HDR: its code in i1.y, parameters and the BT.2020 luma weights)
            let (gm, gp) = LumetriHdr::code(h.grade_space());
            let l = luma2020();
            let mut key =
                vec![step(OP_HSL_KEY, [0, gm, 0, 0], &[h.hc, h.hr, h.smin, h.lmin, h.lmax, h.soft, 0.0, 0.0, gp[0], gp[1], 0.0, 0.0, l[0], l[1], l[2]])];
            if h.median_r > 0 {
                key.push(step(OP_HSL_MEDIAN, [h.median_r as u32, 0, 0, 0], &[h.denoise.min(1.0)]));
            }
            let radii: Vec<u32> = h.blur_radii.iter().map(|r| *r as u32).collect();
            boxes(&radii, false, true, 0, &mut key);
            boxes(&radii, true, true, 0, &mut key);
            for s in &mut key {
                s.unsharp_blur = true;
            }
            out.append(&mut key);
            let mut apply = step(OP_HSL_APPLY, [h.show_mask, gm, 0, 0], &[h.htemp, h.htint, h.hsat, h.hshift, gp[0], gp[1], 0.0, 0.0, l[0], l[1], l[2]]);
            apply.combine = true;
            out.push(apply);
        }
        FxOp::UltraKey(k) => {
            // the matte (and its Choke / Soften blurs) is made beside the image, as HSL Secondary's key
            let m = [k.k[0], k.k[1], k.k[2], k.tol, k.gain, k.highlight, k.shadow, k.pedestal];
            let mut key = vec![step(OP_UKEY_MATTE, [0; 4], &m)];
            if k.choke > 0.0 {
                boxes(&k.choke_boxes.0, false, false, 0, &mut key);
                boxes(&k.choke_boxes.1, true, false, 0, &mut key);
                let mut p = m.to_vec();
                p.push(k.choke);
                let mut choke = step(OP_UKEY_CHOKE, [0; 4], &p);
                choke.aux_orig = true;
                key.push(choke);
            }
            boxes(&k.soften_boxes.0, false, false, 0, &mut key);
            boxes(&k.soften_boxes.1, true, false, 0, &mut key);
            for s in &mut key {
                s.unsharp_blur = true;
            }
            out.append(&mut key);
            let cc = k.cc.unwrap_or([0.0, 1.0, 1.0]);
            let mut apply = step(
                OP_UKEY_APPLY,
                [k.dom, k.output, k.cc.is_some() as u32, 0],
                &[k.contrast, k.mid, k.spill, k.desat, k.range, k.spill_luma, 0.0, 0.0, cc[0], cc[1], cc[2]],
            );
            apply.combine = true;
            out.push(apply);
        }
        FxOp::Chain(ops) => {
            for op in ops {
                out.extend(steps(op));
            }
        }
        FxOp::Ext(x) => ext_steps(x, &mut out),
        FxOp::More(m) => more_steps(m, &mut out),
    }
    out
}

/// The passes of a GPU3 op (`gpufx_ext`): one per pixel, but Drop Shadow, whose shadow (and its
/// blur) is made beside the layer, which the last pass puts over it (as Unsharp keeps its original).
fn ext_steps(x: &ExtOp, out: &mut Vec<Step>) {
    match x {
        ExtOp::Nop => {}
        ExtOp::Noise { amount, colored, clip, grain, frame } => {
            // (`gpu_ok`: a whole number of pixels, below 4096)
            let flags = u32::from(*colored) | (u32::from(*clip) << 1);
            out.push(step(OP_NOISE, [flags, *grain as u32, *frame as u32, (*frame >> 32) as u32], &[*amount]));
        }
        ExtOp::Warp(w) => {
            let (kind, p): (u32, Vec<f64>) = match w {
                Warp::CornerPin { inv, w, h } => (1, inv.iter().copied().chain([*w, *h]).collect()),
                Warp::Lens { k, cx, cy, norm } => (2, vec![*k, *cx, *cy, *norm]),
                Warp::Twirl { cx, cy, ang, rad } => (3, vec![*cx, *cy, *ang, *rad]),
                // the phase in cycles, reduced here in f64 (sin of a large f32 angle is coarse)
                Warp::Wave { hgt, wid, ux, uy, phase } => (4, vec![*hgt, *wid, *ux, *uy, (phase / std::f64::consts::TAU).rem_euclid(1.0)]),
                Warp::Spherize { cx, cy, r } => (5, vec![*cx, *cy, *r]),
                Warp::Basic3d { n, u, v, z0, focal, w, h } => (6, vec![n[0], n[1], n[2], u[0], u[1], u[2], v[0], v[1], v[2], *z0, *focal, 0.0, *w, *h]),
            };
            let p: Vec<f32> = p.iter().map(|v| *v as f32).collect();
            out.push(step(OP_WARP, [kind, 0, 0, 0], &p));
        }
        ExtOp::Magnify { cx, cy, m, r, fe, op, square, mode } => {
            out.push(step(OP_MAGNIFY, [u32::from(*square), *mode, 0, 0], &[*cx as f32, *cy as f32, *m as f32, *r, *fe, *op]));
        }
        ExtOp::FindEdges { invert, blend } => out.push(step(OP_FIND_EDGES, [u32::from(*invert), 0, 0, 0], &[*blend])),
        ExtOp::Emboss { dx, dy, contrast, blend } => out.push(step(OP_EMBOSS, [0; 4], &[*dx, *dy, *contrast, *blend])),
        ExtOp::BevelAlpha { th, lx, ly, intensity } => out.push(step(OP_BEVEL_ALPHA, [0; 4], &[*th, *lx, *ly, *intensity])),
        ExtOp::Replicate { n } => out.push(step(OP_REPLICATE, [(*n).clamp(1, 16) as u32, 0, 0, 0], &[])),
        ExtOp::Strobe { color: c, blend } => out.push(step(OP_STROBE, [0; 4], &[c[0], c[1], c[2], *blend])),
        ExtOp::DropShadow(d) => {
            let c = d.color;
            let mut shadow = vec![step(OP_SHADOW, d.rect, &[(-d.dx) as f32, (-d.dy) as f32, d.opacity, 0.0, c[0], c[1], c[2]])];
            boxes(&d.boxes.0, false, false, 0, &mut shadow);
            boxes(&d.boxes.1, true, false, 0, &mut shadow);
            for s in &mut shadow {
                s.unsharp_blur = true;
            }
            out.append(&mut shadow);
            let mut over = step(OP_SHADOW_OVER, [u32::from(d.only), 0, 0, 0], &[]);
            over.combine = true;
            out.push(over);
        }
        ExtOp::Generate(g) => {
            let s = match g {
                Generator::Ramp { sx, sy, vx, vy, sc, ec, radial, blend } => step(
                    OP_GENERATE,
                    [1, u32::from(*radial), 0, 0],
                    &[*sx as f32, *sy as f32, *vx as f32, *vy as f32, sc[0], sc[1], sc[2], *blend, ec[0], ec[1], ec[2]],
                ),
                Generator::FourColor { colors: c, opacity, exp } => step(
                    OP_GENERATE,
                    [2, 0, 0, 0],
                    &[c[0][0], c[0][1], c[0][2], *opacity, c[1][0], c[1][1], c[1][2], *exp, c[2][0], c[2][1], c[2][2], 0.0, c[3][0], c[3][1], c[3][2]],
                ),
                Generator::Circle { cx, cy, r, color: c, opacity } => {
                    step(OP_GENERATE, [3, 0, 0, 0], &[*cx as f32, *cy as f32, *r, *opacity, c[0], c[1], c[2]])
                }
                Generator::Grid { size, border, color: c, opacity } => step(OP_GENERATE, [4, 0, 0, 0], &[*size, *border, *opacity, 0.0, c[0], c[1], c[2]]),
            };
            out.push(s);
        }
    }
}

/// Gaussian box passes (both axes) on the channels of `mask`.
fn side_boxes(b: &(Vec<u32>, Vec<u32>), mask: u32, repeat: bool, out: &mut Vec<Step>) {
    boxes(&b.0, false, repeat, mask, out);
    boxes(&b.1, true, repeat, mask, out);
}

/// Low and high halves of a 64-bit seed (the shader's 64-bit hash arithmetic).
fn split64(v: u64) -> (u32, u32) {
    (v as u32, (v >> 32) as u32)
}

/// Mark passes as made beside the layer (kept until the combining pass, as Unsharp's blur).
fn beside(mut key: Vec<Step>, out: &mut Vec<Step>) {
    for s in &mut key {
        s.unsharp_blur = true;
    }
    out.append(&mut key);
}

/// The passes of a GPU4 op (`gpufx_more`): one per pixel, but the glows and Roughen Edges, whose
/// planes (edge mask, blurred alpha) are made beside the layer, which the last pass reads back
/// (as Unsharp keeps its original), and Channel Blur's per-channel box passes.
fn more_steps(m: &MoreOp, out: &mut Vec<Step>) {
    match m {
        MoreOp::Nop => {}
        MoreOp::LensFlare(l) => {
            let ghosts: Vec<f32> = l.ghosts.iter().take(8).flat_map(|g| [g.0 as f32, g.1 as f32, g.2 as f32, g.3[0], g.3[1], g.3[2]]).collect();
            let n = (ghosts.len() / 6) as u32;
            let mut s =
                step(OP_LENS_FLARE, [l.lens, n, 0, 0], &[l.cx as f32, l.cy as f32, l.scale as f32, l.k, l.tint[0], l.tint[1], l.tint[2], l.core_k as f32]);
            // (no table without ghosts: a storage binding cannot be empty)
            s.data = (n > 0).then_some(ghosts);
            out.push(s);
        }
        MoreOp::RgbSplit { radial, ox, oy, cx, cy, maxd, amt, blend } => {
            out.push(step(OP_RGB_SPLIT, [u32::from(*radial), 0, 0, 0], &[*ox, *oy, *cx, *cy, *maxd, *amt, *blend]));
        }
        MoreOp::Lighting(l) => {
            let lights: Vec<f32> = l
                .lights
                .iter()
                .take(5)
                .flat_map(|li| {
                    [
                        li.kind.min(3) as f32,
                        li.cx as f32,
                        li.cy as f32,
                        li.major as f32,
                        li.minor as f32,
                        li.rot.0 as f32,
                        li.rot.1 as f32,
                        li.intensity,
                        li.focus,
                        li.color[0],
                        li.color[1],
                        li.color[2],
                    ]
                })
                .collect();
            let n = (lights.len() / 12) as u32;
            let mut s = step(OP_LIGHTING, [u32::from(l.white_high), n, 0, 0], &[l.amb[0], l.amb[1], l.amb[2], l.gloss, l.material, l.exposure, l.bump]);
            s.data = (n > 0).then_some(lights);
            out.push(s);
        }
        MoreOp::ColorEmboss { dx, dy, contrast, blend } => out.push(step(OP_COLOR_EMBOSS, [0; 4], &[*dx, *dy, *contrast, *blend])),
        MoreOp::RoundedCrop { l, t, r, b, rad, fe, border, bc } => {
            out.push(step(OP_ROUNDED_CROP, [0; 4], &[*l, *t, *r, *b, *rad, *fe, *border, 0.0, bc[0], bc[1], bc[2]]));
        }
        MoreOp::Gradient { sx, sy, vx, vy, sc, ec, shape, scatter, gamma, blend } => out.push(step(
            OP_GRADIENT,
            [*shape, 0, 0, 0],
            &[*sx, *sy, *vx, *vy, sc[0], sc[1], sc[2], sc[3], ec[0], ec[1], ec[2], ec[3], *scatter, *gamma, *blend],
        )),
        MoreOp::LightLeaks { c1, c2, inten, scale, dx, dy, t, seed, mode } => {
            let (lo, hi) = split64(*seed);
            out.push(step(OP_LIGHT_LEAKS, [*mode, lo, hi, 0], &[c1[0], c1[1], c1[2], *inten, c2[0], c2[1], c2[2], *scale, *dx, *dy, *t]));
        }
        MoreOp::Turbulent { kind, amount, size, ox, oy, cplx, seed, ex, ey, ez, pin } => {
            let (lo, hi) = split64(*seed);
            out.push(step(OP_TURBULENT, [*kind, lo, hi, *pin], &[*amount, *size, *ox, *oy, *cplx, *ex, *ey, *ez]));
        }
        MoreOp::Median { r } => out.push(step(OP_MEDIAN, [(*r).clamp(1, 4), 0, 0, 0], &[])),
        MoreOp::ChannelBlur { boxes: per, repeat } => {
            // channels with the same blur share their passes
            for (ch, b) in per.iter().enumerate() {
                if per.iter().take(ch).any(|o| o == b) {
                    continue;
                }
                let mask = per.iter().enumerate().filter(|(_, o)| *o == b).fold(0, |m, (k, _)| m | (1 << k));
                side_boxes(b, mask, *repeat, out);
            }
        }
        MoreOp::AlphaGlow { boxes: b, bright, sc, ec, use_end, fade } => {
            let mut key = Vec::new();
            side_boxes(b, CH_A, false, &mut key);
            beside(key, out);
            let mut glow = step(OP_ALPHA_GLOW, [u32::from(*use_end), u32::from(*fade), 0, 0], &[sc[0], sc[1], sc[2], *bright, ec[0], ec[1], ec[2]]);
            glow.combine = true;
            out.push(glow);
        }
        MoreOp::EdgeGlow { th, width, halo, inten, col, only } => {
            // the edge mask in R (becomes the halo) and G (the core)
            let mut key = vec![step(OP_EDGE_GLOW_EDGES, [0; 4], &[*th])];
            if let Some(wb) = width {
                side_boxes(wb, CH_R | CH_G, false, &mut key);
                key.push(step(OP_EDGE_GLOW_X3, [0; 4], &[]));
            }
            side_boxes(halo, CH_R, false, &mut key);
            beside(key, out);
            let mut add = step(OP_EDGE_GLOW_ADD, [u32::from(*only), 0, 0, 0], &[col[0], col[1], col[2], *inten]);
            add.combine = true;
            out.push(add);
        }
        MoreOp::Roughen { kind, depth, sharp, infl, scale, stretch, offx, offy, cplx, evo, seed, ec } => {
            let mut key = Vec::new();
            side_boxes(depth, CH_A, false, &mut key);
            beside(key, out);
            let (lo, hi) = split64(*seed);
            let mut r = step(OP_ROUGHEN, [*kind, lo, hi, 0], &[*sharp, *infl, *scale, *stretch, *offx, *offy, *cplx, *evo, ec[0], ec[1], ec[2]]);
            r.combine = true;
            out.push(r);
        }
    }
}

impl FxStage {
    /// Whether `device` can run the stage: compute shaders with 16×16 workgroups, a storage
    /// texture and two storage buffers (WebGL2-class devices can't; they render effects on the
    /// CPU).
    pub(crate) fn supported(device: &wgpu::Device) -> bool {
        let l = device.limits();
        l.max_storage_textures_per_shader_stage >= 1
            && l.max_storage_buffers_per_shader_stage >= 2
            && l.max_compute_invocations_per_workgroup >= 256
            && l.max_compute_workgroup_size_x >= 64
            && l.max_compute_workgroup_size_y >= 16
            && l.max_compute_workgroups_per_dimension >= 1
    }

    /// The stage, or `None` when this GPU can't build it: the working format (`Rgba32Float`) is
    /// not renderable or not a storage texture everywhere (OpenGL / llvmpipe). Effects then run on
    /// the CPU and the rest of the compositor stays on the GPU.
    pub(crate) fn try_new(device: &wgpu::Device, composite: &wgpu::ShaderModule, layer_bgl: &wgpu::BindGroupLayout) -> Option<Self> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let stage = Self::new(device, composite, layer_bgl);
        // native backends report at once; a scope still pending (web) counts as success, its
        // error then reaches the device's uncaptured-error handler
        let mut pop = std::pin::pin!(scope.pop());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match std::future::Future::poll(pop.as_mut(), &mut cx) {
            std::task::Poll::Ready(Some(e)) => {
                log::warn!("GPU effect stage unavailable on this GPU, effects run on the CPU: {e}");
                None
            }
            _ => Some(stage),
        }
    }

    fn new(device: &wgpu::Device, composite: &wgpu::ShaderModule, layer_bgl: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("filmcraft-fx"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fx.wgsl").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let masks = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let no_masks = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx-no-masks"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: FX_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                tex(3),
                masks(4),
                masks(5),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("fx"), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fx"),
                layout: Some(&pl),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let spl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx-source"),
            bind_group_layouts: &[Some(layer_bgl)],
            immediate_size: 0,
        });
        let source = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx-source"),
            layout: Some(&spl),
            vertex: wgpu::VertexState { module: composite, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: composite,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FX_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            px: compute("fx_px"),
            run: compute("fx_run"),
            more: compute("fx_more"),
            median: compute("fx_median"),
            bgl,
            source,
            no_masks,
            pool: HashMap::new(),
            used: HashSet::new(),
            uniform_pool: Vec::new(),
            used_uniforms: Vec::new(),
        }
    }

    /// Start a frame: pooled textures of sizes no layer uses by [`end_frame`](Self::end_frame)
    /// are released then.
    pub(crate) fn begin_frame(&mut self) {
        self.used.clear();
        for b in self.used_uniforms.drain(..) {
            if self.uniform_pool.len() < 64 {
                self.uniform_pool.push(b);
            }
        }
    }

    pub(crate) fn end_frame(&mut self) {
        let used = &self.used;
        self.pool.retain(|k, _| used.contains(k));
        if self.pool.is_empty() {
            self.uniform_pool.clear();
        }
    }

    /// No working textures held.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.pool.is_empty()
    }

    fn targets(&mut self, device: &wgpu::Device, size: (u32, u32), n: usize) -> Vec<Target> {
        self.used.insert(size);
        let v = self.pool.entry(size).or_default();
        while v.len() < n {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("filmcraft-fx"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FX_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = t.create_view(&Default::default());
            v.push((t, view));
        }
        v.iter().take(n).cloned().collect()
    }

    /// Prepare the passes of `fx`, whose source draw uses `source_bg` (a layer bind group whose
    /// uniforms map the frame onto the working image). None when the working image does not fit
    /// in a texture (the layer is then drawn without its effects — never larger than the source
    /// frame, which would not have uploaded either).
    pub(crate) fn job(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fx: &LayerFx,
        source_bg: wgpu::BindGroup,
        dummy: &wgpu::TextureView,
    ) -> Option<FxJob> {
        use wgpu::util::DeviceExt;
        let (w, h) = (fx.size.0.max(1), fx.size.1.max(1));
        let max = device.limits().max_texture_dimension_2d;
        if w > max || h > max {
            return None;
        }
        let all = all_steps(fx);
        let n = 2 + usize::from(all.iter().any(|s| s.combine)) + usize::from(all.iter().any(|s| s.mask_begin));
        let targets = self.targets(device, (w, h), n);
        let views: Vec<wgpu::TextureView> = targets.iter().map(|t| t.1.clone()).collect();
        let mut dispatches = Vec::with_capacity(all.len());
        let mut cur = 0usize;
        // Unsharp: the image its blur started from (kept while its blur passes run)
        let mut orig: Option<usize> = None;
        // an effect with masks: the image it started from (kept until the mix)
        let mut before: Option<usize> = None;
        for s in &all {
            if s.mask_begin {
                before = Some(cur);
            }
            // (an Unsharp without blur passes, radius 0, combines the original with itself)
            if (s.unsharp_blur || s.combine) && orig.is_none() {
                orig = Some(cur);
            }
            let keep = if s.unsharp_blur || s.combine { orig } else { None };
            let dst = (0..n).find(|k| *k != cur && Some(*k) != keep && Some(*k) != before)?;
            let aux = match (s.combine || s.aux_orig, orig, s.masks.is_some(), before) {
                (true, Some(o), ..) => views.get(o)?,
                (_, _, true, Some(b)) => views.get(b)?,
                _ => dummy,
            };
            let mask_buffers = s.masks.as_ref().map(|m| {
                let (infos, pts) = crate::mask::pack(&m.iter().collect::<Vec<_>>());
                let init = |contents: &[u8]| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("fx-masks"), contents, usage: wgpu::BufferUsages::STORAGE })
                };
                (init(&infos), init(&pts))
            });
            let data_buffer = s.data.as_ref().map(|d| {
                let mut bytes: Vec<u8> = d.iter().flat_map(|v| v.to_le_bytes()).collect();
                // the shader reads the data as `array<vec2<f32>>`: an odd last float would be out
                // of bounds (read as 0); whole 16-byte units keep every float addressable
                bytes.resize(bytes.len().div_ceil(16) * 16, 0);
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("fx-data"), contents: &bytes, usage: wgpu::BufferUsages::STORAGE })
            });
            let (infos, pts) = match (&mask_buffers, &data_buffer) {
                (Some((i, p)), _) => (i, p),
                (None, Some(d)) => (&self.no_masks, d),
                (None, None) => (&self.no_masks, &self.no_masks),
            };
            let mut bytes = [0u8; 96];
            for (idx, v) in [s.code, w, h, 0].iter().chain(&s.i1).enumerate() {
                bytes[idx * 4..idx * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            for (idx, v) in s.p.iter().enumerate() {
                let off = 32 + idx * 4;
                bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
            }
            let buf = self.uniform_pool.pop().unwrap_or_else(|| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fx-u"),
                    size: bytes.len() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            queue.write_buffer(&buf, 0, &bytes);
            self.used_uniforms.push(buf.clone());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(views.get(cur)?) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(views.get(dst)?) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(aux) },
                    wgpu::BindGroupEntry { binding: 4, resource: infos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: pts.as_entire_binding() },
                ],
            });
            let groups = if s.run {
                let lines = if s.i1[2] != 0 { w } else { h };
                (lines.div_ceil(64), 1)
            } else {
                (w.div_ceil(16), h.div_ceil(16))
            };
            let pipe = match s.code {
                _ if s.run => Pipe::Run,
                OP_MEDIAN => Pipe::Median,
                c if c >= OP_LENS_FLARE => Pipe::More,
                _ => Pipe::Px,
            };
            dispatches.push(Dispatch { pipe, bg, groups });
            cur = dst;
            if s.combine {
                orig = None;
            }
            if s.masks.is_some() {
                before = None;
            }
        }
        Some(FxJob {
            source_bg,
            source_target: views.first()?.clone(),
            dispatches,
            result: views.get(cur)?.clone(),
            result_texture: targets.get(cur)?.0.clone(),
        })
    }

    /// Record a job (source draw, then its compute passes) into `enc`.
    pub(crate) fn record(&self, enc: &mut wgpu::CommandEncoder, job: &FxJob) {
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fx-source"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &job.source_target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.source);
            pass.set_bind_group(0, &job.source_bg, &[]);
            pass.draw(0..6, 0..1);
        }
        if job.dispatches.is_empty() {
            return;
        }
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("fx"), timestamp_writes: None });
        for d in &job.dispatches {
            pass.set_pipeline(match d.pipe {
                Pipe::Px => &self.px,
                Pipe::Run => &self.run,
                Pipe::More => &self.more,
                Pipe::Median => &self.median,
            });
            pass.set_bind_group(0, &d.bg, &[]);
            pass.dispatch_workgroups(d.groups.0, d.groups.1, 1);
        }
    }
}
