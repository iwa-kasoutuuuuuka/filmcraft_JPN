// FilmCraft GPU effect stage: the standard effects of `filmcraft_render::gpufx::FxOp`, one
// compute pass each, reading the working image (`src`, linear premultiplied RGBA f32) and writing
// the next one (`dst`). The math mirrors `FxOp::apply` (the CPU reference) operation by
// operation; parameters arrive evaluated (keyframes, defaults and clamps applied on the CPU).

struct U {
    i0: vec4<u32>,  // op, width, height, unused
    i1: vec4<u32>,  // integer parameters (box radius, repeat, steps, channel, rect…)
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;
// Unsharp Mask: the image before blurring. Mask pass: the image before the masked effect.
@group(0) @binding(3) var aux: texture_2d<f32>;

// Mask pass (OP_MASK): the active masks, flattened to polygons in working-image pixels (the
// layout and the math of `mask.wgsl`, the CPU reference is `filmcraft_render::mask`).
struct MaskInfo {
    start: u32,
    len: u32,
    mode: u32,
    inverted: u32,
    feather: f32,
    expansion: f32,
    opacity: f32,
    _pad: f32,
};
@group(0) @binding(4) var<storage, read> mask_infos: array<MaskInfo>;
@group(0) @binding(5) var<storage, read> mask_pts: array<vec2<f32>>;

const OP_BRIGHTNESS_CONTRAST: u32 = 1u;
const OP_PROC_AMP: u32 = 2u;
const OP_TINT: u32 = 3u;
const OP_BLACK_WHITE: u32 = 4u;
const OP_COLOR_BALANCE: u32 = 5u;
const OP_LEAVE_COLOR: u32 = 6u;
const OP_CHANGE_TO_COLOR: u32 = 7u;
const OP_COLOR_PASS: u32 = 8u;
const OP_GAMMA: u32 = 9u;
const OP_LEVELS: u32 = 10u;
const OP_EXTRACT: u32 = 11u;
const OP_INVERT: u32 = 12u;
const OP_INVERT_ALPHA: u32 = 13u;
const OP_POSTERIZE: u32 = 14u;
const OP_ASC_CDL: u32 = 15u;
const OP_CHANNEL_MIX: u32 = 16u;
const OP_COLOR_REPLACE: u32 = 17u;
const OP_ALPHA_ADJUST: u32 = 18u;
const OP_BOX: u32 = 20u;
const OP_DIRECTIONAL: u32 = 22u;
const OP_UNSHARP: u32 = 23u;
const OP_CROP: u32 = 24u;
const OP_RESAMPLE: u32 = 25u;
const OP_HFLIP: u32 = 26u;
const OP_VFLIP: u32 = 27u;
const OP_MIRROR: u32 = 28u;
const OP_OFFSET: u32 = 29u;
const OP_LUMETRI: u32 = 30u;
const OP_MOSAIC: u32 = 31u;
const OP_VIGNETTE: u32 = 32u;
const OP_MASK: u32 = 33u;
const OP_HSL_KEY: u32 = 34u;
const OP_HSL_MEDIAN: u32 = 35u;
const OP_HSL_APPLY: u32 = 36u;
const OP_KEY: u32 = 37u;
const OP_LUMA_KEY: u32 = 38u;
const OP_VIDEO_LIMITER: u32 = 39u;
const OP_UKEY_MATTE: u32 = 40u;
const OP_UKEY_CHOKE: u32 = 41u;
const OP_UKEY_APPLY: u32 = 42u;
// GPU3 (`filmcraft_render::gpufx_ext`): `ext_pixel`
const OP_NOISE: u32 = 43u;
const OP_WARP: u32 = 44u;
const OP_MAGNIFY: u32 = 45u;
const OP_FIND_EDGES: u32 = 46u;
const OP_EMBOSS: u32 = 47u;
const OP_BEVEL_ALPHA: u32 = 48u;
const OP_REPLICATE: u32 = 49u;
const OP_STROBE: u32 = 50u;
const OP_SHADOW: u32 = 51u;
const OP_SHADOW_OVER: u32 = 52u;
const OP_GENERATE: u32 = 53u;

fn size() -> vec2<i32> {
    return vec2<i32>(i32(u.i0.y), i32(u.i0.z));
}

fn ld(p: vec2<i32>) -> vec4<f32> {
    return textureLoad(src, p, 0);
}

// `Image::get_or_clear`
fn ld_or_clear(p: vec2<i32>) -> vec4<f32> {
    let s = size();
    if p.x < 0 || p.y < 0 || p.x >= s.x || p.y >= s.y {
        return vec4(0.0);
    }
    return textureLoad(src, p, 0);
}

// `Image::sample_bilinear`: pixel centres at +0.5, transparent outside.
fn sample_bilinear(x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let i = vec2<i32>(i32(x0), i32(y0));
    let a = ld_or_clear(i);
    let b = ld_or_clear(i + vec2(1, 0));
    let c = ld_or_clear(i + vec2(0, 1));
    let d = ld_or_clear(i + vec2(1, 1));
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// `Image::sample_bilinear_clamped`
fn sample_clamped(x: f32, y: f32) -> vec4<f32> {
    let s = size();
    let fx = clamp(x - 0.5, 0.0, f32(s.x) - 1.0);
    let fy = clamp(y - 0.5, 0.0, f32(s.y) - 1.0);
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let x1 = min(x0 + 1, s.x - 1);
    let y1 = min(y0 + 1, s.y - 1);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let a = ld(vec2(x0, y0));
    let b = ld(vec2(x1, y0));
    let c = ld(vec2(x0, y1));
    let d = ld(vec2(x1, y1));
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// ---- colour helpers (`filmcraft_color`, `effects::enc` / `dec`)

fn linear_to_srgb1(v: f32) -> f32 {
    if v <= 0.0031308 {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_to_linear1(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn enc(c: vec3<f32>) -> vec3<f32> {
    let m = max(c, vec3(0.0));
    return vec3(linear_to_srgb1(m.x), linear_to_srgb1(m.y), linear_to_srgb1(m.z));
}

fn dec(c: vec3<f32>) -> vec3<f32> {
    let m = clamp(c, vec3(0.0), vec3(1.0));
    return vec3(srgb_to_linear1(m.x), srgb_to_linear1(m.y), srgb_to_linear1(m.z));
}

fn luma709(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

// `f32::powf` for x >= 0 (WGSL leaves pow(0, y) undefined).
fn powf(x: f32, y: f32) -> f32 {
    if x == 0.0 {
        if y == 0.0 {
            return 1.0;
        }
        if y > 0.0 {
            return 0.0;
        }
        return 3.0e38;
    }
    return pow(x, y);
}

// `f32::round`: halves away from zero (WGSL `round` rounds them to even).
fn round_away(x: f32) -> f32 {
    let a = abs(x);
    let f = floor(a);
    let r = select(f, f + 1.0, a - f >= 0.5);
    return select(r, -r, x < 0.0);
}

// ---- grading signals (`filmcraft_color::GradeSpace`): mode 0 SDR, 1 PQ (pa: normalisation),
// 2 HLG (pa: HDR White, pb: system gamma); lw: the BT.2020 luma weights

fn pq_inverse_eotf1(y: f32) -> f32 {
    let m1 = 0.15930176;
    let m2 = 78.84375;
    let p = powf(max(y, 0.0), m1);
    return powf((0.8359375 + 18.851563 * p) / (1.0 + 18.6875 * p), m2);
}

fn pq_eotf1(e: f32) -> f32 {
    let m1 = 0.15930176;
    let m2 = 78.84375;
    let p = powf(max(e, 0.0), 1.0 / m2);
    return powf(max(p - 0.8359375, 0.0) / (18.851563 - 18.6875 * p), 1.0 / m1);
}

fn hlg_oetf1(l: f32) -> f32 {
    if l <= 1.0 / 12.0 {
        return sqrt(3.0 * max(l, 0.0));
    }
    return 0.17883277 * log(12.0 * l - 0.28466892) + 0.5599107;
}

fn hlg_inverse_oetf1(e: f32) -> f32 {
    if e <= 0.5 {
        return e * e / 3.0;
    }
    return (exp((e - 0.5599107) / 0.17883277) + 0.28466892) / 12.0;
}

// `GradeSpace::encode`: working linear -> grading signal.
fn gs_encode(c: vec3<f32>, mode: u32, pa: f32, pb: f32, lw: vec3<f32>) -> vec3<f32> {
    if mode == 1u {
        let y = min(max(c, vec3<f32>(0.0)) * 203.0 / 10000.0, vec3<f32>(1.0));
        return vec3<f32>(pq_inverse_eotf1(y.x), pq_inverse_eotf1(y.y), pq_inverse_eotf1(y.z)) / pa;
    }
    if mode == 2u {
        let fd = max(c, vec3<f32>(0.0)) * 203.0 / pa;
        let yd = lw.x * fd.x + lw.y * fd.y + lw.z * fd.z;
        if yd <= 0.0 {
            return vec3<f32>(0.0);
        }
        let k = powf(yd, (1.0 - pb) / pb);
        return vec3<f32>(hlg_oetf1(fd.x * k), hlg_oetf1(fd.y * k), hlg_oetf1(fd.z * k));
    }
    return enc(c);
}

// `GradeSpace::decode`: grading signal -> working linear.
fn gs_decode(v: vec3<f32>, mode: u32, pa: f32, pb: f32, lw: vec3<f32>) -> vec3<f32> {
    if mode == 1u {
        let e = clamp(v * pa, vec3<f32>(0.0), vec3<f32>(1.0));
        return vec3<f32>(pq_eotf1(e.x), pq_eotf1(e.y), pq_eotf1(e.z)) * 10000.0 / 203.0;
    }
    if mode == 2u {
        let q = clamp(v, vec3<f32>(0.0), vec3<f32>(1.5));
        let es = vec3<f32>(hlg_inverse_oetf1(q.x), hlg_inverse_oetf1(q.y), hlg_inverse_oetf1(q.z));
        let ys = lw.x * es.x + lw.y * es.y + lw.z * es.z;
        if ys <= 0.0 {
            return vec3<f32>(0.0);
        }
        let k = powf(ys, pb - 1.0);
        return es * k * pa / 203.0;
    }
    return dec(v);
}

// `effects::within_unit`'s scale: an HDR signal above 1 is divided by its largest channel.
fn unit_scale(v: vec3<f32>) -> f32 {
    return max(max(v.x, v.y), max(v.z, 1.0));
}

// `rem_euclid(1.0)`
fn fract_euclid(x: f32) -> f32 {
    return x - floor(x);
}

// `vfx::smoothstep` (not WGSL's: a degenerate edge pair divides by 1e-6)
fn smoothstep_fx(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / max(e1 - e0, 1e-6), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let r = c.x; let g = c.y; let b = c.z;
    let mx = max(max(r, g), b);
    let mn = min(min(r, g), b);
    let l = (mx + mn) / 2.0;
    if abs(mx - mn) < 1e-7 {
        return vec3(0.0, 0.0, l);
    }
    let d = mx - mn;
    var s: f32;
    if l > 0.5 {
        s = d / (2.0 - mx - mn);
    } else {
        s = d / (mx + mn);
    }
    var h: f32;
    if mx == r {
        h = (g - b) / d + select(0.0, 6.0, g < b);
    } else if mx == g {
        h = (b - r) / d + 2.0;
    } else {
        h = (r - g) / d + 4.0;
    }
    return vec3(h / 6.0, s, l);
}

fn hsl_channel(p: f32, q: f32, t0: f32) -> f32 {
    let t = fract_euclid(t0);
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    return p;
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> vec3<f32> {
    if s <= 0.0 {
        return vec3(l);
    }
    var q: f32;
    if l < 0.5 {
        q = l * (1.0 + s);
    } else {
        q = l + s - l * s;
    }
    let p = 2.0 * l - q;
    return vec3(hsl_channel(p, q, h + 1.0 / 3.0), hsl_channel(p, q, h), hsl_channel(p, q, h - 1.0 / 3.0));
}

// ---- per-pixel colour effects on straight colour (`Image::map_rgb`)

fn color_op(op: u32, c: vec3<f32>) -> vec3<f32> {
    switch op {
        case OP_BRIGHTNESS_CONTRAST: {
            let br = u.p0.x; let co = u.p0.y;
            return dec((enc(c) - 0.5) * co + 0.5 + br);
        }
        case OP_PROC_AMP: {
            let br = u.p0.x; let co = u.p0.y; let hue = u.p0.z; let sat = u.p0.w;
            var hsl = rgb_to_hsl(enc(c));
            hsl.x = fract_euclid(hsl.x + hue);
            hsl.y = clamp(hsl.y * sat, 0.0, 1.0);
            return dec((hsl_to_rgb(hsl.x, hsl.y, hsl.z) - 0.5) * co + 0.5 + br);
        }
        case OP_TINT: {
            let bl = u.p0.xyz; let wh = u.p1.xyz; let amt = u.p0.w;
            let l = linear_to_srgb1(max(luma709(c), 0.0));
            let t = bl + (wh - bl) * l;
            let e = enc(c);
            return dec(e + (t - e) * amt);
        }
        case OP_BLACK_WHITE: {
            return vec3(luma709(c));
        }
        case OP_COLOR_BALANCE: {
            let e = enc(c);
            let l = 0.2126 * e.x + 0.7152 * e.y + 0.0722 * e.z;
            let ws = (1.0 - l) * (1.0 - l);
            let wh = l * l;
            let wm = 1.0 - ws - wh;
            var o = e + u.p0.xyz * ws + u.p1.xyz * max(wm, 0.0) + u.p2.xyz * wh;
            if u.i1.x != 0u {
                let l2 = 0.2126 * o.x + 0.7152 * o.y + 0.0722 * o.z;
                o = o + (l - l2);
            }
            return dec(o);
        }
        case OP_LEAVE_COLOR: {
            let amt = u.p0.x; let kh = u.p0.y; let tol = u.p0.z; let soft = u.p0.w;
            let h = rgb_to_hsl(enc(c)).x;
            let d = min(abs(h - kh), 1.0 - abs(h - kh)) * 2.0;
            let keep = 1.0 - clamp((d - tol) / soft, 0.0, 1.0);
            let l = luma709(c);
            let k = amt * (1.0 - keep);
            return c + (l - c) * k;
        }
        case OP_CHANGE_TO_COLOR: {
            let fh = u.p0.x; let th = u.p0.y; let tol = u.p0.z; let soft = u.p0.w;
            var hsl = rgb_to_hsl(enc(c));
            let d = min(abs(hsl.x - fh), 1.0 - abs(hsl.x - fh));
            let w = 1.0 - clamp((d - tol) / soft, 0.0, 1.0);
            hsl.x = fract_euclid(hsl.x + (th - fh) * w);
            return dec(hsl_to_rgb(hsl.x, hsl.y, hsl.z));
        }
        case OP_COLOR_PASS: {
            let e = enc(c);
            let k = e - u.p0.xyz;
            let d = sqrt(k.x * k.x + k.y * k.y + k.z * k.z);
            let passes = (d <= u.p0.w * 1.2) != (u.i1.x != 0u);
            if passes {
                return c;
            }
            return vec3(luma709(c));
        }
        case OP_GAMMA: {
            let e = enc(c);
            let g = u.p0.x;
            return dec(vec3(powf(max(e.x, 0.0), g), powf(max(e.y, 0.0), g), powf(max(e.z, 0.0), g)));
        }
        case OP_LEVELS: {
            let ib = u.p0.x; let iw = u.p0.y; let ob = u.p0.z; let ow = u.p0.w; let g = u.p1.x;
            let v = clamp((enc(c) - ib) / (iw - ib), vec3(0.0), vec3(1.0));
            return dec(ob + vec3(powf(v.x, g), powf(v.y, g), powf(v.z, g)) * (ow - ob));
        }
        case OP_EXTRACT: {
            let lo = u.p0.x; let hi = u.p0.y; let soft = u.p0.z;
            let l = linear_to_srgb1(max(luma709(c), 0.0));
            let inside = min(clamp((l - lo) / soft, 0.0, 1.0), clamp((hi - l) / soft, 0.0, 1.0));
            return vec3(select(inside, 1.0 - inside, u.i1.x != 0u));
        }
        case OP_INVERT: {
            let ch = u.i1.x;
            let blend = u.p0.x;
            let e = enc(c);
            var o = e;
            if ch == 0u || ch == 1u { o.x = 1.0 - e.x; }
            if ch == 0u || ch == 2u { o.y = 1.0 - e.y; }
            if ch == 0u || ch == 3u { o.z = 1.0 - e.z; }
            return dec(o + (e - o) * blend);
        }
        case OP_POSTERIZE: {
            let n = u.p0.x;
            let e = enc(c) * n;
            return dec(vec3(round_away(e.x), round_away(e.y), round_away(e.z)) / n);
        }
        case OP_ASC_CDL: {
            let e = enc(c);
            let s = u.p0.xyz; let o = u.p1.xyz; let pw = max(u.p2.xyz, vec3(0.0)); let sat = u.p0.w;
            let q = clamp(e * s + o, vec3(0.0), vec3(1.0));
            let v = vec3(powf(q.x, pw.x), powf(q.y, pw.y), powf(q.z, pw.z));
            let l = luma709(v);
            return dec(clamp(l + sat * (v - l), vec3(0.0), vec3(1.0)));
        }
        case OP_CHANNEL_MIX: {
            let v = enc(c);
            let r = u.p0; let g = u.p1; let b = u.p2;
            return dec(clamp(vec3(r.x * v.x + r.y * v.y + r.z * v.z + r.w, g.x * v.x + g.y * v.y + g.z * v.z + g.w, b.x * v.x + b.y * v.y + b.z * v.z + b.w), vec3(0.0), vec3(1.0)));
        }
        case OP_COLOR_REPLACE: {
            let v = enc(c);
            let t = u.p0.xyz; let sim = u.p0.w;
            let dd = v - t;
            let d = sqrt(dd.x * dd.x + dd.y * dd.y + dd.z * dd.z);
            let k = 1.0 - smoothstep_fx(sim * 0.85, max(sim, 1e-4), d);
            if k <= 0.0 {
                return c;
            }
            var repl = u.p1.xyz;
            if u.i1.x == 0u {
                let rh = u.p2.xyz;
                repl = hsl_to_rgb(rh.x, rh.y, rgb_to_hsl(v).z);
            }
            return dec(v + (repl - v) * k);
        }
        default: {
            return c;
        }
    }
}

fn mask_falloff(s: f32, feather: f32) -> f32 {
    let w = max(feather, 1.0);
    let t = clamp(s / w + 0.5, 0.0, 1.0);
    let smooth_t = t * t * (3.0 - 2.0 * t);
    return t + (smooth_t - t) * min(feather, 1.0);
}

// MaskMode index: None 0, Add 1, Subtract 2, Intersect 3, Lighten 4, Darken 5, Difference 6.
fn mask_combine(mode: u32, a: f32, m: f32) -> f32 {
    switch mode {
        case 1u: { return a + m - a * m; }
        case 2u: { return a * (1.0 - m); }
        case 3u: { return a * m; }
        case 4u: { return max(a, m); }
        case 5u: { return min(a, m); }
        case 6u: { return a + m - 2.0 * a * m; }
        default: { return a; }
    }
}

fn mask_seg_dist2(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = b - a;
    let q = p - a;
    let l2 = dot(d, d);
    var t = 0.0;
    if l2 > 0.0 {
        t = clamp(dot(q, d) / l2, 0.0, 1.0);
    }
    let e = q - d * t;
    return dot(e, e);
}

fn mask_value(m: MaskInfo, p: vec2<f32>) -> f32 {
    let band = abs(m.expansion) + max(m.feather, 1.0) * 0.5 + 1.0;
    var sd = -band;
    if m.len >= 3u {
        var wind = 0;
        var d2 = band * band;
        for (var i = 0u; i < m.len; i++) {
            let a = mask_pts[m.start + i];
            let c = mask_pts[m.start + (i + 1u) % m.len];
            if (a.y <= p.y) != (c.y <= p.y) {
                let t = (p.y - a.y) / (c.y - a.y);
                if a.x + (c.x - a.x) * t < p.x {
                    if c.y > a.y { wind += 1; } else { wind -= 1; }
                }
            }
            d2 = min(d2, mask_seg_dist2(p, a, c));
        }
        let d = sqrt(d2);
        if wind != 0 { sd = d; } else { sd = -d; }
    }
    let c = mask_falloff(sd + m.expansion, m.feather);
    if m.inverted != 0u {
        return m.opacity * (1.0 - c);
    }
    return m.opacity * c;
}

// Combined coverage of the i1.x masks at a pixel centre (`mask::coverage`).
fn mask_coverage(p: vec2<f32>) -> f32 {
    let first = mask_infos[0].mode;
    var acc = 0.0;
    if first == 2u || first == 3u || first == 5u {
        acc = 1.0;
    }
    for (var i = 0u; i < u.i1.x; i++) {
        let m = mask_infos[i];
        acc = clamp(mask_combine(m.mode, acc, mask_value(m, p)), 0.0, 1.0);
    }
    return acc;
}

// One output pixel of every op but the running-sum box blur.
// ---------- Lumetri beyond Basic (`LumetriExtra` in filmcraft_render::gpufx) ----------
// Data (bound where the mask points go): 64 header floats (split tone 0..6, wheels 8..20, look
// intensity 20, the input / look LUT descriptions at 24 and 42), the 1024-sample curve tables at
// 64 + k * 1024, then the LUT tables.

const LUMETRI_CURVE_N: u32 = 1024u;
// `gpufx::HEADER` (the HDR block, `LumetriHdr::pack`, at 64)
const LUMETRI_HEADER: u32 = 72u;

fn lumetri_data(i: u32) -> f32 {
    let q = mask_pts[i / 2u];
    if (i & 1u) == 0u {
        return q.x;
    }
    return q.y;
}

fn lumetri_has_curve(k: u32) -> bool {
    return (u.i1.x & (4u << k)) != 0u;
}

// `sample` of the CPU grade: linear interpolation over the table, input clamped to 0..1.
fn lumetri_curve(k: u32, x: f32) -> f32 {
    let base = LUMETRI_HEADER + k * LUMETRI_CURVE_N;
    // HDR: above HDR White a curve continues with slope 1 from its end point
    if u.i1.w != 0u && x > 1.0 {
        return lumetri_data(base + LUMETRI_CURVE_N - 1u) + (x - 1.0);
    }
    let pos = clamp(x, 0.0, 1.0) * f32(LUMETRI_CURVE_N - 1u);
    let i = u32(pos);
    let j = min(i + 1u, LUMETRI_CURVE_N - 1u);
    let a = lumetri_data(base + i);
    let b = lumetri_data(base + j);
    return a + (b - a) * (pos - f32(i));
}

// A LUT entry (r, g, b) at `off`.
fn lumetri_rgb(off: u32) -> vec3<f32> {
    return vec3<f32>(lumetri_data(off), lumetri_data(off + 1u), lumetri_data(off + 2u));
}

// `Lut1d::apply`: per-channel linear interpolation over the shaper of the LUT described at `at`.
// (One channel per call, no dynamically indexed vector write: FXC, the DX12 compiler without
// dxcompiler.dll, aborts on `o[k] = …` in a loop.)
fn lumetri_lut1d(at: u32, c: vec3<f32>) -> vec3<f32> {
    let n = u32(lumetri_data(at + 1u)) - 1u;
    let off = u32(lumetri_data(at + 2u));
    let lo = lumetri_rgb(at + 3u);
    let hi = lumetri_rgb(at + 6u);
    return vec3(
        lumetri_lut1d_channel(n, off, 0u, c.x, lo.x, hi.x),
        lumetri_lut1d_channel(n, off, 1u, c.y, lo.y, hi.y),
        lumetri_lut1d_channel(n, off, 2u, c.z, lo.z, hi.z),
    );
}

fn lumetri_lut1d_channel(n: u32, off: u32, k: u32, c: f32, lo: f32, hi: f32) -> f32 {
    let t = clamp((c - lo) / (hi - lo), 0.0, 1.0) * f32(n);
    let i = min(u32(t), max(n, 1u) - 1u);
    let f = t - f32(i);
    let a = lumetri_data(off + i * 3u + k);
    let b = lumetri_data(off + min(i + 1u, n) * 3u + k);
    return a + (b - a) * f;
}

// `Lut3d::apply`: tetrahedral interpolation in the cube of the LUT described at `at`.
fn lumetri_lut3d(at: u32, c: vec3<f32>) -> vec3<f32> {
    let n = u32(lumetri_data(at + 9u));
    let off = u32(lumetri_data(at + 10u));
    let lo = lumetri_rgb(at + 11u);
    let hi = lumetri_rgb(at + 14u);
    let t = clamp((c - lo) / (hi - lo), vec3<f32>(0.0), vec3<f32>(1.0)) * f32(n - 1u);
    let i = min(vec3<u32>(t), vec3<u32>(n - 2u));
    let fr = t.x - f32(i.x);
    let fg = t.y - f32(i.y);
    let fb = t.z - f32(i.z);
    let c000 = lumetri_rgb(off + (i.x + i.y * n + i.z * n * n) * 3u);
    let c111 = lumetri_rgb(off + ((i.x + 1u) + (i.y + 1u) * n + (i.z + 1u) * n * n) * 3u);
    var ca: vec3<f32>;
    var cb: vec3<f32>;
    var w: vec4<f32>;
    let r1 = i.x + 1u;
    let g1 = (i.y + 1u) * n;
    let b1 = (i.z + 1u) * n * n;
    let r0 = i.x;
    let g0 = i.y * n;
    let b0 = i.z * n * n;
    if fr > fg {
        if fg > fb {
            ca = lumetri_rgb(off + (r1 + g0 + b0) * 3u); cb = lumetri_rgb(off + (r1 + g1 + b0) * 3u); w = vec4(1.0 - fr, fr - fg, fg - fb, fb);
        } else if fr > fb {
            ca = lumetri_rgb(off + (r1 + g0 + b0) * 3u); cb = lumetri_rgb(off + (r1 + g0 + b1) * 3u); w = vec4(1.0 - fr, fr - fb, fb - fg, fg);
        } else {
            ca = lumetri_rgb(off + (r0 + g0 + b1) * 3u); cb = lumetri_rgb(off + (r1 + g0 + b1) * 3u); w = vec4(1.0 - fb, fb - fr, fr - fg, fg);
        }
    } else if fb > fg {
        ca = lumetri_rgb(off + (r0 + g0 + b1) * 3u); cb = lumetri_rgb(off + (r0 + g1 + b1) * 3u); w = vec4(1.0 - fb, fb - fg, fg - fr, fr);
    } else if fb > fr {
        ca = lumetri_rgb(off + (r0 + g1 + b0) * 3u); cb = lumetri_rgb(off + (r0 + g1 + b1) * 3u); w = vec4(1.0 - fg, fg - fb, fb - fr, fr);
    } else {
        ca = lumetri_rgb(off + (r0 + g1 + b0) * 3u); cb = lumetri_rgb(off + (r1 + g1 + b0) * 3u); w = vec4(1.0 - fg, fg - fr, fr - fb, fb);
    }
    return w.x * c000 + w.y * ca + w.z * cb + w.w * c111;
}

// `Lut::apply` of the LUT described at `at`: its shaper, then its cube.
fn lumetri_lut(at: u32, c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    if lumetri_data(at) > 0.5 {
        c = lumetri_lut1d(at, c);
    }
    if lumetri_data(at + 9u) > 1.5 {
        c = lumetri_lut3d(at, c);
    }
    return c;
}

fn look_s_curve(v: f32, k: f32) -> f32 {
    let x = clamp(v, 0.0, 1.0);
    return x + (x * x * (3.0 - 2.0 * x) - x) * k;
}

// `effects::apply_look`: the procedural Looks.
fn lumetri_look(look: u32, c: vec3<f32>) -> vec3<f32> {
    let l = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
    let g = vec3<f32>(l);
    switch look {
        case 1u: {
            let t = c + vec3<f32>(0.0, 0.08, 0.1) * (1.0 - l) + vec3<f32>(0.1, 0.04, -0.06) * l;
            return vec3<f32>(look_s_curve(t.x, 0.35), look_s_curve(t.y, 0.35), look_s_curve(t.z, 0.35));
        }
        case 2u: {
            let t = vec3<f32>(c.x * 1.06 + 0.02, c.y * 1.0 + 0.01, c.z * 0.9);
            return (t + (g - t) * 0.15) * 0.94 + vec3<f32>(0.04);
        }
        case 3u: {
            let t = vec3<f32>(c.x * 0.9, c.y * 0.98, c.z * 1.1 + 0.02);
            return vec3<f32>(look_s_curve(t.x, 0.2), look_s_curve(t.y, 0.2), look_s_curve(t.z, 0.2));
        }
        case 4u: {
            let t = c + (g - c) * 0.55;
            return vec3<f32>(look_s_curve(t.x, 0.6), look_s_curve(t.y, 0.6), look_s_curve(t.z, 0.6));
        }
        case 5u: {
            let t = c + (g - c) * 0.25;
            return vec3<f32>(0.08) + t * 0.84;
        }
        case 6u: {
            return vec3<f32>(look_s_curve(l, 0.3));
        }
        case 7u: {
            let t = vec3<f32>(c.x * 1.1 + 0.03, c.y * 1.02 + 0.01, c.z * 0.82);
            return vec3<f32>(look_s_curve(t.x, 0.15), look_s_curve(t.y, 0.15), look_s_curve(t.z, 0.15));
        }
        case 8u: {
            return vec3<f32>(c.x * 0.75, c.y * 0.85, c.z * 1.15) * 0.8;
        }
        default: {
            return c;
        }
    }
}

// Lumetri's grading signals: Basic (mode u.i1.z) and Advanced (mode u.i1.w), parameters in the
// data header (only read in HDR, where the data buffer always exists).
fn lumetri_lw() -> vec3<f32> {
    return vec3<f32>(lumetri_data(65u), lumetri_data(66u), lumetri_data(67u));
}
fn lumetri_enc(c: vec3<f32>, basic: bool) -> vec3<f32> {
    let mode = select(u.i1.w, u.i1.z, basic);
    if mode == 0u {
        return enc(c);
    }
    let at = select(70u, 68u, basic);
    return gs_encode(c, mode, lumetri_data(at), lumetri_data(at + 1u), lumetri_lw());
}
fn lumetri_dec(v: vec3<f32>, basic: bool) -> vec3<f32> {
    let mode = select(u.i1.w, u.i1.z, basic);
    if mode == 0u {
        return dec(v);
    }
    let at = select(70u, 68u, basic);
    return gs_decode(v, mode, lumetri_data(at), lumetri_data(at + 1u), lumetri_lw());
}

// A LUT on a grading signal (`within_unit`: SDR clamps to 0..1, HDR scales into it and back).
fn lumetri_lut_unit(at: u32, v: vec3<f32>, hdr: bool) -> vec3<f32> {
    if !hdr {
        return lumetri_lut(at, clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    let m = unit_scale(v);
    return lumetri_lut(at, max(v / m, vec3<f32>(0.0))) * m;
}

// `LumetriExtra::advanced`: Look, color wheels, RGB curves, hue / saturation / luma curves.
fn lumetri_advanced(v0: vec3<f32>) -> vec3<f32> {
    var v = v0;
    let look = u.i1.y;
    let hdr = u.i1.w != 0u;
    if (u.i1.x & (1u << 12u)) != 0u {
        let lk = lumetri_lut_unit(42u, v, hdr);
        v = v + (lk - v) * lumetri_data(20u);
    } else if look > 0u {
        var lk: vec3<f32>;
        if hdr {
            let m = unit_scale(v);
            lk = lumetri_look(look, max(v / m, vec3<f32>(0.0))) * m;
        } else {
            lk = lumetri_look(look, v);
        }
        v = v + (lk - v) * lumetri_data(20u);
    }
    if (u.i1.x & 2u) != 0u {
        let l = clamp(0.2126 * v.x + 0.7152 * v.y + 0.0722 * v.z, 0.0, 1.0);
        let wsh = (1.0 - l) * (1.0 - l);
        let whi = l * l;
        let wmid = max(1.0 - wsh - whi, 0.0);
        let ws = vec3<f32>(lumetri_data(8u), lumetri_data(9u), lumetri_data(10u));
        let wm = vec3<f32>(lumetri_data(11u), lumetri_data(12u), lumetri_data(13u));
        let wh = vec3<f32>(lumetri_data(14u), lumetri_data(15u), lumetri_data(16u));
        let ls = lumetri_data(17u);
        let lm = lumetri_data(18u);
        let lh = lumetri_data(19u);
        v = v + (ws * 0.3 + vec3<f32>(ls * 0.3)) * wsh;
        v = v + (wm * 0.3 + vec3<f32>(lm * 0.3)) * wmid;
        v = v * (vec3<f32>(1.0) + (wh * 0.5 + vec3<f32>(lh * 0.5)) * whi);
    }
    if lumetri_has_curve(0u) {
        v = vec3<f32>(lumetri_curve(0u, v.x), lumetri_curve(0u, v.y), lumetri_curve(0u, v.z));
    }
    if lumetri_has_curve(1u) {
        v.x = lumetri_curve(1u, v.x);
    }
    if lumetri_has_curve(2u) {
        v.y = lumetri_curve(2u, v.y);
    }
    if lumetri_has_curve(3u) {
        v.z = lumetri_curve(3u, v.z);
    }
    if (u.i1.x & (31u << 6u)) != 0u {
        var m = 1.0;
        var uu = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
        if hdr {
            m = unit_scale(v);
            uu = max(v / m, vec3<f32>(0.0));
        }
        var hsl = rgb_to_hsl(uu);
        let h0 = hsl.x;
        let s0 = hsl.y;
        let l0 = hsl.z;
        if lumetri_has_curve(5u) {
            hsl.x = fract_euclid(hsl.x + (lumetri_curve(5u, h0) - 0.5));
        }
        var sm = 1.0;
        if lumetri_has_curve(4u) {
            sm = sm * lumetri_curve(4u, h0) * 2.0;
        }
        if lumetri_has_curve(7u) {
            sm = sm * lumetri_curve(7u, l0) * 2.0;
        }
        if lumetri_has_curve(8u) {
            sm = sm * lumetri_curve(8u, s0) * 2.0;
        }
        hsl.y = clamp(hsl.y * sm, 0.0, 1.0);
        if lumetri_has_curve(6u) {
            hsl.z = clamp(hsl.z + (lumetri_curve(6u, h0) - 0.5) * 0.5, 0.0, 1.0);
        }
        v = hsl_to_rgb(hsl.x, hsl.y, hsl.z) * m;
    }
    return v;
}
// ---------- Lumetri ▸ HSL Secondary (`effects::HslSecondary`) ----------

// `effects::hsl_key` (key parameters in p0.x..p1.y: hue centre / range, saturation minimum,
// lightness range, softness).
fn hsl_secondary_key(v: vec3<f32>) -> f32 {
    let hsl = rgb_to_hsl(v);
    let hc = u.p0.x; let hr = u.p0.y; let smin = u.p0.z; let lmin = u.p0.w; let lmax = u.p1.x; let soft = u.p1.y;
    let d = abs(hsl.x - hc);
    let dh = min(d, 1.0 - d);
    let mh = 1.0 - clamp((dh - hr / 2.0) / soft, 0.0, 1.0);
    let ms = clamp((hsl.y - smin) / soft, 0.0, 1.0);
    let ml = min(clamp((hsl.z - lmin) / soft, 0.0, 1.0), clamp((lmax - hsl.z) / soft, 0.0, 1.0));
    return mh * ms * ml;
}

// `effects::median_filter`: the median of the window (shrunk at the edges), mixed into the key
// by the Denoise amount.
fn hsl_secondary_median(p: vec2<i32>) -> f32 {
    let r = i32(u.i1.x);
    let s = size();
    var win: array<f32, 49>;
    var n = 0u;
    for (var y = max(p.y - r, 0); y < min(p.y + r + 1, s.y); y = y + 1) {
        for (var x = max(p.x - r, 0); x < min(p.x + r + 1, s.x); x = x + 1) {
            if n < 49u {
                win[n] = ld(vec2<i32>(x, y)).x;
                n = n + 1u;
            }
        }
    }
    // the (n / 2)-th smallest, by selection
    let k = n / 2u;
    for (var i = 0u; i <= k; i = i + 1u) {
        var m = i;
        for (var j = i + 1u; j < n; j = j + 1u) {
            if win[j] < win[m] {
                m = j;
            }
        }
        let t = win[i];
        win[i] = win[m];
        win[m] = t;
    }
    let c = ld(p).x;
    return c + (win[k] - c) * u.p0.x;
}
// ---------- Video Limiter (`vfx::color::VideoLimiter`) ----------

// sRGB encoding that keeps super-whites (above 1: a 1/2.4 slope) and its inverse.
fn limiter_enc(v: f32) -> f32 {
    if v <= 1.0 {
        return linear_to_srgb1(max(v, 0.0));
    }
    return 1.0 + (v - 1.0) / 2.4;
}
fn limiter_dec(v: f32) -> f32 {
    if v <= 1.0 {
        return srgb_to_linear1(max(v, 0.0));
    }
    return 1.0 + (v - 1.0) * 2.4;
}

fn limiter_knee(v: f32, mx: f32, comp: f32) -> f32 {
    let k = mx * (1.0 - comp);
    if comp <= 0.0 || v <= k {
        return min(v, mx);
    }
    let r = mx - k;
    return k + r * (1.0 - exp(-(v - k) / r));
}

fn limiter_rgb(y: f32, cb: f32, cr: f32) -> vec3<f32> {
    let kr = 0.2126;
    let kb = 0.0722;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    return vec3<f32>(r, g, b);
}

// `vfx::color::limit` (1e-4 = `GAMUT_EPS`)
fn limiter_limit(v: vec3<f32>, mx: f32, comp: f32, axis: u32) -> vec3<f32> {
    let kr = 0.2126;
    let kb = 0.0722;
    let kg = 1.0 - kr - kb;
    var y = kr * v.x + kg * v.y + kb * v.z;
    var cb = (v.z - y) / (2.0 * (1.0 - kb));
    var cr = (v.x - y) / (2.0 * (1.0 - kr));
    if axis != 1u {
        y = limiter_knee(max(y, 0.0), mx, comp);
    }
    if axis != 0u {
        let rgb = limiter_rgb(y, cb, cr);
        var s = 1.0;
        for (var k = 0u; k < 3u; k = k + 1u) {
            let ch = rgb[k];
            let d = ch - y;
            if ch > mx + 1e-4 && d > 1e-6 {
                s = min(s, max((mx - y) / d, 0.0));
            }
            if ch < -1e-4 && d < -1e-6 {
                s = min(s, max(y / -d, 0.0));
            }
        }
        if axis == 3u && s < 1.0 {
            s = min(limiter_knee(s, 1.0, max(comp, 0.03)), 1.0);
        }
        cb = cb * s;
        cr = cr * s;
    }
    return clamp(limiter_rgb(y, cb, cr), vec3<f32>(0.0), vec3<f32>(mx));
}
// ---------- Ultra Key (`vfx::color::UltraKey`) ----------

// `UltraKey::matte` of a premultiplied pixel; the key in p0 (Y′CbCr, tolerance), gain, highlight,
// shadow and pedestal in p1.
fn ukey_matte(o: vec4<f32>) -> f32 {
    if o.a <= 1e-6 {
        return 0.0;
    }
    let v = enc(o.rgb / o.a);
    let kr = 0.2126;
    let kb = 0.0722;
    let y = kr * v.x + (1.0 - kr - kb) * v.y + kb * v.z;
    let cb = (v.z - y) / (2.0 * (1.0 - kb));
    let cr = (v.x - y) / (2.0 * (1.0 - kr));
    let k = u.p0.xyz;
    let kmag2 = max(k.y * k.y + k.z * k.z, 1e-6);
    let proj = (cb * k.y + cr * k.z) / kmag2;
    let perp = sqrt((cb - proj * k.y) * (cb - proj * k.y) + (cr - proj * k.z) * (cr - proj * k.z)) / sqrt(kmag2);
    let keyness = clamp(proj, 0.0, 1.5) * (1.0 - smoothstep_fx(0.0, u.p0.w, perp));
    var m = clamp(1.0 - keyness * u.p1.x, 0.0, 1.0);
    m = m + max(y - k.x, 0.0) * u.p1.y * 2.0 * min(keyness, 1.0);
    m = m + max(k.x - y, 0.0) * u.p1.z * 2.0 * min(keyness, 1.0);
    return clamp((clamp(m, 0.0, 1.0) - u.p1.w) / (1.0 - u.p1.w), 0.0, 1.0);
}

// `UltraKey::finish`: contrast of the cleaned matte, spill suppression, colour correction, output.
fn ukey_finish(o: vec4<f32>, a0: f32) -> vec4<f32> {
    var a = a0;
    if u.p0.x > 1.0 {
        a = clamp((a - u.p0.y) * u.p0.x + u.p0.y, 0.0, 1.0);
    }
    let src_a = o.a;
    var v = vec3<f32>(0.0);
    if src_a > 1e-6 {
        v = enc(o.rgb / src_a);
    }
    let dom = u.i1.x;
    let o1 = v[(dom + 1u) % 3u];
    let o2 = v[(dom + 2u) % 3u];
    let range = u.p1.x;
    let limit = max(o1, o2) * (1.0 - range) + (o1 + o2) * 0.5 * range;
    let excess = max(v[dom] - limit, 0.0);
    if excess > 0.0 && u.p0.z > 0.0 {
        let l0 = luma709(v);
        // (a mask, not `v[dom] = …`: FXC aborts on a dynamically indexed vector write)
        v = v - select(vec3(0.0), vec3(excess * u.p0.z), vec3(0u, 1u, 2u) == vec3(dom));
        let l1 = luma709(v);
        v = vec3<f32>(l1) + (v - vec3<f32>(l1)) * (1.0 - u.p0.w * min(excess * 4.0, 1.0));
        v = v + vec3<f32>((l0 - l1) * u.p1.y);
    }
    if u.i1.z != 0u {
        var h = rgb_to_hsl(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)));
        h.x = fract_euclid(h.x + u.p2.x);
        h.y = clamp(h.y * u.p2.y, 0.0, 1.0);
        h.z = clamp(h.z * u.p2.z, 0.0, 1.0);
        v = hsl_to_rgb(h.x, h.y, h.z);
    }
    let l = dec(v);
    switch u.i1.y {
        case 1u: {
            let g = srgb_to_linear1(a);
            return vec4<f32>(g * src_a, g * src_a, g * src_a, src_a);
        }
        case 2u: {
            return vec4(l * src_a, src_a);
        }
        default: {
            let na = a * src_a;
            return vec4(l * na, na);
        }
    }
}

fn pixel(op: u32, p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let pc = vec2<f32>(p) + 0.5;
    switch op {
        case OP_MASK: {
            let cov = mask_coverage(pc);
            let e = ld(p);
            // i1.y: Opacity masks scale the layer (`mask::scale_by`) …
            if u.i1.y != 0u {
                return e * cov;
            }
            // … effect masks mix the effect with the image before it (`mask::mix`)
            let o = textureLoad(aux, p, 0);
            return o + (e - o) * cov;
        }
        case OP_INVERT_ALPHA: {
            let o = ld(p);
            let a = o.a;
            let na = 1.0 - a;
            let k = select(0.0, na / a, a > 1e-6);
            let blend = u.p0.x;
            return vec4(o.rgb * k, na * (1.0 - blend) + a * blend);
        }
        case OP_ALPHA_ADJUST: {
            let o = ld(p);
            let c = select(o.rgb / o.a, vec3(0.0), o.a <= 1e-6);
            var a = select(o.a, 1.0, u.i1.x != 0u);
            if u.i1.y != 0u {
                a = 1.0 - a;
            }
            a = clamp(a * u.p0.x, 0.0, 1.0);
            if u.i1.z != 0u {
                return vec4(vec3(srgb_to_linear1(a)), 1.0);
            }
            return vec4(c * a, a);
        }
        case OP_BOX: {
            // one box pass along x (i1.z = 0) or y (1): mean over 2r+1 pixels, clamped to the
            // edge (repeat) or skipping pixels outside, as `effects::box_rows`
            let r = i32(u.i1.x);
            let repeat = u.i1.y != 0u;
            let vertical = u.i1.z != 0u;
            let n = select(s.x, s.y, vertical);
            let x = select(p.x, p.y, vertical);
            var acc = vec4(0.0);
            for (var i = x - r; i <= x + r; i++) {
                if repeat || (i >= 0 && i < n) {
                    let j = clamp(i, 0, n - 1);
                    acc += ld(select(vec2(j, p.y), vec2(p.x, j), vertical));
                }
            }
            return box_masked(p, acc * (1.0 / f32(2 * r + 1)));
        }
        case OP_DIRECTIONAL: {
            let steps = u.i1.x;
            let dx = u.p0.x; let dy = u.p0.y;
            var acc = vec4(0.0);
            for (var i = 0u; i < steps; i++) {
                let t = f32(i) / f32(steps - 1u) - 0.5;
                acc += sample_clamped(pc.x + dx * t, pc.y + dy * t);
            }
            return acc / f32(steps);
        }
        case OP_UNSHARP: {
            // src: the blurred image, aux: the original
            // per channel without a dynamically indexed vector write: FXC (DX12 without
            // dxcompiler.dll) aborts on `o[k] = …` in a loop
            let o = textureLoad(aux, p, 0);
            let bq = ld(p);
            let amount = u.p0.x; let th = u.p0.y;
            let d = o.rgb - bq.rgb;
            let sharpened = max(o.rgb + d * amount, vec3(0.0));
            return vec4(select(o.rgb, sharpened, abs(d) >= vec3(th * o.a)), o.a);
        }
        case OP_CROP: {
            let x0 = u.p0.x; let x1 = u.p0.y; let y0 = u.p0.z; let y1 = u.p0.w; let fe = u.p1.x;
            let d = min(min(pc.x - x0, x1 - pc.x), min(pc.y - y0, y1 - pc.y));
            var a: f32;
            if fe > 0.0 {
                a = clamp(d / fe, 0.0, 1.0);
            } else {
                a = clamp(d + 0.5, 0.0, 1.0);
            }
            let o = ld(p);
            return select(o, o * a, a < 1.0);
        }
        case OP_RESAMPLE: {
            // inverse map p0 = (a b c d), p1 = (e f opacity valid); rect i1 = x0 x1 y0 y1
            if u.p1.w == 0.0 || u32(p.x) < u.i1.x || u32(p.x) >= u.i1.y || u32(p.y) < u.i1.z || u32(p.y) >= u.i1.w {
                return vec4(0.0);
            }
            let m0 = u.p0; let m1 = u.p1;
            let uu = m0.x * pc.x + m0.z * pc.y + m1.x;
            let vv = m0.y * pc.x + m0.w * pc.y + m1.y;
            if uu < -1.0 || vv < -1.0 || uu > f32(s.x) + 1.0 || vv > f32(s.y) + 1.0 {
                return vec4(0.0);
            }
            return sample_bilinear(uu, vv) * m1.z;
        }
        case OP_HFLIP: {
            return ld(vec2(s.x - 1 - p.x, p.y));
        }
        case OP_VFLIP: {
            return ld(vec2(p.x, s.y - 1 - p.y));
        }
        case OP_MIRROR: {
            let c = u.p0.xy; let nrm = u.p0.zw;
            let d = (pc.x - c.x) * nrm.x + (pc.y - c.y) * nrm.y;
            if d > 0.0 {
                return sample_bilinear(pc.x - 2.0 * d * nrm.x, pc.y - 2.0 * d * nrm.y);
            }
            return ld(p);
        }
        case OP_OFFSET: {
            let w = f32(s.x); let h = f32(s.y);
            // dx, dy in 0..w / 0..h, so the shifted coordinate is within (-w, w + 0.5)
            var uu = pc.x - u.p0.x;
            var vv = pc.y - u.p0.y;
            if uu < 0.0 { uu += w; }
            if uu >= w { uu -= w; }
            if vv < 0.0 { vv += h; }
            if vv >= h { vv -= h; }
            let q = sample_clamped(uu, vv);
            let o = ld(p);
            return q + (o - q) * u.p0.z;
        }
        case OP_MOSAIC: {
            // `effects::mosaic_blocks`: every pixel takes the picture at its block's centre
            let bw = u.p0.x; let bh = u.p0.y;
            return sample_clamped((floor(f32(p.x) / bw) + 0.5) * bw, (floor(f32(p.y) / bh) + 0.5) * bh);
        }
        case OP_VIGNETTE: {
            // `vfx::color::vignette_px`: p0 = amount, midpoint, roundness, feather; p1.xyz = target
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 || u.p0.x < 1e-5 {
                return o;
            }
            let w = f32(s.x); let h = f32(s.y);
            let round = u.p0.z;
            var nx = pc.x / w * 2.0 - 1.0;
            let ny = pc.y / h * 2.0 - 1.0;
            if round > 0.0 {
                nx *= 1.0 + (w / h - 1.0) * round;
            }
            var pw = 2.0;
            if round < 0.0 {
                pw = 2.0 + (-round) * 6.0;
            }
            let d = powf(powf(abs(nx), pw) + powf(abs(ny), pw), 1.0 / pw) / powf(2.0, 1.0 / pw);
            let edge = smoothstep_fx(u.p0.y - u.p0.w * 0.5, u.p0.y + u.p0.w * 0.5, d);
            let e = enc(o.rgb / a);
            return vec4(dec(e + (u.p1.xyz - e) * (edge * u.p0.x)) * a, a);
        }
        case OP_HSL_KEY: {
            let o = ld(p);
            if o.a <= 1e-6 {
                return vec4<f32>(0.0);
            }
            // the grading signal in 0..1 (HDR: scaled into it), grade space in i1.y / p2.xy / p3.xyz
            let v = gs_encode(o.rgb / o.a, u.i1.y, u.p2.x, u.p2.y, u.p3.xyz);
            var uu = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
            if u.i1.y != 0u {
                uu = max(v / unit_scale(v), vec3<f32>(0.0));
            }
            let k = hsl_secondary_key(uu);
            return vec4<f32>(k);
        }
        case OP_HSL_MEDIAN: {
            return vec4<f32>(hsl_secondary_median(p));
        }
        case OP_HSL_APPLY: {
            // src: the key, aux: the image
            let o = textureLoad(aux, p, 0);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let m = ld(p).x;
            // the grading signal (grade space in i1.y / p1.xy / p2.xyz); HDR scales it into 0..1
            let v = gs_encode(o.rgb / a, u.i1.y, u.p1.x, u.p1.y, u.p2.xyz);
            var scale = 1.0;
            var uu = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
            if u.i1.y != 0u {
                scale = unit_scale(v);
                uu = max(v / scale, vec3<f32>(0.0));
            }
            var out: vec3<f32>;
            switch u.i1.x {
                case 1u: {
                    let g = rgb_to_hsl(uu).z;
                    out = vec3<f32>(g) + (uu - vec3<f32>(g)) * m;
                }
                case 2u: {
                    out = uu * m;
                }
                case 3u: {
                    out = vec3<f32>(m);
                }
                default: {
                    var hh = rgb_to_hsl(uu);
                    hh.x = fract_euclid(hh.x + u.p0.w);
                    hh.y = clamp(hh.y * u.p0.z, 0.0, 1.0);
                    var c2 = hsl_to_rgb(hh.x, hh.y, hh.z);
                    c2.x = c2.x * (1.0 + 0.25 * u.p0.x);
                    c2.z = c2.z * (1.0 - 0.25 * u.p0.x);
                    c2.y = c2.y * (1.0 - 0.2 * u.p0.y);
                    out = uu + (c2 - uu) * m;
                }
            }
            // the mask views are display images (no HDR scaling)
            if u.i1.x == 0u {
                out = out * scale;
            }
            return vec4(gs_decode(out, u.i1.y, u.p1.x, u.p1.y, u.p2.xyz) * a, a);
        }
        case OP_KEY: {
            // `effects::Key::apply`: distance to the key colour in Y'CbCr (Rec. 709)
            let o = ld(p);
            if o.a <= 0.0 {
                return o;
            }
            let c = enc(o.rgb / o.a);
            let kr = 0.2126;
            let kb = 0.0722;
            let kg = 1.0 - kr - kb;
            let y = kr * c.x + kg * c.y + kb * c.z;
            let cb = (c.z - y) / (2.0 * (1.0 - kb));
            let cr = (c.x - y) / (2.0 * (1.0 - kr));
            let k = u.p0.xyz;
            let tol = u.p0.w;
            let soft = u.p1.x;
            let d = sqrt((cb - k.y) * (cb - k.y) + (cr - k.z) * (cr - k.z)) + abs(y - k.x) * 0.15;
            let a = o.a * clamp((d - tol) / soft, 0.0, 1.0);
            return vec4(dec(c) * a, a);
        }
        case OP_LUMA_KEY: {
            // `effects::luma_key`
            let o = ld(p);
            if o.a <= 0.0 {
                return o;
            }
            let th = u.p0.x;
            let cut = u.p0.y;
            let l = linear_to_srgb1(max(luma709(o.rgb / o.a), 0.0));
            var a: f32;
            if l <= cut {
                a = 0.0;
            } else if l >= max(th, cut + 1e-3) {
                a = 1.0;
            } else {
                a = (l - cut) / max(th - cut, 1e-3);
            }
            return o * a;
        }
        case OP_VIDEO_LIMITER: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
            let v = vec3<f32>(limiter_enc(c.x), limiter_enc(c.y), limiter_enc(c.z));
            let out = limiter_limit(v, u.p0.x, u.p0.y, u.i1.x);
            if u.i1.y != 0u && any(abs(v - out) > vec3<f32>(1e-4)) {
                return vec4(u.p1.xyz * a, a);
            }
            return vec4(vec3<f32>(limiter_dec(out.x), limiter_dec(out.y), limiter_dec(out.z)) * a, a);
        }
        case OP_UKEY_MATTE: {
            return vec4<f32>(ukey_matte(ld(p)), 0.0, 0.0, 0.0);
        }
        case OP_UKEY_CHOKE: {
            // src: the matte's blur, aux: the image (its matte made again)
            let c = u.p2.x;
            let m = ukey_matte(textureLoad(aux, p, 0));
            return vec4<f32>(min(m, clamp((ld(p).x - c) / (1.0 - c), 0.0, 1.0)), 0.0, 0.0, 0.0);
        }
        case OP_UKEY_APPLY: {
            // src: the cleaned matte, aux: the image
            return ukey_finish(textureLoad(aux, p, 0), ld(p).x);
        }
        case OP_LUMETRI: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            var c = o.rgb / a;
            // Basic ▸ Input LUT, first (on the display-encoded value, clamped to 0..1)
            let hdr = u.i1.z != 0u;
            if (u.i1.x & (1u << 11u)) != 0u {
                c = lumetri_dec(lumetri_lut_unit(24u, lumetri_enc(c, true), hdr), true);
            }
            let temp = u.p0.x;
            let tint = u.p0.y;
            let exposure = u.p0.z;
            let contrast = u.p0.w;

            let hl = u.p1.x;
            let sh = u.p1.y;
            let wh = u.p1.z;
            let bl = u.p1.w;

            let sat = u.p2.x;
            let vib = u.p2.y;
            let faded = u.p2.z;
            let va = u.p2.w;

            let vmid = u.p3.x;
            let vround = u.p3.y;
            let vfeather = u.p3.z;
            let aspect = u.p3.w;

            let gains = vec3<f32>(1.0 + 0.35 * temp, 1.0 - 0.3 * tint, 1.0 - 0.35 * temp);
            let lin = vec3<f32>(c.x * gains.x * exposure, c.y * gains.y * exposure, c.z * gains.z * exposure);
            var v = lumetri_enc(lin, true);

            let b0 = -bl * 0.15;
            let w0 = 1.0 - wh * 0.15;
            let denom = max(w0 - b0, 1e-3);
            v = (v - vec3<f32>(b0)) / denom;

            let l = 0.2126 * v.x + 0.7152 * v.y + 0.0722 * v.z;
            let ws = powf(clamp(1.0 - l, 0.0, 1.0), 3.0);
            let whl = powf(clamp(l, 0.0, 1.0), 3.0);
            let nl = max(l + sh * 0.35 * ws + hl * 0.35 * whl, 0.0);
            if l > 1e-5 {
                let k = nl / l;
                v = v * k;
            }

            if abs(contrast) > 1e-4 {
                let k = 1.0 + contrast;
                let q = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
                let s = q * q * (vec3<f32>(3.0) - 2.0 * q);
                var cv: vec3<f32>;
                if k >= 1.0 {
                    cv = q + (s - q) * (k - 1.0);
                } else {
                    cv = vec3<f32>(0.5) + (q - vec3<f32>(0.5)) * k;
                }
                // HDR: speculars above HDR White keep their distance from white
                v = select(cv, v, (v > vec3<f32>(1.0)) & vec3<bool>(hdr));
            }

            // HDR Specular: brightness of the highlights above HDR White
            if hdr {
                let sp = lumetri_data(64u);
                if abs(sp) > 1e-4 {
                    v = select(v, vec3<f32>(1.0) + (v - vec3<f32>(1.0)) * max(1.0 + sp, 0.0), v > vec3<f32>(1.0));
                }
            }

            if faded > 0.0 {
                v = v * (1.0 - 0.25 * faded) + vec3<f32>(0.12 * faded);
            }

            // Creative split tone (shadow / highlight tint minus 0.5 in data 0..6)
            if (u.i1.x & 1u) != 0u {
                let l2 = clamp(0.2126 * v.x + 0.7152 * v.y + 0.0722 * v.z, 0.0, 1.0);
                let st = vec3<f32>(lumetri_data(0u), lumetri_data(1u), lumetri_data(2u));
                let ht = vec3<f32>(lumetri_data(3u), lumetri_data(4u), lumetri_data(5u));
                v = v + st * 0.3 * (1.0 - l2) + ht * 0.3 * l2;
            }

            let l3 = 0.2126 * v.x + 0.7152 * v.y + 0.0722 * v.z;
            let max_c = max(v.x, max(v.y, v.z));
            let min_c = min(v.x, min(v.y, v.z));
            let cur_sat = max_c - min_c;
            let s_factor = sat * (1.0 + vib * (1.0 - clamp(cur_sat, 0.0, 1.0)));
            v = vec3<f32>(l3) + (v - vec3<f32>(l3)) * s_factor;

            if abs(va) > 1e-4 {
                var round_scale = 1.0;
                if vround < 0.0 {
                    round_scale = powf(aspect, -vround);
                }
                let s = size();
                let nx = (f32(p.x) / f32(s.x) - 0.5) * 2.0 * round_scale;
                let ny = (f32(p.y) / f32(s.y) - 0.5) * 2.0;
                let d = sqrt(nx * nx + ny * ny) / 1.41421356;
                let edge = clamp((d - vmid * 0.9) / (max(vfeather, 0.01) * 0.9), 0.0, 1.0);
                let e2 = edge * edge * (3.0 - 2.0 * edge);
                let k = 1.0 + va * 0.2 * e2;
                if va < 0.0 {
                    v = v * max(k, 0.0);
                } else {
                    v = v + (vec3<f32>(1.0) - v) * (k - 1.0);
                }
            }

            // the Advanced stage (Look, wheels, curves) reads the pixel the Basic stage wrote
            if u.i1.y > 0u || (u.i1.x & ~(1u | (1u << 11u))) != 0u {
                v = lumetri_advanced(lumetri_enc(lumetri_dec(v, true), false));
                return vec4(lumetri_dec(v, false) * a, a);
            }
            return vec4(lumetri_dec(v, true) * a, a);
        }
        default: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            return vec4(color_op(op, o.rgb / a) * a, a);
        }
    }
}

@compute @workgroup_size(16, 16)
fn fx_px(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u.i0.y || id.y >= u.i0.z {
        return;
    }
    let p = vec2<i32>(id.xy);
    if u.i0.x >= OP_NOISE {
        textureStore(dst, p, ext_pixel(u.i0.x, p));
        return;
    }
    textureStore(dst, p, pixel(u.i0.x, p));
}

// A box pass on the channels of the mask i1.w (bit k: channel k; 0: every channel): the others
// are copied (Channel Blur, and the planes the glows keep in single channels).
fn box_masked(p: vec2<i32>, v: vec4<f32>) -> vec4<f32> {
    let m = u.i1.w;
    if m == 0u {
        return v;
    }
    return select(ld(p), v, (vec4(m) & vec4(1u, 2u, 4u, 8u)) != vec4(0u));
}

// A box pass as a running sum along one row (i1.z = 0) or column (1) per invocation: O(1) per
// pixel for any radius, the arithmetic of `effects::box_rows`.
@compute @workgroup_size(64)
fn fx_run(@builtin(global_invocation_id) id: vec3<u32>) {
    let s = size();
    let vertical = u.i1.z != 0u;
    let n = select(s.x, s.y, vertical);
    let lines = select(s.y, s.x, vertical);
    let line = i32(id.x);
    if line >= lines {
        return;
    }
    let r = i32(u.i1.x);
    let repeat = u.i1.y != 0u;
    let inv = 1.0 / f32(2 * r + 1);
    var acc = vec4(0.0);
    for (var i = -r; i <= r; i++) {
        if repeat || (i >= 0 && i < n) {
            let j = clamp(i, 0, n - 1);
            acc += ld(select(vec2(j, line), vec2(line, j), vertical));
        }
    }
    for (var x = 0; x < n; x++) {
        let at = select(vec2(x, line), vec2(line, x), vertical);
        textureStore(dst, at, box_masked(at, acc * inv));
        let out_i = x - r;
        let in_i = x + r + 1;
        if repeat || out_i >= 0 {
            let j = clamp(out_i, 0, n - 1);
            acc -= ld(select(vec2(j, line), vec2(line, j), vertical));
        }
        if repeat || in_i < n {
            let j = clamp(in_i, 0, n - 1);
            acc += ld(select(vec2(j, line), vec2(line, j), vertical));
        }
    }
}

// ---- GPU3: `filmcraft_render::gpufx_ext::ExtOp::apply`

// 64-bit unsigned arithmetic on (lo, hi) pairs, wrapping as Rust's `wrapping_*` (Noise's hash).
fn mul32_wide(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu; let a1 = a >> 16u;
    let b0 = b & 0xffffu; let b1 = b >> 16u;
    let p00 = a0 * b0; let p01 = a0 * b1; let p10 = a1 * b0; let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    return vec2((p00 & 0xffffu) | (mid << 16u), p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u));
}

fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let p = mul32_wide(a.x, b.x);
    return vec2(p.x, p.y + a.x * b.y + a.y * b.x);
}

fn shr64(a: vec2<u32>, n: u32) -> vec2<u32> {
    return vec2((a.x >> n) | (a.y << (32u - n)), a.y >> n);
}

fn add64(a: vec2<u32>, c: u32) -> vec2<u32> {
    let lo = a.x + c;
    return vec2(lo, a.y + select(0u, 1u, lo < a.x));
}

// `effects::hash3`
fn hash3(x: u32, y: u32, z: vec2<u32>) -> f32 {
    var h = mul64(vec2(x, 0u), vec2(0x7F4A7C15u, 0x9E3779B9u)) ^ mul64(vec2(y, 0u), vec2(0x27D4EB4Fu, 0xC2B2AE3Du))
        ^ mul64(z, vec2(0x9E3779F9u, 0x165667B1u));
    h = h ^ shr64(h, 31u);
    h = mul64(h, vec2(0x1CE4E5B9u, 0xBF58476Du));
    h = h ^ shr64(h, 29u);
    return f32(h.y >> 8u) / 16777216.0;
}

// `Image::unpremul`
fn unpremul(p: vec4<f32>) -> vec3<f32> {
    return select(p.rgb / p.a, vec3(0.0), p.a <= 1e-6);
}

// `vfx::blend_simple`
fn blend_simple(mode: u32, b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return b + s; }
        case 2u: { return 1.0 - (1.0 - min(b, vec3(1.0))) * (1.0 - min(s, vec3(1.0))); }
        case 3u: { return b * s; }
        case 4u: {
            let hi = 1.0 - 2.0 * (1.0 - min(b, vec3(1.0))) * (1.0 - min(s, vec3(1.0)));
            return select(hi, 2.0 * b * s, b < vec3(0.5));
        }
        default: { return s; }
    }
}

// `Warp::map`; (1e30, 1e30) for "transparent" (None): outside any image.
fn warp_map(x: f32, y: f32) -> vec2<f32> {
    let none = vec2(1e30, 1e30);
    let p0 = u.p0; let p1 = u.p1; let p2 = u.p2; let p3 = u.p3;
    switch u.i1.x {
        case 1u: {
            // Corner Pin: p0..p2.x the inverse homography (row-major), p2.y / p2.z the size
            let w = p1.z * x + p1.w * y + p2.x;
            if abs(w) < 1e-12 {
                return none;
            }
            let uu = (p0.x * x + p0.y * y + p0.z) / w;
            let vv = (p0.w * x + p1.x * y + p1.y) / w;
            if uu >= -1e-3 && vv >= -1e-3 && uu <= 1.0 + 1e-3 && vv <= 1.0 + 1e-3 {
                return vec2(uu * p2.y, vv * p2.z);
            }
            return none;
        }
        case 2u: {
            // Lens Distortion: k, centre, norm
            let d = (vec2(x, y) - p0.yz) / p0.w;
            let s = 1.0 - p0.x * dot(d, d);
            return p0.yz + d * s * p0.w;
        }
        case 3u: {
            // Twirl: centre, angle, radius
            let d = vec2(x, y) - p0.xy;
            let dist = sqrt(dot(d, d));
            if dist >= p0.w {
                return vec2(x, y);
            }
            let k = 1.0 - dist / p0.w;
            let a = p0.z * k * k;
            let sn = sin(a);
            let cs = cos(a);
            return p0.xy + vec2(d.x * cs - d.y * sn, d.x * sn + d.y * cs);
        }
        case 4u: {
            // Wave Warp: height, width, direction, phase in cycles (reduced on the CPU)
            let along = x * p0.z + y * p0.w;
            let cyc = fract_euclid(along / p0.y - p1.x);
            let off = p0.x * sin(cyc * 6.283185307179586);
            return vec2(x - p0.w * off, y + p0.z * off);
        }
        case 5u: {
            // Spherize: centre, radius
            let d = vec2(x, y) - p0.xy;
            let dist = sqrt(dot(d, d));
            if dist >= p0.z || dist < 1e-9 {
                return vec2(x, y);
            }
            let n = dist / p0.z;
            let k = 1.0 - 0.5 * sqrt(1.0 - n * n);
            return p0.xy + d * k;
        }
        case 6u: {
            // Basic 3D: n = p0.xyz, u = (p0.w, p1.xy), v = (p1.zw, p2.x), z0 p2.y, focal p2.z,
            // size p3.xy
            let px = x - p3.x / 2.0;
            let py = y - p3.y / 2.0;
            let n = p0.xyz;
            let denom = n.x * px + n.y * py + n.z * p2.z;
            if abs(denom) < 1e-9 {
                return none;
            }
            let tt = (n.z * p2.y) / denom;
            if tt <= 0.0 {
                return none;
            }
            // (dir.z tt - z0, rearranged: no cancellation of two large numbers in f32)
            let hz = p2.y * (-(n.x * px + n.y * py)) / denom;
            let hit = vec3(px * tt, py * tt, hz);
            let su = dot(hit, vec3(p0.w, p1.x, p1.y));
            let sv = dot(hit, vec3(p1.z, p1.w, p2.x));
            return vec2(su + p3.x / 2.0, sv + p3.y / 2.0);
        }
        default: {
            return vec2(x, y);
        }
    }
}

// `effects::fill_over` for one pixel
fn fill_over(o: vec4<f32>, c: vec3<f32>, op: f32) -> vec4<f32> {
    return vec4(c * op + o.rgb * (1.0 - op), op + o.a * (1.0 - op));
}

// `f32::rem_euclid` (the division's rounding corrected into the remainder's range)
fn rem_euclid(x: f32, m: f32) -> f32 {
    var r = x - m * floor(x / m);
    if r < 0.0 { r += m; }
    if r >= m { r -= m; }
    return r;
}

fn generate(p: vec2<i32>, o: vec4<f32>) -> vec4<f32> {
    let s = size();
    let pc = vec2<f32>(p) + 0.5;
    let p0 = u.p0; let p1 = u.p1; let p2 = u.p2; let p3 = u.p3;
    switch u.i1.x {
        case 1u: {
            // Ramp: start p0.xy, vector p0.zw, colours p1.xyz / p2.xyz (sRGB), blend p1.w
            let v = p0.zw;
            let len2 = max(dot(v, v), 1e-9);
            let d = pc - p0.xy;
            var t: f32;
            if u.i1.y != 0u {
                t = sqrt(dot(d, d)) / sqrt(len2);
            } else {
                t = dot(d, v) / len2;
            }
            t = clamp(t, 0.0, 1.0);
            return fill_over(o, dec(p1.xyz + (p2.xyz - p1.xyz) * t), 1.0 - p1.w);
        }
        case 2u: {
            // 4-Color Gradient: colours p0..p3 .xyz, opacity p0.w, exponent p1.w
            let w = f32(s.x);
            let h = f32(s.y);
            var pts = array<vec2<f32>, 4>(vec2(0.25 * w, 0.25 * h), vec2(0.75 * w, 0.25 * h), vec2(0.25 * w, 0.75 * h), vec2(0.75 * w, 0.75 * h));
            var cs = array<vec3<f32>, 4>(p0.xyz, p1.xyz, p2.xyz, p3.xyz);
            var acc = vec3(0.0);
            var wsum = 0.0;
            for (var i = 0; i < 4; i++) {
                let dd = pc - pts[i];
                let d = max(sqrt(dd.x * dd.x + dd.y * dd.y), 1.0);
                let wgt = 1.0 / pow(d, p1.w);
                acc += cs[i] * wgt;
                wsum += wgt;
            }
            return fill_over(o, dec(acc / wsum), p0.w);
        }
        case 3u: {
            // Circle: centre p0.xy, radius p0.z, opacity p0.w, linear colour p1.xyz
            let dd = pc - p0.xy;
            let a = clamp(p0.z - sqrt(dot(dd, dd)) + 0.5, 0.0, 1.0) * p0.w;
            if a > 0.0 {
                return fill_over(o, p1.xyz, a);
            }
            return o;
        }
        case 4u: {
            // Grid: size p0.x, border p0.y, opacity p0.z, linear colour p1.xyz, from the centre
            let sz = p0.x;
            let rx = rem_euclid(f32(p.x) - f32(s.x) / 2.0, sz);
            let ry = rem_euclid(f32(p.y) - f32(s.y) / 2.0, sz);
            let g = min(min(rx, sz - rx), min(ry, sz - ry));
            let a = clamp(p0.y * 0.5 - g + 0.5, 0.0, 1.0) * p0.z;
            if a > 0.0 {
                return fill_over(o, p1.xyz, a);
            }
            return o;
        }
        default: {
            return o;
        }
    }
}

fn ext_pixel(op: u32, p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let pc = vec2<f32>(p) + 0.5;
    switch op {
        case OP_NOISE: {
            // i1: flags (colour, clip), grain (whole pixels), frame (lo, hi); p0.x amount
            let o = ld(p);
            let a = o.a;
            if a <= 0.0 {
                return o;
            }
            let g = max(u.i1.y, 1u);
            let gx = u32(p.x) / g;
            let gy = u32(p.y) / g;
            let frame = vec2(u.i1.z, u.i1.w);
            let n0 = hash3(gx, gy, frame) - 0.5;
            var ns = vec3(n0);
            if (u.i1.x & 1u) != 0u {
                ns = vec3(n0, hash3(gx, gy, add64(frame, 7777u)) - 0.5, hash3(gx, gy, add64(frame, 99991u)) - 0.5);
            }
            var c = enc(o.rgb / a) + ns * u.p0.x;
            if (u.i1.x & 2u) != 0u {
                c = clamp(c, vec3(0.0), vec3(1.0));
            }
            return vec4(dec(c) * a, a);
        }
        case OP_WARP: {
            let q = warp_map(pc.x, pc.y);
            // (far outside, or not a number: transparent, as every tap there)
            if !(abs(q.x) < 1e7 && abs(q.y) < 1e7) {
                return vec4(0.0);
            }
            return sample_bilinear(q.x, q.y);
        }
        case OP_MAGNIFY: {
            // p0: centre, magnification, size; p1.xy: feather, opacity; i1: square, mode
            let o = ld(p);
            let c = u.p0.xy;
            let m = u.p0.z;
            let r = u.p0.w;
            let fe = u.p1.x;
            let op = u.p1.y;
            let d2 = pc - c;
            var d: f32;
            if u.i1.x != 0u {
                d = max(abs(d2.x), abs(d2.y));
            } else {
                d = sqrt(dot(d2, d2));
            }
            var k: f32;
            if fe > 0.0 {
                k = clamp((r - d) / fe, 0.0, 1.0);
            } else {
                k = clamp(r - d + 0.5, 0.0, 1.0);
            }
            k = k * op;
            if k <= 0.0 {
                return o;
            }
            let sm = sample_bilinear(c.x + d2.x / m, c.y + d2.y / m);
            let mixed = blend_simple(u.i1.y, unpremul(o), unpremul(sm));
            let out = vec4(mixed * sm.a, sm.a);
            return o + (out - o) * k;
        }
        case OP_FIND_EDGES: {
            // luma of the 3x3 neighbourhood (edge-clamped), l[(dy + 1) * 3 + dx + 1]
            var l: array<f32, 9>;
            for (var j = 0; j < 3; j++) {
                for (var i = 0; i < 3; i++) {
                    let q = clamp(p + vec2(i - 1, j - 1), vec2(0), s - 1);
                    l[j * 3 + i] = luma709(ld(q).rgb);
                }
            }
            let gx = l[2] + 2.0 * l[5] + l[8] - l[0] - 2.0 * l[3] - l[6];
            let gy = l[6] + 2.0 * l[7] + l[8] - l[0] - 2.0 * l[1] - l[2];
            var m = min(sqrt(gx * gx + gy * gy), 1.0);
            if u.i1.x == 0u {
                m = 1.0 - m;
            }
            let o = ld(p);
            let blend = u.p0.x;
            return vec4(m * o.a * (1.0 - blend) + o.rgb * blend, o.a);
        }
        case OP_EMBOSS: {
            let dx = u.p0.x;
            let dy = u.p0.y;
            let contrast = u.p0.z;
            let blend = u.p0.w;
            let a = sample_clamped(pc.x + dx, pc.y + dy);
            let b = sample_clamped(pc.x - dx, pc.y - dy);
            let v = srgb_to_linear1(clamp(0.5 + (luma709(a.rgb) - luma709(b.rgb)) * contrast * 2.0, 0.0, 1.0));
            let o = ld(p);
            return vec4(v * o.a * (1.0 - blend) + o.rgb * blend, o.a);
        }
        case OP_BEVEL_ALPHA: {
            let th = u.p0.x;
            let lx = u.p0.y;
            let ly = u.p0.z;
            let inten = u.p0.w;
            let gx = sample_clamped(pc.x + th, pc.y).a - sample_clamped(pc.x - th, pc.y).a;
            let gy = sample_clamped(pc.x, pc.y + th).a - sample_clamped(pc.x, pc.y - th).a;
            let shade = -(gx * lx + gy * ly) * inten;
            let o = ld(p);
            let al = max(o.a, 0.0);
            return vec4(clamp(o.rgb + shade * al, vec3(0.0), vec3(al)), o.a);
        }
        case OP_REPLICATE: {
            let n = u.i1.x;
            let uu = (u32(p.x) * n) % u32(s.x);
            let vv = (u32(p.y) * n) % u32(s.y);
            let fnn = f32(n);
            return sample_clamped((f32(uu) + 0.5) + (fnn - 1.0) * 0.5 / fnn, f32(vv) + 0.5);
        }
        case OP_STROBE: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let lc = u.p0.xyz;
            return vec4((lc + (o.rgb / a - lc) * u.p0.w) * a, a);
        }
        case OP_SHADOW: {
            // the layer moved (inverse offset p0.xy, rect i1), its alpha x p0.z in colour p1.xyz
            if u32(p.x) < u.i1.x || u32(p.x) >= u.i1.y || u32(p.y) < u.i1.z || u32(p.y) >= u.i1.w {
                return vec4(0.0);
            }
            let uu = pc.x + u.p0.x;
            let vv = pc.y + u.p0.y;
            if uu < -1.0 || vv < -1.0 || uu > f32(s.x) + 1.0 || vv > f32(s.y) + 1.0 {
                return vec4(0.0);
            }
            let a = sample_bilinear(uu, vv).a * u.p0.z;
            return vec4(u.p1.xyz * a, a);
        }
        case OP_SHADOW_OVER: {
            // src: the (blurred) shadow, aux: the layer; i1.x: shadow only
            let sh = ld(p);
            if u.i1.x != 0u {
                return sh;
            }
            let o = textureLoad(aux, p, 0);
            if o.a <= 0.0 {
                return sh;
            }
            return o + sh * (1.0 - o.a);
        }
        case OP_GENERATE: {
            return generate(p, ld(p));
        }
        default: {
            return ld(p);
        }
    }
}

