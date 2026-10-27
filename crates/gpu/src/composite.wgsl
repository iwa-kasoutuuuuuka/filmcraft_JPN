// FilmCraft GPU compositor.
// Layers are drawn as transformed quads; the fragment shader samples the source (YUV planes or
// RGBA) with manual bilinear filtering (textureLoad, so any float format works) and N×N
// supersampling over the pixel footprint when minifying, converts to linear light, premultiplies
// and scales by opacity. Normal layers blend into the Rgba16Float accumulator with fixed-function
// premultiplied "over"; Dissolve does too (each pixel is either dropped or drawn opaque). The other
// blend modes (`fs_blend`) read the accumulator under the layer from a copy (`backdrop`) and write
// the composited result, with the math of `filmcraft_render::blend::composite`.

struct U {
    m0: vec4<f32>,   // a b c d
    m1: vec4<f32>,   // e f out_w out_h
    src: vec4<f32>,  // src_w src_h chroma_w chroma_h
    p0: vec4<f32>,   // opacity, kind (0 rgba8 srgb straight, 1 rgba16f premul linear, 2 yuv), taps, transfer (0 srgb, 1 linear, 2 pq, 3 hlg)
    p1: vec4<f32>,   // y_off y_scale c_off c_scale (code units)
    p2: vec4<f32>,   // kr kb code_scale footprint
    p3: vec4<f32>,   // blend mode (index into filmcraft_render::Blend::ALL), BT.2020 luma weights (HLG OOTF)
    cm0: vec4<f32>,  // gamut matrix into the working gamut, row 0 (w: on) — YUV / RGBA8 sources
    cm1: vec4<f32>,  // row 1
    cm2: vec4<f32>,  // row 2
};

// The decoded (linear) colour in the working gamut (`plan::input_matrix`).
fn to_working(c: vec3<f32>) -> vec3<f32> {
    if u.cm0.w == 0.0 {
        return c;
    }
    return vec3(dot(u.cm0.xyz, c), dot(u.cm1.xyz, c), dot(u.cm2.xyz, c));
}

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex0: texture_2d<f32>;
@group(0) @binding(2) var tex1: texture_2d<f32>;
@group(0) @binding(3) var tex2: texture_2d<f32>;
// The accumulator under the layer (blend modes other than Normal / Dissolve only).
@group(0) @binding(4) var backdrop: texture_2d<f32>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) sp: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let c = corners[vi];
    let s = c * u.src.xy;
    let o = vec2(u.m0.x * s.x + u.m0.z * s.y + u.m1.x, u.m0.y * s.x + u.m0.w * s.y + u.m1.y);
    var out: VOut;
    out.pos = vec4(o.x / u.m1.z * 2.0 - 1.0, 1.0 - o.y / u.m1.w * 2.0, 0.0, 1.0);
    out.sp = s;
    return out;
}

fn load4(t: texture_2d<f32>, p: vec2<f32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(t));
    // `p` is an interpolated varying: at a pixel centre it is off by a few ULP (up to ~8e-6 near 64),
    // which would mix ~4e-6 of the neighbour into the pixel. Alpha-dividing effects (Invert Alpha)
    // amplify that into a visible colour on transparent pixels, so a centre hit within 1e-4 of a
    // pixel snaps to the exact pixel (as the CPU's integer sampling does).
    var q = p - 0.5;
    let r = round(q);
    q = select(q, r, abs(q - r) < vec2(1e-4));
    let i = vec2<i32>(floor(q));
    let f = q - floor(q);
    let a = textureLoad(t, clamp(i, vec2(0), d - 1), 0);
    let b = textureLoad(t, clamp(i + vec2(1, 0), vec2(0), d - 1), 0);
    let cc = textureLoad(t, clamp(i + vec2(0, 1), vec2(0), d - 1), 0);
    let dd = textureLoad(t, clamp(i + vec2(1, 1), vec2(0), d - 1), 0);
    return mix(mix(a, b, f.x), mix(cc, dd, f.x), f.y);
}

