//! Morph Cut: a jump cut between two shots of the same subject. Both shots move toward one meeting point,
//! where their faces agree, while they dissolve; each starts as shot and ends as shot at the other end.
//! Without a face in both shots it is a plain cross dissolve.
//!
//! The face is found by the Haar cascades in [`crate::facedet`] on a copy of each shot at most
//! [`ANALYSIS_W`] pixels wide. Where both eyes are also found (on the shot at full resolution), the
//! meeting is a similarity fitted to the eyes: it matches their centre, their spacing and their slant.
//! Otherwise it matches the face boxes' centres and sizes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use filmcraft_color::{linear_to_srgb, luma709};
use filmcraft_geom::Affine;
use rayon::prelude::*;

use crate::facedet::{Face, eyes_in, largest};
use crate::image::Image;

/// Width of the copy the face detector runs on.
const ANALYSIS_W: usize = 320;

/// A shot's face, and its two eye centres when both were found, in the shot's own pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shot {
    face: Face,
    eyes: Option<[(f64, f64); 2]>,
}

/// A similarity of the plane, `z ↦ a·z + b`, with complex numbers written as `(re, im)`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sim {
    a: (f64, f64),
    b: (f64, f64),
}

impl Sim {
    const IDENTITY: Sim = Sim { a: (1.0, 0.0), b: (0.0, 0.0) };

    /// Part of the way from the identity (`q` = 0) to this similarity (`q` = 1). The scale and the turn are
    /// interpolated along the logarithm, so the path turns and grows at a steady rate; the shift is linear.
    fn at(self, q: f64) -> Sim {
        Sim { a: cexp(cscale(clog(self.a), q)), b: (self.b.0 * q, self.b.1 * q) }
    }

    fn to_affine(self) -> Affine {
        let (ar, ai) = self.a;
        Affine { a: ar, b: ai, c: -ai, d: ar, e: self.b.0, f: self.b.1 }
    }
}

fn cmul(x: (f64, f64), y: (f64, f64)) -> (f64, f64) {
    (x.0 * y.0 - x.1 * y.1, x.0 * y.1 + x.1 * y.0)
}

fn cdiv(x: (f64, f64), y: (f64, f64)) -> (f64, f64) {
    let d = y.0 * y.0 + y.1 * y.1;
    cmul(x, (y.0 / d, -y.1 / d))
}

fn csqrt(z: (f64, f64)) -> (f64, f64) {
    let r = (z.0 * z.0 + z.1 * z.1).sqrt().sqrt();
    let t = z.1.atan2(z.0) / 2.0;
    (r * t.cos(), r * t.sin())
}

fn clog(z: (f64, f64)) -> (f64, f64) {
    ((z.0 * z.0 + z.1 * z.1).sqrt().ln(), z.1.atan2(z.0))
}

fn cexp(z: (f64, f64)) -> (f64, f64) {
    let r = z.0.exp();
    (r * z.1.cos(), r * z.1.sin())
}

fn cscale(z: (f64, f64), k: f64) -> (f64, f64) {
    (z.0 * k, z.1 * k)
}

