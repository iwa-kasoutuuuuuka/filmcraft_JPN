//! GPU-vs-CPU parity of the effect stage (`filmcraft_render::gpufx::FxOp::apply` is the
//! reference).
//!
//! Tolerances:
//! - Exact checks ([`gpu_effects_match_cpu_exactly`]): the working image read back as f32 from a
//!   half-float-representable source (so the source draw is lossless) must match the CPU op to
//!   [`EXACT`] per value (f32 arithmetic and the GPU's `pow` / `sqrt` differ in the last bits).
//!   Step functions (Posterize, Color Pass, Leave Color and Change to Color keys, Extract) may
//!   flip a pixel that lands on a threshold; at most [`FLIPS`] of the values may differ by more.
//! - Composited plans ([`gpu_effect_layers_match_cpu_plan`]): the compositor's criterion (p99 of
//!   the per-pixel max 8-bit channel difference ≤ 6, mean < 1.5) away from layer outlines, as for
//!   the blend modes.

use super::blend_tests::{device, interior, ramp_layer, stats8, yuv_frame};
use super::*;
use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::{EffectInstance, ParamValue, find_effect};
use filmcraft_render::effects::FxCtx;
use filmcraft_render::gpufx::FxOp;
use filmcraft_render::plan::{LayerFx, execute_cpu};
use filmcraft_time::Tick;

const EXACT: f32 = 1e-4;
/// Effects built on `pow` / `exp` / `sin` / `cos`: GPU drivers implement these with a few ULP of
/// error (measured up to 1.8e-3 relative on Windows / D3D12), so they get a wider bound than the
/// arithmetic-only effects, which stay at `EXACT`.
const TRANSCENDENTAL: f32 = 4e-3;
const TRANSCENDENTAL_EFFECTS: &[&str] = &[
    "proc_amp",
    "gamma_correction",
    "extract",
    "invert",
    "sharpen",
    "color_replace",
    "lumetri",
    "vignette",
    "ultra_key",
    "color_key",
    "luma_key",
    "video_limiter",
    // GPU3: warps through sin / cos, and pow (measured below 5e-6 on Windows / D3D12)
    "twirl",
    "twirl_legacy",
    "wave_warp",
    "four_color_gradient",
];
const FLIPS: f64 = 0.01;

/// Turbulent Displace: fractal noise at positions `x / size`, where the GPU's f32 division (2.5
/// ULP in WGSL) moves the noise by a few 1e-7, which the displacement (× Amount, × 3 through
/// Bulge's derivative) and a picture with one-pixel stripes turn into up to ~1e-4.
const NOISE_WARP: f32 = 5e-4;

fn tolerance(id: &str) -> f32 {
    if id == "turbulent_displace" {
        NOISE_WARP
    } else if TRANSCENDENTAL_EFFECTS.contains(&id) {
        TRANSCENDENTAL
    } else {
        EXACT
    }
}

fn cx(t: Tick, px_scale: f32) -> FxCtx<'static> {
    FxCtx { t, px_scale, seconds: 0.0, timecode: "", clip_name: "", project: None, env: None, working: filmcraft_color::WorkingSpace::Rec709 }
}

fn effect(id: &str, params: &[(&str, ParamValue)]) -> EffectInstance {
    let mut e = find_effect(id).unwrap_or_else(|| panic!("{id}")).instance();
    for (k, v) in params {
        e.params.get_mut(*k).unwrap_or_else(|| panic!("{id}.{k}")).value = v.clone();
    }
    e
}

fn fl(v: f64) -> ParamValue {
    ParamValue::Float(v)
}
fn col(r: f32, g: f32, b: f32) -> ParamValue {
    ParamValue::Color([r, g, b, 1.0])
}
fn ch(v: u32) -> ParamValue {
    ParamValue::Choice(v)
}
fn pt(x: f64, y: f64) -> ParamValue {
    ParamValue::Vec2(Vec2::new(x, y))
}

fn half(v: f32) -> f32 {
    super::blend_tests::f16_to_f32(f32_to_f16(v))
}

/// Linear premultiplied test picture, every value representable in half floats: hue sweep along
/// x, lightness along y, a grey ramp, black and white columns, super-whites (up to 1.6), and
/// alpha 1 / partial / 0 bands.
fn picture(w: u32, h: u32) -> Vec<f32> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / (w - 1) as f32;
            let fy = y as f32 / (h - 1) as f32;
            let c = match x % 11 {
                0 => [0.0, 0.0, 0.0],
                1 => [1.0, 1.0, 1.0],
                2 => [fy, fy, fy],
                3 => [1.6 * fy, 1.2 * fy, 0.3],
                _ => {
                    let rgb = filmcraft_color::hsl_to_rgb(fx, 0.25 + 0.75 * ((y % 5) as f32 / 4.0), 0.1 + 0.8 * fy);
                    rgb.map(filmcraft_color::srgb_to_linear)
                }
            };
            let a = match y % 7 {
                0 => 0.0,
                1 | 2 => 0.5 + 0.4 * fx,
                3 => 0.25,
                _ => 1.0,
            };
            px.extend_from_slice(&[c[0] * a, c[1] * a, c[2] * a, a].map(half));
        }
    }
    px
}

/// Max |cpu − gpu| (finite values required) and the share of values off by more than 0.02.
fn compare(cpu: &[f32], gpu: &[f32]) -> (f32, f64) {
    assert_eq!(cpu.len(), gpu.len());
    let mut worst = 0f32;
    let mut flips = 0usize;
    for (a, b) in cpu.iter().zip(gpu) {
        assert!(b.is_finite(), "non-finite GPU value");
        let d = (a - b).abs() / (1.0 + a.abs());
        if d > 0.02 {
            flips += 1;
        } else {
            worst = worst.max(d);
        }
    }
    (worst, flips as f64 / cpu.len() as f64)
}

