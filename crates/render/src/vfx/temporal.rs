//! Effects that look at other frames: Posterize Time, Echo, Auto Reframe and Warp Stabilizer.
//!
//! Warp Stabilizer analyses the clip on first use — every frame decoded small, Shi–Tomasi features
//! tracked frame to frame with pyramidal Lucas–Kanade (the mask tracker in [`crate::track`]) — and
//! caches the per-frame steps in memory. Position and Position, Scale, Rotation fit a similarity
//! (RANSAC); Perspective fits a full projective map ([`Homography`]). Rendering smooths the
//! accumulated camera path with a Gaussian (Smoothness) or holds it fixed (No Motion), warps each
//! frame onto the smoothed path and frames the result (crop / auto-scale / synthesise edges).
//! Subspace Warp reads the steps as a rotating camera (focal length estimated from the steps) and
//! corrects pan, tilt and roll; if the steps are not rotation-like it falls back to Perspective.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use filmcraft_project::{EffectInstance, TrackMethod};
use rayon::prelude::*;

use super::*;
use crate::track::{Gray, Homography, Prepared, fit_homography, track_homography_step, track_step};

/// Posterize Time: the clip shows a new frame only at the effect's frame rate.
pub fn posterize_time(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(env) = cx.env else { return };
    let rate = fv(e, "rate", cx).max(0.01) as f64;
    let t = cx.seconds;
    let q = (t * rate + 1e-6).floor() / rate;
    let dt = q - t;
    if dt.abs() < 1e-6 {
        return;
    }
    if let Some(f) = env.frame(dt, e) {
        *img = f;
    }
}

/// Echo: composites copies of the clip from earlier (negative Echo Time) or later frames.
pub fn echo(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(env) = cx.env else { return };
    let dt = fv(e, "time", cx) as f64;
    let n = fv(e, "count", cx).clamp(0.0, 30.0) as usize;
    if n == 0 || dt.abs() < 1e-6 {
        return;
    }
    let start = fv(e, "start", cx).clamp(0.0, 1.0);
    let decay = fv(e, "decay", cx).max(0.0);
    let op = chv(e, "operator");
    let mut layers: Vec<(Image, f32)> = vec![(img.clone(), start)];
    for k in 1..=n {
        if let Some(f) = env.frame(dt * k as f64, e) {
            layers.push((f, start * decay.powi(k as i32)));
        }
    }
    *img = combine_echoes(&layers, op);
}

/// Combine echo layers (index 0 = the current frame) with an Echo Operator.
pub(crate) fn combine_echoes(layers: &[(Image, f32)], op: u32) -> Image {
    let (w, h) = (layers[0].0.w, layers[0].0.h);
    let mut out = Image::new(w, h);
    let wsum: f32 = layers.iter().map(|l| l.1).sum::<f32>().max(1e-6);
    out.px.par_chunks_mut(4).enumerate().for_each(|(i, o)| {
        let px = |l: &(Image, f32)| -> [f32; 4] {
            if l.0.w != w || l.0.h != h {
                return [0.0; 4];
            }
            let p = &l.0.px[i * 4..i * 4 + 4];
            [p[0] * l.1, p[1] * l.1, p[2] * l.1, p[3] * l.1]
        };
        let mut acc = match op {
            2 => [f32::INFINITY; 4],
            3 => [1.0; 4],
            _ => [0.0; 4],
        };
        match op {
            4 => {
                // composite in back: the current frame in front, older echoes behind
                for l in layers.iter().rev() {
                    over(&mut acc, px(l));
                }
            }
            5 => {
                // composite in front: echoes over the current frame
                for l in layers {
                    over(&mut acc, px(l));
                }
            }
            _ => {
                for l in layers {
                    let p = px(l);
                    for k in 0..4 {
                        acc[k] = match op {
                            1 => acc[k].max(p[k]),
                            2 => acc[k].min(p[k]),
                            3 => acc[k] * (1.0 - p[k].min(1.0)),
                            6 => acc[k] + p[k] / wsum,
                            _ => acc[k] + p[k],
                        };
                    }
                }
                if op == 3 {
                    acc = acc.map(|v| 1.0 - v);
                }
            }
        }
        if op == 0 {
            acc[3] = acc[3].min(1.0);
        }
        o.copy_from_slice(&acc.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 }));
    });
    out
}

// ------------------------------------------------------------------ analysis helpers

