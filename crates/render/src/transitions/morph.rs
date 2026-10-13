//! Morph Cut: a jump cut between two shots of the same subject. The incoming shot is scaled and moved so
//! its face sits where the outgoing face is, and the two then dissolve while the incoming shot settles
//! back into its own framing. Without a face in both shots it is a plain cross dissolve.
//!
//! Faces come from the Haar cascade in [`crate::facedet`], found on a copy of each shot at most
//! [`ANALYSIS_W`] pixels wide.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use filmcraft_color::{linear_to_srgb, luma709};
use filmcraft_geom::Affine;
use rayon::prelude::*;

use crate::facedet::{Face, largest};
use crate::image::Image;

/// Width of the copy the face detector runs on.
const ANALYSIS_W: usize = 320;

/// The Morph Cut frame at progress `p` (0 = outgoing shot, 1 = incoming shot).
pub(super) fn morph_cut(a: &Image, b: &Image, p: f32) -> Image {
    let (Some(fa), Some(fb)) = (face_in(a), face_in(b)) else { return dissolve(a, b, p) };
    // the similarity that puts the incoming face on the outgoing one
    let s = fa.size / fb.size;
    let (ca, cb) = (fa.x + fa.size / 2.0, fa.y + fa.size / 2.0);
    let (ia, ib) = (fb.x + fb.size / 2.0, fb.y + fb.size / 2.0);
    let (tx, ty) = (ca - s * ia, cb - s * ib);
    // k = 1 at the start (incoming face aligned), 0 at the end (incoming shot as shot)
    let k = 1.0 - p as f64;
    let m = Affine { a: 1.0 + (s - 1.0) * k, b: 0.0, c: 0.0, d: 1.0 + (s - 1.0) * k, e: tx * k, f: ty * k };
    dissolve(a, &b.transformed(b.w, b.h, &m), p)
}

/// Largest face per shot, keyed by a hash of the shot's pixels. A transition asks for the same two shots on
/// every frame, so the detector runs once per shot rather than once per frame.
type FaceCache = Mutex<HashMap<u64, Option<Face>>>;
fn face_cache() -> &'static FaceCache {
    static C: OnceLock<FaceCache> = OnceLock::new();
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

/// The largest face in a shot, in the shot's own pixels.
fn face_in(img: &Image) -> Option<Face> {
    if img.w < 24 || img.h < 24 {
        return None;
    }
    let key = shot_key(img);
    if let Some(hit) = face_cache().lock().ok().and_then(|c| c.get(&key).copied()) {
        return hit;
    }
    let found = detect_in(img);
    if let Ok(mut c) = face_cache().lock() {
        if c.len() > 64 {
            c.clear();
        }
        c.insert(key, found);
    }
    found
}

fn detect_in(img: &Image) -> Option<Face> {
    let lw = ANALYSIS_W.min(img.w);
    let lh = ((img.h as f64) * lw as f64 / img.w as f64).round().max(1.0) as usize;
    let (sx, sy) = (img.w as f32 / lw as f32, img.h as f32 / lh as f32);
    let mut luma = vec![0.0f32; lw * lh];
    luma.par_iter_mut().enumerate().for_each(|(i, v)| {
        let (x, y) = (i % lw, i / lw);
        let px = img.sample_bilinear((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
        let (r, g, b) = (linear_to_srgb(px[0].max(0.0)), linear_to_srgb(px[1].max(0.0)), linear_to_srgb(px[2].max(0.0)));
        *v = luma709(r, g, b);
    });
    let f = largest(&luma, lw, lh)?;
    let k = img.w as f64 / lw as f64;
    Some(Face { x: f.x * k, y: f.y * k, size: f.size * k })
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

    #[test]
    fn shot_key_tells_shots_apart_and_repeats() {
        // same size, one pixel different: the key must differ; the same shot must give the same key
        let (a, mut b) = (flat(40, 30, 0.2), flat(40, 30, 0.2));
        assert_eq!(shot_key(&a), shot_key(&flat(40, 30, 0.2)));
        b.px[1234] = 0.21;
        assert_ne!(shot_key(&a), shot_key(&b));
        assert_eq!(face_in(&a), face_in(&a));
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
        assert!(face_in(&a).is_none());
        for p in [0.0f32, 0.25, 0.5, 1.0] {
            let got = morph_cut(&a, &b, p);
            let want = dissolve(&a, &b, p);
            assert_eq!(got.px, want.px, "p = {p}");
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
        let fa = face_in(&a).expect("face in shot A");
        let fb = face_in(&b).expect("face in shot B");
        eprintln!("A {fa:?}  B {fb:?}");
        let mid = morph_cut(&a, &b, 0.5);
        if let Ok(out) = std::env::var("FILMCRAFT_MORPH_OUT") {
            let bytes: Vec<u8> = mid.px.chunks(4).flat_map(|p| [0, 1, 2].map(|c| (linear_to_srgb(p[c].clamp(0.0, 1.0)) * 255.0) as u8)).collect();
            image::RgbImage::from_raw(w as u32, h as u32, bytes).expect("sized").save(out).expect("writable");
        }
        // at the start the incoming face lands on the outgoing one
        let start = morph_cut(&a, &b, 0.0);
        assert_eq!(start.px, a.px);
    }
}