/// Effect cases: (effect id, parameters, whether a threshold may flip pixels).
fn cases() -> Vec<(&'static str, Vec<(&'static str, ParamValue)>, bool)> {
    vec![
        ("brightness_contrast", vec![("brightness", fl(30.0)), ("contrast", fl(40.0))], false),
        ("brightness_contrast", vec![("brightness", fl(-100.0)), ("contrast", fl(-100.0))], false),
        ("brightness_contrast", vec![("brightness", fl(100.0)), ("contrast", fl(100.0))], false),
        ("brightness_contrast", vec![("brightness", fl(-20.0)), ("contrast", fl(900.0))], false),
        ("proc_amp", vec![("brightness", fl(10.0)), ("contrast", fl(130.0)), ("hue", fl(200.0)), ("saturation", fl(150.0))], false),
        ("proc_amp", vec![("hue", fl(-400.0)), ("saturation", fl(0.0))], false),
        ("tint", vec![], false),
        ("tint", vec![("black", col(0.2, 0.1, 0.5)), ("white", col(1.0, 0.9, 0.3)), ("amount", fl(60.0))], false),
        ("black_white", vec![], false),
        ("color_balance", vec![("shadow_r", fl(50.0)), ("mid_g", fl(-40.0)), ("hi_b", fl(80.0)), ("shadow_b", fl(-100.0)), ("hi_r", fl(100.0))], false),
        ("color_balance", vec![("mid_r", fl(70.0)), ("hi_g", fl(-60.0)), ("preserve", ParamValue::Bool(true))], false),
        ("leave_color", vec![("amount", fl(80.0)), ("tolerance", fl(20.0)), ("softness", fl(30.0))], true),
        ("leave_color", vec![("amount", fl(100.0)), ("color", col(0.1, 0.8, 0.2)), ("tolerance", fl(0.0)), ("softness", fl(0.0))], true),
        ("change_to_color", vec![], true),
        ("change_to_color", vec![("hue_tol", fl(30.0)), ("softness", fl(10.0)), ("to", col(0.9, 0.9, 0.1))], true),
        ("color_pass", vec![("similarity", fl(30.0))], true),
        ("color_pass", vec![("similarity", fl(45.0)), ("reverse", ParamValue::Bool(true)), ("color", col(0.2, 0.4, 0.9))], true),
        ("gamma_correction", vec![("gamma", fl(5.0))], false),
        ("gamma_correction", vec![("gamma", fl(28.0))], false),
        ("gamma_correction", vec![("gamma", fl(0.0))], false),
        ("gamma_correction", vec![("gamma", fl(-3.0))], false),
        ("levels", vec![("in_black", fl(20.0)), ("in_white", fl(230.0)), ("out_black", fl(10.0)), ("out_white", fl(240.0)), ("gamma", fl(150.0))], false),
        ("levels", vec![("in_black", fl(200.0)), ("in_white", fl(0.0)), ("gamma", fl(5.0)), ("out_white", fl(-50.0))], true),
        ("extract", vec![("black", fl(40.0)), ("white", fl(200.0)), ("softness", fl(30.0))], true),
        ("extract", vec![("black", fl(90.0)), ("white", fl(160.0)), ("invert", ParamValue::Bool(true))], true),
        ("invert", vec![("blend", fl(30.0))], false),
        ("invert", vec![("channel", ParamValue::Choice(1))], false),
        ("invert", vec![("channel", ParamValue::Choice(2)), ("blend", fl(70.0))], false),
        ("invert", vec![("channel", ParamValue::Choice(3))], false),
        ("invert", vec![("channel", ParamValue::Choice(4)), ("blend", fl(20.0))], false),
        ("invert", vec![("channel", ParamValue::Choice(9))], false),
        ("posterize", vec![("levels", fl(2.0))], true),
        ("posterize", vec![("levels", fl(7.0))], true),
        ("posterize", vec![("levels", fl(255.0))], true),
        ("gaussian_blur", vec![("blurriness", fl(0.0))], false),
        ("gaussian_blur", vec![("blurriness", fl(5.0))], false),
        ("gaussian_blur", vec![("blurriness", fl(5.0)), ("repeat_edge", ParamValue::Bool(true))], false),
        ("gaussian_blur", vec![("blurriness", fl(80.0))], false),
        ("gaussian_blur", vec![("blurriness", fl(80.0)), ("repeat_edge", ParamValue::Bool(true))], false),
        ("gaussian_blur", vec![("blurriness", fl(12.0)), ("dimensions", ParamValue::Choice(1))], false),
        ("gaussian_blur", vec![("blurriness", fl(12.0)), ("dimensions", ParamValue::Choice(2)), ("repeat_edge", ParamValue::Bool(true))], false),
        ("gaussian_blur", vec![("blurriness", fl(3000.0)), ("repeat_edge", ParamValue::Bool(true))], false),
        ("gaussian_blur", vec![("blurriness", fl(1e9))], false),
        ("gaussian_blur", vec![("blurriness", fl(-50.0))], false),
        ("camera_blur", vec![("percent", fl(20.0))], false),
        ("directional_blur", vec![("length", fl(10.0)), ("direction", fl(30.0))], false),
        ("directional_blur", vec![("length", fl(0.2))], false),
        ("directional_blur", vec![("length", fl(1000.0)), ("direction", fl(-100.0))], false),
        ("sharpen", vec![("amount", fl(80.0))], false),
        ("sharpen", vec![("amount", fl(4000.0))], false),
        ("unsharp_mask", vec![("amount", fl(150.0)), ("radius", fl(3.0)), ("threshold", fl(10.0))], true),
        ("unsharp_mask", vec![("radius", fl(0.1))], false),
        ("unsharp_mask", vec![("amount", fl(300.0)), ("radius", fl(250.0))], false),
        ("crop", vec![("left", fl(10.0)), ("top", fl(5.0)), ("right", fl(20.0)), ("bottom", fl(15.0))], false),
        ("crop", vec![("left", fl(10.0)), ("top", fl(5.0)), ("right", fl(20.0)), ("bottom", fl(15.0)), ("feather", fl(12.0))], false),
        ("crop", vec![("left", fl(12.0)), ("top", fl(5.0)), ("right", fl(20.0)), ("zoom", ParamValue::Bool(true))], false),
        ("crop", vec![("left", fl(70.0)), ("right", fl(70.0)), ("feather", fl(5000.0))], false),
        ("edge_feather", vec![("amount", fl(30.0))], false),
        ("transform", vec![("scale_height", fl(150.0)), ("rotation", fl(30.0)), ("position", pt(30.0, 20.0)), ("opacity", fl(60.0))], false),
        (
            "transform",
            vec![
                ("uniform_scale", ParamValue::Bool(false)),
                ("scale_width", fl(80.0)),
                ("scale_height", fl(120.0)),
                ("skew", fl(20.0)),
                ("skew_axis", fl(45.0)),
                ("anchor", pt(10.0, 30.0)),
            ],
            false,
        ),
        ("horizontal_flip", vec![], false),
        ("vertical_flip", vec![], false),
        ("mirror", vec![("center", pt(20.0, 10.0)), ("angle", fl(30.0))], false),
        ("mirror", vec![("angle", fl(200.0))], false),
        ("offset", vec![("shift", pt(10.0, 30.0)), ("blend", fl(25.0))], false),
        ("offset", vec![("shift", pt(-1e5, 7.25))], false),
        ("asc_cdl", vec![("r_slope", fl(1.4)), ("g_offset", fl(-0.1)), ("b_power", fl(2.2)), ("saturation", fl(1.6))], false),
        ("asc_cdl", vec![("r_power", fl(0.0)), ("g_slope", fl(0.0)), ("saturation", fl(0.0))], false),
        ("channel_mix", vec![("rr", fl(50.0)), ("rg", fl(60.0)), ("gb", fl(-80.0)), ("bc", fl(30.0))], false),
        ("channel_mix", vec![("rb", fl(120.0)), ("monochrome", ParamValue::Bool(true))], false),
        ("color_replace", vec![("similarity", fl(40.0))], true),
        ("color_replace", vec![("similarity", fl(25.0)), ("solid", ParamValue::Bool(true)), ("target", col(0.3, 0.6, 0.2))], true),
        ("color_replace", vec![("similarity", fl(0.0))], true),
        ("alpha_adjust", vec![("opacity", fl(60.0))], false),
        ("alpha_adjust", vec![("ignore", ParamValue::Bool(true)), ("invert", ParamValue::Bool(true))], false),
        ("alpha_adjust", vec![("invert", ParamValue::Bool(true)), ("mask_only", ParamValue::Bool(true)), ("opacity", fl(150.0))], false),
        ("mosaic", vec![], false),
        ("mosaic", vec![("horizontal", fl(7.0)), ("vertical", fl(3.0))], false),
        ("mosaic", vec![("horizontal", fl(1.0)), ("vertical", fl(4000.0))], false),
        ("mosaic", vec![("horizontal", fl(23.0)), ("vertical", fl(17.0))], false),
        ("vignette", vec![], false),
        ("vignette", vec![("amount", fl(-100.0)), ("midpoint", fl(30.0)), ("feather", fl(80.0)), ("color", col(0.2, 0.05, 0.4))], false),
        ("vignette", vec![("amount", fl(70.0)), ("roundness", fl(100.0)), ("feather", fl(0.0))], false),
        ("vignette", vec![("amount", fl(-60.0)), ("roundness", fl(-100.0)), ("midpoint", fl(90.0))], false),
        ("vignette", vec![("amount", fl(0.0))], false),
        ("lumetri", vec![], false),
        ("lumetri", vec![("exposure", fl(1.5)), ("contrast", fl(30.0)), ("temperature", fl(20.0)), ("tint", fl(-15.0))], false),
        ("lumetri", vec![("highlights", fl(40.0)), ("shadows", fl(-30.0)), ("whites", fl(20.0)), ("blacks", fl(-10.0)), ("saturation", fl(130.0))], false),
        ("lumetri", vec![("faded_film", fl(25.0)), ("vibrance", fl(40.0))], false),
        ("lumetri", vec![("vignette_amount", fl(-50.0)), ("vignette_midpoint", fl(40.0)), ("vignette_feather", fl(60.0))], false),
        // beyond Basic: split tone, Looks, color wheels, RGB and hue curves, Sharpen
        ("lumetri", vec![("sharpen", fl(60.0))], false),
        // keys
        ("ultra_key", vec![("key_color", col(0.1, 0.8, 0.2))], false),
        (
            "ultra_key",
            vec![("key_color", col(0.1, 0.3, 0.9)), ("tolerance", fl(70.0)), ("spill", fl(80.0)), ("desaturate", fl(50.0)), ("spill_luma", fl(60.0))],
            false,
        ),
        (
            "ultra_key",
            vec![("key_color", col(0.2, 0.8, 0.3)), ("choke", fl(40.0)), ("soften", fl(30.0)), ("contrast", fl(50.0)), ("mid_point", fl(40.0))],
            false,
        ),
        (
            "ultra_key",
            vec![
                ("key_color", col(0.9, 0.1, 0.1)),
                ("pedestal", fl(30.0)),
                ("transparency", fl(50.0)),
                ("highlight", fl(40.0)),
                ("shadow", fl(40.0)),
                ("setting", ParamValue::Choice(2)),
            ],
            false,
        ),
        (
            "ultra_key",
            vec![("key_color", col(0.1, 0.8, 0.2)), ("cc_hue", fl(90.0)), ("cc_saturation", fl(150.0)), ("cc_luminance", fl(80.0)), ("range", fl(100.0))],
            false,
        ),
        ("ultra_key", vec![("key_color", col(0.1, 0.8, 0.2)), ("soften", fl(20.0)), ("output", ParamValue::Choice(1))], false),
        ("ultra_key", vec![("key_color", col(0.1, 0.8, 0.2)), ("choke", fl(100.0)), ("output", ParamValue::Choice(2))], false),
        ("color_key", vec![("color", col(0.2, 0.7, 0.3)), ("tolerance", fl(60.0)), ("feather", fl(10.0))], false),
        ("color_key", vec![("color", col(1.0, 1.0, 1.0)), ("tolerance", fl(20.0))], false),
        ("luma_key", vec![("threshold", fl(60.0)), ("cutoff", fl(20.0))], false),
        ("luma_key", vec![("threshold", fl(10.0)), ("cutoff", fl(40.0))], false),
        // Video Limiter: every axis, compression knees, clip level and the gamut warning
        ("video_limiter", vec![], false),
        ("video_limiter", vec![("axis", ParamValue::Choice(0)), ("compression", ParamValue::Choice(4))], false),
        ("video_limiter", vec![("axis", ParamValue::Choice(1)), ("compression", ParamValue::Choice(0))], false),
        ("video_limiter", vec![("axis", ParamValue::Choice(2)), ("clip_level", ParamValue::Choice(5)), ("compression", ParamValue::Choice(3))], false),
        ("video_limiter", vec![("axis", ParamValue::Choice(3)), ("clip_level", ParamValue::Choice(9)), ("compression", ParamValue::Choice(0))], false),
        ("video_limiter", vec![("gamut_warning", ParamValue::Bool(true)), ("warning_color", col(0.0, 1.0, 0.5))], true),
        // HSL Secondary: key, denoise (median), blur, correction and the mask views
        (
            "lumetri",
            vec![("hsl_on", ParamValue::Bool(true)), ("hsl_hue", fl(30.0)), ("hsl_hue_range", fl(90.0)), ("hsl_sat", fl(160.0)), ("hsl_temp", fl(-30.0))],
            false,
        ),
        ("lumetri", vec![("hsl_on", ParamValue::Bool(true)), ("hsl_hue", fl(200.0)), ("hsl_hue_shift", fl(40.0)), ("hsl_denoise", fl(60.0))], false),
        ("lumetri", vec![("hsl_on", ParamValue::Bool(true)), ("hsl_hue", fl(120.0)), ("hsl_tint", fl(25.0)), ("hsl_blur", fl(30.0))], false),
        ("lumetri", vec![("hsl_on", ParamValue::Bool(true)), ("hsl_denoise", fl(100.0)), ("hsl_blur", fl(10.0)), ("hsl_sat", fl(0.0))], false),
        ("lumetri", vec![("hsl_on", ParamValue::Bool(true)), ("hsl_show_mask", ParamValue::Choice(1)), ("hsl_blur", fl(20.0))], false),
        ("lumetri", vec![("hsl_on", ParamValue::Bool(true)), ("hsl_show_mask", ParamValue::Choice(2))], false),
        ("lumetri", vec![("hsl_on", ParamValue::Bool(true)), ("hsl_show_mask", ParamValue::Choice(3)), ("hsl_denoise", fl(40.0))], false),
        (
            "lumetri",
            vec![
                ("hsl_on", ParamValue::Bool(true)),
                ("hsl_hue", fl(30.0)),
                ("exposure", fl(0.4)),
                ("curve_luma", curve(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]])),
                ("sharpen", fl(30.0)),
            ],
            false,
        ),
        ("lumetri", vec![("input_lut", ParamValue::Text(builtin_lut("-to-rec709")))], false),
        ("lumetri", vec![("look_lut", ParamValue::Text(builtin_lut("look-"))), ("look_intensity", fl(75.0)), ("contrast", fl(10.0))], false),
        ("lumetri", vec![("sharpen", fl(-40.0)), ("contrast", fl(15.0)), ("curve_luma", curve(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]))], false),
        ("lumetri", vec![("shadow_tint", col(0.3, 0.5, 0.8)), ("highlight_tint", col(0.8, 0.6, 0.4))], false),
        ("lumetri", vec![("look", ParamValue::Choice(1)), ("look_intensity", fl(80.0))], false),
        ("lumetri", vec![("look", ParamValue::Choice(4))], false),
        ("lumetri", vec![("look", ParamValue::Choice(6)), ("look_intensity", fl(50.0))], false),
        ("lumetri", vec![("look", ParamValue::Choice(8))], false),
        (
            "lumetri",
            vec![("wheel_shadows", pt(0.3, -0.2)), ("wheel_midtones", pt(-0.1, 0.4)), ("wheel_highlights", pt(0.5, 0.1)), ("wheel_midtones_l", fl(-20.0))],
            false,
        ),
        (
            "lumetri",
            vec![("curve_luma", curve(&[[0.0, 0.05], [0.3, 0.2], [0.7, 0.85], [1.0, 0.95]])), ("curve_red", curve(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]))],
            false,
        ),
        ("lumetri", vec![("hue_vs_sat", curve(&[[0.0, 0.5], [0.33, 0.8], [0.66, 0.2]])), ("hue_vs_hue", curve(&[[0.1, 0.55], [0.6, 0.45]]))], false),
        (
            "lumetri",
            vec![("hue_vs_luma", curve(&[[0.5, 0.7]])), ("luma_vs_sat", curve(&[[0.2, 0.3], [0.8, 0.6]])), ("sat_vs_sat", curve(&[[0.5, 0.4]]))],
            false,
        ),
        (
            "lumetri",
            vec![
                ("exposure", fl(0.5)),
                ("contrast", fl(20.0)),
                ("shadow_tint", col(0.4, 0.5, 0.6)),
                ("look", ParamValue::Choice(2)),
                ("wheel_highlights", pt(-0.3, 0.2)),
                ("curve_blue", curve(&[[0.0, 0.1], [1.0, 0.9]])),
                ("hue_vs_sat", curve(&[[0.0, 0.6], [0.5, 0.4]])),
                ("vignette_amount", fl(-30.0)),
            ],
            false,
        ),
        // GPU3 (`gpufx_ext`)
        ("mosaic_legacy", vec![("horizontal", fl(9.0)), ("vertical", fl(5.0))], false),
        ("noise", vec![("amount", fl(30.0))], false),
        ("noise", vec![("amount", fl(80.0)), ("color", ParamValue::Bool(false)), ("clip", ParamValue::Bool(false))], false),
        ("noise", vec![("amount", fl(50.0)), ("grain_size", fl(3.0))], false),
        ("noise_legacy", vec![("amount", fl(40.0))], false),
        ("drop_shadow", vec![], false),
        (
            "drop_shadow",
            vec![("softness", fl(20.0)), ("distance", fl(12.0)), ("direction", fl(200.0)), ("color", col(0.2, 0.1, 0.6)), ("opacity", fl(80.0))],
            false,
        ),
        ("drop_shadow", vec![("only", ParamValue::Bool(true)), ("softness", fl(120.0)), ("distance", fl(7.5))], false),
        ("drop_shadow", vec![("distance", fl(500.0))], false),
        ("corner_pin", vec![("upper_left", pt(5.0, 3.0)), ("lower_right", pt(60.0, 38.0)), ("upper_right", pt(70.0, -4.0))], true),
        ("corner_pin", vec![("lower_left", pt(20.0, 30.0)), ("upper_right", pt(40.0, 10.0))], true),
        ("corner_pin", vec![], false),
        ("lens_distortion", vec![("curvature", fl(40.0))], false),
        ("lens_distortion", vec![("curvature", fl(-60.0)), ("h_decentering", fl(30.0)), ("v_decentering", fl(-20.0))], false),
        ("twirl", vec![("angle", fl(120.0)), ("radius", fl(60.0))], false),
        ("twirl_legacy", vec![("center", pt(20.0, 15.0))], false),
        ("wave_warp", vec![], false),
        ("wave_warp", vec![("height", fl(-6.0)), ("width", fl(9.0)), ("direction", fl(30.0))], false),
        ("spherize", vec![("radius", fl(25.0)), ("center", pt(30.0, 20.0))], false),
        ("spherize", vec![("radius", fl(300.0))], false),
        ("basic_3d", vec![("swivel", fl(30.0)), ("tilt", fl(-20.0)), ("distance", fl(5.0))], false),
        ("basic_3d", vec![("swivel", fl(80.0)), ("tilt", fl(10.0)), ("distance", fl(-3.0))], false),
        ("magnify", vec![("size", fl(15.0)), ("magnification", fl(300.0))], false),
        (
            "magnify",
            vec![("size", fl(18.0)), ("feather", fl(6.0)), ("shape", ParamValue::Choice(1)), ("mode", ParamValue::Choice(2)), ("opacity", fl(70.0))],
            false,
        ),
        ("magnify_legacy", vec![("size", fl(20.0)), ("mode", ParamValue::Choice(4)), ("center", pt(20.0, 30.0))], false),
        ("find_edges", vec![], false),
        ("find_edges", vec![("invert", ParamValue::Bool(true)), ("blend", fl(30.0))], false),
        ("emboss", vec![], false),
        ("emboss", vec![("direction", fl(100.0)), ("relief", fl(4.0)), ("contrast", fl(300.0)), ("blend", fl(20.0))], false),
        ("bevel_alpha", vec![], false),
        ("bevel_alpha", vec![("thickness", fl(5.0)), ("angle", fl(30.0)), ("intensity", fl(90.0))], false),
        ("replicate", vec![], false),
        ("replicate", vec![("count", fl(5.0))], false),
        ("strobe", vec![], false),
        ("strobe", vec![("color", col(0.9, 0.2, 0.1)), ("blend", fl(40.0))], false),
        ("ramp", vec![], false),
        ("ramp", vec![("shape", ParamValue::Choice(1)), ("blend", fl(30.0)), ("start_color", col(0.9, 0.3, 0.1)), ("end", pt(10.0, 40.0))], false),
        ("four_color_gradient", vec![], false),
        ("four_color_gradient", vec![("blend", fl(400.0)), ("opacity", fl(50.0))], false),
        ("circle", vec![], false),
        ("circle", vec![("radius", fl(15.0)), ("opacity", fl(60.0)), ("color", col(0.1, 0.7, 0.3))], false),
        ("grid", vec![], false),
        ("grid", vec![("size", fl(7.0)), ("border", fl(2.5)), ("opacity", fl(80.0))], false),
        // GPU4 (`gpufx_more`; Lens Flare's ghost discs and Roughen Edges' Cut / Photocopy have
        // hard edges a pixel may flip on)
        ("lens_flare", vec![], true),
        ("lens_flare", vec![("lens_type", ch(1)), ("center", pt(20.0, 12.0)), ("ghosts", fl(8.0)), ("brightness", fl(150.0))], true),
        ("lens_flare", vec![("lens_type", ch(3)), ("size", fl(40.0)), ("color", col(0.4, 0.7, 1.0)), ("ghosts", fl(0.0))], true),
        ("rgb_split", vec![], false),
        ("rgb_split", vec![("mode", ch(1)), ("amount", fl(25.0)), ("blend", fl(20.0)), ("center", pt(30.0, 15.0))], false),
        ("rgb_split", vec![("amount", fl(7.3)), ("angle", fl(130.0))], false),
        ("lighting_effects", vec![], false),
        (
            "lighting_effects",
            vec![
                ("l1_type", ch(2)),
                ("l2_type", ch(1)),
                ("l2_angle", fl(60.0)),
                ("l2_color", col(0.9, 0.5, 0.2)),
                ("bump_height", fl(40.0)),
                ("gloss", fl(50.0)),
                ("material", fl(30.0)),
                ("exposure", fl(20.0)),
            ],
            false,
        ),
        (
            "lighting_effects",
            vec![
                ("l1_center", pt(10.0, 30.0)),
                ("l1_focus", fl(80.0)),
                ("l1_angle", fl(35.0)),
                ("l1_minor", fl(45.0)),
                ("l3_type", ch(3)),
                ("l3_center", pt(50.0, 10.0)),
                ("white_high", ParamValue::Bool(false)),
                ("bump_height", fl(-70.0)),
            ],
            false,
        ),
        ("color_emboss", vec![], false),
        ("color_emboss", vec![("direction", fl(200.0)), ("relief", fl(3.5)), ("contrast", fl(250.0)), ("blend", fl(30.0))], false),
        ("rounded_crop", vec![("left", fl(10.0)), ("top", fl(15.0)), ("right", fl(5.0)), ("bottom", fl(20.0)), ("radius", fl(9.0))], false),
        ("rounded_crop", vec![("left", fl(5.0)), ("radius", fl(14.0)), ("feather", fl(4.0)), ("border", fl(3.0)), ("border_color", col(0.9, 0.2, 0.3))], false),
        ("gradient", vec![], false),
        ("gradient", vec![("shape", ch(1)), ("midpoint", fl(30.0)), ("blend", fl(25.0)), ("start", pt(10.0, 10.0)), ("end", pt(50.0, 35.0))], false),
        ("gradient", vec![("shape", ch(3)), ("scatter", fl(60.0))], false),
        ("gradient", vec![("shape", ch(2)), ("start_color", ParamValue::Color([0.9, 0.3, 0.1, 0.5]))], false),
        ("light_leaks", vec![], false),
        ("light_leaks", vec![("mode", ch(1)), ("scale", fl(40.0)), ("seed", fl(7.0))], false),
        ("light_leaks", vec![("mode", ch(2)), ("direction", fl(250.0)), ("intensity", fl(100.0))], false),
        ("turbulent_displace", vec![("size", fl(20.0)), ("amount", fl(30.0)), ("pinning", ch(0))], false),
        ("turbulent_displace", vec![("displacement", ch(1)), ("size", fl(15.0)), ("complexity", fl(3.5))], false),
        ("turbulent_displace", vec![("displacement", ch(5)), ("evolution", fl(200.0)), ("seed", fl(3.0)), ("pinning", ch(2))], false),
        ("turbulent_displace", vec![("displacement", ch(8)), ("cycle", ParamValue::Bool(true)), ("evolution", fl(500.0)), ("pinning", ch(3))], false),
        ("turbulent_displace", vec![("displacement", ch(6)), ("amount", fl(-80.0)), ("offset", pt(5.0, 9.0)), ("complexity", fl(10.0))], false),
        ("median", vec![("radius", fl(1.0))], false),
        ("median", vec![("radius", fl(4.0))], false),
        ("channel_blur", vec![("red", fl(6.0)), ("blue", fl(40.0)), ("alpha", fl(6.0))], false),
        ("channel_blur", vec![("green", fl(150.0)), ("repeat_edge", ParamValue::Bool(true)), ("dimensions", ch(1))], false),
        ("alpha_glow", vec![], false),
        (
            "alpha_glow",
            vec![("glow", fl(60.0)), ("use_end", ParamValue::Bool(true)), ("end_color", col(0.9, 0.2, 0.1)), ("fade_out", ParamValue::Bool(false))],
            false,
        ),
        ("edge_glow", vec![], false),
        ("edge_glow", vec![("width", fl(0.5)), ("radius", fl(30.0)), ("only", ParamValue::Bool(true)), ("threshold", fl(5.0))], false),
        ("roughen_edges", vec![], true),
        ("roughen_edges", vec![("edge_type", ch(5)), ("border", fl(12.0)), ("complexity", fl(4.0)), ("stretch", fl(1.0))], true),
        ("roughen_edges", vec![("edge_type", ch(6)), ("evolution", fl(90.0)), ("offset", pt(13.0, -7.0))], true),
        ("roughen_edges", vec![("edge_type", ch(2)), ("scale", fl(40.0))], true),
        ("reduce_interlace_flicker", vec![("softness", fl(3.0))], false),
    ]
}

