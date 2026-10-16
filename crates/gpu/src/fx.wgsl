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
// Data (bound where the mask points go): 32 header floats (split tone 0..6, wheels 8..20, look
// intensity 20), then the 1024-sample curve tables at 32 + k * 1024.

const LUMETRI_CURVE_N: u32 = 1024u;

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
    let base = 32u + k * LUMETRI_CURVE_N;
    let pos = clamp(x, 0.0, 1.0) * f32(LUMETRI_CURVE_N - 1u);
    let i = u32(pos);
    let j = min(i + 1u, LUMETRI_CURVE_N - 1u);
    let a = lumetri_data(base + i);
    let b = lumetri_data(base + j);
    return a + (b - a) * (pos - f32(i));
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

// `LumetriExtra::advanced`: Look, color wheels, RGB curves, hue / saturation / luma curves.
fn lumetri_advanced(v0: vec3<f32>) -> vec3<f32> {
    var v = v0;
    let look = u.i1.y;
    if look > 0u {
        let lk = lumetri_look(look, v);
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
        var hsl = rgb_to_hsl(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)));
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
        v = hsl_to_rgb(hsl.x, hsl.y, hsl.z);
    }
    return v;
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
            return acc * (1.0 / f32(2 * r + 1));
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
            var o = textureLoad(aux, p, 0);
            let bq = ld(p);
            let amount = u.p0.x; let th = u.p0.y;
            for (var k = 0; k < 3; k++) {
                let d = o[k] - bq[k];
                if abs(d) >= th * o.a {
                    o[k] = max(o[k] + d * amount, 0.0);
                }
            }
            return o;
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
        case OP_LUMETRI: {
            let o = ld(p);
            let a = o.a;
            if a <= 1e-6 {
                return o;
            }
            let c = o.rgb / a;
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
            var v = enc(lin);

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
                if k >= 1.0 {
                    v = q + (s - q) * (k - 1.0);
                } else {
                    v = vec3<f32>(0.5) + (q - vec3<f32>(0.5)) * k;
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
            if u.i1.y > 0u || (u.i1.x & ~1u) != 0u {
                v = lumetri_advanced(enc(dec(v)));
            }
            return vec4(dec(v) * a, a);
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
    textureStore(dst, p, pixel(u.i0.x, p));
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
        textureStore(dst, select(vec2(x, line), vec2(line, x), vertical), acc * inv);
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