// ---- GPU4: `filmcraft_render::gpufx_more::MoreOp::apply`

const OP_LENS_FLARE: u32 = 54u;
const OP_RGB_SPLIT: u32 = 55u;
const OP_LIGHTING: u32 = 56u;
const OP_COLOR_EMBOSS: u32 = 57u;
const OP_ROUNDED_CROP: u32 = 58u;
const OP_GRADIENT: u32 = 59u;
const OP_LIGHT_LEAKS: u32 = 60u;
const OP_TURBULENT: u32 = 61u;
const OP_MEDIAN: u32 = 62u;
const OP_ALPHA_GLOW: u32 = 63u;
const OP_EDGE_GLOW_EDGES: u32 = 64u;
const OP_EDGE_GLOW_X3: u32 = 65u;
const OP_EDGE_GLOW_ADD: u32 = 66u;
const OP_ROUGHEN: u32 = 67u;

// A pass's table (Lens Flare's ghosts, Lighting Effects' lights), bound where the mask points go.
fn more_data(i: u32) -> f32 {
    let q = mask_pts[i / 2u];
    if (i & 1u) == 0u {
        return q.x;
    }
    return q.y;
}

// `vfx::add_light`: light added to a pixel, opaque where it lands on transparency.
fn add_light(p: vec4<f32>, add: vec3<f32>) -> vec4<f32> {
    let m = clamp(max(add.x, max(add.y, add.z)), 0.0, 1.0);
    return vec4(p.rgb + add, p.a + (1.0 - p.a) * m);
}