/// The id (uiltin:…) of the first built-in LUT whose id contains part.
fn builtin_lut(part: &str) -> String {
    let b = filmcraft_render::luts::builtins().iter().find(|b| b.id.contains(part)).unwrap_or_else(|| panic!("no built-in LUT {part}"));
    format!("builtin:{}", b.id)
}

fn curve(points: &[[f32; 2]]) -> ParamValue {
    ParamValue::Curve(points.to_vec())
}

/// Every effect case on the GPU against its CPU reference, on the working image read back as f32.
#[test]
fn gpu_effects_match_cpu_exactly() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (67u32, 41u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let mut failures = Vec::new();
    for (id, params, steps) in cases() {
        let e = effect(id, &params);
        let op = FxOp::eval(&e, &cx(Tick::ZERO, 1.0), w as usize, h as usize).unwrap_or_else(|| panic!("{id} has no GPU op"));
        assert!(op.gpu_ok(), "{id} {params:?}: not GPU-capable");
        let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
        op.apply(&mut cpu);
        let fx = LayerFx { size: (w, h), decimation: 1, ops: vec![op], ..Default::default() };
        let (_, _, gpu) = c.effect_image(&frame, &fx).expect("effect image");

        let (worst, flips) = compare(&cpu.px, &gpu);
        eprintln!("{id} {params:?}: max rel diff {worst:.2e}, {:.3}% flipped", flips * 100.0);
        if worst > tolerance(id) || flips > if steps { FLIPS } else { 0.0 } {
            failures.push(format!("{id} {params:?}: {worst:.2e}, flips {flips:.4}"));
        }
    }
    assert!(failures.is_empty(), "GPU effects differ from the CPU:\n{}", failures.join("\n"));
}