fn gray_of(img: &Image) -> Gray {
    let px = img.px.par_chunks(4).map(|p| filmcraft_color::linear_to_srgb(luma([p[0], p[1], p[2]]).clamp(0.0, 1.0))).collect();
    Gray::new(img.w, img.h, px)
}

/// Saliency centroid (0..1 of the frame): gradient energy weighted, mildly centre-biased.
pub(crate) fn saliency_centroid(img: &Image) -> (f32, f32) {
    let g = gray_of(img);
    let (w, h) = (g.w, g.h);
    let (mut sx, mut sy, mut sw) = (0.0f64, 0.0f64, 0.0f64);
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let gx = g.px[y * w + x + 1] - g.px[y * w + x - 1];
            let gy = g.px[(y + 1) * w + x] - g.px[(y - 1) * w + x];
            let (u, v) = (x as f32 / w as f32 - 0.5, y as f32 / h as f32 - 0.5);
            let bias = 1.0 - (u * u + v * v) * 0.8;
            let e = ((gx * gx + gy * gy) * bias.max(0.1)) as f64;
            let e = e * e;
            sx += e * x as f64;
            sy += e * y as f64;
            sw += e;
        }
    }
    if sw <= 1e-12 {
        return (0.5, 0.5);
    }
    (((sx / sw + 0.5) / w as f64) as f32, ((sy / sw + 0.5) / h as f64) as f32)
}