// `vfx::over`: premultiplied `top` over `under`.
fn over_px(under: vec4<f32>, top: vec4<f32>) -> vec4<f32> {
    return top + under * (1.0 - top.a);
}

// ---- `vfx::noise3` / `vfx::fbm`: 64-bit hash (`vfx::hash_u`) on (lo, hi) pairs

// an i32 sign-extended to 64 bits (Rust's `i64 as u64`)
fn i64v(v: i32) -> vec2<u32> {
    return vec2(bitcast<u32>(v), select(0u, 0xffffffffu, v < 0));
}

// `seed.wrapping_mul(0x27D4_EB2F_1656_67C5)`: the seed's term of `hash_u`
fn hash_seed(seed: vec2<u32>) -> vec2<u32> {
    return mul64(seed, vec2(0x165667C5u, 0x27D4EB2Fu));
}

// `vfx::hash_u` with the seed term made by `hash_seed`
fn hash_u(x: i32, y: i32, z: i32, sk: vec2<u32>) -> vec2<u32> {
    var h = mul64(i64v(x), vec2(0x7F4A7C15u, 0x9E3779B9u)) ^ mul64(i64v(y), vec2(0x27D4EB4Fu, 0xC2B2AE3Du))
        ^ mul64(i64v(z), vec2(0x9E3779F9u, 0x165667B1u)) ^ sk;
    h = h ^ shr64(h, 31u);
    h = mul64(h, vec2(0x1CE4E5B9u, 0xBF58476Du));
    h = h ^ shr64(h, 29u);
    h = mul64(h, vec2(0x133111EBu, 0x94D049BBu));
    return vec2(h.x ^ h.y, h.y);
}