/// Chains run in order (and Unsharp keeps its original while blurring, between other blurs).
#[test]
fn gpu_effect_chains_match_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (96u32, 54u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let chains: Vec<Vec<EffectInstance>> = vec![
        vec![
            effect("brightness_contrast", &[("brightness", fl(20.0)), ("contrast", fl(30.0))]),
            effect("gaussian_blur", &[("blurriness", fl(8.0))]),
            effect("tint", &[("amount", fl(50.0))]),
        ],
        vec![
            effect("gaussian_blur", &[("blurriness", fl(90.0)), ("repeat_edge", ParamValue::Bool(true))]),
            effect("unsharp_mask", &[("amount", fl(200.0)), ("radius", fl(4.0))]),
            effect("gaussian_blur", &[("blurriness", fl(3.0)), ("dimensions", ParamValue::Choice(1))]),
            effect("sharpen", &[("amount", fl(50.0))]),
        ],
        vec![
            effect("crop", &[("left", fl(10.0)), ("bottom", fl(20.0)), ("feather", fl(4.0))]),
            effect("transform", &[("rotation", fl(-15.0)), ("scale_height", fl(110.0))]),
            effect("horizontal_flip", &[]),
            effect("levels", &[("in_black", fl(15.0)), ("gamma", fl(80.0))]),
            effect("offset", &[("shift", pt(70.0, 10.0))]),
            effect("invert", &[("channel", ParamValue::Choice(4))]),
            effect("directional_blur", &[("length", fl(6.0)), ("direction", fl(90.0))]),
        ],
    ];
    for chain in chains {
        let ops: Vec<FxOp> = chain.iter().filter_map(|e| FxOp::eval(e, &cx(Tick::ZERO, 1.0), w as usize, h as usize)).collect();
        assert!(ops.iter().all(FxOp::gpu_ok));
        let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
        for op in &ops {
            op.apply(&mut cpu);
        }
        let names: Vec<&str> = chain.iter().map(|e| e.effect.as_str()).collect();
        let (_, _, gpu) = c.effect_image(&frame, &LayerFx { size: (w, h), decimation: 1, ops, ..Default::default() }).expect("effect image");
        let (worst, flips) = compare(&cpu.px, &gpu);
        eprintln!("{names:?}: max rel diff {worst:.2e}, {:.3}% flipped", flips * 100.0);
        assert!(worst < EXACT * 2.0 && flips == 0.0, "{names:?}: {worst}, {flips}");
    }
}

/// Effects limited by masks, and Opacity masks, against `LayerFx::apply`: the mask pass keeps the
/// image before the effect (also while Unsharp keeps its own original) and mixes by the coverage.
#[test]
fn masked_effects_match_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (320u32, 180u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let all = crate::tests::test_masks(1.0);
    let one = || all.iter().take(1).cloned().collect::<Vec<_>>();
    let chain = [
        effect("gaussian_blur", &[("blurriness", fl(40.0))]),
        effect("tint", &[("amount", fl(80.0))]),
        effect("unsharp_mask", &[("amount", fl(200.0)), ("radius", fl(4.0))]),
        // no passes: its masks must not start a mix
        effect("gaussian_blur", &[("blurriness", fl(0.0))]),
        effect("invert", &[]),
        // keeps the layer beside its shadow while the mask keeps the image before it
        effect("drop_shadow", &[("distance", fl(15.0)), ("softness", fl(25.0))]),
    ];
    let ops: Vec<FxOp> = chain.iter().filter_map(|e| FxOp::eval(e, &cx(Tick::ZERO, 1.0), w as usize, h as usize)).collect();
    assert!(ops.len() == chain.len() && ops.iter().all(FxOp::gpu_ok));
    let unmasked = LayerFx { size: (w, h), decimation: 1, ops: ops.clone(), ..Default::default() };
    let cases = [
        ("every effect", LayerFx { masks: vec![all.clone(), one(), all.clone(), one(), all.clone(), one()], ..unmasked.clone() }),
        ("some effects and opacity", LayerFx { masks: vec![one(), Vec::new(), all.clone()], opacity_masks: all.clone(), ..unmasked.clone() }),
        ("opacity only", LayerFx { ops: Vec::new(), opacity_masks: one(), ..unmasked.clone() }),
        ("blur only", LayerFx { ops: ops.iter().take(1).cloned().collect(), masks: vec![all.clone()], ..unmasked.clone() }),
    ];
    let mut plain = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
    unmasked.apply(&mut plain);
    for (name, fx) in cases {
        let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
        fx.apply(&mut cpu);
        assert!(cpu.px != plain.px && cpu.px != px, "{name}: the masks change the picture");
        let (_, _, gpu) = c.effect_image(&frame, &fx).expect("effect image");
        let (worst, flips) = compare(&cpu.px, &gpu);
        eprintln!("masked, {name}: max rel diff {worst:.2e}, {:.3}% flipped", flips * 100.0);
        assert!(worst < 5e-4 && flips == 0.0, "{name}: {worst}, {flips}");
    }
    // a mask without points covers nothing (or everything, inverted): never an out-of-range read
    let mut empty = all.iter().take(1).cloned().collect::<Vec<_>>();
    empty.iter_mut().for_each(|m| m.pts.clear());
    let fx = LayerFx { masks: vec![empty], ..unmasked.clone() };
    let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
    fx.apply(&mut cpu);
    let (_, _, gpu) = c.effect_image(&frame, &fx).expect("effect image");
    let (worst, flips) = compare(&cpu.px, &gpu);
    assert!(worst < 5e-4 && flips == 0.0, "empty mask: {worst}, {flips}");
}

