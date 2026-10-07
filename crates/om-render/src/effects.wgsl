// SPDX-License-Identifier: Apache-2.0
// Per-surface effect passes on linear, premultiplied media textures.
// Mirrored exactly by effects.rs (CPU reference).

struct Fx {
    // Color: x brightness, y contrast, z saturation, w gamma
    a: vec4<f32>,
    // Color: x cos(hue), y sin(hue). Blur: x tap radius. Pixelate: x size.
    b: vec4<f32>,
    // Blur weights for offsets 0..63, four per vec4.
    w: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<uniform> fx: Fx;
@group(1) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn load(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    return textureLoad(src, clamp(p, vec2<i32>(0, 0), size - vec2<i32>(1, 1)), 0);
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

fn to_linear(s: vec3<f32>) -> vec3<f32> {
    let x = clamp(s, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(pow((x + 0.055) / 1.055, vec3<f32>(2.4)), x / 12.92, x <= vec3<f32>(0.04045));
}

// Un-premultiply and encode to sRGB for perceptual colour operations.
fn unpack(c: vec4<f32>) -> vec3<f32> {
    if c.a <= 0.0 {
        return vec3<f32>(0.0);
    }
    return to_srgb(c.rgb / c.a);
}

fn pack(s: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(to_linear(s) * a, a);
}

@fragment
fn fs_color(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(frag.xy));
    var s = unpack(c) + vec3<f32>(fx.a.x);
    s = (s - vec3<f32>(0.5)) * fx.a.y + vec3<f32>(0.5);
    let luma = dot(s, vec3<f32>(0.2126, 0.7152, 0.0722));
    s = mix(vec3<f32>(luma), s, fx.a.z);
    let co = fx.b.x;
    let si = fx.b.y;
    // W3C Filter Effects hue-rotate matrix (rows).
    let r0 = vec3<f32>(0.213 + co * 0.787 - si * 0.213, 0.715 - co * 0.715 - si * 0.715, 0.072 - co * 0.072 + si * 0.928);
    let r1 = vec3<f32>(0.213 - co * 0.213 + si * 0.143, 0.715 + co * 0.285 + si * 0.140, 0.072 - co * 0.072 - si * 0.283);
    let r2 = vec3<f32>(0.213 - co * 0.213 - si * 0.787, 0.715 - co * 0.715 + si * 0.715, 0.072 + co * 0.928 + si * 0.072);
    s = vec3<f32>(dot(r0, s), dot(r1, s), dot(r2, s));
    s = pow(max(s, vec3<f32>(0.0)), vec3<f32>(1.0 / fx.a.w));
    return pack(clamp(s, vec3<f32>(0.0), vec3<f32>(1.0)), c.a);
}

@fragment
fn fs_invert(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = load(vec2<i32>(frag.xy));
    return pack(vec3<f32>(1.0) - unpack(c), c.a);
}

fn weight(i: i32) -> f32 {
    let k = u32(abs(i));
    return fx.w[k / 4u][k % 4u];
}

fn blur(frag: vec4<f32>, dir: vec2<i32>) -> vec4<f32> {
    let p = vec2<i32>(frag.xy);
    let r = i32(fx.b.x);
    var acc = vec4<f32>(0.0);
    for (var i = -r; i <= r; i = i + 1) {
        acc = acc + load(p + dir * i) * weight(i);
    }
    return acc;
}

@fragment
fn fs_blur_h(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(frag, vec2<i32>(1, 0));
}

@fragment
fn fs_blur_v(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(frag, vec2<i32>(0, 1));
}

@fragment
fn fs_pixelate(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let n = i32(fx.b.x);
    let p = vec2<i32>(frag.xy);
    let centre = (p / vec2<i32>(n)) * n + vec2<i32>(n / 2);
    return load(centre);
}
