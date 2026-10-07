// SPDX-License-Identifier: Apache-2.0
// Draws one surface into the linear, premultiplied working canvas.
// The canvas-to-UV map is evaluated per pixel, so perspective is exact.

struct Item {
    // Columns of the 3x3 canvas->UV homography (xyz used).
    c0: vec4<f32>,
    c1: vec4<f32>,
    c2: vec4<f32>,
    // Columns of the canvas->local homography used for clipping (xyz).
    l0: vec4<f32>,
    l1: vec4<f32>,
    l2: vec4<f32>,
    // x: opacity, y: canvas width, z: canvas height, w: mapping kind:
    // 0 projective; 1 projective + ellipse clip (l0..l2 = canvas->local);
    // 2 bilinear patch (c0 = P00 P10, c1 = P11 P01, l0/l1 = their UVs).
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

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

// Inverse bilinear (see om_geom::inverse_bilinear). Returns (s, t, ok, 0).
fn inverse_bilinear(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, d: vec2<f32>) -> vec4<f32> {
    let e = b - a;
    let f = d - a;
    let g = a - b + c - d;
    let h = p - a;
    let k2 = cross2(g, f);
    let k1 = cross2(e, f) + cross2(h, g);
    let k0 = cross2(h, e);
    // Stable quadratic roots (see om_geom::inverse_bilinear).
    let disc = k1 * k1 - 4.0 * k0 * k2;
    if disc < 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let sgn = select(-1.0, 1.0, k1 >= 0.0);
    let q = -0.5 * (k1 + sgn * sqrt(disc));
    let v0 = select(-1.0, k0 / q, q != 0.0);
    let v1 = select(-1.0, q / k2, k2 != 0.0);
    for (var i = 0; i < 2; i = i + 1) {
        let v = select(v1, v0, i == 0);
        let den = e + g * v;
        let u = select((h.y - f.y * v) / den.y, (h.x - f.x * v) / den.x, abs(den.x) >= abs(den.y));
        if u >= -1e-4 && u <= 1.0001 && v >= -1e-4 && v <= 1.0001 {
            return vec4<f32>(clamp(u, 0.0, 1.0), clamp(v, 0.0, 1.0), 1.0, 0.0);
        }
    }
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec3<f32>(frag.x / item.params.y, frag.y / item.params.z, 1.0);
    if item.params.w > 1.5 {
        let st = inverse_bilinear(p.xy, item.c0.xy, item.c0.zw, item.c1.xy, item.c1.zw);
        if st.z < 0.5 {
            discard;
        }
        let top = mix(item.l0.xy, item.l0.zw, st.x);
        let bottom = mix(item.l1.zw, item.l1.xy, st.x);
        let uv = mix(top, bottom, st.y);
        return textureSampleLevel(media, media_sampler, uv, 0.0) * item.params.x;
    }
    if item.params.w > 0.5 {
        let l = mat3x3<f32>(item.l0.xyz, item.l1.xyz, item.l2.xyz) * p;
        let d = l.xy / l.z * 2.0 - vec2<f32>(1.0, 1.0);
        if dot(d, d) > 1.0 {
            discard;
        }
    }
    let m = mat3x3<f32>(item.c0.xyz, item.c1.xyz, item.c2.xyz);
    let q = m * p;
    let uv = q.xy / q.z;
    // Media textures are already linear and premultiplied.
    let c = textureSampleLevel(media, media_sampler, uv, 0.0);
    return c * item.params.x;
}