fn srgb_to_linear(v: vec3<f32>) -> vec3<f32> {
    let lo = v / 12.92;
    let hi = pow((max(v, vec3(0.0)) + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, v <= vec3(0.04045));
}

// ST 2084 in working units (1.0 = 203 cd/m², `decode_channel`).
fn pq_eotf(e: vec3<f32>) -> vec3<f32> {
    let m1 = 0.15930176; let m2 = 78.84375; let c1 = 0.8359375; let c2 = 18.851563; let c3 = 18.6875;
    // the signal clamps to 0..1 as the CPU's decode table does (out-of-gamut YCbCr gives R'G'B'
    // past 1, beyond which the formula breaks down)
    let p = pow(clamp(e, vec3(1e-12), vec3(1.0)), vec3(1.0 / m2));
    return pow(max(p - c1, vec3(0.0)) / (c2 - c3 * p), vec3(1.0 / m1)) * (10000.0 / 203.0);
}

fn hlg_inv(e: vec3<f32>) -> vec3<f32> {
    let a = 0.17883277; let b = 0.28466892; let c = 0.55991073;
    let lo = e * e / 3.0;
    let hi = (exp((e - c) / a) + b) / 12.0;
    return select(hi, lo, e <= vec3(0.5));
}

// HLG: scene light (inverse OETF), then the BT.2100 OOTF of a 1000 cd/m² display (γ 1.2) in
// working units (`InputTransform::apply`).
fn hlg_display(v: vec3<f32>) -> vec3<f32> {
    let e = hlg_inv(clamp(v, vec3(0.0), vec3(1.0)));
    let ys = max(dot(u.p3.yzw, e), 0.0);
    var g = 0.0;
    if ys > 0.0 {
        g = (1000.0 / 203.0) * pow(ys, 0.2);
    }
    return e * g;
}

fn to_linear(v: vec3<f32>) -> vec3<f32> {
    let t = u32(u.p0.w);
    if t == 1u { return v; }
    if t == 2u { return pq_eotf(v); }
    if t == 3u { return hlg_display(v); }
    return srgb_to_linear(clamp(v, vec3(0.0), vec3(1.0)));
}

// One linear premultiplied sample at source position p.
fn sample(p: vec2<f32>) -> vec4<f32> {
    let kind = u32(u.p0.y);
    if kind == 0u {
        // rgba8 sRGB texture: loads are already linear, straight alpha
        let c = load4(tex0, p);
        return vec4(to_working(c.rgb) * c.a, c.a);
    }
    if kind == 1u {
        return load4(tex0, p);
    }
    let cs = u.p2.z;
    let yc = load4(tex0, p).r * cs;
    let cp = p * u.src.zw / u.src.xy;
    let cb = load4(tex1, cp).r * cs;
    let cr = load4(tex2, cp).r * cs;
    let y = (yc - u.p1.x) / u.p1.y;
    let b = (cb - u.p1.z) / u.p1.w;
    let r = (cr - u.p1.z) / u.p1.w;
    let kr = u.p2.x; let kb = u.p2.y; let kg = 1.0 - kr - kb;
    let R = y + 2.0 * (1.0 - kr) * r;
    let B = y + 2.0 * (1.0 - kb) * b;
    let G = (y - kr * R - kb * B) / kg;
    return vec4(to_working(to_linear(vec3(R, G, B))), 1.0);
}

// The layer's premultiplied linear colour at this pixel, opacity applied.
fn layer_color(sp: vec2<f32>) -> vec4<f32> {
    let n = max(u32(u.p0.z), 1u);
    let fp = u.p2.w;
    var acc = vec4(0.0);
    for (var j = 0u; j < n; j++) {
        for (var i = 0u; i < n; i++) {
            let off = (vec2(f32(i), f32(j)) + 0.5) / f32(n) - 0.5;
            acc += sample(sp + off * fp);
        }
    }
    return acc / f32(n * n) * u.p0.x;
}

// ---- Dissolve: the CPU's 64-bit pixel hash (`blend::hash2`), on 32-bit halves (lo, hi).

// Full 64-bit product of two u32.
fn mul32(a: u32, b: u32) -> vec2<u32> {
    let a0 = a & 0xffffu; let a1 = a >> 16u; let b0 = b & 0xffffu; let b1 = b >> 16u;
    let p00 = a0 * b0; let p01 = a0 * b1; let p10 = a1 * b0; let p11 = a1 * b1;
    let mid = (p00 >> 16u) + (p01 & 0xffffu) + (p10 & 0xffffu);
    return vec2((mid << 16u) | (p00 & 0xffffu), p11 + (p01 >> 16u) + (p10 >> 16u) + (mid >> 16u));
}

// Wrapping 64-bit multiply.
fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let p = mul32(a.x, b.x);
    return vec2(p.x, p.y + a.x * b.y + a.y * b.x);
}

// 64-bit shift right by 0 < n < 32.
fn shr64(a: vec2<u32>, n: u32) -> vec2<u32> {
    return vec2((a.x >> n) | (a.y << (32u - n)), a.y >> n);
}

fn hash2(x: u32, y: u32) -> f32 {
    var h = mul64(vec2(x, 0u), vec2(0x7F4A7C15u, 0x9E3779B9u)) ^ mul64(vec2(y, 0u), vec2(0x27D4EB4Fu, 0xC2B2AE3Du));
    h = h ^ shr64(h, 31u);
    h = mul64(h, vec2(0x1CE4E5B9u, 0xBF58476Du));
    h = h ^ shr64(h, 29u);
    return f32(h.y >> 8u) / 16777216.0;
}

