// SPDX-License-Identifier: Apache-2.0
// 3-D mapping: draws a model as seen by a calibrated projector, textured
// with the canvas through the model's UVs (OBJ origin bottom-left).

override encode_srgb: bool = false;

@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

struct Projection {
    view_proj: mat4x4<f32>,
    // x: master gain.
    gain: vec4<f32>,
};
@group(1) @binding(0) var<uniform> proj: Projection;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.pos = proj.view_proj * vec4<f32>(v.pos, 1.0);
    out.uv = v.uv;
    return out;
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = vec2<f32>(in.uv.x, 1.0 - in.uv.y);
    let px = textureSampleLevel(canvas, canvas_sampler, c, 0.0);
    var rgb = clamp(px.rgb * proj.gain.x, vec3<f32>(0.0), vec3<f32>(1.0));
    if encode_srgb {
        rgb = to_srgb(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