/// The two similarities that carry the shots to the meeting point: `(for a, for b)`. At the meeting, the
/// eyes (or the face boxes) of both shots coincide. Each shot is taken from its own pose to the meeting pose.
fn meeting(sa: &Shot, sb: &Shot) -> (Sim, Sim) {
    if let (Some(ea), Some(eb)) = (sa.eyes, sb.eyes) {
        let eye_vector = |e: [(f64, f64); 2]| (e[1].0 - e[0].0, e[1].1 - e[0].1);
        let midpoint = |e: [(f64, f64); 2]| ((e[0].0 + e[1].0) / 2.0, (e[0].1 + e[1].1) / 2.0);
        let (va, vb) = (eye_vector(ea), eye_vector(eb));
        if (va.0 * va.0 + va.1 * va.1) > 1e-9 && (vb.0 * vb.0 + vb.1 * vb.1) > 1e-9 {
            let (ma, mb) = (midpoint(ea), midpoint(eb));
            // the meeting eye vector is the geometric mean of the two: halfway in turn and in log-scale
            let mut t = csqrt(cmul(va, vb));
            let neg = (-t.0, -t.1);
            if (t.0 - va.0).powi(2) + (t.1 - va.1).powi(2) > (neg.0 - va.0).powi(2) + (neg.1 - va.1).powi(2) {
                t = neg;
            }
            let target = ((ma.0 + mb.0) / 2.0, (ma.1 + mb.1) / 2.0);
            let to_meeting = |v: (f64, f64), m: (f64, f64)| {
                let a = cdiv(t, v);
                let am = cmul(a, m);
                Sim { a, b: (target.0 - am.0, target.1 - am.1) }
            };
            return (to_meeting(va, ma), to_meeting(vb, mb));
        }
    }
    // no eyes in one shot: match the face boxes' centres and sizes
    let centre = |f: &Face| (f.x + f.size / 2.0, f.y + f.size / 2.0);
    let (ca, cb) = (centre(&sa.face), centre(&sb.face));
    let target = ((ca.0 + cb.0) / 2.0, (ca.1 + cb.1) / 2.0);
    let size = (sa.face.size * sb.face.size).sqrt();
    let to_meeting = |f: &Face| {
        let s = size / f.size;
        let c = centre(f);
        Sim { a: (s, 0.0), b: (target.0 - s * c.0, target.1 - s * c.1) }
    };
    (to_meeting(&sa.face), to_meeting(&sb.face))
}

/// The Morph Cut frame at progress `p` (0 = outgoing shot, 1 = incoming shot).
pub(super) fn morph_cut(a: &Image, b: &Image, p: f32) -> Image {
    let (Some(sa), Some(sb)) = (shot_in(a), shot_in(b)) else { return dissolve(a, b, p) };
    let (ma, mb) = meeting(&sa, &sb);
    let p = p as f64;
    // outgoing: identity at p = 0, its full move at p = 1 (where it has faded out); incoming the reverse
    dissolve(&similarity(a, ma.at(p)), &similarity(b, mb.at(1.0 - p)), p as f32)
}

/// `img` moved by the similarity `s`; the identity returns the shot unchanged. Where the moved shot no
/// longer covers the frame, the shot as shot shows through, so a move never opens a transparent border.
fn similarity(img: &Image, s: Sim) -> Image {
    if s == Sim::IDENTITY {
        return img.clone();
    }
    let mut moved = img.transformed(img.w, img.h, &s.to_affine());
    // premultiplied "moved over original"
    moved.px.par_chunks_mut(4).zip(img.px.par_chunks(4)).for_each(|(m, o)| {
        let k = 1.0 - m[3];
        if k > 0.0 {
            for c in 0..4 {
                m[c] += o[c] * k;
            }
        }
    });
    moved
}

/// Detections per shot, keyed by a hash of the shot's pixels. A transition asks for the same two shots on
/// every frame, so the detectors run once per shot rather than once per frame.
type ShotCache = Mutex<HashMap<u64, Option<Shot>>>;
fn shot_cache() -> &'static ShotCache {
    static C: OnceLock<ShotCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Hash of a shot's size and every pixel's bits (FNV-1a over the words).
fn shot_key(img: &Image) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
    mix(img.w as u64);
    mix(img.h as u64);
    for v in &img.px {
        mix(v.to_bits() as u64);
    }
    h
}

/// The largest face in a shot, and its eyes, in the shot's own pixels.
fn shot_in(img: &Image) -> Option<Shot> {
    if img.w < 24 || img.h < 24 {
        return None;
    }
    let key = shot_key(img);
    if let Some(hit) = shot_cache().lock().ok().and_then(|c| c.get(&key).copied()) {
        return hit;
    }
    let found = detect_in(img);
    if let Ok(mut c) = shot_cache().lock() {
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, found);
    }
    found
}