type CentroidCache = Mutex<HashMap<(u64, i64), (f32, f32)>>;
fn centroid_cache() -> &'static CentroidCache {
    static C: OnceLock<CentroidCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Auto Reframe: keeps the subject (a saliency centroid averaged over a time window set by the
/// Motion Preset) in frame while zooming the clip to fill the sequence (or a chosen aspect).
pub fn auto_reframe(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let (lw, lh) = (img.w as f64, img.h as f64);
    // the frame the layer must fill, in layer pixels (Motion at its defaults places the layer
    // centred in the sequence 1:1)
    let target = match (chv(e, "aspect"), cx.env) {
        (0, Some(env)) => {
            let (sw, sh) = env.sequence_size();
            let (src_w, _) = env.source_size();
            let k = lw / src_w.max(1) as f64;
            (sw as f64 * k, sh as f64 * k)
        }
        (0, None) => return,
        (a, _) => {
            let r = [16.0 / 9.0, 9.0 / 16.0, 1.0, 4.0 / 5.0, 16.0 / 9.0][a.min(4) as usize];
            if lw / lh > r { (lh * r, lh) } else { (lw, lw / r) }
        }
    };
    let zoom_user = (fv(e, "zoom", cx) as f64 / 100.0).max(1.0);
    let z = (target.0 / lw).max(target.1 / lh).max(1.0) * zoom_user;
    // window of the source that stays visible
    let (vw, vh) = (target.0.min(lw * z) / z, target.1.min(lh * z) / z);
    let (half, step) = [(1.0, 0.25), (0.5, 0.125), (0.15, 0.05)][chv(e, "preset").min(2) as usize];
    let subject = match cx.env {
        Some(env) => {
            let key = env.clip_key();
            let fps = env.frame_rate().max(1.0);
            let mut acc = (0.0f64, 0.0f64, 0.0f64);
            let mut k = -half;
            while k <= half + 1e-9 {
                let t = cx.seconds + k;
                if t >= -1e-9 && t <= env.clip_seconds() + 1e-9 {
                    let id = (t * fps).round() as i64;
                    let cached = centroid_cache().lock().ok().and_then(|c| c.get(&(key, id)).copied());
                    let c = cached.or_else(|| {
                        let f = env.source_frame(id as f64 / fps - cx.seconds, (160.0 / env.source_size().0.max(1) as f32).min(1.0))?;
                        let c = saliency_centroid(&f);
                        if let Ok(mut m) = centroid_cache().lock() {
                            if m.len() > 100_000 {
                                m.clear();
                            }
                            m.insert((key, id), c);
                        }
                        Some(c)
                    });
                    if let Some((u, v)) = c {
                        let wgt = 1.0 - (k / (half + step)).abs();
                        acc = (acc.0 + u as f64 * wgt, acc.1 + v as f64 * wgt, acc.2 + wgt);
                    }
                }
                k += step;
            }
            if acc.2 > 0.0 { (acc.0 / acc.2, acc.1 / acc.2) } else { (0.5, 0.5) }
        }
        None => {
            let (u, v) = saliency_centroid(&img.downsample2());
            (u as f64, v as f64)
        }
    };
    let off = offv(e, "offset", cx);
    let sx = (subject.0 * lw + off.x).clamp(vw / 2.0, lw - vw / 2.0);
    let sy = (subject.1 * lh + off.y).clamp(vh / 2.0, lh - vh / 2.0);
    // output(p) = input(subject + (p − centre) / z)
    let m = Affine::translate(lw / 2.0, lh / 2.0).then_apply(&Affine::scale(z, z)).then_apply(&Affine::translate(-sx, -sy));
    if (z - 1.0).abs() < 1e-9 && (sx - lw / 2.0).abs() < 1e-6 && (sy - lh / 2.0).abs() < 1e-6 {
        return;
    }
    affine_warp(img, &m);
}

// ------------------------------------------------------------------ Warp Stabilizer

/// A clip's analysed camera motion: per-frame steps (frame k → k+1) in analysis pixels.
#[derive(Clone, Debug)]
pub struct StabilizerPath {
    pub w: usize,
    pub h: usize,
    pub steps: Vec<Affine>,
    pub fps: f64,
}

type PathCache = Mutex<HashMap<u64, Arc<StabilizerPath>>>;
fn path_cache() -> &'static PathCache {
    static C: OnceLock<PathCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// A clip's analysed camera motion as projective steps (frame k → k+1), in analysis pixels.
#[derive(Clone, Debug)]
pub struct HomographyPath {
    pub w: usize,
    pub h: usize,
    pub steps: Vec<Homography>,
    pub fps: f64,
    /// Focal length (pixels) when the steps fit a rotating pinhole camera well enough; `None` otherwise.
    pub focal: Option<f64>,
}

type HomographyCache = Mutex<HashMap<u64, Arc<HomographyPath>>>;
fn homography_cache() -> &'static HomographyCache {
    static C: OnceLock<HomographyCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Forget every cached stabilizer analysis (e.g. after the media changed).
pub fn clear_stabilizer_cache() {
    if let Ok(mut c) = path_cache().lock() {
        c.clear();
    }
    if let Ok(mut c) = homography_cache().lock() {
        c.clear();
    }
}

/// Every frame of a clip decoded small and prepared for tracking, with the tracking region.
struct Analysis {
    w: usize,
    h: usize,
    fps: f64,
    region: [Vec2; 4],
    frames: Vec<Option<Prepared>>,
}

fn analyse_frames(env: &dyn FxEnv, now: f64, detailed: bool) -> Option<Analysis> {
    let fps = env.frame_rate().max(1.0);
    let n = ((env.clip_seconds() * fps).round() as usize).clamp(1, 20_000);
    let src_w = env.source_size().0.max(1) as f32;
    let target_w = if detailed { 960.0 } else { 480.0 };
    let scale = (target_w / src_w).min(1.0);
    let frames: Vec<Option<Prepared>> = (0..n)
        .into_par_iter()
        .map(|k| {
            if filmcraft_media::cancel::cancelled() {
                return None;
            }
            env.source_frame(k as f64 / fps - now, scale).map(|f| Prepared::new(gray_of(&f)))
        })
        .collect();
    if filmcraft_media::cancel::cancelled() {
        return None;
    }
    let (w, h) = frames.iter().flatten().next().map(|p| (p.base().w, p.base().h))?;
    let m = 8.0;
    let region = [Vec2::new(m, m), Vec2::new(w as f64 - m, m), Vec2::new(w as f64 - m, h as f64 - m), Vec2::new(m, h as f64 - m)];
    Some(Analysis { w, h, fps, region, frames })
}

/// Analyse (or fetch the cached analysis of) the clip `env` describes.
pub fn stabilizer_path(env: &dyn FxEnv, now: f64, method: TrackMethod, detailed: bool) -> Option<Arc<StabilizerPath>> {
    let key = env.clip_key() ^ (method as u64).wrapping_mul(0x9E37) ^ (detailed as u64).wrapping_mul(0x51_7CC1);
    if let Some(p) = path_cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Some(p);
    }
    let an = analyse_frames(env, now, detailed)?;
    let steps: Vec<Affine> = (0..an.frames.len().saturating_sub(1))
        .into_par_iter()
        .map(|k| match (&an.frames[k], &an.frames[k + 1]) {
            (Some(a), Some(b)) if a.base().w == b.base().w => track_step(a, b, &an.region, method).map(|s| s.transform).unwrap_or(Affine::IDENTITY),
            _ => Affine::IDENTITY,
        })
        .collect();
    let path = Arc::new(StabilizerPath { w: an.w, h: an.h, steps, fps: an.fps });
    if let Ok(mut c) = path_cache().lock() {
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, path.clone());
    }
    Some(path)
}