// `vfx::hash01`
fn hash01(x: i32, y: i32, z: i32, sk: vec2<u32>) -> f32 {
    return f32(hash_u(x, y, z, sk).y >> 8u) / 16777216.0;
}

fn noise_fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

// `vfx::grad3` for the hash h (h % 12 of the 64-bit value: 2^32 = 4 mod 12)
fn noise_grad(h: vec2<u32>, x: f32, y: f32, z: f32) -> f32 {
    switch ((h.y % 12u) * 4u + h.x % 12u) % 12u {
        case 0u: { return x + y; }
        case 1u: { return -x + y; }
        case 2u: { return x - y; }
        case 3u: { return -x - y; }
        case 4u: { return x + z; }
        case 5u: { return -x + z; }
        case 6u: { return x - z; }
        case 7u: { return -x - z; }
        case 8u: { return y + z; }
        case 9u: { return -y + z; }
        case 10u: { return y - z; }
        default: { return -y - z; }
    }
}

// `vfx::noise3`
fn noise3(x: f32, y: f32, z: f32, seed: vec2<u32>) -> f32 {
    let xi = floor(x);
    let yi = floor(y);
    let zi = floor(z);
    let xf = x - xi;
    let yf = y - yi;
    let zf = z - zi;
    let ix = i32(xi);
    let iy = i32(yi);
    let iz = i32(zi);
    let uu = noise_fade(xf);
    let vv = noise_fade(yf);
    let ww = noise_fade(zf);
    let sk = hash_seed(seed);
    let g000 = noise_grad(hash_u(ix, iy, iz, sk), xf, yf, zf);
    let g100 = noise_grad(hash_u(ix + 1, iy, iz, sk), xf - 1.0, yf, zf);
    let g010 = noise_grad(hash_u(ix, iy + 1, iz, sk), xf, yf - 1.0, zf);
    let g110 = noise_grad(hash_u(ix + 1, iy + 1, iz, sk), xf - 1.0, yf - 1.0, zf);
    let g001 = noise_grad(hash_u(ix, iy, iz + 1, sk), xf, yf, zf - 1.0);
    let g101 = noise_grad(hash_u(ix + 1, iy, iz + 1, sk), xf - 1.0, yf, zf - 1.0);
    let g011 = noise_grad(hash_u(ix, iy + 1, iz + 1, sk), xf, yf - 1.0, zf - 1.0);
    let g111 = noise_grad(hash_u(ix + 1, iy + 1, iz + 1, sk), xf - 1.0, yf - 1.0, zf - 1.0);
    let x00 = g000 + (g100 - g000) * uu;
    let x10 = g010 + (g110 - g010) * uu;
    let x01 = g001 + (g101 - g001) * uu;
    let x11 = g011 + (g111 - g011) * uu;
    let y0 = x00 + (x10 - x00) * vv;
    let y1 = x01 + (x11 - x01) * vv;
    return (y0 + (y1 - y0) * ww) * 0.9;
}