/// Keyframed parameters are evaluated at the layer's time on the CPU: the GPU result follows them.
#[test]
fn keyframed_effects_follow_time() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (48u32, 32u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let mut blur = effect("gaussian_blur", &[]);
    let mut bc = effect("brightness_contrast", &[]);
    let p = blur.params.get_mut("blurriness").expect("blurriness");
    p.put_keyframe(Tick::ZERO, fl(0.0));
    p.put_keyframe(Tick::from_seconds_f64(1.0), fl(30.0));
    let p = bc.params.get_mut("brightness").expect("brightness");
    p.put_keyframe(Tick::ZERO, fl(-50.0));
    p.put_keyframe(Tick::from_seconds_f64(1.0), fl(60.0));
    let mut last: Option<Vec<f32>> = None;
    for s in [0.0, 0.3, 0.75, 1.0] {
        let cx = cx(Tick::from_seconds_f64(s), 1.0);
        let ops: Vec<FxOp> = [&bc, &blur].iter().filter_map(|e| FxOp::eval(e, &cx, w as usize, h as usize)).collect();
        let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
        for op in &ops {
            op.apply(&mut cpu);
        }
        let (_, _, gpu) = c.effect_image(&frame, &LayerFx { size: (w, h), decimation: 1, ops, ..Default::default() }).expect("effect image");
        let (worst, flips) = compare(&cpu.px, &gpu);
        assert!(worst < EXACT && flips == 0.0, "t = {s}: {worst}, {flips}");
        assert_ne!(last.as_ref(), Some(&gpu), "t = {s}: the keyframes change the picture");
        last = Some(gpu);
    }
}

/// Hostile values: non-finite parameters stay on the CPU (`gpu_ok` is false), huge radii and
/// sizes are capped as on the CPU and run in bounded time on the GPU.
#[test]
fn hostile_parameters_are_bounded() {
    let (w, h) = (40usize, 24usize);
    let nan = [
        effect("brightness_contrast", &[("brightness", fl(f64::NAN))]),
        effect("gaussian_blur", &[("blurriness", fl(f64::INFINITY))]),
        effect("directional_blur", &[("length", fl(f64::NAN))]),
        effect("transform", &[("rotation", fl(f64::NAN))]),
        effect("crop", &[("left", fl(f64::NAN))]),
        effect("offset", &[("shift", pt(f64::INFINITY, 0.0))]),
        effect("levels", &[("in_black", fl(f64::NEG_INFINITY))]),
        effect("transform", &[("scale_height", fl(1e300))]),
        // minifying resamples (the CPU's mip path) stay on the CPU
        effect("transform", &[("scale_height", fl(20.0))]),
        effect("crop", &[("left", fl(-300.0)), ("zoom", ParamValue::Bool(true))]),
    ];
    for e in &nan {
        let op = FxOp::eval(e, &cx(Tick::ZERO, 1.0), w, h);
        // gaussian with an infinite radius is capped (no blur), which the GPU runs
        if e.effect == "gaussian_blur" {
            assert!(op.as_ref().is_some_and(FxOp::gpu_ok));
            continue;
        }
        assert!(op.as_ref().is_some_and(|o| !o.gpu_ok()), "{}: {op:?}", e.effect);
        // and the CPU reference copes
        let mut img = filmcraft_render::Image::filled(w, h, [0.3, 0.2, 0.1, 1.0]);
        if let Some(op) = op {
            op.apply(&mut img);
        }
    }
    // huge radii: box radii capped by the image size
    let op = FxOp::eval(&effect("gaussian_blur", &[("blurriness", fl(1e30))]), &cx(Tick::ZERO, 1.0), w, h);
    let Some(FxOp::Gaussian { rx, ry, .. }) = op else { panic!("{op:?}") };
    assert!(rx.iter().chain(&ry).all(|r| *r as usize <= 4 * w.max(h).max(8)), "{rx:?} {ry:?}");
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let frame = VideoFrame::rgba_f32(w as u32, h as u32, picture(w as u32, h as u32));
    let ops = vec![
        FxOp::eval(&effect("gaussian_blur", &[("blurriness", fl(1e30))]), &cx(Tick::ZERO, 1.0), w, h).expect("op"),
        FxOp::eval(&effect("unsharp_mask", &[("radius", fl(1e30)), ("amount", fl(1e30))]), &cx(Tick::ZERO, 1.0), w, h).expect("op"),
        FxOp::eval(&effect("directional_blur", &[("length", fl(1e30))]), &cx(Tick::ZERO, 1.0), w, h).expect("op"),
    ];
    let t0 = std::time::Instant::now();
    let (_, _, gpu) = c.effect_image(&frame, &LayerFx { size: (w as u32, h as u32), decimation: 1, ops, ..Default::default() }).expect("effect image");
    assert!(t0.elapsed().as_secs() < 30);
    assert_eq!(gpu.len(), w * h * 4);
    // a working image larger than any texture is refused, not attempted
    assert!(c.effect_image(&frame, &LayerFx { size: (1 << 20, 4), decimation: 1, ops: vec![FxOp::BlackWhite], ..Default::default() }).is_none());
}

/// Layers with effect chains in a composited plan — YUV and RGBA sources, a decimated working
/// image, rotated / scaled placement, partial opacity and alpha, blend modes — match the CPU plan
/// executor.
#[test]
fn gpu_effect_layers_match_cpu_plan() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (320usize, 180usize);
    let ops = |list: &[EffectInstance], lw: u32, lh: u32, px_scale: f32| -> Vec<FxOp> {
        list.iter().filter_map(|e| FxOp::eval(e, &cx(Tick::ZERO, px_scale), lw as usize, lh as usize)).collect()
    };
    let chain_a = [
        effect("brightness_contrast", &[("brightness", fl(15.0)), ("contrast", fl(25.0))]),
        effect("gaussian_blur", &[("blurriness", fl(6.0))]),
        effect("tint", &[("amount", fl(40.0))]),
    ];
    let chain_b = [
        effect("crop", &[("left", fl(8.0)), ("right", fl(12.0)), ("feather", fl(6.0))]),
        effect("unsharp_mask", &[("amount", fl(120.0)), ("radius", fl(2.0))]),
        effect("color_balance", &[("hi_b", fl(60.0)), ("shadow_r", fl(40.0))]),
    ];
    let chain_c = [effect("mirror", &[("angle", fl(90.0))]), effect("gaussian_blur", &[("blurriness", fl(50.0)), ("dimensions", ParamValue::Choice(1))])];
    let ramp = ramp_layer(160, 120, 0);
    for mode in [Blend::Normal, Blend::Screen, Blend::Multiply, Blend::Overlay] {
        let plan = FramePlan::Layers {
            color: Default::default(),
            width: w,
            height: h,
            layers: vec![
                // YUV source decoded at half size (decimation 2) with effects, filling the output
                PlanLayer {
                    frame: yuv_frame(640, 360),
                    matrix: Affine::IDENTITY,
                    opacity: 1.0,
                    blend: Blend::Normal,
                    fx: Some(Arc::new(LayerFx { size: (320, 180), decimation: 2, ops: ops(&chain_c, 320, 180, 0.5), ..Default::default() })),
                    is_adjustment: false,
                },
                PlanLayer {
                    frame: ramp.clone(),
                    matrix: Affine::motion(Vec2::new(170.0, 95.0), Vec2::new(1.1, 1.1), 17.0, Vec2::new(80.0, 60.0)),
                    opacity: 0.8,
                    blend: mode,
                    fx: Some(Arc::new(LayerFx { size: (160, 120), decimation: 1, ops: ops(&chain_a, 160, 120, 1.0), ..Default::default() })),
                    is_adjustment: false,
                },
                PlanLayer::new(ramp.clone(), Affine::translate(10.0, 20.0), 0.5, Blend::Normal),
                PlanLayer {
                    frame: yuv_frame(320, 180),
                    matrix: Affine::motion(Vec2::new(250.0, 70.0), Vec2::new(0.45, 0.45), -9.0, Vec2::new(160.0, 90.0)),
                    opacity: 0.7,
                    blend: mode,
                    fx: Some(Arc::new(LayerFx { size: (320, 180), decimation: 1, ops: ops(&chain_b, 320, 180, 1.0), ..Default::default() })),
                    is_adjustment: false,
                },
            ],
        };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let keep = interior(&plan);
        let (p99, mean, max) = stats8(&cpu, &gpu, &keep);
        let (ap99, amean, _) = stats8(&cpu, &gpu, &vec![true; keep.len()]);
        eprintln!("{mode:?}: interior 8-bit p99 {p99} mean {mean:.3} max {max}; with edges p99 {ap99} mean {amean:.3}");
        assert!(p99 <= 6 && mean < 1.5, "{mode:?}: p99 {p99}, mean {mean}");
    }
}

/// Plans without effects never create working textures.
#[test]
fn plain_layers_skip_the_effect_stage() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let plan = FramePlan::Layers {
        color: Default::default(),
        width: 64,
        height: 36,
        layers: vec![PlanLayer::new(yuv_frame(64, 36), Affine::IDENTITY, 1.0, Blend::Normal)],
    };
    c.composite(&plan);
    assert!(c.fx.as_ref().is_some_and(fx::FxStage::is_idle));
    let fx = LayerFx { size: (64, 36), decimation: 1, ops: vec![FxOp::BlackWhite], ..Default::default() };
    let with = FramePlan::Layers {
        color: Default::default(),
        width: 64,
        height: 36,
        layers: vec![PlanLayer { fx: Some(Arc::new(fx)), ..PlanLayer::new(yuv_frame(64, 36), Affine::IDENTITY, 1.0, Blend::Normal) }],
    };
    c.composite(&with);
    assert!(!c.fx.as_ref().is_some_and(fx::FxStage::is_idle));
    // and they are released once no layer needs them
    c.composite(&plan);
    assert!(c.fx.as_ref().is_some_and(fx::FxStage::is_idle));
}