/// Analyse (or fetch the cached analysis of) the clip `env` describes, tracking projective steps.
pub fn homography_path(env: &dyn FxEnv, now: f64, detailed: bool) -> Option<Arc<HomographyPath>> {
    let key = env.clip_key() ^ (detailed as u64).wrapping_mul(0x51_7CC1);
    if let Some(p) = homography_cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Some(p);
    }
    let an = analyse_frames(env, now, detailed)?;
    let steps: Vec<Homography> = (0..an.frames.len().saturating_sub(1))
        .into_par_iter()
        .map(|k| match (&an.frames[k], &an.frames[k + 1]) {
            (Some(a), Some(b)) if a.base().w == b.base().w => track_homography_step(a, b, &an.region).unwrap_or(Homography::IDENTITY),
            _ => Homography::IDENTITY,
        })
        .collect();
    let focal = estimate_focal(&steps, an.w as f64, an.h as f64).filter(|&(_, fit)| fit < ROTATION_FIT_MAX).map(|(f, _)| f);
    let path = Arc::new(HomographyPath { w: an.w, h: an.h, steps, fps: an.fps, focal });
    if let Ok(mut c) = homography_cache().lock() {
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, path.clone());
    }
    Some(path)
}

/// Similarity parameters (tx, ty, angle, log-scale) of an affine.
fn decompose(m: &Affine) -> [f64; 4] {
    let s = (m.a * m.a + m.b * m.b).sqrt().max(1e-9);
    [m.e, m.f, m.b.atan2(m.a), s.ln()]
}
fn compose(p: [f64; 4]) -> Affine {
    let s = p[3].exp();
    let (sn, cs) = p[2].sin_cos();
    Affine { a: s * cs, b: s * sn, c: -s * sn, d: s * cs, e: p[0], f: p[1] }
}

/// Per-frame correction transforms (current frame px → stabilised px, analysis pixels).
pub(crate) fn corrections(path: &StabilizerPath, smooth_frames: f64, no_motion: bool, preserve_scale: bool) -> Vec<Affine> {
    let n = path.steps.len() + 1;
    // cumulative: frame 0 → frame k, about the frame centre for stable angle/scale decomposition
    let (cxp, cyp) = (path.w as f64 / 2.0, path.h as f64 / 2.0);
    let to_c = Affine::translate(-cxp, -cyp);
    let from_c = Affine::translate(cxp, cyp);
    let mut cum = vec![Affine::IDENTITY; n];
    for k in 1..n {
        cum[k] = path.steps[k - 1].then_apply(&cum[k - 1]);
    }
    let params: Vec<[f64; 4]> = cum.iter().map(|c| decompose(&to_c.then_apply(c).then_apply(&from_c))).collect();
    // unwrap angles
    let mut params = params;
    for k in 1..n {
        let d = params[k][2] - params[k - 1][2];
        let wrap = (d / std::f64::consts::TAU).round() * std::f64::consts::TAU;
        params[k][2] -= wrap;
    }
    let smoothed: Vec<[f64; 4]> = if no_motion {
        let mean = params.iter().fold([0.0; 4], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2], a[3] + p[3]]).map(|v| v / n as f64);
        vec![mean; n]
    } else {
        let sigma = smooth_frames.max(0.01);
        // (a window wider than the clip covers every frame already: a huge Smoothness from a
        // project file must not make this loop run for ever)
        let r = ((sigma * 3.0).ceil() as isize).clamp(1, n as isize);
        (0..n as isize)
            .map(|k| {
                let mut acc = [0.0; 4];
                let mut ws = 0.0;
                for j in -r..=r {
                    let i = (k + j).clamp(0, n as isize - 1) as usize;
                    let w = (-(j as f64).powi(2) / (2.0 * sigma * sigma)).exp();
                    for c in 0..4 {
                        acc[c] += params[i][c] * w;
                    }
                    ws += w;
                }
                acc.map(|v| v / ws)
            })
            .collect()
    };
    (0..n)
        .map(|k| {
            let mut sp = smoothed[k];
            if preserve_scale {
                sp[3] = params[k][3];
            }
            // in centred coordinates: smoothed ∘ raw⁻¹
            let raw = compose(params[k]);
            let sm = compose(sp);
            let corr = raw.inverse().map(|ri| sm.then_apply(&ri)).unwrap_or(Affine::IDENTITY);
            from_c.then_apply(&corr).then_apply(&to_c)
        })
        .collect()
}