// `vfx::fbm`: `octaves` (1..10, a fractional last one fades in)
fn fbm(x: f32, y: f32, z: f32, octaves: f32, seed: vec2<u32>) -> f32 {
    let n = clamp(octaves, 1.0, 10.0);
    var sum = 0.0;
    var amp = 1.0;
    var freq = 1.0;
    var norm = 0.0;
    for (var i = 0u; i < 10u; i++) {
        if !(f32(i) < n) {
            break;
        }
        let k = min(n - f32(i), 1.0);
        sum += noise3(x * freq, y * freq, z * freq + f32(i) * 17.13, add64(seed, i)) * amp * k;
        norm += amp * k;
        amp *= 0.5;
        freq *= 2.0;
    }
    return sum / max(norm, 1e-6);
}

// `vfx::distort::round_rect_sdf`
fn round_rect_sdf(p: vec2<f32>, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hx = (x1 - x0) / 2.0 - r;
    let hy = (y1 - y0) / 2.0 - r;
    let qx = abs(p.x - cx) - hx;
    let qy = abs(p.y - cy) - hy;
    let ox = max(qx, 0.0);
    let oy = max(qy, 0.0);
    return sqrt(ox * ox + oy * oy) + min(max(qx, qy), 0.0) - r;
}

// Lighting Effects' bump source: the (inverted) luma of a clamped neighbour
fn light_lum(p: vec2<i32>) -> f32 {
    let l = luma709(unpremul(ld(clamp(p, vec2(0), size() - 1))));
    return select(1.0 - l, l, u.i1.x != 0u);
}