/// Without an effect stage (devices without compute shaders) the compositor renders a layer's
/// effects on the CPU itself: the picture is the same.
#[test]
fn layers_with_effects_fall_back_to_the_cpu_without_a_stage() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let ops: Vec<FxOp> = [effect("tint", &[("amount", fl(70.0))]), effect("gaussian_blur", &[("blurriness", fl(10.0))])]
        .iter()
        .filter_map(|e| FxOp::eval(e, &cx(Tick::ZERO, 1.0), 160, 120))
        .collect();
    let plan = FramePlan::Layers {
        color: Default::default(),
        width: 200,
        height: 150,
        layers: vec![
            PlanLayer::new(yuv_frame(200, 150), Affine::IDENTITY, 1.0, Blend::Normal),
            PlanLayer {
                fx: Some(Arc::new(LayerFx { size: (160, 120), decimation: 1, ops, ..Default::default() })),
                is_adjustment: false,
                ..PlanLayer::new(
                    ramp_layer(160, 120, 2),
                    Affine::motion(Vec2::new(100.0, 75.0), Vec2::new(0.9, 0.9), 8.0, Vec2::new(80.0, 60.0)),
                    0.8,
                    Blend::Screen,
                )
            },
        ],
    };
    let cpu = execute_cpu(&plan).over_black_rgba8();
    for stage in [true, false] {
        let mut c = GpuCompositor::new(&dev, &q);
        if !stage {
            c.fx = None;
        }
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let (p99, mean, _) = stats8(&cpu, &gpu, &interior(&plan));
        eprintln!("effect stage {stage}: interior p99 {p99}, mean {mean:.3}");
        assert!(p99 <= 6 && mean < 1.5, "stage {stage}: p99 {p99}, mean {mean}");
    }
}

/// Mosaic and Vignette: the reference the GPU is held to ([`FxOp::apply`]) is what the CPU
/// renderer draws for the effect (`filmcraft_render::effects::apply`), value for value; and the
/// case the GPU does not take (Mosaic with Softness) is declined.
#[test]
fn mosaic_and_vignette_references_are_the_cpu_effects() {
    let (w, h) = (67usize, 41usize);
    let px = picture(w as u32, h as u32);
    let mut n = 0;
    for (id, params, _) in cases().into_iter().filter(|c| matches!(c.0, "mosaic" | "vignette")) {
        let e = effect(id, &params);
        let c = cx(Tick::ZERO, 1.0);
        let op = FxOp::eval(&e, &c, w, h).unwrap_or_else(|| panic!("{id} has no GPU op"));
        let mut reference = filmcraft_render::Image { w, h, px: px.clone() };
        op.apply(&mut reference);
        let mut real = filmcraft_render::Image { w, h, px: px.clone() };
        filmcraft_render::effects::apply(&mut real, &e, &c);
        assert_eq!(reference.px, real.px, "{id} {params:?}");
        if !matches!(params.first(), Some((_, ParamValue::Float(v))) if *v == 0.0) {
            assert_ne!(real.px, px, "{id} {params:?} changes the picture");
        }
        n += 1;
    }
    assert_eq!(n, 9);
    let soft = effect("mosaic", &[("softness", fl(40.0))]);
    assert!(FxOp::eval(&soft, &cx(Tick::ZERO, 1.0), w, h).is_none(), "Mosaic with Softness stays on the CPU");
    // hostile sizes: block counts beyond the picture, a 1-pixel picture
    for (bx, by, w, h) in [(1e30, 1e30, 8usize, 8usize), (0.0, -5.0, 8, 8), (4.0, 4.0, 1, 1), (f64::NAN, 2.0, 5, 3)] {
        let e = effect("mosaic", &[("horizontal", fl(bx)), ("vertical", fl(by))]);
        if let Some(op) = FxOp::eval(&e, &cx(Tick::ZERO, 1.0), w, h) {
            let mut img = filmcraft_render::Image { w, h, px: vec![0.5; w * h * 4] };
            op.apply(&mut img);
            assert!(img.px.iter().all(|v| v.is_finite()), "{bx} x {by} blocks on {w}x{h}");
        }
    }
}

/// A project LUT with a 1D shaper and a 3D cube (non-unit domains), as Input and Look LUT: the
/// GPU Lumetri pass against its CPU twin.
#[test]
fn lumetri_shaper_and_cube_luts_on_the_gpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use filmcraft_color::{Lut, Lut1d, Lut3d};
    let shaper = Lut1d { domain_min: [0.0; 3], domain_max: [1.0; 3], data: (0..9).map(|i| [((i as f32) / 8.0).sqrt(); 3]).collect() };
    let mut cube = Lut3d::from_fn(9, |c| [c[1] * 0.9 + 0.05, c[2], c[0] * 0.8 + 0.1]);
    cube.domain_max = [1.2; 3];
    let lut = Lut { title: "t".into(), shaper: Some(shaper), cube: Some(cube) };
    let mut p = filmcraft_project::Project::new("t");
    p.luts.push(filmcraft_project::ProjectLut { id: "s".into(), name: "s".into(), source_path: None, format: "cube".into(), text: lut.to_cube().into() });
    let p: &'static filmcraft_project::Project = Box::leak(Box::new(p));
    let cx = FxCtx { project: Some(p), ..cx(Tick::ZERO, 1.0) };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (67u32, 41u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    for params in [vec![("input_lut", ParamValue::Text("lib:s".into()))], vec![("look_lut", ParamValue::Text("lib:s".into())), ("look_intensity", fl(100.0))]] {
        let e = effect("lumetri", &params);
        let op = FxOp::eval(&e, &cx, w as usize, h as usize).expect("GPU op");
        assert!(op.gpu_ok());
        let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
        op.apply(&mut cpu);
        let fx = LayerFx { size: (w, h), decimation: 1, ops: vec![op], ..Default::default() };
        let (_, _, gpu) = c.effect_image(&frame, &fx).expect("effect image");
        let (worst, flips) = compare(&cpu.px, &gpu);
        assert!(worst <= TRANSCENDENTAL && flips == 0.0, "{params:?}: {worst:.2e}, flips {flips}");
    }
}

/// Lumetri in the HDR working spaces (PQ, HLG): the grading signals of Basic and of the curves,
/// contrast and HDR Specular around HDR White, Looks / LUTs / hue curves scaled into 0…1, curves
/// continuing above HDR White, and HSL Secondary, on an image with highlights up to ~2000 cd/m².
#[test]
fn lumetri_hdr_on_the_gpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use filmcraft_color::{Lut, Lut3d, WorkingSpace};
    let cube = Lut3d::from_fn(9, |c| [c[1] * 0.9 + 0.05, c[2], c[0] * 0.8 + 0.1]);
    let lut = Lut { title: "t".into(), shaper: None, cube: Some(cube) };
    let mut p = filmcraft_project::Project::new("t");
    p.luts.push(filmcraft_project::ProjectLut { id: "s".into(), name: "s".into(), source_path: None, format: "cube".into(), text: lut.to_cube().into() });
    let p: &'static filmcraft_project::Project = Box::leak(Box::new(p));
    let curve = |pts: &[[f32; 2]]| ParamValue::Curve(pts.to_vec());
    let cases: Vec<Vec<(&str, ParamValue)>> = vec![
        vec![],
        vec![("exposure", fl(0.7)), ("contrast", fl(40.0)), ("hdr_specular", fl(-50.0)), ("hdr_white", fl(600.0)), ("highlights", fl(-30.0))],
        vec![("contrast", fl(-30.0)), ("hdr_specular", fl(80.0)), ("whites", fl(20.0)), ("blacks", fl(-15.0)), ("saturation", fl(130.0))],
        vec![("curve_luma", curve(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]])), ("curve_red", curve(&[[0.0, 0.1], [1.0, 0.9]])), ("curves_hdr_range", fl(2000.0))],
        vec![("look", ParamValue::Choice(1)), ("look_intensity", fl(80.0)), ("hue_vs_luma", curve(&[[0.6, 0.7]])), ("curves_hdr_range", fl(400.0))],
        vec![
            ("wheel_highlights", ParamValue::Vec2(Vec2::new(0.4, 0.1))),
            ("hue_vs_sat", curve(&[[0.0, 0.5], [0.33, 0.8]])),
            ("shadow_tint", col(0.3, 0.5, 0.8)),
        ],
        vec![("input_lut", ParamValue::Text("lib:s".into())), ("look_lut", ParamValue::Text("lib:s".into())), ("look_intensity", fl(60.0))],
        vec![("hsl_on", ParamValue::Bool(true)), ("hsl_hue", fl(200.0)), ("hsl_sat", fl(150.0)), ("hsl_hue_shift", fl(30.0)), ("hsl_blur", fl(20.0))],
        vec![("hsl_on", ParamValue::Bool(true)), ("hsl_show_mask", ParamValue::Choice(1)), ("hsl_denoise", fl(40.0))],
    ];
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (67u32, 41u32);
    // highlights up to 10× reference white (~2000 cd/m²)
    let px: Vec<f32> = picture(w, h).chunks(4).flat_map(|p| [p[0] * 10.0, p[1] * 3.0, p[2] * 6.0, p[3]].map(half)).collect();
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let mut failures = Vec::new();
    for working in [WorkingSpace::Rec2100Pq, WorkingSpace::Rec2100Hlg] {
        let cx = FxCtx { project: Some(p), working, ..cx(Tick::ZERO, 1.0) };
        for params in &cases {
            let e = effect("lumetri", params);
            let op = FxOp::eval(&e, &cx, w as usize, h as usize).unwrap_or_else(|| panic!("{working:?} {params:?}: no GPU op"));
            assert!(op.gpu_ok(), "{working:?} {params:?}: not GPU-capable");
            let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
            op.apply(&mut cpu);
            let fx = LayerFx { size: (w, h), decimation: 1, ops: vec![op], ..Default::default() };
            let (_, _, gpu) = c.effect_image(&frame, &fx).expect("effect image");
            let (worst, flips) = compare(&cpu.px, &gpu);
            eprintln!("{working:?} {params:?}: max rel diff {worst:.2e}, {:.3}% flipped", flips * 100.0);
            if worst > TRANSCENDENTAL || flips > 0.0 {
                failures.push(format!("{working:?} {params:?}: {worst:.2e}, flips {flips:.4}"));
            }
        }
    }
    assert!(failures.is_empty(), "HDR Lumetri differs on the GPU:\n{}", failures.join("\n"));
}

