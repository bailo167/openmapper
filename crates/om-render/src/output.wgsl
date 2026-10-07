// SPDX-License-Identifier: Apache-2.0
// Presents the canvas on one output through its mapping: corner pin
// (warp), canvas region and soft edges. Output position o ∈ [0,1]² maps
// to region space s = W⁻¹ o, then to canvas c = R s. Soft edges are
// applied to the display signal (after sRGB encoding) so that light from
// overlapping projectors sums to one.

override encode_srgb: bool = false;

@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

struct Master {
    gain: vec4<f32>,
};
@group(1) @binding(0) var<uniform> master: Master;

struct OutputMap {
    // Columns of W⁻¹ (output → region space) and R (region → canvas).
    warp_inv: mat3x3<f32>,
    region: mat3x3<f32>,
    // Soft-edge widths: left, right, top, bottom (fractions of the region).
    edges: vec4<f32>,
    // x: curve, y: gamma.
    shape: vec4<f32>,
};
@group(2) @binding(0) var<uniform> map: OutputMap;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    var out: VsOut;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Light weight of a ramp at t (0 dark .. 1 full); symmetric S-curve.
fn light_weight(t: f32, curve: f32) -> f32 {
    let x = clamp(t, 0.0, 1.0);
    if x < 0.5 {
        return 0.5 * pow(2.0 * x, curve);
    }
    return 1.0 - 0.5 * pow(2.0 * (1.0 - x), curve);
}

fn ramp(pos: f32, width: f32) -> f32 {
    if width <= 0.0 || pos >= width {
        return 1.0;
    }
    return pow(light_weight(pos / width, map.shape.x), 1.0 / map.shape.y);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sh = map.warp_inv * vec3<f32>(in.uv, 1.0);
    if abs(sh.z) < 1e-7 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let s = sh.xy / sh.z;
    let eps = 1e-5;
    if any(s < vec2<f32>(-eps)) || any(s > vec2<f32>(1.0 + eps)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let ch = map.region * vec3<f32>(s, 1.0);
    if abs(ch.z) < 1e-7 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let c = ch.xy / ch.z;
    var rgb = vec3<f32>(0.0);
    if all(c >= vec2<f32>(0.0)) && all(c <= vec2<f32>(1.0)) {
        let px = textureSampleLevel(canvas, canvas_sampler, c, 0.0);
        rgb = clamp(px.rgb * master.gain.x, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let mask = ramp(s.x, map.edges.x) * ramp(1.0 - s.x, map.edges.y)
        * ramp(s.y, map.edges.z) * ramp(1.0 - s.y, map.edges.w);
    let signal = to_srgb(rgb) * mask;
    if encode_srgb {
        return vec4<f32>(signal, 1.0);
    }
    return vec4<f32>(to_linear(signal), 1.0);
}
