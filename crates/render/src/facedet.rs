//! Face detection for Morph Cut: the Viola–Jones cascades shipped with OpenCV, frontal
//! (`haarcascade_frontalface_default.xml`, by Rainer Lienhart) and profile (`haarcascade_profileface.xml`,
//! by David Bradley and Princeton University); both under the Intel licence — see ATTRIBUTION.md.
//!
//! Each model is an upright detector of stumps over Haar-like rectangle features, 24×24 for frontal and
//! 20×20 for profile faces. It is evaluated on an image pyramid with integral images (Viola & Jones 2001),
//! variance-normalised as in Lienhart & Maydt (2002). Profile faces are searched on the picture and on its
//! mirror image, so both sides are found. Overlapping hits are grouped into faces.

use std::sync::OnceLock;

const FRONTAL: &str = include_str!("../assets/haarcascade_frontalface_default.xml");
const PROFILE: &str = include_str!("../assets/haarcascade_profileface.xml");
const EYE: &str = include_str!("../assets/haarcascade_eye.xml");
const SMILE: &str = include_str!("../assets/haarcascade_smile.xml");
/// Overlapping smile hits needed before a mouth is reported.
const MIN_MOUTH_NEIGHBOURS: usize = 4;
/// Overlapping eye hits needed before an eye is reported.
const MIN_EYE_NEIGHBOURS: usize = 3;
/// Share of the face box, from its top, in which the eyes are searched.
const EYE_BAND: f64 = 0.65;
/// Ratio between successive pyramid levels (OpenCV's scaleFactor is typically 1.1–1.3).
const SCALE_STEP: f64 = 1.2;
/// Overlapping hits needed before a face is reported (OpenCV's minNeighbors).
const MIN_NEIGHBOURS: usize = 3;
/// The same for profile faces, which the profile detector fires on in backgrounds far more often.
const PROFILE_NEIGHBOURS: usize = 8;
/// Largest detector window a model may ask for, in pixels (the bundled ones are 24 and 20).
const MAX_WINDOW: usize = 256;
/// Intersection-over-union above which two hits belong to the same face.
const SAME_FACE: f64 = 0.4;

/// A detected face, in the pixels of the image searched (a square of side `size`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

/// A weighted rectangle inside the window: (x, y, width, height, weight). For a tilted feature (45 degrees)
/// the top corner is (x, y), the arms run to (x - h, y + h) and (x + w, y + w), and the fourth corner is
/// (x + w - h, y + w + h).
type Rect = (usize, usize, usize, usize, f64);

/// The rectangles of one Haar-like feature, and whether they are tilted by 45 degrees.
struct Feature {
    rects: Vec<Rect>,
    tilted: bool,
}

struct Stump {
    feature: usize,
    threshold: f64,
    left: f64,
    right: f64,
}

struct Stage {
    threshold: f64,
    stumps: Vec<Stump>,
}

struct Cascade {
    /// Detector window, in pixels of the level it runs on (the smile model is wider than it is high).
    win_w: usize,
    win_h: usize,
    features: Vec<Feature>,
    stages: Vec<Stage>,
}

/// The frontal and profile cascades, parsed once. `None` if a bundled model cannot be read (they always can;
/// see the tests).
fn cascades() -> Option<&'static [Cascade; 2]> {
    static C: OnceLock<Option<[Cascade; 2]>> = OnceLock::new();
    C.get_or_init(|| Some([load(FRONTAL)?, load(PROFILE)?])).as_ref()
}

/// A minimal XML element tree: enough for OpenCV's cascade files (no attributes, no entities).
struct Node {
    name: String,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn new(name: &str) -> Self {
        Self { name: name.to_string(), text: String::new(), children: Vec::new() }
    }
}

fn parse_xml(s: &str) -> Option<Node> {
    let mut stack = vec![Node::new("")];
    let mut i = 0;
    while let Some(off) = s[i..].find('<') {
        let at = i + off;
        stack.last_mut()?.text.push_str(&s[i..at]);
        let rest = &s[at..];
        if rest.starts_with("<!--") {
            i = at + rest.find("-->")? + 3;
            continue;
        }
        if rest.starts_with("<?") {
            i = at + rest.find("?>")? + 2;
            continue;
        }
        let end = at + rest.find('>')?;
        let tag = s[at + 1..end].trim();
        if let Some(name) = tag.strip_prefix('/') {
            let node = stack.pop()?;
            if node.name != name.trim() {
                return None;
            }
            stack.last_mut()?.children.push(node);
        } else {
            // attributes (e.g. `<cascade type_id=…>`) are ignored: only the element name matters
            stack.push(Node::new(tag.split_whitespace().next().unwrap_or("")));
        }
        i = end + 1;
    }
    stack.pop()?.children.into_iter().next()
}