/// HDR and wide-gamut pipelines on the GPU: YUV (BT.709) frames converted into the working gamut
/// (with and without effects: Lumetri in the HDR grade), a CPU-converted float layer with
/// highlights far above reference white, and the monitor conversion (BT.2390 tone mapping, gamut
/// matrix, desaturation) in the final pass, against the plan run on the CPU.
#[test]
fn hdr_and_wide_gamut_plans_match_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use filmcraft_color::{ColorPipeline, WorkingSpace};
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (320usize, 180usize);
    // a float layer in working units: a ramp up to 12× reference white (≈ 2400 cd/m²)
    let hot: Vec<f32> = (0..160 * 90)
        .flat_map(|i| {
            let t = (i % 160) as f32 / 159.0;
            [12.0 * t, 6.0 * t * t, 0.3 + t, 1.0]
        })
        .collect();
    let hot = Arc::new(VideoFrame::rgba_f32(160, 90, hot));
    for (name, color) in [
        ("pq", ColorPipeline { working: WorkingSpace::Rec2100Pq, ..ColorPipeline::REC709 }),
        ("hlg", ColorPipeline { working: WorkingSpace::Rec2100Hlg, ..ColorPipeline::REC709 }),
        ("wide", ColorPipeline { wide_gamut: true, ..ColorPipeline::REC709 }),
    ] {
        let frame = yuv_frame(640, 360);
        let grade = effect("lumetri", &[("exposure", fl(0.6)), ("contrast", fl(35.0)), ("saturation", fl(150.0)), ("hdr_specular", fl(30.0))]);
        let cxw = FxCtx { working: color.working, ..cx(Tick::ZERO, 0.5) };
        let ops: Vec<FxOp> = FxOp::eval(&grade, &cxw, 320, 180).into_iter().collect();
        assert!(ops.iter().all(FxOp::gpu_ok));
        let fx = LayerFx { size: (320, 180), decimation: 2, ops, color, ..Default::default() };
        let plan = FramePlan::Layers {
            width: w,
            height: h,
            color,
            layers: vec![
                PlanLayer { fx: Some(Arc::new(fx)), ..PlanLayer::new(frame.clone(), Affine::IDENTITY, 1.0, Blend::Normal) },
                PlanLayer::new(
                    yuv_frame(320, 180),
                    Affine::motion(Vec2::new(200.0, 60.0), Vec2::new(0.4, 0.4), 10.0, Vec2::new(160.0, 90.0)),
                    0.8,
                    Blend::Normal,
                ),
                PlanLayer::new(hot.clone(), Affine::translate(20.0, 80.0), 0.9, Blend::Normal),
            ],
        };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let keep = interior(&plan);
        let (p99, mean, max) = stats8(&cpu, &gpu, &keep);
        eprintln!("{name}: interior 8-bit p99 {p99} mean {mean:.3} max {max}");
        assert!(p99 <= 2 && mean < 0.5, "{name}: p99 {p99}, mean {mean}, max {max}");
    }
}

/// HDR media decoded on the GPU: 10-bit BT.2020 PQ and HLG pictures (smooth ramps reaching
/// 1000+ cd/m²) in PQ and HLG sequences, drawn directly and through the effect stage (Lumetri),
/// against the plan run on the CPU (`decode_working`: the media's input transform).
#[test]
fn hdr_media_decodes_on_the_gpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    use filmcraft_color::{ColorInfo, ColorPipeline, Matrix, Primaries, Range, Transfer, WorkingSpace};
    use filmcraft_frame::{Chroma, PixelData};
    let (w, h) = (320u32, 180u32);
    let frame = |transfer: Transfer| {
        let (wu, hu) = (w as usize, h as usize);
        // a horizontal luma ramp over the whole code range and gently varying chroma
        let y: Vec<u16> = (0..wu * hu).map(|i| (64 + (i % wu) * 876 / (wu - 1)) as u16).collect();
        let (cw, ch) = (wu / 2, hu / 2);
        let cb: Vec<u16> = (0..cw * ch).map(|i| (512 + ((i / cw) as i32 - 45) * 3) as u16).collect();
        let cr: Vec<u16> = (0..cw * ch).map(|i| (512 + ((i % cw) as i32 - 80) * 2) as u16).collect();
        Arc::new(VideoFrame {
            width: w,
            height: h,
            data: PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C420, bits: 10, alpha: None },
            color: ColorInfo { matrix: Matrix::Bt2020Ncl, transfer, primaries: Primaries::Bt2020, range: Range::Limited },
            par: (1, 1),
            pts: Default::default(),
        })
    };
    let mut c = GpuCompositor::new(&dev, &q);
    for working in [WorkingSpace::Rec2100Pq, WorkingSpace::Rec2100Hlg] {
        let color = ColorPipeline { working, ..ColorPipeline::REC709 };
        for transfer in [Transfer::Pq, Transfer::Hlg] {
            let f = frame(transfer);
            let grade = effect("lumetri", &[("exposure", fl(-0.5)), ("contrast", fl(20.0)), ("hdr_specular", fl(-30.0))]);
            let cxw = FxCtx { working, ..cx(Tick::ZERO, 1.0) };
            let ops: Vec<FxOp> = FxOp::eval(&grade, &cxw, w as usize, h as usize).into_iter().collect();
            let fx = LayerFx { size: (w, h), decimation: 1, ops, color, ..Default::default() };
            for with_fx in [false, true] {
                let mut layer = PlanLayer::new(f.clone(), Affine::IDENTITY, 1.0, Blend::Normal);
                if with_fx {
                    layer.fx = Some(Arc::new(fx.clone()));
                }
                let plan = FramePlan::Layers { width: w as usize, height: h as usize, color, layers: vec![layer] };
                let cpu = execute_cpu(&plan).over_black_rgba8();
                c.composite(&plan);
                let (_, _, gpu) = c.read_output().expect("readback");
                let keep = vec![true; cpu.len() / 4];
                let (p99, mean, max) = stats8(&cpu, &gpu, &keep);
                eprintln!("{working:?} {transfer:?} fx {with_fx}: 8-bit p99 {p99} mean {mean:.3} max {max}");
                // (chroma: the GPU interpolates 4:2:0 bilinearly, the CPU replicates; steep PQ
                // highlights amplify that, as in the SDR tests' bound)
                assert!(p99 <= 6 && mean < 1.5, "{working:?} {transfer:?} fx {with_fx}: p99 {p99}, mean {mean}, max {max}");
            }
        }
    }
}

/// GPU3 effects that follow the clip time (Noise's frame, Wave Warp's phase, Strobe's flashes),
/// at ordinary and huge times, and chained with each other.
#[test]
fn gpu3_effects_over_time_and_in_chains() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (96u32, 54u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let chains: Vec<Vec<EffectInstance>> = vec![
        vec![effect("noise", &[("amount", fl(40.0))])],
        vec![effect("noise", &[("amount", fl(40.0)), ("grain_size", fl(2.0)), ("clip", ParamValue::Bool(false))])],
        vec![effect("wave_warp", &[("speed", fl(3.7))])],
        vec![effect("strobe", &[("period", fl(0.3)), ("duration", fl(0.1)), ("blend", fl(20.0))])],
        vec![
            effect("ramp", &[("blend", fl(60.0))]),
            effect("corner_pin", &[("upper_left", pt(10.0, 5.0)), ("lower_right", pt(80.0, 50.0))]),
            effect("drop_shadow", &[("distance", fl(6.0)), ("softness", fl(8.0))]),
            effect("noise", &[("amount", fl(20.0))]),
            effect("find_edges", &[("blend", fl(50.0))]),
            effect("twirl", &[("angle", fl(-70.0))]),
            effect("drop_shadow", &[("distance", fl(3.0)), ("only", ParamValue::Bool(true))]),
        ],
    ];
    let mut failures = Vec::new();
    for seconds in [0.0, 0.25, 12.34, 36_000.0, 3.0e8, 1e30] {
        let cx = FxCtx { seconds, ..cx(Tick::ZERO, 1.0) };
        for chain in &chains {
            let ops: Vec<FxOp> = chain.iter().filter_map(|e| FxOp::eval(e, &cx, w as usize, h as usize)).collect();
            assert_eq!(ops.len(), chain.len());
            if !ops.iter().all(FxOp::gpu_ok) {
                // only Wave Warp's phase grows out of what the GPU takes (weeks into a clip)
                assert!(seconds > 1e7 && chain[0].effect == "wave_warp", "{seconds} s: {:?} not GPU-capable", chain[0].effect);
                continue;
            }
            let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
            for op in &ops {
                op.apply(&mut cpu);
            }
            let names: Vec<&str> = chain.iter().map(|e| e.effect.as_str()).collect();
            let (_, _, gpu) = c.effect_image(&frame, &LayerFx { size: (w, h), decimation: 1, ops, ..Default::default() }).expect("effect image");
            let (worst, flips) = compare(&cpu.px, &gpu);
            eprintln!("{names:?} at {seconds} s: max rel diff {worst:.2e}, {:.3}% flipped", flips * 100.0);
            // (Corner Pin's edge test may flip a pixel on the quad's outline)
            if worst > TRANSCENDENTAL || flips > 0.002 {
                failures.push(format!("{names:?} at {seconds} s: {worst:.2e}, flips {flips:.4}"));
            }
        }
    }
    assert!(failures.is_empty(), "GPU3 effects differ from the CPU:\n{}", failures.join("\n"));
}

