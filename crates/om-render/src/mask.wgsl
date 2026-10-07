// SPDX-License-Identifier: Apache-2.0
// Mask coverage: signed distance (pixels) from each pixel centre to a closed
// polygon, even-odd inside test, linear feather, optional inversion.
// Mirrors reference::mask_coverage.

const MAX_POINTS: u32 = 1024u;

struct Mask {
    // x: point count, y: feather (px), z: invert (0/1), w: unused
    params: vec4<f32>,
    // Two points per vec4 (x0 y0 x1 y1), in pixels.
    points: array<vec4<f32>, 512>,
};

@group(0) @binding(0) var<uniform> mask: Mask;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn point(i: u32) -> vec2<f32> {
    let v = mask.points[i / 2u];
    return select(v.zw, v.xy, i % 2u == 0u);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let p = frag.xy;
    let n = u32(mask.params.x);
    var d2 = 1e30;
    var inside = false;
    for (var i = 0u; i < n; i = i + 1u) {
        let a = point(i);
        let b = point((i + 1u) % n);
        let ab = b - a;
        let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-12), 0.0, 1.0);
        let q = a + ab * t - p;
        d2 = min(d2, dot(q, q));
        if (a.y > p.y) != (b.y > p.y) {
            let x = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
            if p.x < x {
                inside = !inside;
            }
        }
    }
    var c: f32;
    let feather = mask.params.y;
    if feather <= 0.0 {
        c = select(0.0, 1.0, inside);
    } else {
        let sd = select(-sqrt(d2), sqrt(d2), inside);
        c = clamp(0.5 + sd / feather, 0.0, 1.0);
    }
    if mask.params.z > 0.5 {
        c = 1.0 - c;
    }
    return vec4<f32>(c, 0.0, 0.0, 1.0);
}
