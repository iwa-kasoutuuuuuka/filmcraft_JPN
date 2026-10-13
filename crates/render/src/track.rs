//! Mask tracking: frame-to-frame motion of the picture inside a mask.
//!
//! The method (all textbook, implemented from the papers' equations):
//!
//! 1. **Features** — Shi–Tomasi "good features to track" (Shi & Tomasi, CVPR 1994): the smaller
//!    eigenvalue of the 2×2 gradient structure tensor summed over a 7×7 window, kept above 1 % of
//!    the strongest response inside the mask (shrunk by the window radius), with non-maximum
//!    suppression and a minimum spacing; at most [`MAX_FEATURES`].
//! 2. **Tracking** — pyramidal Lucas–Kanade (Bouguet's formulation of Lucas & Kanade 1981):
//!    a 4-level image pyramid (5-tap binomial blur + 2× decimation), a 15×15 window, Gauss–Newton
//!    iterations per level with bilinear sampling, the estimate doubled down the pyramid.
//!    Each feature is tracked forward and then backward; it is kept only when the backward track
//!    returns within 0.5 px of where it started (forward–backward error, Kalal et al. 2010).
//! 3. **Fit** — RANSAC (Fischler & Bolles 1981) over minimal samples (1 point for Position,
//!    2 for Position & Rotation and Position, Scale & Rotation) with a 1 px inlier threshold and
//!    a fixed-seed generator (deterministic), then a least-squares refit on the inliers:
//!    translation = mean displacement, similarity = the closed-form 2D Umeyama / Procrustes
//!    solution, rigid = the same with the scale normalised away.
//!
//! The result is the transform mapping frame A's pixels to frame B's; the engine applies it to
//! the mask path to write the next Mask Path keyframe.

use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::TrackMethod;
use rayon::prelude::*;

/// Most features tracked per frame.
pub const MAX_FEATURES: usize = 160;
const WIN: i32 = 7;
const LEVELS: usize = 4;
const FB_MAX: f32 = 0.5;
const RANSAC_THRESHOLD: f64 = 1.0;
const RANSAC_ITERS: usize = 300;

/// A single-channel f32 image (0..1, display-referred luma).
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Gray {
    pub fn new(w: usize, h: usize, px: Vec<f32>) -> Self {
        assert_eq!(px.len(), w * h);
        Self { w, h, px }
    }
    /// Luma of 8-bit RGBA (Rec. 709 weights on the encoded values).
    pub fn from_rgba8(w: usize, h: usize, rgba: &[u8]) -> Self {
        let px = rgba.as_chunks::<4>().0.iter().map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).collect();
        Self { w, h, px }
    }
    #[inline]
    fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.px[y * self.w + x]
    }
    /// Bilinear sample at continuous coordinates (pixel centres at integers), clamped.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let a = self.at(x0, y0);
        let b = self.at(x0 + 1, y0);
        let c = self.at(x0, y0 + 1);
        let d = self.at(x0 + 1, y0 + 1);
        let top = a + (b - a) * fx;
        let bot = c + (d - c) * fx;
        top + (bot - top) * fy
    }
    /// 5-tap binomial blur then 2× decimation.
    fn down(&self) -> Gray {
        let k = [1.0, 4.0, 6.0, 4.0, 1.0].map(|v: f32| v / 16.0);
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0.0f32; w * h];
        tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                *o = (0..5).map(|i| k[i] * self.at(x as i32 + i as i32 - 2, y as i32)).sum();
            }
        });
        let t = Gray { w, h, px: tmp };
        let (nw, nh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
        let mut px = vec![0.0f32; nw * nh];
        px.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                *o = (0..5).map(|i| k[i] * t.at(2 * x as i32, 2 * y as i32 + i as i32 - 2)).sum();
            }
        });
        Gray { w: nw, h: nh, px }
    }
    fn pyramid(&self) -> Vec<Gray> {
        let mut v = vec![self.clone()];
        while v.len() < LEVELS && v.last().is_some_and(|g| g.w >= 32 && g.h >= 32) {
            let Some(n) = v.last().map(Gray::down) else { break };
            v.push(n);
        }
        v
    }
    /// Central-difference gradient.
    #[inline]
    fn grad(&self, x: i32, y: i32) -> (f32, f32) {
        ((self.at(x + 1, y) - self.at(x - 1, y)) * 0.5, (self.at(x, y + 1) - self.at(x, y - 1)) * 0.5)
    }
}

