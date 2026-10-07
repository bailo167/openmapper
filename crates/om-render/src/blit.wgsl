// SPDX-License-Identifier: Apache-2.0
// Presents the working canvas on an opaque target (display, preview,
// readback): composites over black and encodes sRGB when the target format
// does not do so in hardware.

override encode_srgb: bool = false;

@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the viewport.
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(canvas, canvas_sampler, in.uv, 0.0);
    // Premultiplied over opaque black is just the premultiplied colour.
    var rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if encode_srgb {
        rgb = to_srgb(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