/// One 1080p frame through each of the most used GPU3 effects: the CPU reference against the GPU
/// effect stage (upload, passes and readback included). Informational; run with
/// `cargo test -p filmcraft-gpu --release gpu3_speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn gpu3_speed() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (1920u32, 1080u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let cases = [
        effect("noise", &[("amount", fl(30.0))]),
        effect("drop_shadow", &[("distance", fl(20.0)), ("softness", fl(40.0))]),
        effect("corner_pin", &[("upper_left", pt(100.0, 50.0)), ("lower_right", pt(1800.0, 1000.0))]),
        effect("lens_distortion", &[("curvature", fl(30.0))]),
        effect("twirl", &[("angle", fl(90.0))]),
        effect("wave_warp", &[]),
        effect("find_edges", &[]),
        effect("ramp", &[("blend", fl(50.0))]),
    ];
    let time = |f: &mut dyn FnMut()| {
        f();
        let t0 = std::time::Instant::now();
        for _ in 0..5 {
            f();
        }
        t0.elapsed().as_secs_f64() * 1000.0 / 5.0
    };
    let base = LayerFx { size: (w, h), decimation: 1, ops: Vec::new(), ..Default::default() };
    let io_ms = time(&mut || {
        let _ = c.effect_image(&frame, &base);
    });
    eprintln!("no effect (upload + source draw + readback): {io_ms:.1} ms");
    for e in &cases {
        let op = FxOp::eval(e, &cx(Tick::ZERO, 1.0), w as usize, h as usize).expect("op");
        let cpu_ms = time(&mut || {
            let mut img = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
            op.apply(&mut img);
        });
        let fx = LayerFx { size: (w, h), decimation: 1, ops: vec![op.clone()], ..Default::default() };
        let gpu_ms = time(&mut || {
            let _ = c.effect_image(&frame, &fx);
        });
        eprintln!("{}: CPU {cpu_ms:.1} ms, GPU {gpu_ms:.1} ms ({:.1} ms over no effect)", e.effect, gpu_ms - io_ms);
    }
}

/// GPU4 effects in chains (the glows' and Roughen Edges' planes kept beside the layer between
/// other effects' passes, Channel Blur's masked box passes), with effect masks, and Light Leaks
/// over time.
#[test]
fn gpu4_effects_in_chains_masks_and_over_time() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (96u32, 54u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let chains: Vec<Vec<EffectInstance>> = vec![
        vec![effect("light_leaks", &[("speed", fl(2.5))])],
        vec![
            effect("edge_glow", &[("width", fl(4.0)), ("radius", fl(12.0))]),
            effect("unsharp_mask", &[("amount", fl(80.0)), ("radius", fl(3.0))]),
            effect("alpha_glow", &[("glow", fl(20.0))]),
            effect("channel_blur", &[("red", fl(10.0)), ("alpha", fl(4.0)), ("dimensions", ch(2))]),
            effect("roughen_edges", &[("border", fl(6.0))]),
            effect("drop_shadow", &[("distance", fl(4.0)), ("softness", fl(6.0))]),
            effect("median", &[("radius", fl(2.0))]),
        ],
        vec![
            effect("rounded_crop", &[("left", fl(8.0)), ("radius", fl(10.0))]),
            effect("turbulent_displace", &[("size", fl(25.0)), ("amount", fl(20.0))]),
            effect("lens_flare", &[("ghosts", fl(5.0))]),
            effect("rgb_split", &[("amount", fl(4.0))]),
            effect("lighting_effects", &[("bump_height", fl(30.0))]),
            effect("gradient", &[("blend", fl(70.0))]),
            effect("color_emboss", &[]),
            effect("reduce_interlace_flicker", &[("softness", fl(2.0))]),
        ],
    ];
    let all = crate::tests::test_masks(96.0 / 320.0);
    let mut failures = Vec::new();
    for seconds in [0.0, 0.4, 37.5, 3600.0] {
        let cx = FxCtx { seconds, ..cx(Tick::ZERO, 1.0) };
        for (k, chain) in chains.iter().enumerate() {
            let ops: Vec<FxOp> = chain.iter().filter_map(|e| FxOp::eval(e, &cx, w as usize, h as usize)).collect();
            assert_eq!(ops.len(), chain.len());
            assert!(ops.iter().all(FxOp::gpu_ok), "{seconds} s: chain {k} not GPU-capable");
            let names: Vec<&str> = chain.iter().map(|e| e.effect.as_str()).collect();
            // unmasked, then every other effect masked
            let masks: Vec<Vec<_>> = (0..ops.len()).map(|i| if i % 2 == 0 { all.clone() } else { Vec::new() }).collect();
            for (label, fx) in [
                ("plain", LayerFx { size: (w, h), decimation: 1, ops: ops.clone(), ..Default::default() }),
                ("masked", LayerFx { size: (w, h), decimation: 1, ops: ops.clone(), masks, ..Default::default() }),
            ] {
                let mut cpu = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
                fx.apply(&mut cpu);
                let (_, _, gpu) = c.effect_image(&frame, &fx).expect("effect image");
                let (worst, flips) = compare(&cpu.px, &gpu);
                eprintln!("{names:?} {label} at {seconds} s: max rel diff {worst:.2e}, {:.3}% flipped", flips * 100.0);
                if worst > TRANSCENDENTAL || flips > 0.002 {
                    failures.push(format!("{names:?} {label} at {seconds} s: {worst:.2e}, flips {flips:.4}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "GPU4 effects differ from the CPU:\n{}", failures.join("\n"));
}

/// The GPU4 references (`MoreOp::apply`, and Reduce Interlace Flicker's `FxOp::Gaussian`) are
/// what the CPU renderer draws for the effect (`filmcraft_render::effects::apply`), value for
/// value, and hostile values are declined by `gpu_ok` rather than drawn wrongly.
#[test]
fn gpu4_references_are_the_cpu_effects() {
    let (w, h) = (67usize, 41usize);
    let px = picture(w as u32, h as u32);
    let ids = filmcraft_render::gpufx_more::MORE_EFFECTS.iter().copied().chain(["reduce_interlace_flicker"]).collect::<Vec<_>>();
    let mut n = 0;
    for (id, params, _) in cases().into_iter().filter(|c| ids.contains(&c.0)) {
        let e = effect(id, &params);
        let c = cx(Tick::ZERO, 1.0);
        let op = FxOp::eval(&e, &c, w, h).unwrap_or_else(|| panic!("{id} has no GPU op"));
        let mut reference = filmcraft_render::Image { w, h, px: px.clone() };
        op.apply(&mut reference);
        let mut real = filmcraft_render::Image { w, h, px: px.clone() };
        filmcraft_render::effects::apply(&mut real, &e, &c);
        assert_eq!(reference.px, real.px, "{id} {params:?}");
        assert_ne!(real.px, px, "{id} {params:?} changes the picture");
        n += 1;
    }
    assert!(n >= 38, "{n} cases");
    for id in &ids {
        assert!(cases().iter().any(|c| c.0 == *id), "{id}: no parity case");
    }
    // non-finite or out-of-range values stay on the CPU
    let declined = [
        effect("lens_flare", &[("size", fl(f64::NAN)), ("center", pt(f64::INFINITY, 3.0))]),
        effect("rgb_split", &[("amount", fl(f64::INFINITY))]),
        effect("turbulent_displace", &[("evolution", fl(1e12))]),
        effect("turbulent_displace", &[("offset", pt(1e12, 0.0))]),
        effect("light_leaks", &[("speed", fl(1e9))]),
        effect("roughen_edges", &[("stretch", fl(-300.0))]),
        effect("gradient", &[("start", pt(f64::NAN, 1e30))]),
        effect("lighting_effects", &[("l1_center", pt(1e30, 0.0))]),
    ];
    for e in &declined {
        let c = FxCtx { seconds: 10.0, ..cx(Tick::ZERO, 1.0) };
        let op = FxOp::eval(e, &c, w, h).unwrap_or_else(|| panic!("{} has no GPU op", e.effect));
        assert!(!op.gpu_ok(), "{}: hostile values must stay on the CPU", e.effect);
    }
}

/// One 1080p frame through GPU4 effects: the CPU reference against the GPU effect stage (upload,
/// passes and readback included). Informational; run with
/// `cargo test -p filmcraft-gpu --release gpu4_speed -- --ignored --nocapture`.
#[test]
#[ignore]
fn gpu4_speed() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (1920u32, 1080u32);
    let px = picture(w, h);
    let frame = VideoFrame::rgba_f32(w, h, px.clone());
    let cases = [
        effect("lens_flare", &[]),
        effect("rgb_split", &[("amount", fl(20.0))]),
        effect("lighting_effects", &[("bump_height", fl(30.0))]),
        effect("turbulent_displace", &[]),
        effect("median", &[("radius", fl(2.0))]),
        effect("median", &[("radius", fl(4.0))]),
        effect("channel_blur", &[("red", fl(20.0)), ("blue", fl(60.0))]),
        effect("alpha_glow", &[]),
        effect("edge_glow", &[]),
        effect("roughen_edges", &[]),
        effect("light_leaks", &[]),
        effect("gradient", &[("blend", fl(50.0))]),
    ];
    let time = |f: &mut dyn FnMut()| {
        f();
        let t0 = std::time::Instant::now();
        for _ in 0..5 {
            f();
        }
        t0.elapsed().as_secs_f64() * 1000.0 / 5.0
    };
    let base = LayerFx { size: (w, h), decimation: 1, ops: Vec::new(), ..Default::default() };
    let io_ms = time(&mut || {
        let _ = c.effect_image(&frame, &base);
    });
    eprintln!("no effect (upload + source draw + readback): {io_ms:.1} ms");
    for e in &cases {
        let op = FxOp::eval(e, &cx(Tick::ZERO, 1.0), w as usize, h as usize).expect("op");
        let cpu_ms = time(&mut || {
            let mut img = filmcraft_render::Image { w: w as usize, h: h as usize, px: px.clone() };
            op.apply(&mut img);
        });
        let fx = LayerFx { size: (w, h), decimation: 1, ops: vec![op.clone()], ..Default::default() };
        let gpu_ms = time(&mut || {
            let _ = c.effect_image(&frame, &fx);
        });
        eprintln!("{}: CPU {cpu_ms:.1} ms, GPU {gpu_ms:.1} ms ({:.1} ms over no effect)", e.effect, gpu_ms - io_ms);
    }
}