/// Per-frame projective corrections (current frame → stabilised frame): the Perspective counterpart of
/// [`corrections`]. Frame 0's corners are traced through the analysed motion; those four trajectories
/// are smoothed (or held, for No Motion) and the correction for each frame is the map that carries
/// frame 0's corners to their smoothed places, applied after undoing the frame's own motion. Unlike
/// the similarity path this keeps the shot's perspective and removes only its wobble.
fn homography_corrections(path: &HomographyPath, smooth_frames: f64, no_motion: bool) -> Vec<Homography> {
    let n = path.steps.len() + 1;
    let (w, h) = (path.w as f64, path.h as f64);
    let canon = [Vec2::new(0.0, 0.0), Vec2::new(w, 0.0), Vec2::new(w, h), Vec2::new(0.0, h)];
    // cum[k]: frame 0 → frame k
    let mut cum = vec![Homography::IDENTITY; n];
    for k in 1..n {
        cum[k] = path.steps[k - 1].then_apply(&cum[k - 1]);
    }
    // where frame 0's corners sit in each frame (a corner sent to infinity stays put)
    let traj: Vec<[Vec2; 4]> = cum.iter().map(|c| canon.map(|p| c.apply(p).unwrap_or(p))).collect();
    let smoothed: Vec<[Vec2; 4]> = if no_motion {
        let mut mean = [Vec2::ZERO; 4];
        for t in &traj {
            for c in 0..4 {
                mean[c] = mean[c] + t[c] * (1.0 / n as f64);
            }
        }
        vec![mean; n]
    } else {
        let sigma = smooth_frames.max(0.01);
        // (a window wider than the clip covers every frame already: a huge Smoothness from a
        // project file must not make this loop run for ever)
        let r = ((sigma * 3.0).ceil() as isize).clamp(1, n as isize);
        (0..n as isize)
            .map(|k| {
                let mut acc = [Vec2::ZERO; 4];
                let mut ws = 0.0;
                for j in -r..=r {
                    let i = (k + j).clamp(0, n as isize - 1) as usize;
                    let wt = (-(j as f64).powi(2) / (2.0 * sigma * sigma)).exp();
                    for c in 0..4 {
                        acc[c] = acc[c] + traj[i][c] * wt;
                    }
                    ws += wt;
                }
                acc.map(|v| v * (1.0 / ws))
            })
            .collect()
    };
    (0..n)
        .map(|k| {
            // smoothed ∘ raw⁻¹: frame k → frame 0 → where the smoothed path places frame 0
            let to_path = fit_homography(&canon, &smoothed[k]).unwrap_or(Homography::IDENTITY);
            cum[k].inverse().map(|raw| to_path.then_apply(&raw)).unwrap_or(Homography::IDENTITY)
        })
        .collect()
}

/// Resample `src` through `m` (source pixels → output pixels). Output pixels whose source lies outside
/// the picture stay transparent, as [`Image::transformed`] leaves them.
fn homography_warp(src: &Image, m: &Homography) -> Image {
    let (w, h) = (src.w, src.h);
    let mut out = Image::new(w, h);
    let Some(inv) = m.inverse() else { return out };
    out.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in 0..w {
            let Some(q) = inv.apply(Vec2::new(x as f64 + 0.5, py)) else { continue };
            if q.x < -1.0 || q.y < -1.0 || q.x > w as f64 + 1.0 || q.y > h as f64 + 1.0 {
                continue;
            }
            row[x * 4..x * 4 + 4].copy_from_slice(&src.sample_bilinear(q.x as f32, q.y as f32));
        }
    });
    out
}

// ------------------------------------------------------------------ rotating camera (Subspace Warp)

/// Largest median fit residual (‖K⁻¹HK/s − R‖) at which the steps count as camera rotation.
const ROTATION_FIT_MAX: f64 = 0.05;

type Mat3 = [f64; 9];
const I3: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut c = [0.0; 9];
    for r in 0..3 {
        for k in 0..3 {
            c[r * 3 + k] = (0..3).map(|j| a[r * 3 + j] * b[j * 3 + k]).sum();
        }
    }
    c
}

fn transpose3(a: &Mat3) -> Mat3 {
    [a[0], a[3], a[6], a[1], a[4], a[7], a[2], a[5], a[8]]
}

fn det3(m: &Mat3) -> f64 {
    m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6])
}