/// Display-referred luma of a shot at a point (linear light in, pixel-centre coordinates).
fn luma_at(img: &Image, x: f32, y: f32) -> f32 {
    let px = img.sample_bilinear(x, y);
    luma709(linear_to_srgb(px[0].max(0.0)), linear_to_srgb(px[1].max(0.0)), linear_to_srgb(px[2].max(0.0)))
}

fn detect_in(img: &Image) -> Option<Shot> {
    let lw = ANALYSIS_W.min(img.w);
    let lh = ((img.h as f64) * lw as f64 / img.w as f64).round().max(1.0) as usize;
    let (sx, sy) = (img.w as f32 / lw as f32, img.h as f32 / lh as f32);
    let mut luma = vec![0.0f32; lw * lh];
    luma.par_iter_mut().enumerate().for_each(|(i, v)| {
        let (x, y) = (i % lw, i / lw);
        *v = luma_at(img, (x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
    });
    let f = largest(&luma, lw, lh)?;
    let k = img.w as f64 / lw as f64;
    let face = Face { x: f.x * k, y: f.y * k, size: f.size * k };
    Some(Shot { face, eyes: eyes_of(img, face) })
}

/// The eyes of a face, found on the shot at full resolution (the eyes are too small in the analysis copy).
fn eyes_of(img: &Image, face: Face) -> Option<[(f64, f64); 2]> {
    let x0 = face.x.max(0.0).floor();
    let y0 = face.y.max(0.0).floor();
    let x1 = (face.x + face.size).min(img.w as f64).ceil();
    let y1 = (face.y + face.size).min(img.h as f64).ceil();
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut region = vec![0.0f32; rw * rh];
    region.par_iter_mut().enumerate().for_each(|(i, v)| {
        let (x, y) = (i % rw, i / rw);
        *v = luma_at(img, x0 as f32 + x as f32 + 0.5, y0 as f32 + y as f32 + 0.5);
    });
    let relative = Face { x: face.x - x0, y: face.y - y0, size: face.size };
    let [l, r] = eyes_in(&region, rw, rh, relative)?;
    Some([(l.0 + x0, l.1 + y0), (r.0 + x0, r.1 + y0)])
}

fn dissolve(a: &Image, b: &Image, p: f32) -> Image {
    let mut out = Image::new(a.w, a.h);
    out.px.par_iter_mut().zip(a.px.par_iter().zip(b.px.par_iter())).for_each(|(o, (x, y))| *o = x * (1.0 - p) + y * p);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_color::srgb_to_linear;

    fn flat(w: usize, h: usize, v: f32) -> Image {
        let mut img = Image::new(w, h);
        img.px.chunks_mut(4).for_each(|p| p.copy_from_slice(&[v, v, v, 1.0]));
        img
    }

    fn shot(x: f64, y: f64, size: f64, eyes: Option<[(f64, f64); 2]>) -> Shot {
        Shot { face: Face { x, y, size }, eyes }
    }

    fn apply(s: Sim, z: (f64, f64)) -> (f64, f64) {
        let az = cmul(s.a, z);
        (az.0 + s.b.0, az.1 + s.b.1)
    }

    #[test]
    fn eyes_meet_when_the_shots_are_scaled_turned_and_moved() {
        // shot B is shot A turned by 25°, scaled to 60 % and moved: the eyes must land on the same two points
        let ea = [(100.0, 120.0), (150.0, 118.0)];
        let (k, th, t) = (0.6, 25f64.to_radians(), (40.0, -30.0));
        let mv = |z: (f64, f64)| {
            let r = cmul((k * th.cos(), k * th.sin()), z);
            (r.0 + t.0, r.1 + t.1)
        };
        let eb = [mv(ea[0]), mv(ea[1])];
        let (sa, sb) = meeting(&shot(90.0, 100.0, 90.0, Some(ea)), &shot(70.0, 90.0, 54.0, Some(eb)));
        for (x, y) in [(0, 0), (1, 1)] {
            let (pa, pb) = (apply(sa, ea[x]), apply(sb, eb[y]));
            assert!((pa.0 - pb.0).abs() < 1e-6 && (pa.1 - pb.1).abs() < 1e-6, "{pa:?} vs {pb:?}");
        }
        // the meeting eye spacing is the geometric mean of the two shots' spacings
        let (m0, m1) = (apply(sa, ea[0]), apply(sa, ea[1]));
        let spacing = |e: [(f64, f64); 2]| (e[1].0 - e[0].0).hypot(e[1].1 - e[0].1);
        let want = (spacing(ea) * spacing(eb)).sqrt();
        assert!(((m1.0 - m0.0).hypot(m1.1 - m0.1) - want).abs() < 1e-6);
    }

    #[test]
    fn a_moved_shot_still_covers_the_frame() {
        // shrunk and shifted, an opaque shot would leave a transparent border; it must stay opaque
        let img = flat(64, 48, 0.4);
        let moved = similarity(&img, Sim { a: (0.7, 0.1), b: (9.0, -4.0) });
        assert!(moved.px.chunks(4).all(|p| (p[3] - 1.0).abs() < 1e-5 && (p[0] - 0.4).abs() < 1e-5), "border left open");
    }

    #[test]
    fn the_ends_are_the_identity() {
        let s = Sim { a: (0.8, 0.3), b: (5.0, -7.0) };
        assert_eq!(s.at(0.0), Sim::IDENTITY);
        let full = s.at(1.0);
        assert!((full.a.0 - s.a.0).abs() < 1e-12 && (full.a.1 - s.a.1).abs() < 1e-12 && full.b == s.b);
    }

    #[test]
    fn shot_key_tells_shots_apart_and_repeats() {
        // same size, one pixel different: the key must differ; the same shot must give the same key
        let (a, mut b) = (flat(40, 30, 0.2), flat(40, 30, 0.2));
        assert_eq!(shot_key(&a), shot_key(&flat(40, 30, 0.2)));
        b.px[1234] = 0.21;
        assert_ne!(shot_key(&a), shot_key(&b));
        assert_eq!(shot_in(&a), shot_in(&a));
    }

    #[test]
    fn tiny_shots_fall_back_to_a_dissolve() {
        // too small for the detector window: no face search, plain dissolve, no NaN
        let (a, b) = (flat(3, 2, 0.1), flat(3, 2, 0.9));
        let got = morph_cut(&a, &b, 0.5);
        assert_eq!(got.px, dissolve(&a, &b, 0.5).px);
        assert!(got.px.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn without_faces_it_is_a_cross_dissolve() {
        let (a, b) = (flat(96, 64, 0.2), flat(96, 64, 0.8));
        assert!(shot_in(&a).is_none());
        for p in [0.0f32, 0.25, 0.5, 1.0] {
            let got = morph_cut(&a, &b, p);
            let want = dissolve(&a, &b, p);
            assert_eq!(got.px, want.px, "p = {p}");
        }
    }

    /// Runs on a real photograph (`FILMCRAFT_FACE_SAMPLE`): the second shot is the same picture turned by 8°
    /// about its centre and zoomed to 90 %. The eyes must be found in both, and must coincide at the meeting.
    #[test]
    #[ignore]
    fn aligns_a_turned_face() {
        let path = std::env::var("FILMCRAFT_FACE_SAMPLE").expect("set FILMCRAFT_FACE_SAMPLE");
        let src = image::open(path).expect("readable picture").to_rgba32f();
        let (w, h) = (src.width() as usize, src.height() as usize);
        let px: Vec<f32> = src.as_raw().chunks(4).flat_map(|p| [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3]]).collect();
        let a = Image { w, h, px };
        let (k, th) = (0.9, 8f64.to_radians());
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let (re, im) = (k * th.cos(), k * th.sin());
        let turn = Sim { a: (re, im), b: (cx - (re * cx - im * cy), cy - (im * cx + re * cy)) };
        let b = a.transformed(w, h, &turn.to_affine());
        let sa = shot_in(&a).expect("face in shot A");
        let sb = shot_in(&b).expect("face in the turned shot");
        let (ea, eb) = (sa.eyes.expect("eyes in shot A"), sb.eyes.expect("eyes in the turned shot"));
        for (x, y) in ea.iter().zip(eb.iter()) {
            let e = apply(turn, *x);
            eprintln!("eye {y:?}, expected ({:.1}, {:.1})", e.0, e.1);
            assert!((e.0 - y.0).hypot(e.1 - y.1) < 5.0, "eye {y:?} vs expected {e:?}");
        }
        let (ma, mb) = meeting(&sa, &sb);
        for i in 0..2 {
            let (pa, pb) = (apply(ma, ea[i]), apply(mb, eb[i]));
            assert!((pa.0 - pb.0).hypot(pa.1 - pb.1) < 1.0, "meeting: {pa:?} vs {pb:?}");
        }
        if let Ok(out) = std::env::var("FILMCRAFT_MORPH_OUT") {
            let mid = morph_cut(&a, &b, 0.5);
            let bytes: Vec<u8> = mid.px.chunks(4).flat_map(|p| [0, 1, 2].map(|c| (linear_to_srgb(p[c].clamp(0.0, 1.0)) * 255.0) as u8)).collect();
            image::RgbImage::from_raw(w as u32, h as u32, bytes).expect("sized").save(out).expect("writable");
        }
    }

    /// Runs on a real photograph whose path is in `FILMCRAFT_FACE_SAMPLE` (a PNG). It writes the morph at
    /// the middle of the transition to `FILMCRAFT_MORPH_OUT` when that is set, for a visual check.
    #[test]
    #[ignore]
    fn aligns_a_face_across_a_jump_cut() {
        let path = std::env::var("FILMCRAFT_FACE_SAMPLE").expect("set FILMCRAFT_FACE_SAMPLE");
        let src = image::open(path).expect("readable png").to_rgba32f();
        let (w, h) = (src.width() as usize, src.height() as usize);
        // the pipeline holds linear light: decode the photograph's sRGB first
        let px: Vec<f32> = src.as_raw().chunks(4).flat_map(|p| [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3]]).collect();
        let a = Image { w, h, px };
        // the second shot: the same picture, zoomed to 80 % and moved, so the face is elsewhere
        let zoom = Affine { a: 0.8, b: 0.0, c: 0.0, d: 0.8, e: 60.0, f: 40.0 };
        let b = a.transformed(w, h, &zoom);
        let sa = shot_in(&a).expect("face in shot A");
        let sb = shot_in(&b).expect("face in shot B");
        eprintln!("A {sa:?}\nB {sb:?}");
        let ea = sa.eyes.expect("both eyes in shot A");
        let eb = sb.eyes.expect("both eyes in shot B");
        // the photograph is zoomed by 0.8 and moved by (60, 40): B's eyes must be A's eyes mapped that way
        for (x, y) in ea.iter().zip(eb.iter()) {
            let (ex, ey) = (x.0 * 0.8 + 60.0, x.1 * 0.8 + 40.0);
            assert!((ex - y.0).hypot(ey - y.1) < 3.0, "eye {y:?} vs expected ({ex:.1}, {ey:.1})");
        }
        // after the meeting, the eyes of both shots coincide
        let (ma, mb) = meeting(&sa, &sb);
        for i in 0..2 {
            let (pa, pb) = (apply(ma, ea[i]), apply(mb, eb[i]));
            assert!((pa.0 - pb.0).hypot(pa.1 - pb.1) < 1.0, "meeting: {pa:?} vs {pb:?}");
        }
        let mid = morph_cut(&a, &b, 0.5);
        if let Ok(out) = std::env::var("FILMCRAFT_MORPH_OUT") {
            let bytes: Vec<u8> = mid.px.chunks(4).flat_map(|p| [0, 1, 2].map(|c| (linear_to_srgb(p[c].clamp(0.0, 1.0)) * 255.0) as u8)).collect();
            image::RgbImage::from_raw(w as u32, h as u32, bytes).expect("sized").save(out).expect("writable");
        }
        // the ends are the shots as shot
        assert_eq!(morph_cut(&a, &b, 0.0).px, a.px);
        assert_eq!(morph_cut(&a, &b, 1.0).px, b.px);
    }
}