/// The first element called `name` in document order.
fn find<'a>(n: &'a Node, name: &str) -> Option<&'a Node> {
    if n.name == name {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, name))
}

fn numbers(text: &str) -> Vec<f64> {
    text.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}

fn load(xml: &str) -> Option<Cascade> {
    let root = parse_xml(xml)?;
    let win_w = *numbers(&find(&root, "width")?.text).first()? as usize;
    let win_h = *numbers(&find(&root, "height")?.text).first()? as usize;
    if win_w == 0 || win_h == 0 || win_w > MAX_WINDOW || win_h > MAX_WINDOW {
        return None;
    }
    let mut features = Vec::new();
    for f in &find(&root, "features")?.children {
        let tilted = find(f, "tilted").and_then(|t| numbers(&t.text).first().copied()).is_some_and(|v| v != 0.0);
        let mut rects = Vec::new();
        for r in &find(f, "rects")?.children {
            let v = numbers(&r.text);
            let (x, y, w, h) = (*v.first()?, *v.get(1)?, *v.get(2)?, *v.get(3)?);
            // inside the window, or the model is not one this evaluator can run: `rect` and `tilted_sum`
            // index the integral images with these
            let (ww, wh) = (win_w as f64, win_h as f64);
            let finite = [x, y, w, h].iter().all(|n| n.is_finite() && *n >= 0.0);
            let fits = if tilted { x >= h && x + w <= ww && y + w + h <= wh } else { x + w <= ww && y + h <= wh };
            if !(finite && fits) {
                return None;
            }
            rects.push((x as usize, y as usize, w as usize, h as usize, *v.get(4)?));
        }
        features.push(Feature { rects, tilted });
    }
    let mut stages = Vec::new();
    for s in &find(&root, "stages")?.children {
        let threshold = *numbers(&find(s, "stageThreshold")?.text).first()?;
        let mut stumps = Vec::new();
        for c in &find(s, "weakClassifiers")?.children {
            let node = numbers(&find(c, "internalNodes")?.text);
            let leaf = numbers(&find(c, "leafValues")?.text);
            let feature = *node.get(2)? as usize;
            if feature >= features.len() {
                return None;
            }
            stumps.push(Stump { feature, threshold: *node.get(3)?, left: *leaf.first()?, right: *leaf.get(1)? });
        }
        stages.push(Stage { threshold, stumps });
    }
    (!stages.is_empty()).then_some(Cascade { win_w, win_h, features, stages })
}

/// Sum over the rectangle `(x, y, w, h)` from an integral image with row stride `stride`.
fn rect(ii: &[f64], stride: usize, x: usize, y: usize, w: usize, h: usize) -> f64 {
    ii[(y + h) * stride + x + w] - ii[y * stride + x + w] - ii[(y + h) * stride + x] + ii[y * stride + x]
}

/// Summed-area tables of the values and of their squares, both (w+1)×(h+1).
fn integrals(px: &[f32], w: usize, h: usize) -> (Vec<f64>, Vec<f64>) {
    let stride = w + 1;
    let mut sum = vec![0.0; stride * (h + 1)];
    let mut sq = vec![0.0; stride * (h + 1)];
    for y in 0..h {
        let (mut row, mut row2) = (0.0, 0.0);
        for x in 0..w {
            let v = px[y * w + x] as f64;
            row += v;
            row2 += v * v;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
            sq[(y + 1) * stride + x + 1] = sq[y * stride + x + 1] + row2;
        }
    }
    (sum, sq)
}

/// The integral images of one pyramid level: upright sums, squared sums, and rotated sums for the tilted
/// features.
struct Tables {
    sum: Vec<f64>,
    sq: Vec<f64>,
    rot: Vec<f64>,
    /// Row length of `rot`.
    rot_stride: usize,
    /// Width of the level (the upright tables' stride is this plus one).
    w: usize,
}