fn inv3(m: &Mat3) -> Option<Mat3> {
    let d = det3(m);
    if d.abs() < 1e-12 {
        return None;
    }
    let adj = [
        m[4] * m[8] - m[5] * m[7],
        m[2] * m[7] - m[1] * m[8],
        m[1] * m[5] - m[2] * m[4],
        m[5] * m[6] - m[3] * m[8],
        m[0] * m[8] - m[2] * m[6],
        m[2] * m[3] - m[0] * m[5],
        m[3] * m[7] - m[4] * m[6],
        m[1] * m[6] - m[0] * m[7],
        m[0] * m[4] - m[1] * m[3],
    ];
    Some(adj.map(|v| v / d))
}

/// Pinhole intrinsics (focal `f`, principal point at the frame centre) and their inverse.
fn intrinsics(f: f64, w: f64, h: f64) -> (Mat3, Mat3) {
    let k = [f, 0.0, w / 2.0, 0.0, f, h / 2.0, 0.0, 0.0, 1.0];
    let ki = [1.0 / f, 0.0, -w / (2.0 * f), 0.0, 1.0 / f, -h / (2.0 * f), 0.0, 0.0, 1.0];
    (k, ki)
}

/// The rotation a homography implies for the camera `k` (H ≈ K R K⁻¹ up to scale), by the polar
/// factor of K⁻¹HK. Returns it with ‖K⁻¹HK/s − R‖, which is small when H really is a rotation.
fn implied_rotation(h: &Homography, k: &Mat3, ki: &Mat3) -> Option<(Mat3, f64)> {
    let a0 = mat_mul(&mat_mul(ki, &h.m), k);
    let det = det3(&a0);
    if det <= 0.0 || !det.is_finite() {
        return None;
    }
    let a = a0.map(|v| v / det.cbrt());
    let mut r = a;
    for _ in 0..12 {
        // r ← (r + r⁻ᵀ) / 2 converges to the orthogonal polar factor
        let rit = transpose3(&inv3(&r)?);
        for i in 0..9 {
            r[i] = 0.5 * (r[i] + rit[i]);
        }
    }
    let fit = a.iter().zip(&r).map(|(x, y)| (x - y).powi(2)).sum::<f64>().sqrt();
    Some((r, fit))
}

/// The focal length (pixels) whose pinhole camera best explains the steps as pure rotation, with the
/// median fit residual. Searched on a log grid from half to four frame widths.
pub(crate) fn estimate_focal(steps: &[Homography], w: f64, h: f64) -> Option<(f64, f64)> {
    if steps.is_empty() {
        return None;
    }
    let fit_at = |f: f64| {
        let (k, ki) = intrinsics(f, w, h);
        let mut fits: Vec<f64> = steps.iter().map(|s| implied_rotation(s, &k, &ki).map_or(f64::INFINITY, |(_, fit)| fit)).collect();
        fits.sort_by(f64::total_cmp);
        fits[fits.len() / 2]
    };
    // coarse log grid over half to four frame widths, then a finer grid around the best candidate
    let mut best: Option<(f64, f64)> = None;
    for i in 0..=36 {
        let f = w * 0.5 * 8f64.powf(i as f64 / 36.0);
        let median = fit_at(f);
        if best.is_none_or(|(_, b)| median < b) {
            best = Some((f, median));
        }
    }
    let (coarse, _) = best?;
    let step = coarse * 0.06 / 20.0;
    for i in -20..=20 {
        let f = coarse + step * i as f64;
        if f <= 0.0 {
            continue;
        }
        let median = fit_at(f);
        if best.is_none_or(|(_, b)| median < b) {
            best = Some((f, median));
        }
    }
    best
}

/// Unit quaternion (w, x, y, z) of a rotation matrix.
fn quat_of(r: &Mat3) -> [f64; 4] {
    let t = r[0] + r[4] + r[8];
    let q = if t > 0.0 {
        let s = (t + 1.0).sqrt() * 2.0;
        [0.25 * s, (r[7] - r[5]) / s, (r[2] - r[6]) / s, (r[3] - r[1]) / s]
    } else if r[0] > r[4] && r[0] > r[8] {
        let s = (1.0 + r[0] - r[4] - r[8]).sqrt() * 2.0;
        [(r[7] - r[5]) / s, 0.25 * s, (r[1] + r[3]) / s, (r[2] + r[6]) / s]
    } else if r[4] > r[8] {
        let s = (1.0 + r[4] - r[0] - r[8]).sqrt() * 2.0;
        [(r[2] - r[6]) / s, (r[1] + r[3]) / s, 0.25 * s, (r[5] + r[7]) / s]
    } else {
        let s = (1.0 + r[8] - r[0] - r[4]).sqrt() * 2.0;
        [(r[3] - r[1]) / s, (r[2] + r[6]) / s, (r[5] + r[7]) / s, 0.25 * s]
    };
    normalise4(q)
}

