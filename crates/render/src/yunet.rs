//! YuNet face detection (OpenCV Zoo `face_detection_yunet_2023mar`, see ATTRIBUTION.md), run with the
//! pure-Rust ONNX engine `tract-onnx`. The model is anchor-free: three strides (8, 16, 32) each give a score,
//! a box and five facial landmarks per cell of the 640×640 input.

use std::sync::OnceLock;

use filmcraft_color::linear_to_srgb;
use tract_onnx::prelude::*;

use crate::image::Image;

const MODEL: &[u8] = include_bytes!("../assets/face_detection_yunet_2023mar.onnx");
/// The model's fixed square input.
const INPUT: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
/// Detections below this score are dropped (OpenCV's default is 0.9; 0.6 keeps the profiles in).
const SCORE_MIN: f32 = 0.6;
/// Boxes overlapping more than this are the same face; the higher score is kept.
const NMS_IOU: f64 = 0.3;

/// One detected face, in the picture's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Box: top-left corner and size.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f32,
    /// Right eye, left eye, nose tip, right mouth corner, left mouth corner (the model's order).
    pub landmarks: [(f64, f64); 5],
}

type Runnable = std::sync::Arc<TypedRunnableModel>;

/// The model, optimised and ready to run; `None` if it cannot be read (it always can; see the tests).
fn model() -> Option<&'static Runnable> {
    static M: OnceLock<Option<Runnable>> = OnceLock::new();
    M.get_or_init(|| {
        let model = tract_onnx::onnx()
            .model_for_read(&mut &MODEL[..])
            .ok()?
            .with_input_fact(0, f32::fact([1, 3, INPUT, INPUT]).into())
            .ok()?
            .into_optimized()
            .ok()?;
        model.into_runnable().ok()
    })
    .as_ref()
}

/// sRGB 0..255 from linear 0..1.
fn srgb255(v: f32) -> f32 {
    linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0
}

/// Faces in a picture (`img` is linear light, premultiplied), highest score first. The picture is scaled into
/// the square input with its aspect kept (black below and to the right), in the blue, green, red order the
/// model was trained on.
pub fn detect(img: &Image) -> Vec<Detection> {
    let Some(model) = model() else { return Vec::new() };
    if img.w == 0 || img.h == 0 {
        return Vec::new();
    }
    let scale = INPUT as f64 / img.w.max(img.h) as f64;
    let (nw, nh) = (((img.w as f64 * scale).round() as usize).min(INPUT), ((img.h as f64 * scale).round() as usize).min(INPUT));
    let plane = INPUT * INPUT;
    let mut data = vec![0.0f32; 3 * plane];
    for y in 0..nh {
        for x in 0..nw {
            let p = img.sample_bilinear(((x as f64 + 0.5) / scale) as f32, ((y as f64 + 0.5) / scale) as f32);
            let k = y * INPUT + x;
            data[k] = srgb255(p[2]);
            data[plane + k] = srgb255(p[1]);
            data[2 * plane + k] = srgb255(p[0]);
        }
    }
    let Ok(input) = Tensor::from_shape(&[1, 3, INPUT, INPUT], &data) else { return Vec::new() };
    let Ok(outputs) = model.run(tvec!(input.into())) else { return Vec::new() };
    let view = |i: usize| outputs[i].to_plain_array_view::<f32>().ok();
    let mut found = Vec::new();
    for (level, &stride) in STRIDES.iter().enumerate() {
        let (Some(cls), Some(obj), Some(bbox), Some(kps)) = (view(level), view(3 + level), view(6 + level), view(9 + level)) else {
            return Vec::new();
        };
        let cols = INPUT / stride;
        let s = stride as f64;
        for idx in 0..cols * cols {
            let (r, c) = ((idx / cols) as f64, (idx % cols) as f64);
            let score = (cls[[0, idx, 0]].clamp(0.0, 1.0) * obj[[0, idx, 0]].clamp(0.0, 1.0)).sqrt();
            if score < SCORE_MIN {
                continue;
            }
            let cx = (c + bbox[[0, idx, 0]] as f64) * s;
            let cy = (r + bbox[[0, idx, 1]] as f64) * s;
            let w = (bbox[[0, idx, 2]] as f64).exp() * s;
            let h = (bbox[[0, idx, 3]] as f64).exp() * s;
            let mut landmarks = [(0.0, 0.0); 5];
            for (k, point) in landmarks.iter_mut().enumerate() {
                let (lx, ly) = (kps[[0, idx, 2 * k]] as f64, kps[[0, idx, 2 * k + 1]] as f64);
                *point = (((c + lx) * s) / scale, ((r + ly) * s) / scale);
            }
            found.push(Detection {
                x: (cx - w / 2.0) / scale,
                y: (cy - h / 2.0) / scale,
                w: w / scale,
                h: h / scale,
                score,
                landmarks,
            });
        }
    }
    non_max_suppression(found)
}

fn iou(a: &Detection, b: &Detection) -> f64 {
    let ix = ((a.x + a.w).min(b.x + b.w) - a.x.max(b.x)).max(0.0);
    let iy = ((a.y + a.h).min(b.y + b.h) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    inter / (a.w * a.h + b.w * b.h - inter).max(1e-12)
}

/// Keeps the highest-scoring detection of each overlapping group.
fn non_max_suppression(mut found: Vec<Detection>) -> Vec<Detection> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in found {
        if kept.iter().all(|k| iou(k, &d) <= NMS_IOU) {
            kept.push(d);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_color::srgb_to_linear;

    #[test]
    fn the_model_reads_and_runs_on_a_blank_picture() {
        let img = Image::new(64, 48);
        assert!(model().is_some(), "the bundled model loads");
        assert!(detect(&img).is_empty());
    }

    #[test]
    fn overlapping_boxes_keep_the_higher_score() {
        let d = |x: f64, score: f32| Detection { x, y: 0.0, w: 40.0, h: 40.0, score, landmarks: [(0.0, 0.0); 5] };
        let kept = non_max_suppression(vec![d(0.0, 0.7), d(4.0, 0.9), d(200.0, 0.8)]);
        assert_eq!(kept.len(), 2);
        assert!((kept[0].score - 0.9).abs() < 1e-6 && (kept[1].score - 0.8).abs() < 1e-6);
    }

    /// Runs on every PNG in `FILMCRAFT_YUNET_DIR` and prints the faces found in each. Ignored by default: the
    /// pictures are not part of the repository.
    #[test]
    #[ignore]
    fn detects_faces_in_a_folder() {
        let dir = std::env::var("FILMCRAFT_YUNET_DIR").expect("set FILMCRAFT_YUNET_DIR");
        let mut paths: Vec<_> = std::fs::read_dir(&dir).expect("readable folder").filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "png")).collect();
        paths.sort();
        for path in &paths {
            let rgba = image::open(path).expect("readable png").to_rgba32f();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
            let px: Vec<f32> = rgba.as_raw().chunks(4).flat_map(|p| [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2]), p[3]]).collect();
            let img = Image { w, h, px };
            let faces = detect(&img);
            let best = faces.first().map(|f| format!("({:.0}, {:.0}, {:.0}) score {:.2}", f.x, f.y, f.w, f.score));
            eprintln!("{}: {} faces, best {}", path.file_name().unwrap().to_string_lossy(), faces.len(), best.unwrap_or_else(|| "none".into()));
        }
    }
}