impl Tables {
    fn new(px: &[f32], w: usize, h: usize, tilted: bool) -> Self {
        let (sum, sq) = integrals(px, w, h);
        let (rot, rot_stride) = if tilted { rotated(px, w, h) } else { (Vec::new(), 0) };
        Tables { sum, sq, rot, rot_stride, w }
    }
}

/// The rotated summed-area table. A pixel (i, j) sits at (u, v) = (i + j, j - i + w - 1), so the 45-degree
/// diagonals become rows and columns; the table holds the prefix sums over those cells. Returns it with its
/// row length.
fn rotated(px: &[f32], w: usize, h: usize) -> (Vec<f64>, usize) {
    let n = w + h;
    let stride = n + 1;
    let mut t = vec![0.0f64; stride * stride];
    for j in 0..h {
        for i in 0..w {
            let (u, v) = (i + j, j + w - 1 - i);
            t[(u + 1) * stride + v + 1] = px[j * w + i] as f64;
        }
    }
    for u in 1..=n {
        for v in 1..=n {
            t[u * stride + v] += t[(u - 1) * stride + v] + t[u * stride + v - 1] - t[(u - 1) * stride + v - 1];
        }
    }
    (t, stride)
}

/// Sum over a tilted rectangle whose top corner is (ax, ay) and whose size is (w, h): the pixels with
/// 0 <= (i - ax) + (j - ay) < 2w and 0 <= (j - ay) - (i - ax) < 2h.
fn tilted_sum(t: &Tables, ax: usize, ay: usize, w: usize, h: usize) -> f64 {
    let cols = t.rot_stride;
    let lim = cols - 1;
    // u = i + j runs over [ax + ay, ax + ay + 2w); v = j - i + (w_level - 1) over [ay - ax + W - 1, + 2h)
    let (u0, u1) = ((ax + ay).min(lim), (ax + ay + 2 * w).min(lim));
    let v_base = ay + t.w - 1;
    let v0 = v_base.saturating_sub(ax).min(lim);
    let v1 = (v_base + 2 * h).saturating_sub(ax).min(lim);
    let p = |u: usize, v: usize| t.rot[u * cols + v];
    p(u1, v1) - p(u0, v1) - p(u1, v0) + p(u0, v0)
}

impl Cascade {
    /// Whether the window at (x, y) of a level passes every stage.
    fn accepts(&self, t: &Tables, x: usize, y: usize) -> bool {
        let (ww, wh) = (self.win_w, self.win_h);
        let stride = t.w + 1;
        let area = (ww * wh) as f64;
        let s = rect(&t.sum, stride, x, y, ww, wh);
        let q = rect(&t.sq, stride, x, y, ww, wh);
        let var = q / area - (s / area).powi(2);
        if var <= 1e-12 {
            return false;
        }
        // features are compared against the window's contrast, so the model is lighting-invariant
        let norm = var.sqrt() * area;
        self.stages.iter().all(|stage| {
            let total: f64 = stage
                .stumps
                .iter()
                .map(|st| {
                    let feature = &self.features[st.feature];
                    let f: f64 = feature
                        .rects
                        .iter()
                        .map(|&(rx, ry, rw, rh, wt)| {
                            let sum = if feature.tilted { tilted_sum(t, x + rx, y + ry, rw, rh) } else { rect(&t.sum, stride, x + rx, y + ry, rw, rh) };
                            wt * sum
                        })
                        .sum();
                    if f / norm < st.threshold { st.left } else { st.right }
                })
                .sum();
            total >= stage.threshold
        })
    }
}