// Normal and Dissolve (fixed-function premultiplied "over").
@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let c = layer_color(in.sp);
    if u32(u.p3.x) == 1u {
        // Dissolve: keep the pixel with probability alpha, then draw it opaque.
        let sa = c.a;
        if hash2(u32(in.pos.x), u32(in.pos.y)) >= sa {
            discard;
        }
        return vec4(c.rgb / sa, 1.0);
    }
    return c;
}

// ---- the other blend modes: W3C Compositing and Blending formulas on straight, sRGB-encoded
// colour, exactly as `filmcraft_render::blend` (no recursion in WGSL, so helpers are split out).

fn screen(b: f32, s: f32) -> f32 { return b + s - b * s; }

fn color_burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 { return 1.0; }
    if s <= 0.0 { return 0.0; }
    return 1.0 - min((1.0 - b) / s, 1.0);
}

fn color_dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 { return 0.0; }
    if s >= 1.0 { return 1.0; }
    return min(b / (1.0 - s), 1.0);
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 { return b * 2.0 * s; }
    return screen(b, 2.0 * s - 1.0);
}

fn vivid_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 { return color_burn(b, 2.0 * s); }
    return color_dodge(b, 2.0 * s - 1.0);
}

fn sep(mode: u32, b: f32, s: f32) -> f32 {
    switch mode {
        case 2u: { return min(b, s); }                       // Darken
        case 3u: { return b * s; }                           // Multiply
        case 4u: { return color_burn(b, s); }
        case 5u: { return max(b + s - 1.0, 0.0); }           // Linear Burn
        case 7u: { return max(b, s); }                       // Lighten
        case 8u: { return screen(b, s); }
        case 9u: { return color_dodge(b, s); }
        case 10u: { return min(b + s, 1.0); }                // Linear Dodge (Add)
        case 12u: { return hard_light(s, b); }               // Overlay
        case 13u: {                                          // Soft Light
            if s <= 0.5 { return b - (1.0 - 2.0 * s) * b * (1.0 - b); }
            var d = sqrt(b);
            if b <= 0.25 { d = ((16.0 * b - 12.0) * b + 4.0) * b; }
            return b + (2.0 * s - 1.0) * (d - b);
        }
        case 14u: { return hard_light(b, s); }
        case 15u: { return vivid_light(b, s); }
        case 16u: { return clamp(b + 2.0 * s - 1.0, 0.0, 1.0); } // Linear Light
        case 17u: {                                          // Pin Light
            if s <= 0.5 { return min(b, 2.0 * s); }
            return max(b, 2.0 * s - 1.0);
        }
        case 18u: { return select(0.0, 1.0, vivid_light(b, s) >= 0.5); } // Hard Mix
        case 19u: { return abs(b - s); }                     // Difference
        case 20u: { return b + s - 2.0 * b * s; }            // Exclusion
        case 21u: { return max(b - s, 0.0); }                // Subtract
        case 22u: {                                          // Divide
            if s <= 0.0 { return 1.0; }
            return min(b / s, 1.0);
        }
        default: { return s; }
    }
}

fn lum(c: vec3<f32>) -> f32 { return 0.3 * c.r + 0.59 * c.g + 0.11 * c.b; }

fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let n = min(min(c.r, c.g), c.b);
    let x = max(max(c.r, c.g), c.b);
    var o = c;
    if n < 0.0 { o = l + (o - l) * l / max(l - n, 1e-6); }
    if x > 1.0 { o = l + (o - l) * (1.0 - l) / max(x - l, 1e-6); }
    return o;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> { return clip_color(c + (l - lum(c))); }

fn sat(c: vec3<f32>) -> f32 { return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b); }

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(max(c.r, c.g), c.b);
    let mn = min(min(c.r, c.g), c.b);
    if mx - mn <= 1e-6 { return vec3(0.0); }
    return (c - mn) * s / (mx - mn);
}

fn blend_rgb(mode: u32, b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 23u: { return set_lum(set_sat(s, sat(b)), lum(b)); } // Hue
        case 24u: { return set_lum(set_sat(b, sat(s)), lum(b)); } // Saturation
        case 25u: { return set_lum(s, lum(b)); }                  // Color
        case 26u: { return set_lum(b, lum(s)); }                  // Luminosity
        case 6u: { return select(b, s, lum(s) < lum(b)); }        // Darker Color
        case 11u: { return select(b, s, lum(s) > lum(b)); }       // Lighter Color
        default: { return vec3(sep(mode, b.r, s.r), sep(mode, b.g, s.g), sep(mode, b.b, s.b)); }
    }
}