fn point_in(poly: &[Vec2], x: f64, y: f64) -> bool {
    let n = poly.len();
    let mut w = 0;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y <= y) != (b.y <= y) && a.x + (b.x - a.x) * (y - a.y) / (b.y - a.y) < x {
            w += if b.y > a.y { 1 } else { -1 };
        }
    }
    w != 0
}

fn dist_to_poly(poly: &[Vec2], p: Vec2) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let d = b - a;
            let l2 = d.x * d.x + d.y * d.y;
            let t = if l2 > 0.0 { (((p - a).x * d.x + (p - a).y * d.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (p - (a + d * t)).length()
        })
        .fold(f64::INFINITY, f64::min)
}

/// Shi–Tomasi corners inside `region` (a polygon in `g`'s pixels), strongest first.
pub fn features(g: &Gray, region: &[Vec2], max: usize, min_dist: f32) -> Vec<[f32; 2]> {
    if region.len() < 3 {
        return Vec::new();
    }
    let lo = region.iter().fold(Vec2::new(f64::INFINITY, f64::INFINITY), |a, p| Vec2::new(a.x.min(p.x), a.y.min(p.y)));
    let hi = region.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |a, p| Vec2::new(a.x.max(p.x), a.y.max(p.y)));
    let margin = WIN + 2;
    let x0 = (lo.x.floor() as i32).max(margin);
    let y0 = (lo.y.floor() as i32).max(margin);
    let x1 = (hi.x.ceil() as i32).min(g.w as i32 - 1 - margin);
    let y1 = (hi.y.ceil() as i32).min(g.h as i32 - 1 - margin);
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let (bw, bh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
    // structure tensor response (min eigenvalue) over a 7×7 window, at pixels inside the region
    let r = 3;
    let resp: Vec<f32> = (0..bw * bh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (x0 + (i % bw) as i32, y0 + (i / bw) as i32);
            let p = Vec2::new(x as f64, y as f64);
            if !point_in(region, p.x, p.y) || dist_to_poly(region, p) < r as f64 {
                return 0.0;
            }
            let (mut a, mut b, mut c) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (gx, gy) = g.grad(x + dx, y + dy);
                    a += gx * gx;
                    b += gx * gy;
                    c += gy * gy;
                }
            }
            let tr = (a + c) * 0.5;
            tr - ((a - c) * (a - c) * 0.25 + b * b).sqrt()
        })
        .collect();
    let best = resp.iter().copied().fold(0.0f32, f32::max);
    if best <= 1e-9 {
        return Vec::new();
    }
    let thr = best * 0.01;
    // 3×3 non-maximum suppression
    let mut cand: Vec<(f32, i32, i32)> = Vec::new();
    for yy in 1..bh.saturating_sub(1) {
        for xx in 1..bw.saturating_sub(1) {
            let v = resp[yy * bw + xx];
            if v < thr {
                continue;
            }
            let mut is_max = true;
            'n: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if (dx != 0 || dy != 0) && resp[(yy as i32 + dy) as usize * bw + (xx as i32 + dx) as usize] > v {
                        is_max = false;
                        break 'n;
                    }
                }
            }
            if is_max {
                cand.push((v, x0 + xx as i32, y0 + yy as i32));
            }
        }
    }
    cand.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out: Vec<[f32; 2]> = Vec::new();
    let md2 = min_dist * min_dist;
    for (_, x, y) in cand {
        let p = [x as f32, y as f32];
        if out.iter().all(|q| (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) >= md2) {
            out.push(p);
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

/// Pyramidal Lucas–Kanade: where `p` in `a` moved to in `b` (pyramids from [`Gray::pyramid`]).
fn lk(pa: &[Gray], pb: &[Gray], p: [f32; 2]) -> Option<[f32; 2]> {
    let levels = pa.len().min(pb.len());
    let mut d = [0.0f32; 2];
    for lvl in (0..levels).rev() {
        let s = (1u32 << lvl) as f32;
        let (a, b) = (&pa[lvl], &pb[lvl]);
        let (px, py) = (p[0] / s, p[1] / s);
        // spatial gradient matrix and template over the window (bilinear at sub-pixel p)
        let n = (2 * WIN + 1) as usize;
        let mut tpl = Vec::with_capacity(n * n);
        let mut grads = Vec::with_capacity(n * n);
        let (mut gxx, mut gxy, mut gyy) = (0.0f32, 0.0f32, 0.0f32);
        for wy in -WIN..=WIN {
            for wx in -WIN..=WIN {
                let (x, y) = (px + wx as f32, py + wy as f32);
                let ix = (a.sample(x + 1.0, y) - a.sample(x - 1.0, y)) * 0.5;
                let iy = (a.sample(x, y + 1.0) - a.sample(x, y - 1.0)) * 0.5;
                gxx += ix * ix;
                gxy += ix * iy;
                gyy += iy * iy;
                tpl.push(a.sample(x, y));
                grads.push((ix, iy));
            }
        }
        let det = gxx * gyy - gxy * gxy;
        if det < 1e-7 {
            return None;
        }
        let mut v = [0.0f32; 2];
        for _ in 0..30 {
            let (mut bx, mut by) = (0.0f32, 0.0f32);
            let mut k = 0;
            for wy in -WIN..=WIN {
                for wx in -WIN..=WIN {
                    let (x, y) = (px + wx as f32 + d[0] + v[0], py + wy as f32 + d[1] + v[1]);
                    let diff = tpl[k] - b.sample(x, y);
                    let (ix, iy) = grads[k];
                    bx += diff * ix;
                    by += diff * iy;
                    k += 1;
                }
            }
            let ex = (gyy * bx - gxy * by) / det;
            let ey = (gxx * by - gxy * bx) / det;
            v[0] += ex;
            v[1] += ey;
            if ex * ex + ey * ey < 1e-6 {
                break;
            }
        }
        d = [d[0] + v[0], d[1] + v[1]];
        if lvl > 0 {
            d = [d[0] * 2.0, d[1] * 2.0];
        }
    }
    let q = [p[0] + d[0], p[1] + d[1]];
    let (w, h) = (pa[0].w as f32, pa[0].h as f32);
    (q[0].is_finite() && q[1].is_finite() && q[0] >= 0.0 && q[1] >= 0.0 && q[0] < w && q[1] < h).then_some(q)
}

/// Least-squares fit of `method` mapping `src` → `dst`.
pub fn fit_ls(method: TrackMethod, src: &[Vec2], dst: &[Vec2]) -> Option<Affine> {
    let n = src.len();
    if n == 0 || (method != TrackMethod::Position && n < 2) {
        return None;
    }
    let inv = 1.0 / n as f64;
    let cs = src.iter().fold(Vec2::ZERO, |a, p| a + *p) * inv;
    let cd = dst.iter().fold(Vec2::ZERO, |a, p| a + *p) * inv;
    if method == TrackMethod::Position {
        let t = cd - cs;
        return Some(Affine::translate(t.x, t.y));
    }
    // 2D Procrustes: [a −b; b a] minimising Σ |R s + t − d|²
    let (mut sa, mut sb, mut ss) = (0.0, 0.0, 0.0);
    for (s, d) in src.iter().zip(dst) {
        let (x, y) = (s.x - cs.x, s.y - cs.y);
        let (u, v) = (d.x - cd.x, d.y - cd.y);
        sa += x * u + y * v;
        sb += x * v - y * u;
        ss += x * x + y * y;
    }
    if ss < 1e-12 {
        return None;
    }
    let (mut a, mut b) = (sa / ss, sb / ss);
    if method == TrackMethod::PositionRotation {
        let l = (a * a + b * b).sqrt();
        if l < 1e-12 {
            return None;
        }
        a /= l;
        b /= l;
    }
    let lin = Affine { a, b, c: -b, d: a, e: 0.0, f: 0.0 };
    let t = cd - lin.apply(cs);
    Some(Affine { e: t.x, f: t.y, ..lin })
}

/// Deterministic xorshift for RANSAC sampling.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// RANSAC + least-squares refit. Returns (transform, inlier count).
pub fn fit_robust(method: TrackMethod, src: &[Vec2], dst: &[Vec2]) -> Option<(Affine, usize)> {
    let n = src.len();
    let k = if method == TrackMethod::Position { 1 } else { 2 };
    if n < k {
        return None;
    }
    let inliers_of = |m: &Affine| -> Vec<usize> { (0..n).filter(|&i| (m.apply(src[i]) - dst[i]).length() < RANSAC_THRESHOLD).collect() };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ n as u64);
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..RANSAC_ITERS {
        let i = rng.below(n);
        let mut idx = vec![i];
        if k == 2 {
            let mut j = rng.below(n);
            let mut guard = 0;
            while (j == i || (src[j] - src[i]).length() < 4.0) && guard < 16 {
                j = rng.below(n);
                guard += 1;
            }
            if j == i {
                continue;
            }
            idx.push(j);
        }
        let (s, d): (Vec<Vec2>, Vec<Vec2>) = idx.iter().map(|&q| (src[q], dst[q])).unzip();
        let Some(m) = fit_ls(method, &s, &d) else { continue };
        let inl = inliers_of(&m);
        if inl.len() > best.len() {
            best = inl;
            if best.len() == n {
                break;
            }
        }
    }
    if best.len() < k {
        return None;
    }
    // refit on inliers, then once more on the refit's inliers
    for _ in 0..2 {
        let (s, d): (Vec<Vec2>, Vec<Vec2>) = best.iter().map(|&q| (src[q], dst[q])).unzip();
        let m = fit_ls(method, &s, &d)?;
        let inl = inliers_of(&m);
        if inl.len() < best.len() {
            break;
        }
        best = inl;
    }
    let (s, d): (Vec<Vec2>, Vec<Vec2>) = best.iter().map(|&q| (src[q], dst[q])).unzip();
    fit_ls(method, &s, &d).map(|m| (m, best.len()))
}

/// One tracking step's result.
#[derive(Clone, Debug)]
pub struct Step {
    /// Maps frame A pixels to frame B pixels.
    pub transform: Affine,
    pub features: usize,
    pub tracked: usize,
    pub inliers: usize,
}

/// A frame prepared for tracking (its pyramid).
pub struct Prepared {
    levels: Vec<Gray>,
}

impl Prepared {
    pub fn new(g: Gray) -> Self {
        Self { levels: g.pyramid() }
    }
    pub fn base(&self) -> &Gray {
        &self.levels[0]
    }
}

/// Features found in `a` inside `region`, and where each one went in `b` (forward–backward checked).
/// Returns (features found, surviving pairs as (from, to)).
fn track_pairs(a: &Prepared, b: &Prepared, region: &[Vec2]) -> Option<(usize, Vec<Vec2>, Vec<Vec2>)> {
    let feats = features(a.base(), region, MAX_FEATURES, 5.0);
    if feats.len() < 3 {
        return None;
    }
    let pairs: Vec<(Vec2, Vec2)> = feats
        .par_iter()
        .filter_map(|&p| {
            let q = lk(&a.levels, &b.levels, p)?;
            let back = lk(&b.levels, &a.levels, q)?;
            let fb = ((back[0] - p[0]).powi(2) + (back[1] - p[1]).powi(2)).sqrt();
            (fb < FB_MAX).then(|| (Vec2::new(p[0] as f64, p[1] as f64), Vec2::new(q[0] as f64, q[1] as f64)))
        })
        .collect();
    let (src, dst): (Vec<Vec2>, Vec<Vec2>) = pairs.into_iter().unzip();
    Some((feats.len(), src, dst))
}

/// Track the picture inside `region` (polygon in frame pixels) from `a` to `b`.
pub fn track_step(a: &Prepared, b: &Prepared, region: &[Vec2], method: TrackMethod) -> Option<Step> {
    let (features, src, dst) = track_pairs(a, b, region)?;
    let (transform, inliers) = fit_robust(method, &src, &dst)?;
    if inliers < 3 {
        return None;
    }
    Some(Step { transform, features, tracked: src.len(), inliers })
}

/// Track the picture inside `region` from `a` to `b` and fit a projective map (the Perspective method).
pub fn track_homography_step(a: &Prepared, b: &Prepared, region: &[Vec2]) -> Option<Homography> {
    let (_, src, dst) = track_pairs(a, b, region)?;
    fit_homography_robust(&src, &dst).map(|(h, _)| h)
}

// ------------------------------------------------------------------ projective (Perspective)

/// A projective map of the plane: `[x y 1]ᵀ ↦ H [x y 1]ᵀ`, row-major. Every constructor keeps the
/// bottom-right entry at 1; a map with a zero bottom row is affine (see [`Homography::to_affine`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography {
    pub m: [f64; 9],
}

