// SPDX-License-Identifier: Apache-2.0
// Draws one surface into the linear, premultiplied working canvas.
// The canvas-to-UV map is evaluated per pixel, so perspective is exact.

struct Item {
    // Columns of the 3x3 canvas->UV homography (xyz used).
    c0: vec4<f32>,
    c1: vec4<f32>,
    c2: vec4<f32>,
    // x: opacity, y: canvas width, z: canvas height.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> item: Item;
@group(1) @binding(0) var media: texture_2d<f32>;
@group(1) @binding(1) var media_sampler: sampler;

@vertex
fn vs_main(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
    // Canvas space (0..1, y down) to clip space.
    return vec4<f32>(pos.x * 2.0 - 1.0, 1.0 - pos.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec3<f32>(frag.x / item.params.y, frag.y / item.params.z, 1.0);
    let m = mat3x3<f32>(item.c0.xyz, item.c1.xyz, item.c2.xyz);
    let q = m * p;
    let uv = q.xy / q.z;
    // Media textures are already linear and premultiplied.
    let c = textureSampleLevel(media, media_sampler, uv, 0.0);
    return c * item.params.x;
}