fn linear_to_srgb(v: vec3<f32>) -> vec3<f32> {
    let hi = 1.055 * pow(max(v, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055;
    return select(hi, v * 12.92, v <= vec3(0.0031308));
}

// Blend modes that need the destination: written without fixed-function blending.
@fragment
fn fs_blend(in: VOut) -> @location(0) vec4<f32> {
    let sp = layer_color(in.sp);
    let sa = sp.a;
    if sa <= 0.0 {
        discard;
    }
    let d = textureLoad(backdrop, vec2<i32>(in.pos.xy), 0);
    let da = d.a;
    if da <= 0.0 {
        return sp + d * (1.0 - sa);
    }
    let cs = linear_to_srgb(sp.rgb / sa);
    let cb = linear_to_srgb(d.rgb / da);
    let mixed = srgb_to_linear(clamp(blend_rgb(u32(u.p3.x), cb, cs), vec3(0.0), vec3(1.0)));
    return vec4(sp.rgb * (1.0 - da) + d.rgb * (1.0 - sa) + sa * da * mixed, sa + da - sa * da);
}

// ---- final pass: accumulator (linear premul) over black → sRGB target

@group(0) @binding(0) var accum: texture_2d<f32>;

// Working → monitor of an HDR / wide-gamut pipeline (`OutputTransform::display`, see
// `display_uniforms`).
struct D {
    tone: vec4<f32>,   // on, PQ(src peak), PQ(dst peak) / PQ(src peak), knee start
    peaks: vec4<f32>,  // src peak, dst peak (cd/m²)
    m0: vec4<f32>,     // gamut matrix row 0 (w: on)
    m1: vec4<f32>,
    m2: vec4<f32>,
    desat: vec4<f32>,  // target luma weights (w: on)
};
@group(0) @binding(1) var<uniform> disp: D;

fn pq_inv1(y: f32) -> f32 {
    let m1 = 0.15930176; let m2 = 78.84375;
    let p = pow(max(y, 1e-12), m1);
    return pow((0.8359375 + 18.851563 * p) / (1.0 + 18.6875 * p), m2);
}
fn pq_eotf1(e: f32) -> f32 {
    let m1 = 0.15930176; let m2 = 78.84375;
    let p = pow(max(e, 1e-12), 1.0 / m2);
    return pow(max(p - 0.8359375, 0.0) / (18.851563 - 18.6875 * p), 1.0 / m1);
}

// `bt2390_eetf` (cd/m²).
fn eetf(nits: f32) -> f32 {
    let src = disp.peaks.x; let dst = disp.peaks.y;
    if dst >= src || nits <= 0.0 {
        return min(max(nits, 0.0), max(src, dst));
    }
    let smax = disp.tone.y; let max_lum = disp.tone.z; let ks = disp.tone.w;
    let e1 = min(pq_inv1(clamp(nits / 10000.0, 0.0, 1.0)) / smax, 1.0);
    var e2 = e1;
    if e1 >= ks {
        let t = (e1 - ks) / (1.0 - ks);
        let t2 = t * t; let t3 = t2 * t;
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks) + (-2.0 * t3 + 3.0 * t2) * max_lum;
    }
    return pq_eotf1(e2 * smax) * 10000.0;
}

// `OutputTransform::apply` on a straight working colour.
fn to_display(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    if disp.tone.x != 0.0 {
        let m = max(max(c.r, c.g), c.b);
        if m > 0.0 {
            let nits = m * 203.0;
            var k = 1.0;
            if nits >= disp.peaks.x {
                k = disp.peaks.y / 203.0 / m;
            } else if nits > 1e-6 {
                k = eetf(nits) / nits;
            }
            c = c * k;
        }
    }
    if disp.m0.w != 0.0 {
        c = vec3(dot(disp.m0.xyz, c), dot(disp.m1.xyz, c), dot(disp.m2.xyz, c));
    }
    if disp.desat.w != 0.0 {
        let mn = min(min(c.r, c.g), c.b);
        if mn < 0.0 {
            let y = dot(disp.desat.xyz, c);
            if y <= 0.0 {
                return vec3(0.0);
            }
            let t = y / (y - mn);
            c = max(vec3(y) + (c - vec3(y)) * t, vec3(0.0));
        }
    }
    return c;
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    return vec4(x, y, 0.0, 1.0);
}

@fragment
fn fs_full(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = textureLoad(accum, vec2<i32>(pos.xy), 0);
    var rgb = px.rgb;
    // HDR / wide gamut: the monitor conversion on the straight colour (as `map_rgb` on the CPU)
    if (disp.tone.x != 0.0 || disp.m0.w != 0.0 || disp.desat.w != 0.0) && px.a > 1e-6 {
        rgb = to_display(rgb / px.a) * px.a;
    }
    let c = clamp(rgb, vec3(0.0), vec3(1.0));
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
    return vec4(select(hi, lo, c <= vec3(0.0031308)), 1.0);
}