impl Homography {
    pub const IDENTITY: Homography = Homography { m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] };

    /// The same map as an affine transform (same convention as [`Affine`]).
    pub fn from_affine(a: &Affine) -> Self {
        Self { m: [a.a, a.c, a.e, a.b, a.d, a.f, 0.0, 0.0, 1.0] }
    }

    /// The affine transform, when there is no perspective term.
    pub fn to_affine(&self) -> Option<Affine> {
        let m = &self.m;
        (m[6] == 0.0 && m[7] == 0.0).then(|| Affine { a: m[0], b: m[3], c: m[1], d: m[4], e: m[2], f: m[5] })
    }

    /// `self` after `o`: `o` is applied first, as [`Affine::then_apply`].
    pub fn then_apply(&self, o: &Homography) -> Homography {
        let (a, b) = (&self.m, &o.m);
        let mut c = [0.0; 9];
        for r in 0..3 {
            for k in 0..3 {
                c[r * 3 + k] = (0..3).map(|j| a[r * 3 + j] * b[j * 3 + k]).sum();
            }
        }
        Homography { m: normalised(c) }
    }

    /// Where `p` lands, or `None` when it goes to infinity.
    pub fn apply(&self, p: Vec2) -> Option<Vec2> {
        let m = &self.m;
        let w = m[6] * p.x + m[7] * p.y + m[8];
        (w.abs() > 1e-12).then(|| Vec2::new((m[0] * p.x + m[1] * p.y + m[2]) / w, (m[3] * p.x + m[4] * p.y + m[5]) / w))
    }

    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.m;
        let c00 = m[4] * m[8] - m[5] * m[7];
        let c01 = m[5] * m[6] - m[3] * m[8];
        let c02 = m[3] * m[7] - m[4] * m[6];
        let det = m[0] * c00 + m[1] * c01 + m[2] * c02;
        if det.abs() < 1e-12 {
            return None;
        }
        let adj = [
            c00,
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            c01,
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            c02,
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        Some(Homography { m: normalised(adj) })
    }
}

