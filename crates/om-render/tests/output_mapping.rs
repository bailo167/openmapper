// SPDX-License-Identifier: Apache-2.0
//! Output mapping on the GPU: canvas regions, corner pin and soft-edge
//! blending, checked against the canvas readback and the blend maths.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::needless_range_loop
)]

use om_geom::{Homography, Point2};
use om_gpu::GpuContext;
use om_media_core::StillImage;
use om_project::{
    Canvas, Media, MediaSource, OutputMapping, PatternKind, Project, Shape, SoftEdge, Surface,
};
use om_render::Compositor;
use om_types::{Finite, MediaId, SurfaceId};

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(g) => Some(g),
        Err(e) if std::env::var("OM_REQUIRE_GPU").as_deref() == Ok("1") => {
            panic!("OM_REQUIRE_GPU=1 but no GPU: {e}")
        }
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            None
        }
    }
}

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y).unwrap()
}

fn quad(x0: f64, y0: f64, x1: f64, y1: f64) -> [Point2; 4] {
    [p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)]
}

const MEDIA: MediaId = MediaId::from_u128(7);

/// A full-canvas surface showing `image`.
fn render(w: u32, h: u32, image: &StillImage) -> Option<Compositor> {
    let mut pr = Project::new("output mapping");
    pr.canvas = Canvas {
        width: w,
        height: h,
    };
    pr.media.push(Media {
        id: MEDIA,
        name: "m".into(),
        source: MediaSource::Pattern {
            pattern: PatternKind::White,
        },
        playback: Default::default(),
        extensions: Default::default(),
    });
    let mut s = Surface::new(SurfaceId::from_u128(1), "full");
    s.shape = Shape::Quad {
        corners: quad(0.0, 0.0, 1.0, 1.0),
        uv: quad(0.0, 0.0, 1.0, 1.0),
    };
    s.media = Some(MEDIA);
    pr.surfaces.push(s);
    pr.validate().unwrap();
    let mut c = Compositor::new(gpu()?);
    c.set_image(MEDIA, image).unwrap();
    c.render(&pr).unwrap();
    Some(c)
}

fn gradient(w: u32, h: u32) -> StillImage {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[
                (x * 255 / (w - 1)) as u8,
                (y * 255 / (h - 1)) as u8,
                // Smooth everywhere: hard edges would measure the driver's
                // sub-texel filter precision, not the mapping
                // (tests/tolerances.md).
                ((x + y) * 255 / (w + h - 2)) as u8,
                255,
            ]);
        }
    }
    StillImage::from_rgba8(w, h, px).unwrap()
}

fn srgb_to_linear(v: u8) -> f64 {
    let c = f64::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f64) -> f64 {
    let l = l.clamp(0.0, 1.0);
    255.0
        * if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
}

/// CPU bilinear sample (linear light) of an sRGB RGBA8 frame at canvas
/// position `c` (texel centres at (i + 0.5) / w), clamped at the edges.
fn sample(frame: &[u8], w: u32, h: u32, c: (f64, f64), ch: usize) -> f64 {
    let fx = (c.0 * f64::from(w) - 0.5).clamp(0.0, f64::from(w - 1));
    let fy = (c.1 * f64::from(h) - 0.5).clamp(0.0, f64::from(h - 1));
    let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - fx.floor(), fy - fy.floor());
    let at = |x: u32, y: u32| srgb_to_linear(frame[((y * w + x) * 4) as usize + ch]);
    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
    let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
    linear_to_srgb(top * (1.0 - ty) + bottom * ty)
}

#[test]
fn region_shows_exactly_that_part_of_the_canvas() {
    let Some(mut c) = render(128, 64, &gradient(128, 64)) else {
        return;
    };
    let canvas = c.read_rgba8().unwrap();
    let mapping = OutputMapping {
        region: quad(0.5, 0.0, 1.0, 1.0),
        ..OutputMapping::default()
    };
    let out = c.read_mapped_rgba8(&mapping, 64, 64).unwrap();
    let mut worst = 0u8;
    for y in 0..64usize {
        for x in 0..64usize {
            for ch in 0..3 {
                let a = out[(y * 64 + x) * 4 + ch];
                let b = canvas[(y * 128 + 64 + x) * 4 + ch];
                worst = worst.max(a.abs_diff(b));
            }
        }
    }
    assert!(worst <= 1, "max difference {worst}");
}