// Turbulent Displace's noise: p0 = (amount, size, ox, oy), p1 = (complexity, ex, ey, ez)
fn turb_n(x: f32, y: f32, ch: f32) -> f32 {
    return fbm(x + u.p1.y + ch * 31.7, y + u.p1.z, u.p1.w + ch * 3.1, u.p1.x, vec2(u.i1.y, u.i1.z));
}

fn pin_edge(d: f32, len: f32) -> f32 {
    return smoothstep_fx(1.0, max(len * 0.1, 2.0), d);
}

fn more_pixel(op: u32, p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let pc = vec2<f32>(p) + 0.5;
    switch op {
        case OP_LENS_FLARE: {
            // p0: centre, scale, k; p1: tint, core falloff; i1: lens type, ghosts (6 floats each)
            let o = ld(p);
            let c = u.p0.xy;
            let scale = u.p0.z;
            let lens = u.i1.x;
            let dv = pc - c;
            let d = sqrt(dot(dv, dv)) / scale;
            let core = 0.35 * exp(-d * u.p1.w) + 0.08 * exp(-d * 3.0);
            var add = vec3(core);
            if lens == 1u || lens == 2u {
                let ring = max(1.0 - abs((d - 0.12) / 0.012), 0.0) * 0.08;
                add += ring * vec3(0.6, 0.8, 1.0);
            }
            if lens == 3u {
                let st = exp(-(abs(pc.y - c.y) / scale) * 160.0) * exp(-(abs(pc.x - c.x) / scale) * 2.0) * 0.5;
                add += st * vec3(0.5, 0.7, 1.0);
            }
            for (var i = 0u; i < min(u.i1.y, 8u); i++) {
                let b = i * 6u;
                let gd = pc - vec2(more_data(b), more_data(b + 1u));
                let rad = more_data(b + 2u);
                let dd = sqrt(dot(gd, gd)) / scale;
                var gv = max(1.0 - abs((dd - rad) / 0.01), 0.0) * 0.12;
                if dd < rad {
                    gv += 0.05;
                }
                add += vec3(more_data(b + 3u), more_data(b + 4u), more_data(b + 5u)) * gv;
            }
            return add_light(o, add * u.p1.xyz * u.p0.w);
        }
        case OP_RGB_SPLIT: {
            // p0: offset, centre; p1: max distance, amount, blend; i1.x: radial
            var off = u.p0.xy;
            if u.i1.x != 0u {
                let d = pc - u.p0.zw;
                off = vec2(d.x / u.p1.x * u.p1.y, d.y / u.p1.x * u.p1.y);
            }
            let r = sample_bilinear(pc.x - off.x, pc.y - off.y);
            let g = ld(p);
            let b = sample_bilinear(pc.x + off.x, pc.y + off.y);
            let o = vec4(r.r, g.g, b.b, max(max(r.a, g.a), b.a));
            return o + (g - o) * u.p1.z;
        }
        case OP_LIGHTING: {
            // p0: ambience, gloss; p1: material, exposure, bump; i1: white is high, lights (12 floats each)
            let o = ld(p);
            if o.a <= 1e-6 {
                return o;
            }
            let bump = u.p1.z;
            var gx = 0.0;
            var gy = 0.0;
            if bump != 0.0 {
                gx = (light_lum(p + vec2(1, 0)) - light_lum(p - vec2(1, 0))) * bump;
                gy = (light_lum(p + vec2(0, 1)) - light_lum(p - vec2(0, 1))) * bump;
            }
            var light = u.p0.xyz;
            var spec = vec3(0.0);
            for (var i = 0u; i < min(u.i1.y, 5u); i++) {
                let b = i * 12u;
                let kind = u32(more_data(b));
                let dx = pc.x - more_data(b + 1u);
                let dy = pc.y - more_data(b + 2u);
                let major = more_data(b + 3u);
                let minor = more_data(b + 4u);
                let sn = more_data(b + 5u);
                let cs = more_data(b + 6u);
                let inten = more_data(b + 7u);
                let focus = more_data(b + 8u);
                let col = vec3(more_data(b + 9u), more_data(b + 10u), more_data(b + 11u));
                var fall = 1.0;
                if kind == 2u {
                    let q = max(1.0 - sqrt(dx * dx + dy * dy) / major, 0.0);
                    fall = q * q;
                } else if kind != 1u {
                    let a = (dx * cs + dy * sn) / major;
                    let bb = (-dx * sn + dy * cs) / minor;
                    fall = 1.0 - smoothstep_fx(focus * 0.95, 1.0, sqrt(a * a + bb * bb));
                }
                var lx = cs;
                var ly = sn;
                if kind != 1u {
                    lx = -dx;
                    ly = -dy;
                }
                let ln = max(sqrt(lx * lx + ly * ly), 1e-6);
                let shade = max(1.0 - (gx * lx + gy * ly) / ln, 0.0);
                let f2 = fall * fall;
                let f4 = f2 * f2;
                light += col * inten * fall * shade;
                spec += col * max(u.p0.w, 0.0) * (f4 * f4) * max(inten, 0.0);
            }
            let c = unpremul(o);
            let mat = u.p1.x;
            let sv = spec * (1.0 - mat) + spec * c * mat;
            return vec4(max((c * light + sv) * u.p1.y, vec3(0.0)) * o.a, o.a);
        }
        case OP_COLOR_EMBOSS: {
            let o = ld(p);
            if o.a <= 1e-6 {
                return o;
            }
            let a = sample_clamped(pc.x + u.p0.x, pc.y + u.p0.y);
            let b = sample_clamped(pc.x - u.p0.x, pc.y - u.p0.y);
            let d = luma709(unpremul(a)) - luma709(unpremul(b));
            let v = enc(o.rgb / o.a);
            let e = clamp(v + d * u.p0.z * 2.0, vec3(0.0), vec3(1.0));
            return vec4(dec(e + (v - e) * u.p0.w) * o.a, o.a);
        }
        case OP_ROUNDED_CROP: {
            // p0: left, top, right, bottom; p1: radius, feather, border; p2: border colour
            var o = ld(p);
            let d = round_rect_sdf(pc, u.p0.x, u.p0.y, u.p0.z, u.p0.w, u.p1.x);
            let fe = u.p1.y;
            let border = u.p1.z;
            if border > 0.0 {
                let k = clamp(0.5 - d, 0.0, 1.0) - clamp(0.5 - d - border, 0.0, 1.0);
                if k > 0.0 {
                    o = over_px(o, vec4(u.p2.xyz * k, k));
                }
            }
            var a: f32;
            if fe > 0.0 {
                a = clamp(-d / fe, 0.0, 1.0);
            } else {
                a = clamp(0.5 - d, 0.0, 1.0);
            }
            if a < 1.0 {
                o = o * a;
            }
            return o;
        }
        case OP_GRADIENT: {
            // p0: start, vector; p1 / p2: sRGB start / end colours with alpha; p3: scatter, gamma,
            // blend; i1.x: shape
            let o = ld(p);
            let v = u.p0.zw;
            let l2 = max(v.x * v.x + v.y * v.y, 1e-6);
            let dx = pc.x - u.p0.x;
            let dy = pc.y - u.p0.y;
            let along = (dx * v.x + dy * v.y) / l2;
            let across = (-dx * v.y + dy * v.x) / l2;
            var t: f32;
            switch u.i1.x {
                case 1u: { t = sqrt(dx * dx + dy * dy) / sqrt(l2); }
                case 2u: { t = abs(along); }
                case 3u: { t = abs(along) + abs(across); }
                default: { t = along; }
            }
            if u.p3.x > 0.0 {
                t += (hash01(p.x, p.y, 0, hash_seed(vec2(13u, 0u))) - 0.5) * u.p3.x * 0.1;
            }
            t = powf(clamp(t, 0.0, 1.0), u.p3.y);
            let c = dec(u.p1.rgb + (u.p2.rgb - u.p1.rgb) * t);
            let a = u.p1.a + (u.p2.a - u.p1.a) * t;
            let g = vec4(c * a, a);
            return g + (o - g) * u.p3.z;
        }
        case OP_LIGHT_LEAKS: {
            // p0: colour 1, intensity; p1: colour 2, scale; p2: direction, time; i1: mode, seed
            let o = ld(p);
            let c1 = u.p0.xyz;
            let c2 = u.p1.xyz;
            let scale = u.p1.w;
            let dx = u.p2.x;
            let dy = u.p2.y;
            let t = u.p2.z;
            let seed = vec2(u.i1.y, u.i1.z);
            let uu = pc.x / scale;
            let vv = pc.y / scale;
            let along = ((pc.x / f32(s.x) - 0.5) * dx + (pc.y / f32(s.y) - 0.5) * dy) * 2.0;
            let bias = smoothstep_fx(-0.2, 1.0, -along);
            let n = fbm(uu + dx * t * 0.3, vv + dy * t * 0.3, t * 0.15, 3.0, seed) * 0.5 + 0.5;
            let m = fbm(uu * 0.7 + 5.2, vv * 0.7, t * 0.1 + 9.0, 2.0, seed) * 0.5 + 0.5;
            let k = smoothstep_fx(0.35, 0.85, n) * bias * u.p0.w;
            let light = (c1 + (c2 - c1) * m) * k;
            let base = unpremul(o);
            var oc: vec3<f32>;
            switch u.i1.x {
                case 1u: { oc = base + light; }
                case 2u: { oc = blend_simple(4u, base, base + (light + 0.5 - base) * min(k, 1.0)); }
                default: { oc = blend_simple(2u, base, light); }
            }
            let a = o.a + (1.0 - o.a) * min(k, 1.0);
            return vec4(oc * a, a);
        }
        case OP_TURBULENT: {
            // i1: displacement kind, seed, pinning
            let size = u.p0.y;
            let uu = pc.x / size + u.p0.z;
            let vv = pc.y / size + u.p0.w;
            let kind = u.i1.x;
            var d: vec2<f32>;
            if kind == 0u || kind == 3u {
                d = vec2(turb_n(uu, vv, 0.0), turb_n(uu, vv, 1.0));
            } else if kind == 1u || kind == 4u || kind == 2u || kind == 5u {
                let eps = 0.05;
                let gx = (turb_n(uu + eps, vv, 0.0) - turb_n(uu - eps, vv, 0.0)) / (2.0 * eps);
                let gy = (turb_n(uu, vv + eps, 0.0) - turb_n(uu, vv - eps, 0.0)) / (2.0 * eps);
                if kind == 1u || kind == 4u {
                    d = vec2(gx * 0.3, gy * 0.3);
                } else {
                    d = vec2(-gy * 0.3, gx * 0.3);
                }
            } else if kind == 6u {
                d = vec2(0.0, turb_n(uu, vv, 0.0));
            } else if kind == 7u {
                d = vec2(turb_n(uu, vv, 0.0), 0.0);
            } else {
                let q = turb_n(uu, vv, 0.0);
                d = vec2(q, q);
            }
            let w = f32(s.x);
            let h = f32(s.y);
            var k = 1.0;
            switch u.i1.w {
                case 1u: { k = min(pin_edge(min(pc.x, w - pc.x), w), pin_edge(min(pc.y, h - pc.y), h)); }
                case 2u: { k = pin_edge(min(pc.x, w - pc.x), w); }
                case 3u: { k = pin_edge(min(pc.y, h - pc.y), h); }
                default: {}
            }
            let amount = u.p0.x;
            let q = vec2(pc.x - d.x * amount * k, pc.y - d.y * amount * k);
            if !(abs(q.x) < 1e7 && abs(q.y) < 1e7) {
                return vec4(0.0);
            }
            return sample_bilinear(q.x, q.y);
        }
        case OP_ALPHA_GLOW: {
            // src: the blurred alpha (in A), aux: the layer; p0: start colour, brightness; p1: end
            // colour; i1: use end colour, fade out
            let o = textureLoad(aux, p, 0);
            let gv = min(ld(p).a * 2.0, 1.0);
            var k = 0.0;
            if u.i1.y != 0u {
                k = gv * u.p0.w;
            } else if gv > 0.02 {
                k = u.p0.w;
            }
            if k <= 0.0 || o.a >= 1.0 {
                return o;
            }
            var col = u.p0.xyz;
            if u.i1.x != 0u {
                col = u.p1.xyz + (u.p0.xyz - u.p1.xyz) * gv;
            }
            return over_px(vec4(col * k, k), o);
        }
        case OP_EDGE_GLOW_EDGES: {
            // Sobel of display luma × alpha (edge-clamped), into R and G
            var l: array<f32, 9>;
            for (var j = 0; j < 3; j++) {
                for (var i = 0; i < 3; i++) {
                    let q = ld(clamp(p + vec2(i - 1, j - 1), vec2(0), s - 1));
                    l[j * 3 + i] = linear_to_srgb1(clamp(luma709(q.rgb), 0.0, 1.0)) * min(q.a, 1.0);
                }
            }
            let gx = l[2] + 2.0 * l[5] + l[8] - l[0] - 2.0 * l[3] - l[6];
            let gy = l[6] + 2.0 * l[7] + l[8] - l[0] - 2.0 * l[1] - l[2];
            let th = u.p0.x;
            let m = smoothstep_fx(th, th + 0.1, sqrt(gx * gx + gy * gy) / 4.0);
            return vec4(m, m, 0.0, 0.0);
        }
        case OP_EDGE_GLOW_X3: {
            let o = ld(p);
            return vec4(min(o.x * 3.0, 1.0), min(o.y * 3.0, 1.0), o.z, o.w);
        }
        case OP_EDGE_GLOW_ADD: {
            // src: halo (R) and core (G), aux: the layer; p0: colour, intensity; i1.x: glow only
            let e = ld(p);
            var o = textureLoad(aux, p, 0);
            if u.i1.x != 0u {
                o = vec4(0.0);
            }
            let v = (e.y * 0.6 + e.x * 1.6) * u.p0.w;
            return add_light(o, u.p0.xyz * v);
        }
        case OP_ROUGHEN: {
            // src: the blurred alpha (in A), aux: the layer; p0: sharpness, influence, scale,
            // stretch; p1: offset, complexity, evolution; p2: edge colour; i1: edge type, seed
            var o = textureLoad(aux, p, 0);
            if o.a <= 1e-6 {
                return o;
            }
            let d = ld(p).a;
            let kind = u.i1.x;
            let seed = vec2(u.i1.y, u.i1.z);
            let scale = u.p0.z;
            let stretch = u.p0.w;
            let cplx = u.p1.z;
            let evo = u.p1.w;
            let uu = (f32(p.x) + u.p1.x) / (scale * stretch);
            let vv = (f32(p.y) + u.p1.y) * stretch / scale;
            var n = fbm(uu, vv, evo, cplx, seed);
            if kind == 3u {
                n = 1.0 - 2.0 * abs(n);
            }
            var k = clamp((d - 0.5 + n * u.p0.y * 0.5) * (1.0 + u.p0.x * 4.0) + 0.5, 0.0, 1.0);
            if kind == 2u {
                k = select(0.0, 1.0, k > 0.5);
            }
            if kind >= 4u && kind <= 7u {
                let hole = fbm(uu * 2.3 + 11.0, vv * 2.3, evo, cplx, add64(seed, 1u));
                if d < 0.97 && hole > 0.25 {
                    k *= 1.0 - smoothstep_fx(0.25, 0.4, hole) * (1.0 - d);
                }
            }
            if (kind == 1u || kind == 5u || kind == 7u) && d < 0.97 {
                let t = (1.0 - d) * 1.5;
                let c = unpremul(o);
                let col = c + (u.p2.xyz - c) * min(t, 1.0);
                o = vec4(col * o.a, o.a);
            }
            if kind == 6u {
                let l = select(0.0, 1.0, luma709(enc(unpremul(o))) > 0.5);
                o = vec4(vec3(l * o.a), o.a);
            }
            return o * k;
        }
        default: {
            return ld(p);
        }
    }
}