fn matrix_of(q: &[f64; 4]) -> Mat3 {
    let [w, x, y, z] = *q;
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y - w * z),
        2.0 * (x * z + w * y),
        2.0 * (x * y + w * z),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z - w * x),
        2.0 * (x * z - w * y),
        2.0 * (y * z + w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

fn normalise4(q: [f64; 4]) -> [f64; 4] {
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    if n < 1e-12 { [1.0, 0.0, 0.0, 0.0] } else { q.map(|v| v / n) }
}

/// Per-frame corrections for a camera that rotates about its centre (Subspace Warp). Each step is read
/// as a rotation, accumulated from frame 0, and the rotation path is smoothed (quaternions, Gaussian or
/// held). Frame k's correction is the rotation from its own orientation to the smoothed one, carried
/// back to pixels through the camera intrinsics, so it removes pan, tilt and roll while keeping the shot's
/// framing. Translation is not modelled; a camera that moves through the scene is only partly corrected.
fn rotation_corrections(path: &HomographyPath, f: f64, smooth_frames: f64, no_motion: bool) -> Vec<Homography> {
    let n = path.steps.len() + 1;
    let (k, ki) = intrinsics(f, path.w as f64, path.h as f64);
    // rc[k]: frame 0 → frame k orientation
    let mut rc = vec![I3; n];
    for i in 1..n {
        let r = implied_rotation(&path.steps[i - 1], &k, &ki).map_or(I3, |(r, _)| r);
        rc[i] = mat_mul(&r, &rc[i - 1]);
    }
    // sign-continuous quaternions, so neighbours average along the short way round
    let mut q: Vec<[f64; 4]> = rc.iter().map(quat_of).collect();
    for i in 1..n {
        if q[i].iter().zip(&q[i - 1]).map(|(a, b)| a * b).sum::<f64>() < 0.0 {
            q[i] = q[i].map(|v| -v);
        }
    }
    let smoothed: Vec<[f64; 4]> = if no_motion {
        let mut mean = [0.0; 4];
        for v in &q {
            for c in 0..4 {
                mean[c] += v[c] / n as f64;
            }
        }
        vec![normalise4(mean); n]
    } else {
        let sigma = smooth_frames.max(0.01);
        // (a window wider than the clip covers every frame already: a huge Smoothness from a
        // project file must not make this loop run for ever)
        let r = ((sigma * 3.0).ceil() as isize).clamp(1, n as isize);
        (0..n as isize)
            .map(|kk| {
                let mut acc = [0.0; 4];
                for j in -r..=r {
                    let i = (kk + j).clamp(0, n as isize - 1) as usize;
                    let wt = (-(j as f64).powi(2) / (2.0 * sigma * sigma)).exp();
                    for c in 0..4 {
                        acc[c] += q[i][c] * wt;
                    }
                }
                normalise4(acc)
            })
            .collect()
    };
    (0..n)
        .map(|kk| {
            // K · R_smoothed · R_rawᵀ · K⁻¹, normalised so the bottom-right entry is 1
            let c = mat_mul(&mat_mul(&k, &mat_mul(&matrix_of(&smoothed[kk]), &transpose3(&rc[kk]))), &ki);
            let s = c[8];
            if s.abs() < 1e-12 { Homography::IDENTITY } else { Homography { m: c.map(|v| v / s) } }
        })
        .collect()
}

/// Smallest zoom about the centre (≥ 1) that hides the borders a correction exposes.
pub(crate) fn cover_scale(corr: &Homography, w: f64, h: f64, max: f64) -> f64 {
    let Some(inv) = corr.inverse() else { return 1.0 };
    let (cx, cy) = (w / 2.0, h / 2.0);
    let covered = |z: f64| {
        [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().all(|&(x, y)| {
            let Some(q) = inv.apply(Vec2::new(cx + (x - cx) / z, cy + (y - cy) / z)) else { return false };
            q.x >= -0.5 && q.y >= -0.5 && q.x <= w + 0.5 && q.y <= h + 0.5
        })
    };
    if covered(1.0) {
        return 1.0;
    }
    let (mut lo, mut hi) = (1.0, max.max(1.0));
    if !covered(hi) {
        return hi;
    }
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if covered(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Warp Stabilizer: see the module docs.
pub fn warp_stabilizer(img: &mut Image, e: &EffectInstance, cx: &FxCtx) {
    let Some(env) = cx.env else { return };
    let no_motion = chv(e, "result") == 1;
    let crop_less = fv(e, "crop_less", cx) as f64 / 100.0;
    // analysis size, frame rate and one correction per frame (current frame → stabilised frame)
    let method = chv(e, "method");
    let (aw, ah, fps, corr): (f64, f64, f64, Vec<Homography>) = if method >= 2 {
        let Some(path) = homography_path(env, cx.seconds, bv(e, "detailed")) else { return };
        let smooth = fv(e, "smoothness", cx) as f64 / 100.0 * path.fps * (0.5 + crop_less);
        // Subspace Warp models the camera as rotating when the steps allow it, else as a projective path
        let corr = match (method, path.focal) {
            (3, Some(f)) => rotation_corrections(&path, f, smooth, no_motion),
            _ => homography_corrections(&path, smooth, no_motion),
        };
        (path.w as f64, path.h as f64, path.fps, corr)
    } else {
        let method = if chv(e, "method") == 0 { TrackMethod::Position } else { TrackMethod::PositionScaleRotation };
        let Some(path) = stabilizer_path(env, cx.seconds, method, bv(e, "detailed")) else { return };
        let smooth = fv(e, "smoothness", cx) as f64 / 100.0 * path.fps;
        let affine = corrections(&path, smooth * (0.5 + crop_less), no_motion, bv(e, "preserve_scale"));
        (path.w as f64, path.h as f64, path.fps, affine.iter().map(Homography::from_affine).collect())
    };
    let n = corr.len();
    let k = ((cx.seconds * fps).round().max(0.0) as usize).min(n - 1);
    let framing = chv(e, "framing");
    let max_scale = (fv(e, "max_scale", cx) as f64 / 100.0).max(1.0);
    let safe = 1.0 + fv(e, "action_safe", cx) as f64 / 100.0;
    let extra = fv(e, "additional_scale", cx) as f64 / 100.0;
    let zoom = match framing {
        2 => corr.iter().map(|c| cover_scale(c, aw, ah, max_scale)).fold(1.0, f64::max) * safe,
        _ => 1.0,
    } * extra;
    // analysis px → layer px
    let s = img.w as f64 / aw;
    let to_layer = Homography::from_affine(&Affine::scale(s, s));
    let from_layer = Homography::from_affine(&Affine::scale(1.0 / s, 1.0 / s));
    let (lw, lh) = (img.w as f64, img.h as f64);
    let z = Homography::from_affine(
        &Affine::translate(lw / 2.0, lh / 2.0).then_apply(&Affine::scale(zoom, zoom)).then_apply(&Affine::translate(-lw / 2.0, -lh / 2.0)),
    );
    let m = z.then_apply(&to_layer).then_apply(&corr[k]).then_apply(&from_layer);
    let src = img.clone();
    match m.to_affine() {
        Some(a) => affine_warp(img, &a),
        None => *img = homography_warp(&src, &m),
    }
    match framing {
        1 => {
            // crop to the area every frame covers
            let worst = corr.iter().map(|c| cover_scale(c, aw, ah, 4.0)).fold(1.0, f64::max);
            let (mx, my) = (lw * (1.0 - 1.0 / worst) / 2.0, lh * (1.0 - 1.0 / worst) / 2.0);
            crate::effects::crop(img, (mx / lw) as f32, (my / lh) as f32, (mx / lw) as f32, (my / lh) as f32, 0.0);
        }
        3 => {
            // synthesise edges: fill uncovered pixels from the clamped source
            if let Some(inv) = m.inverse() {
                let w = img.w;
                img.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..w {
                        let p = &mut row[x * 4..x * 4 + 4];
                        if p[3] < 0.999 {
                            let Some(q) = inv.apply(Vec2::new(x as f64 + 0.5, y as f64 + 0.5)) else { continue };
                            let fill = src.sample_bilinear_clamped(q.x as f32, q.y as f32);
                            let mut b = fill;
                            over(&mut b, [p[0], p[1], p[2], p[3]]);
                            p.copy_from_slice(&b);
                        }
                    }
                });
            }
        }
        _ => {}
    }
}