/// Scale so the bottom-right entry is 1 (left alone when that entry is ~0).
fn normalised(m: [f64; 9]) -> [f64; 9] {
    let s = m[8];
    if s.abs() < 1e-12 { m } else { m.map(|v| v / s) }
}

/// Solve the 8×8 system held in an augmented matrix by Gauss–Jordan with partial pivoting.
fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let piv = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        let pivot_row = a[col];
        for (r, row) in a.iter_mut().enumerate() {
            if r != col {
                let f = row[col] / pivot_row[col];
                for k in col..9 {
                    row[k] -= f * pivot_row[k];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for (i, xi) in x.iter_mut().enumerate() {
        *xi = a[i][8] / a[i][i];
    }
    Some(x)
}

/// Hartley normalisation: a similarity that moves the points' centroid to the origin and their mean
/// distance to √2. Returns the matrix and the normalised points.
fn normaliser(pts: &[Vec2]) -> Option<([f64; 9], Vec<Vec2>)> {
    let n = pts.len() as f64;
    let (mx, my) = pts.iter().fold((0.0, 0.0), |(x, y), p| (x + p.x / n, y + p.y / n));
    let md = pts.iter().map(|p| ((p.x - mx).powi(2) + (p.y - my).powi(2)).sqrt()).sum::<f64>() / n;
    if md < 1e-9 {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / md;
    let t = [s, 0.0, -s * mx, 0.0, s, -s * my, 0.0, 0.0, 1.0];
    Some((t, pts.iter().map(|p| Vec2::new((p.x - mx) * s, (p.y - my) * s)).collect()))
}

/// Least-squares projective map `src` → `dst` (normalised DLT, Hartley & Zisserman §4.1), needing four
/// or more pairs with no three collinear.
pub fn fit_homography(src: &[Vec2], dst: &[Vec2]) -> Option<Homography> {
    if src.len() < 4 || dst.len() != src.len() {
        return None;
    }
    let (ts, ps) = normaliser(src)?;
    let (td, pd) = normaliser(dst)?;
    // Normal equations AᵀA h = Aᵀb for h = (h0..h7) with h8 = 1, held as an augmented 8×9 matrix.
    let mut ata = [[0.0; 9]; 8];
    for (s, d) in ps.iter().zip(&pd) {
        let rows = [([s.x, s.y, 1.0, 0.0, 0.0, 0.0, -d.x * s.x, -d.x * s.y], d.x), ([0.0, 0.0, 0.0, s.x, s.y, 1.0, -d.y * s.x, -d.y * s.y], d.y)];
        for (r, b) in rows {
            for i in 0..8 {
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
                ata[i][8] += r[i] * b;
            }
        }
    }
    let h = solve8(ata)?;
    let hn = Homography { m: [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0] };
    let td_inv = Homography { m: td }.inverse()?;
    Some(td_inv.then_apply(&hn).then_apply(&Homography { m: ts }))
}

/// RANSAC over four-point samples with the same 1 px threshold, then a refit on the inliers.
/// Returns (map, inlier count).
pub fn fit_homography_robust(src: &[Vec2], dst: &[Vec2]) -> Option<(Homography, usize)> {
    let n = src.len();
    if n < 4 || dst.len() != n {
        return None;
    }
    let inliers_of = |m: &Homography| -> Vec<usize> { (0..n).filter(|&i| m.apply(src[i]).is_some_and(|q| (q - dst[i]).length() < RANSAC_THRESHOLD)).collect() };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ n as u64);
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..RANSAC_ITERS {
        let mut idx = [0usize; 4];
        for k in 0..4 {
            loop {
                let c = rng.below(n);
                if !idx[..k].contains(&c) {
                    idx[k] = c;
                    break;
                }
            }
        }
        let s: Vec<Vec2> = idx.iter().map(|&q| src[q]).collect();
        let d: Vec<Vec2> = idx.iter().map(|&q| dst[q]).collect();
        let Some(m) = fit_homography(&s, &d) else { continue };
        let inl = inliers_of(&m);
        if inl.len() > best.len() {
            best = inl;
            if best.len() == n {
                break;
            }
        }
    }
    if best.len() < 4 {
        return None;
    }
    for _ in 0..2 {
        let (s, d): (Vec<Vec2>, Vec<Vec2>) = best.iter().map(|&q| (src[q], dst[q])).unzip();
        let m = fit_homography(&s, &d)?;
        let inl = inliers_of(&m);
        if inl.len() < best.len() {
            break;
        }
        best = inl;
    }
    let (s, d): (Vec<Vec2>, Vec<Vec2>) = best.iter().map(|&q| (src[q], dst[q])).unzip();
    fit_homography(&s, &d).map(|m| (m, best.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_fit_recovers_a_projective_map() {
        let h = Homography { m: [1.02, 0.05, 3.0, -0.03, 0.98, -2.0, 1e-4, -2e-4, 1.0] };
        let src: Vec<Vec2> = (0..12).map(|i| Vec2::new((i % 4) as f64 * 40.0 + 5.0, (i / 4) as f64 * 30.0 + 7.0)).collect();
        let dst: Vec<Vec2> = src.iter().map(|&p| h.apply(p).unwrap()).collect();
        let f = fit_homography(&src, &dst).unwrap();
        for (a, b) in f.m.iter().zip(&h.m) {
            assert!((a - b).abs() < 1e-9, "{:?} vs {:?}", f.m, h.m);
        }
        // the inverse and composition round-trip
        let back = f.inverse().unwrap().then_apply(&f);
        for (a, b) in back.m.iter().zip(Homography::IDENTITY.m.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn homography_robust_fit_rejects_outliers() {
        let h = Homography { m: [0.97, -0.02, -4.0, 0.01, 1.03, 2.5, -2e-4, 1e-4, 1.0] };
        let src: Vec<Vec2> = (0..30).map(|i| Vec2::new((i % 6) as f64 * 40.0 + 5.0, (i / 6) as f64 * 35.0 + 7.0)).collect();
        let dst: Vec<Vec2> =
            src.iter().enumerate().map(|(i, &p)| if i % 4 == 0 { h.apply(p).unwrap() + Vec2::new(30.0, -25.0) } else { h.apply(p).unwrap() }).collect();
        let (f, inliers) = fit_homography_robust(&src, &dst).unwrap();
        assert!(inliers >= 20, "inliers {inliers}");
        for (a, b) in f.m.iter().zip(&h.m) {
            assert!((a - b).abs() < 1e-6, "{:?} vs {:?}", f.m, h.m);
        }
    }

    /// A deterministic texture: smooth blobs + fine detail.
    fn texture(u: f64, v: f64) -> f32 {
        let s = (u * 0.21).sin() * (v * 0.17).cos() + 0.5 * ((u + v) * 0.43).sin() + 0.35 * ((u * 0.9 - v * 0.6).sin() * (v * 0.75).cos());
        let h = {
            let (iu, iv) = ((u / 6.0).floor() as i64, (v / 6.0).floor() as i64);
            let mut x = (iu.wrapping_mul(73_856_093) ^ iv.wrapping_mul(19_349_663)) as u64;
            x ^= x >> 13;
            x = x.wrapping_mul(0x5bd1_e995);
            (x >> 40) as f64 / (1u64 << 24) as f64
        };
        (0.5 + 0.22 * s + 0.12 * h).clamp(0.0, 1.0) as f32
    }

    /// Frame with a textured disc (radius 70, object coords centred) under `pose`, over a static
    /// differently-textured background.
    fn frame(w: usize, h: usize, pose: &Affine) -> Gray {
        let inv = pose.inverse().unwrap();
        let px = (0..w * h)
            .into_par_iter()
            .map(|i| {
                // 2×2 supersampling
                let mut acc = 0.0;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let p = Vec2::new((i % w) as f64 + ox - 0.5, (i / w) as f64 + oy - 0.5);
                    let o = inv.apply(p);
                    acc += if o.length() < 70.0 { texture(o.x + 200.0, o.y + 300.0) } else { 0.25 + 0.1 * texture(p.x * 0.5 + 900.0, p.y * 0.5) };
                }
                acc / 4.0
            })
            .collect();
        Gray::new(w, h, px)
    }

    fn pose(t: f64, rot_per: f64, scale_per: f64) -> Affine {
        Affine::motion(Vec2::new(150.0 + 3.2 * t, 120.0 - 1.7 * t), Vec2::new(1.0 + scale_per * t, 1.0 + scale_per * t), rot_per * t, Vec2::ZERO)
    }

    fn circle(c: Vec2, r: f64) -> Vec<Vec2> {
        (0..48).map(|i| c + Vec2::new((i as f64 / 48.0 * std::f64::consts::TAU).cos() * r, (i as f64 / 48.0 * std::f64::consts::TAU).sin() * r)).collect()
    }

    fn run(method: TrackMethod, rot: f64, scale: f64, frames: usize) -> f64 {
        let (w, h) = (320, 240);
        let mut region = circle(Vec2::new(150.0, 120.0), 50.0);
        let mut prev = Prepared::new(frame(w, h, &pose(0.0, rot, scale)));
        let mut worst = 0.0f64;
        let probe = [Vec2::new(30.0, 0.0), Vec2::new(-20.0, 25.0), Vec2::new(0.0, -40.0)];
        let mut tracked: Vec<Vec2> = probe.iter().map(|o| pose(0.0, rot, scale).apply(*o)).collect();
        for k in 1..=frames {
            let next = Prepared::new(frame(w, h, &pose(k as f64, rot, scale)));
            let st = track_step(&prev, &next, &region, method).expect("tracked");
            region = region.iter().map(|p| st.transform.apply(*p)).collect();
            tracked = tracked.iter().map(|p| st.transform.apply(*p)).collect();
            for (o, t) in probe.iter().zip(&tracked) {
                let truth = pose(k as f64, rot, scale).apply(*o);
                worst = worst.max((truth - *t).length());
            }
            prev = next;
        }
        worst
    }

    #[test]
    fn tracks_translation() {
        let e = run(TrackMethod::Position, 0.0, 0.0, 10);
        assert!(e < 0.5, "max drift {e} px");
    }

    #[test]
    fn tracks_rotation() {
        let e = run(TrackMethod::PositionRotation, 2.0, 0.0, 10);
        assert!(e < 0.75, "max drift {e} px");
    }

    #[test]
    fn tracks_similarity() {
        let e = run(TrackMethod::PositionScaleRotation, -1.5, 0.012, 10);
        assert!(e < 0.75, "max drift {e} px");
    }

    #[test]
    fn ransac_rejects_outliers() {
        let truth = Affine::motion(Vec2::new(5.0, -3.0), Vec2::new(1.1, 1.1), 7.0, Vec2::ZERO);
        let mut src = Vec::new();
        let mut dst = Vec::new();
        for i in 0..40 {
            let p = Vec2::new((i * 37 % 100) as f64, (i * 61 % 90) as f64);
            src.push(p);
            dst.push(if i % 4 == 0 { p + Vec2::new(20.0, -13.0) } else { truth.apply(p) });
        }
        let (m, inl) = fit_robust(TrackMethod::PositionScaleRotation, &src, &dst).unwrap();
        assert_eq!(inl, 30);
        for p in &src {
            assert!((m.apply(*p) - truth.apply(*p)).length() < 1e-6);
        }
    }
}