/// Bilinear resample of a `w`×`h` image to `lw`×`lh`.
fn resample(px: &[f32], w: usize, h: usize, lw: usize, lh: usize) -> Vec<f32> {
    let (sx, sy) = (w as f64 / lw as f64, h as f64 / lh as f64);
    let mut out = vec![0.0f32; lw * lh];
    for y in 0..lh {
        let fy = ((y as f64 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f64);
        let (y0, y1) = (fy.floor() as usize, (fy.floor() as usize + 1).min(h - 1));
        let ty = (fy - y0 as f64) as f32;
        for x in 0..lw {
            let fx = ((x as f64 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f64);
            let (x0, x1) = (fx.floor() as usize, (fx.floor() as usize + 1).min(w - 1));
            let tx = (fx - x0 as f64) as f32;
            let top = px[y0 * w + x0] * (1.0 - tx) + px[y0 * w + x1] * tx;
            let bottom = px[y1 * w + x0] * (1.0 - tx) + px[y1 * w + x1] * tx;
            out[y * lw + x] = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}

fn overlap(a: &Face, b: &Face) -> f64 {
    let ix = ((a.x + a.size).min(b.x + b.size) - a.x.max(b.x)).max(0.0);
    let iy = ((a.y + a.size).min(b.y + b.size) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    inter / (a.size * a.size + b.size * b.size - inter).max(1e-12)
}

/// Groups raw hits into faces: each hit joins the first face it overlaps (its running mean moves),
/// and faces with fewer than `min_neighbours` hits are dropped. Largest first.
fn group(hits: &[Face], min_neighbours: usize) -> Vec<Face> {
    let mut clusters: Vec<(Face, usize)> = Vec::new();
    for hit in hits {
        match clusters.iter().position(|(c, _)| overlap(c, hit) > SAME_FACE) {
            Some(i) => {
                let (c, n) = &mut clusters[i];
                let k = *n as f64;
                c.x = (c.x * k + hit.x) / (k + 1.0);
                c.y = (c.y * k + hit.y) / (k + 1.0);
                c.size = (c.size * k + hit.size) / (k + 1.0);
                *n += 1;
            }
            None => clusters.push((*hit, 1)),
        }
    }
    let mut faces: Vec<Face> = clusters.into_iter().filter(|(_, n)| *n >= min_neighbours).map(|(f, _)| f).collect();
    faces.sort_by(|a, b| b.size.total_cmp(&a.size));
    faces
}

/// One cascade over the image pyramid, grouped into faces.
fn scan(cascade: &Cascade, px: &[f32], w: usize, h: usize, min_neighbours: usize) -> Vec<Face> {
    let (ww, wh) = (cascade.win_w, cascade.win_h);
    let tilted = cascade.features.iter().any(|f| f.tilted);
    let mut hits = Vec::new();
    let mut scale = 1.0;
    loop {
        let (lw, lh) = ((w as f64 / scale).floor() as usize, (h as f64 / scale).floor() as usize);
        if lw < ww || lh < wh {
            break;
        }
        let level = resample(px, w, h, lw, lh);
        let tables = Tables::new(&level, lw, lh, tilted);
        for y in 0..=lh - wh {
            for x in 0..=lw - ww {
                if cascade.accepts(&tables, x, y) {
                    // a detection is reported as a square of the window's larger side
                    let side = ww.max(wh) as f64;
                    hits.push(Face { x: x as f64 * scale, y: y as f64 * scale, size: side * scale });
                }
            }
        }
        scale *= SCALE_STEP;
    }
    group(&hits, min_neighbours)
}

/// Frontal and profile faces in a greyscale image (`px` row-major, values in 0..1), each largest first.
/// Frontal faces are searched on the picture; profile faces on the picture and on its mirror image, which
/// turns one profile into the other.
fn detect_by_pose(px: &[f32], w: usize, h: usize) -> (Vec<Face>, Vec<Face>) {
    let Some([frontal, profile]) = cascades() else { return (Vec::new(), Vec::new()) };
    // (sizes come from the caller: a product that overflows is not a picture)
    if w.checked_mul(h) != Some(px.len()) || w == 0 || h == 0 {
        return (Vec::new(), Vec::new());
    }
    let mirrored: Vec<f32> = (0..w * h).map(|i| px[(i / w) * w + (w - 1 - i % w)]).collect();
    let mut side = scan(profile, px, w, h, PROFILE_NEIGHBOURS);
    for mut f in scan(profile, &mirrored, w, h, PROFILE_NEIGHBOURS) {
        f.x = w as f64 - f.x - f.size;
        side.push(f);
    }
    side.sort_by(|a, b| b.size.total_cmp(&a.size));
    (scan(frontal, px, w, h, MIN_NEIGHBOURS), side)
}

/// Faces in a greyscale image, largest first.
pub fn detect(px: &[f32], w: usize, h: usize) -> Vec<Face> {
    let (mut faces, side) = detect_by_pose(px, w, h);
    faces.extend(side);
    faces.sort_by(|a, b| b.size.total_cmp(&a.size));
    faces
}

/// The eye cascade (20×20, upright), parsed once.
fn eye_cascade() -> Option<&'static Cascade> {
    static C: OnceLock<Option<Cascade>> = OnceLock::new();
    C.get_or_init(|| load(EYE)).as_ref()
}

/// The smile cascade (36×18, with tilted features), parsed once.
fn smile_cascade() -> Option<&'static Cascade> {
    static C: OnceLock<Option<Cascade>> = OnceLock::new();
    C.get_or_init(|| load(SMILE)).as_ref()
}

/// The mouth of a face, as a centre in the picture's pixels. `px` is a greyscale picture (`w`×`h`, values
/// 0..1) and `face` a box in its pixels. The mouth is searched in the lower half of the box, near its middle,
/// with the smile model: it finds a mouth that is smiling, and usually not one that is not. `None` when there
/// is no such detection.
pub fn mouth_in(px: &[f32], w: usize, h: usize, face: Face) -> Option<(f64, f64)> {
    let smile = smile_cascade()?;
    if w.checked_mul(h) != Some(px.len()) {
        return None;
    }
    let x0 = face.x.max(0.0).floor() as usize;
    let x1 = ((face.x + face.size).min(w as f64).floor() as usize).max(x0);
    let y0 = ((face.y + face.size * 0.5).max(0.0).floor() as usize).min(h);
    let y1 = ((face.y + face.size).min(h as f64).floor() as usize).max(y0);
    let (cw, ch) = (x1 - x0, y1 - y0);
    if cw < smile.win_w || ch < smile.win_h {
        return None;
    }
    let crop: Vec<f32> = (y0..y1).flat_map(|y| px[y * w + x0..y * w + x1].iter().copied()).collect();
    let mouths = scan(smile, &crop, cw, ch, MIN_MOUTH_NEIGHBOURS);
    // detections are reported as squares of the model's wider side; the box is only win_h/win_w of that high,
    // so its centre sits a quarter of the square's side below its top
    let quarter = smile.win_h as f64 / smile.win_w as f64 / 2.0;
    let middle = cw as f64 / 2.0;
    let best = mouths.iter().find(|m| (m.x + m.size / 2.0 - middle).abs() < 0.3 * cw as f64)?;
    Some((x0 as f64 + best.x + best.size / 2.0, y0 as f64 + best.y + best.size * quarter))
}

/// The two eyes of a face, as centres in the picture's pixels, left one first. `px` is a greyscale picture
/// (`w`×`h`, values 0..1) and `face` a box in its pixels. Eyes are searched in the upper part of the box:
/// the largest pair of detections that sit level with each other, a plausible distance apart and of similar size.
/// `None` unless such a pair is found.
pub fn eyes_in(px: &[f32], w: usize, h: usize, face: Face) -> Option<[(f64, f64); 2]> {
    let eye = eye_cascade()?;
    if w.checked_mul(h) != Some(px.len()) {
        return None;
    }
    // the upper part of the face box, clipped to the picture; more than half, so that the lower eye of a
    // tilted head is not cut off
    let x0 = face.x.max(0.0).floor() as usize;
    let y0 = face.y.max(0.0).floor() as usize;
    let x1 = ((face.x + face.size).min(w as f64).floor() as usize).max(x0);
    let y1 = ((face.y + face.size * EYE_BAND).min(h as f64).floor() as usize).max(y0);
    let (cw, ch) = (x1 - x0, y1 - y0);
    if cw < eye.win_w || ch < eye.win_h {
        return None;
    }
    let crop: Vec<f32> = (y0..y1).flat_map(|y| px[y * w + x0..y * w + x1].iter().copied()).collect();
    let eyes = scan(eye, &crop, cw, ch, MIN_EYE_NEIGHBOURS);
    // an eye is a modest part of a face: drop detections too small or too large to be one
    let eyes: Vec<Face> = eyes.into_iter().filter(|e| e.size >= 0.1 * face.size && e.size <= 0.4 * face.size).collect();
    let centre = |e: &Face| (e.x + e.size / 2.0, e.y + e.size / 2.0);
    // every pair that sits level, a plausible distance apart and of similar size; the largest such pair wins
    let mut best: Option<(f64, (f64, f64), (f64, f64))> = None;
    for (i, e1) in eyes.iter().enumerate() {
        for e2 in &eyes[i + 1..] {
            let (c1, c2) = (centre(e1), centre(e2));
            let (dx, dy) = ((c2.0 - c1.0).abs(), (c2.1 - c1.1).abs());
            let ratio = e2.size / e1.size;
            let plausible = dy < 0.25 * face.size && dx > 0.15 * face.size && dx < 0.8 * face.size && (0.6..=1.6).contains(&ratio);
            if plausible && best.is_none_or(|(s, _, _)| e1.size + e2.size > s) {
                best = Some((e1.size + e2.size, c1, c2));
            }
        }
    }
    let (_, c1, c2) = best?;
    let (l, r) = if c1.0 <= c2.0 { (c1, c2) } else { (c2, c1) };
    Some([(x0 as f64 + l.0, y0 as f64 + l.1), (x0 as f64 + r.0, y0 as f64 + r.1)])
}

/// The largest face in the image, if there is one. A frontal face is preferred to a profile: the profile
/// detector also fires on textured backgrounds, and a larger false profile must not win over a real face.
pub fn largest(px: &[f32], w: usize, h: usize) -> Option<Face> {
    let (frontal, side) = detect_by_pose(px, w, h);
    frontal.into_iter().next().or_else(|| side.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sum over a tilted rectangle by looking at every pixel, with the definition in `tilted_sum`.
    fn tilted_brute(px: &[f32], w: usize, h: usize, ax: usize, ay: usize, rw: usize, rh: usize) -> f64 {
        let mut total = 0.0;
        for j in 0..h as i64 {
            for i in 0..w as i64 {
                let (dx, dy) = (i - ax as i64, j - ay as i64);
                let s = dx + dy;
                let t = dy - dx;
                if (0..2 * rw as i64).contains(&s) && (0..2 * rh as i64).contains(&t) {
                    total += px[j as usize * w + i as usize] as f64;
                }
            }
        }
        total
    }

    #[test]
    fn tilted_sums_match_the_pixels_they_cover() {
        // a textured level; every tilted rectangle that fits, against a sum over every pixel
        let (w, h) = (23usize, 17usize);
        let px: Vec<f32> = (0..w * h).map(|k| ((k * 37 + 11) % 97) as f32 / 97.0).collect();
        let t = Tables::new(&px, w, h, true);
        for (ax, ay, rw, rh) in [(5usize, 0usize, 3usize, 2usize), (8, 1, 4, 3), (12, 2, 2, 5), (2, 3, 1, 1), (10, 4, 3, 3)] {
            let got = tilted_sum(&t, ax, ay, rw, rh);
            let want = tilted_brute(&px, w, h, ax, ay, rw, rh);
            assert!((got - want).abs() < 1e-9, "rect at ({ax},{ay}) {rw}x{rh}: {got} vs {want}");
        }
    }

    #[test]
    fn the_smile_model_is_wider_than_high_and_tilted() {
        let c = smile_cascade().expect("the smile model parses");
        assert_eq!((c.win_w, c.win_h), (36, 18));
        assert!(c.features.iter().any(|f| f.tilted));
    }

    #[test]
    fn bundled_model_parses_to_the_opencv_frontal_cascade() {
        // OpenCV's haarcascade_frontalface_default: 25 stages over 2913 upright features
        let [c, profile] = cascades().expect("models parse");
        assert_eq!((profile.win_w, profile.stages.len()), (20, 26));
        assert_eq!(c.stages.len(), 25);
        assert_eq!(c.features.len(), 2913);
        assert_eq!(c.win_w, 24);
        assert!(c.features.iter().all(|f| f.rects.iter().all(|r| r.0 + r.2 <= c.win_w && r.1 + r.3 <= c.win_h)));
    }

    #[test]
    fn flat_and_empty_images_have_no_faces() {
        assert!(detect(&vec![0.5; 120 * 90], 120, 90).is_empty());
        assert!(detect(&[], 0, 0).is_empty());
        assert!(detect(&[0.0; 10], 5, 2).is_empty());
    }

    /// A model whose rectangles leave the window, or that is cut short, is refused when it is
    /// loaded: nothing indexes outside the integral image later.
    #[test]
    fn damaged_models_are_refused() {
        assert!(load("").is_none() && load("<a><b></a>").is_none() && load("<opencv_storage></opencv_storage>").is_none());
        // the real model with one rectangle pushed out of the window
        let bad = FRONTAL.replacen("6 4 12 9 -1.", "6 4 30 9 -1.", 1);
        assert!(bad != FRONTAL && load(&bad).is_none());
        // a window no model has
        assert!(load(&FRONTAL.replacen("<width>24</width>", "<width>1e300</width>", 1)).is_none());
        assert!(load(&FRONTAL.replacen("<width>24</width>", "<width>0</width>", 1)).is_none());
        // cut in the middle, at many places
        for cut in (1..40).map(|i| FRONTAL.len() * i / 40) {
            let end = (0..=cut).rev().find(|i| FRONTAL.is_char_boundary(*i)).unwrap_or(0);
            let _ = load(&FRONTAL[..end]);
        }
        // sizes that do not describe the buffer
        assert!(detect(&[0.0; 16], usize::MAX, 2).is_empty());
        assert!(detect(&[0.0; 16], 4, 5).is_empty());
    }

    #[test]
    fn grouping_needs_neighbours_and_merges_overlaps() {
        let hit = |x: f64| Face { x, y: 10.0, size: 24.0 };
        // three overlapping hits make one face; a lone hit is dropped
        let faces = group(&[hit(10.0), hit(11.0), hit(10.5), Face { x: 200.0, y: 5.0, size: 30.0 }], MIN_NEIGHBOURS);
        assert_eq!(faces.len(), 1);
        assert!((faces[0].x - 10.5).abs() < 0.6);
    }

    /// Runs the profile detector over a folder of PNG photographs of turned faces (`FILMCRAFT_PROFILE_DIR`);
    /// the OpenCV samples `left01…left09` and `right01…right09` are such photographs. Two of them were checked
    /// by eye on 2026-10-13: left06 and right06 show a profile face at the boxes below. The other photographs
    /// show a face only partly turned, or none the profile model can see; any detection there was a monitor
    /// or a shirt. Ignored by default: the photographs are not part of the repository.
    #[test]
    #[ignore]
    fn detects_profile_faces_in_a_folder() {
        let dir = std::env::var("FILMCRAFT_PROFILE_DIR").expect("set FILMCRAFT_PROFILE_DIR");
        let checked = [("left06", (221.0, 117.0)), ("right06", (138.0, 133.0))];
        for (name, (x, y)) in checked {
            let img = image::open(format!("{dir}/{name}.png")).expect("readable png").to_luma32f();
            let (w, h) = (img.width() as usize, img.height() as usize);
            let (_, side) = detect_by_pose(img.as_raw(), w, h);
            let best = side.first().unwrap_or_else(|| panic!("{name}: no profile face"));
            eprintln!("{name}: profile face at ({:.0}, {:.0}), {:.0} px", best.x, best.y, best.size);
            assert!((best.x - x).abs() < 25.0 && (best.y - y).abs() < 25.0, "{name}: face at ({}, {}), expected near ({x}, {y})", best.x, best.y);
        }
    }

    /// Runs on a real photograph whose path is in `FILMCRAFT_FACE_SAMPLE` (PNG or JPEG, greyscale or
    /// RGB). Ignored by default because no photograph is part of the repository. Checked by hand on
    /// 2026-10-13 with NASA's portrait of Neil Armstrong (public domain, 960×1200): `largest` is the
    /// frontal detection on the face, at (490, 237) and 186 px. `detect` also lists a profile hit
    /// around the same head and smaller false positives on the suit and the background.
    #[test]
    #[ignore]
    fn detects_a_face_in_a_photograph() {
        let path = std::env::var("FILMCRAFT_FACE_SAMPLE").expect("set FILMCRAFT_FACE_SAMPLE");
        let img = image::open(path).expect("readable png").to_luma32f();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let faces = detect(img.as_raw(), w, h);
        assert!(!faces.is_empty(), "no face found in {w}x{h}");
        eprintln!("largest: {:?}; {} detections: {faces:?}", largest(img.as_raw(), w, h), faces.len());
    }
}

#[cfg(test)]
mod mouth_tests {
    use super::*;

    /// Runs the mouth detector on the largest face of a photograph (`FILMCRAFT_SMILE_SAMPLE`, a PNG), and
    /// prints where the mouth is. Ignored by default: the photograph is not part of the repository.
    #[test]
    #[ignore]
    fn finds_a_mouth_in_a_photograph() {
        let path = std::env::var("FILMCRAFT_SMILE_SAMPLE").expect("set FILMCRAFT_SMILE_SAMPLE");
        let img = image::open(path).expect("readable png").to_luma32f();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let face = largest(img.as_raw(), w, h).expect("a face");
        eprintln!("face {face:?}");
        match mouth_in(img.as_raw(), w, h, face) {
            Some(m) => eprintln!("MOUTH at ({:.0}, {:.0})", m.0, m.1),
            None => eprintln!("NO MOUTH"),
        }
    }
}