// `effects::median`: per channel, the middle of the (2r + 1)² edge-clamped window (its own
// entry point: the windows' arrays would cost every other pass registers)
fn median_pixel(p: vec2<i32>) -> vec4<f32> {
    let s = size();
    let r = i32(clamp(u.i1.x, 1u, 4u));
    var px: array<vec4<f32>, 81>;
    var n = 0u;
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            px[n] = ld(clamp(p + vec2(dx, dy), vec2(0), s - 1));
            n++;
        }
    }
    let k = n / 2u;
    var out = vec4(0.0);
    var win: array<f32, 81>;
    for (var ch = 0; ch < 4; ch++) {
        for (var i = 0u; i < n; i++) {
            win[i] = px[i][ch];
        }
        // the k-th smallest, by selection
        for (var i = 0u; i <= k; i++) {
            var m = i;
            for (var j = i + 1u; j < n; j++) {
                if win[j] < win[m] {
                    m = j;
                }
            }
            let t = win[i];
            win[i] = win[m];
            win[m] = t;
        }
        out[ch] = win[k];
    }
    return out;
}

// GPU4 passes (`more_pixel`), in their own entry point like Median's, so the GPU2 / GPU3 passes
// keep their register budget.
@compute @workgroup_size(16, 16)
fn fx_more(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u.i0.y || id.y >= u.i0.z {
        return;
    }
    let p = vec2<i32>(id.xy);
    textureStore(dst, p, more_pixel(u.i0.x, p));
}

@compute @workgroup_size(16, 16)
fn fx_median(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u.i0.y || id.y >= u.i0.z {
        return;
    }
    let p = vec2<i32>(id.xy);
    textureStore(dst, p, median_pixel(p));
}