#[test]
fn corner_pin_matches_the_cpu_reference_and_blacks_out_the_rest() {
    let (w, h) = (96u32, 64u32);
    let Some(mut c) = render(w, h, &gradient(w, h)) else {
        return;
    };
    let canvas = c.read_rgba8().unwrap();
    let mapping = OutputMapping {
        region: quad(0.1, 0.0, 0.9, 1.0),
        warp: [p(0.2, 0.1), p(0.85, 0.2), p(0.9, 0.95), p(0.1, 0.8)],
        ..OutputMapping::default()
    };
    let (ow, oh) = (80u32, 60u32);
    let out = c.read_mapped_rgba8(&mapping, ow, oh).unwrap();
    let warp_inv = Homography::square_to_quad(&mapping.warp)
        .unwrap()
        .inverse()
        .unwrap();
    let region = Homography::square_to_quad(&mapping.region).unwrap();
    let (mut sum, mut n, mut worst) = (0.0, 0u32, 0.0f64);
    for y in 0..oh {
        for x in 0..ow {
            let o = (
                (f64::from(x) + 0.5) / f64::from(ow),
                (f64::from(y) + 0.5) / f64::from(oh),
            );
            let s = warp_inv.apply(o).unwrap();
            let px = &out[((y * ow + x) * 4) as usize..][..3];
            // Skip a one-pixel band around the warp's edge.
            let margin = 1.5 / f64::from(ow.min(oh));
            let inside = |m: f64| (-m..=1.0 + m).contains(&s.0) && (-m..=1.0 + m).contains(&s.1);
            if !inside(margin) {
                assert_eq!(px, [0, 0, 0], "outside the warp at ({x}, {y})");
                continue;
            }
            if !inside(-margin) {
                continue;
            }
            let cpos = region.apply(s).unwrap();
            for ch in 0..3 {
                let d = (f64::from(px[ch]) - sample(&canvas, w, h, cpos, ch)).abs();
                worst = worst.max(d);
                sum += d;
                n += 1;
            }
        }
    }
    let mean = sum / f64::from(n);
    assert!(mean <= 0.5 && worst <= 3.0, "mean {mean}, max {worst}");
}

#[test]
fn overlapping_soft_edges_sum_to_full_light() {
    // A white canvas split across two "projectors" overlapping by 40 px.
    let (w, h) = (200u32, 20u32);
    let Some(mut c) = render(
        w,
        h,
        &StillImage::pattern(PatternKind::White, 4, 4).unwrap(),
    ) else {
        return;
    };
    let edge = |left: f64, right: f64| SoftEdge {
        left: Finite::new(left).unwrap(),
        right: Finite::new(right).unwrap(),
        ..SoftEdge::default()
    };
    let a = OutputMapping {
        region: quad(0.0, 0.0, 0.6, 1.0),
        soft_edge: edge(0.0, 1.0 / 3.0),
        ..OutputMapping::default()
    };
    let b = OutputMapping {
        region: quad(0.4, 0.0, 1.0, 1.0),
        soft_edge: edge(1.0 / 3.0, 0.0),
        ..OutputMapping::default()
    };
    // One output pixel per canvas pixel.
    let (oa, ob) = (
        c.read_mapped_rgba8(&a, 120, h).unwrap(),
        c.read_mapped_rgba8(&b, 120, h).unwrap(),
    );
    let gamma = 2.2;
    let light = |v: u8| (f64::from(v) / 255.0).powf(gamma);
    let row = 10usize;
    let mut worst = 0.0f64;
    for x in 0..200usize {
        let la = if x < 120 {
            light(oa[(row * 120 + x) * 4])
        } else {
            0.0
        };
        let lb = if x >= 80 {
            light(ob[(row * 120 + x - 80) * 4])
        } else {
            0.0
        };
        worst = worst.max((la + lb - 1.0).abs());
        if x < 78 {
            assert_eq!(
                oa[(row * 120 + x) * 4],
                255,
                "A is untouched outside its edge"
            );
        }
        if x >= 122 {
            assert_eq!(
                ob[(row * 120 + x - 80) * 4],
                255,
                "B is untouched outside its edge"
            );
        }
    }
    // 8-bit signal quantisation bounds the error (≈1 code at mid-ramp).
    assert!(worst <= 0.02, "overlap light sums deviate by {worst}");
}
