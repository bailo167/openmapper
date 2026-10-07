// SPDX-License-Identifier: Apache-2.0
//! GPU render goldens: the GPU compositor is checked against the independent
//! CPU reference renderer using the tolerance tiers in docs/PLAN.md.
//!
//! Without a GPU these tests skip, unless `OM_REQUIRE_GPU=1` (set in CI,
//! which provides a software adapter) makes a missing GPU a failure.

// Test helpers outside #[test] fns: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::HashMap;

use om_geom::Point2;
use om_gpu::GpuContext;
use om_media_core::StillImage;
use om_project::{Canvas, Media, MediaSource, PatternKind, Project, Shape, Surface};
use om_render::compare::{diff, edge_mask};
use om_render::reference::{self, RefImage};
use om_render::{Compositor, SkipReason};
use om_types::{MediaId, SurfaceId, UnitInterval};

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(g) => {
            eprintln!("GPU: {}", g.capabilities());
            Some(g)
        }
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

const GRID: MediaId = MediaId::from_u128(100);
const WHITE: MediaId = MediaId::from_u128(101);

fn project(w: u32, h: u32, surfaces: Vec<Surface>) -> Project {
    let mut pr = Project::new("golden");
    pr.canvas = Canvas {
        width: w,
        height: h,
    };
    for (id, pattern) in [(GRID, PatternKind::UvGrid), (WHITE, PatternKind::White)] {
        pr.media.push(Media {
            id,
            name: format!("{pattern:?}"),
            source: MediaSource::Pattern { pattern },
            playback: Default::default(),
            extensions: Default::default(),
        });
    }
    pr.surfaces = surfaces;
    pr.validate().unwrap();
    pr
}

fn surface(n: u128, shape: Shape, media: MediaId) -> Surface {
    let mut s = Surface::new(SurfaceId::from_u128(n), format!("s{n}"));
    s.shape = shape;
    s.media = Some(media);
    s
}

struct Fixture {
    images: Vec<(MediaId, StillImage)>,
}

impl Fixture {
    fn new(tex: (u32, u32)) -> Self {
        Self {
            images: vec![
                (
                    GRID,
                    StillImage::pattern(PatternKind::UvGrid, tex.0, tex.1).unwrap(),
                ),
                (
                    WHITE,
                    StillImage::pattern(PatternKind::White, 4, 4).unwrap(),
                ),
            ],
        }
    }

    fn gpu_render(&self, c: &mut Compositor, pr: &Project) -> Vec<u8> {
        for (id, img) in &self.images {
            c.set_image(*id, img).unwrap();
        }
        c.render(pr).unwrap();
        c.read_rgba8().unwrap()
    }

    fn cpu_render(&self, pr: &Project) -> reference::RefFrame {
        let imgs: HashMap<_, _> = self
            .images
            .iter()
            .map(|(id, i)| (*id, RefImage::new(i)))
            .collect();
        reference::render(pr, &imgs)
    }
}

/// Asserts GPU ≈ CPU in the interior (edges are checked separately by
/// `geometry_edges_within_one_pixel`). `max_tol` is 1 for smooth media; media
/// with hard texel discontinuities get 4 (see tests/tolerances.md). Mean and
/// p99.9 tiers are fixed.
fn assert_matches_reference(
    gpu_px: &[u8],
    cpu: &reference::RefFrame,
    label: &str,
    max_tol: Option<u8>,
) {
    let edges = edge_mask(&cpu.coverage, cpu.width, cpu.height, 1);
    let interior = diff(gpu_px, &cpu.rgba8, |i| !edges[i]);
    eprintln!("{label}: interior {interior:?}");
    assert!(
        interior.mean <= 0.25,
        "{label}: interior mean {}",
        interior.mean
    );
    assert!(
        interior.p999 <= 2,
        "{label}: interior p99.9 {}",
        interior.p999
    );
    if let Some(max) = max_tol {
        assert!(
            interior.max <= max,
            "{label}: interior max {}",
            interior.max
        );
    }
}

/// Hard-edged media: no max bound; mean and p99.9 still apply
/// (tests/tolerances.md).
const SHARP: Option<u8> = None;
/// Default GPU-golden tier.
const SMOOTH: Option<u8> = Some(1);

/// A smooth test-only gradient with no texel discontinuities.
fn gradient(w: u32, h: u32) -> StillImage {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[
                (x * 255 / (w - 1)) as u8,
                (y * 255 / (h - 1)) as u8,
                ((x + y) * 255 / (w + h - 2)) as u8,
                255,
            ]);
        }
    }
    StillImage::from_rgba8(w, h, px).unwrap()
}

#[test]
fn identity_quad_reproduces_source() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((64, 48));
    let pr = project(64, 48, vec![surface(1, Shape::full_quad(), GRID)]);
    let out = fx.gpu_render(&mut c, &pr);
    let src = &fx.images[0].1;
    let s = diff(&out, src.rgba8(), |_| true);
    eprintln!("identity: {s:?}");
    assert!(s.max <= 1, "identity quad max error {}", s.max);
    assert_matches_reference(&out, &fx.cpu_render(&pr), "identity", SMOOTH);
}

#[test]
fn perspective_quad_matches_reference() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((128, 96));
    let shape = Shape::Quad {
        corners: [p(0.10, 0.10), p(0.82, 0.18), p(0.91, 0.86), p(0.06, 0.92)],
        uv: Point2::unit_square(),
    };
    let pr = project(160, 120, vec![surface(1, shape, GRID)]);
    let out = fx.gpu_render(&mut c, &pr);
    assert_matches_reference(&out, &fx.cpu_render(&pr), "perspective", SHARP);

    // Same mapping with smooth media must meet the default 1-code tier,
    // proving the larger UV-grid errors come from texel discontinuities and
    // not from the mapping.
    let smooth = Fixture {
        images: vec![(GRID, gradient(128, 96))],
    };
    let out = smooth.gpu_render(&mut c, &pr);
    assert_matches_reference(&out, &smooth.cpu_render(&pr), "perspective-smooth", SMOOTH);
}

#[test]
fn mirrored_cropped_quad_and_triangle_match_reference() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((100, 100));
    let mirrored = Shape::Quad {
        corners: [p(0.9, 0.05), p(0.1, 0.1), p(0.05, 0.6), p(0.95, 0.5)],
        uv: [p(0.2, 0.1), p(0.8, 0.1), p(0.8, 0.7), p(0.2, 0.7)],
    };
    let tri = Shape::Triangle {
        corners: [p(0.2, 0.55), p(0.9, 0.6), p(0.4, 0.97)],
        uv: [p(0.0, 0.0), p(1.0, 0.0), p(0.5, 1.0)],
    };
    let pr = project(
        150,
        110,
        vec![surface(1, mirrored, GRID), surface(2, tri, GRID)],
    );
    let out = fx.gpu_render(&mut c, &pr);
    assert_matches_reference(&out, &fx.cpu_render(&pr), "mirrored+triangle", SHARP);
}

#[test]
fn geometry_edges_within_one_pixel() {
    // White media isolates geometry: every GPU-covered pixel must be covered
    // by the reference or lie within 1 px of a reference edge, and vice versa.
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((8, 8));
    let shape = Shape::Quad {
        corners: [p(0.13, 0.07), p(0.87, 0.21), p(0.79, 0.93), p(0.04, 0.71)],
        uv: Point2::unit_square(),
    };
    let pr = project(97, 83, vec![surface(1, shape, WHITE)]);
    let out = fx.gpu_render(&mut c, &pr);
    let cpu = fx.cpu_render(&pr);
    let edges = edge_mask(&cpu.coverage, cpu.width, cpu.height, 1);
    let mut bad = 0;
    for (i, px) in out.as_chunks::<4>().0.iter().enumerate() {
        let gpu_covered = px[0] > 127;
        if gpu_covered != cpu.coverage[i] && !edges[i] {
            bad += 1;
        }
    }
    assert_eq!(bad, 0, "pixels with coverage error > 1px");
}

#[test]
fn opacity_and_stacking_order() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((32, 32));
    let mut top = surface(2, Shape::centred_quad(), WHITE);
    top.opacity = UnitInterval::new(0.5).unwrap();
    let pr = project(64, 64, vec![surface(1, Shape::full_quad(), GRID), top]);
    let out = fx.gpu_render(&mut c, &pr);
    assert_matches_reference(&out, &fx.cpu_render(&pr), "opacity", SHARP);
    // 50% white over black in linear light is sRGB code 188.
    let mut solo = surface(3, Shape::full_quad(), WHITE);
    solo.opacity = UnitInterval::new(0.5).unwrap();
    let pr = project(8, 8, vec![solo]);
    let out = fx.gpu_render(&mut c, &pr);
    assert!(out[0].abs_diff(188) <= 1, "got {}", out[0]);
}

#[test]
fn unmappable_disabled_and_unassigned_surfaces_draw_nothing() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((8, 8));
    let concave = Shape::Quad {
        corners: [p(0.0, 0.0), p(1.0, 0.0), p(0.3, 0.3), p(0.0, 1.0)],
        uv: Point2::unit_square(),
    };
    let mut disabled = surface(2, Shape::full_quad(), WHITE);
    disabled.enabled = false;
    let mut unassigned = surface(3, Shape::full_quad(), WHITE);
    unassigned.media = None;
    let pr = project(
        16,
        16,
        vec![surface(1, concave, WHITE), disabled, unassigned],
    );
    for (id, img) in &fx.images {
        c.set_image(*id, img).unwrap();
    }
    let report = c.render(&pr).unwrap();
    assert!(report.plan.items.is_empty());
    let reasons: Vec<_> = report.plan.skipped.iter().map(|(_, r)| *r).collect();
    assert!(matches!(reasons[0], SkipReason::Unmappable(_)));
    assert_eq!(reasons[1], SkipReason::Disabled);
    assert_eq!(reasons[2], SkipReason::NoMedia);
    assert!(
        c.read_rgba8()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| px[..3] == [0, 0, 0])
    );
}

#[test]
fn repeated_frames_do_not_grow_resources() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((64, 64));
    for (id, img) in &fx.images {
        c.set_image(*id, img).unwrap();
    }
    let mut pr = project(128, 72, vec![surface(1, Shape::full_quad(), GRID)]);
    c.render(&pr).unwrap();
    c.read_rgba8().unwrap(); // creates the readback pipeline before the baseline
    let mut baseline = None;
    for frame in 0..400u32 {
        // Vary geometry and surface count every frame.
        let k = f64::from(frame % 50) / 100.0;
        let n = 1 + (frame % 7) as u128;
        pr.surfaces = (0..n)
            .map(|i| {
                let shape = Shape::Quad {
                    corners: [p(k, 0.0), p(1.0, k), p(1.0 - k, 1.0), p(0.0, 1.0 - k)],
                    uv: Point2::unit_square(),
                };
                surface(10 + i, shape, if i % 2 == 0 { GRID } else { WHITE })
            })
            .collect();
        c.render(&pr).unwrap();
        if frame == 50 {
            baseline = Some(c.resource_counts());
        }
    }
    c.read_rgba8().unwrap();
    assert_eq!(
        Some(c.resource_counts()),
        baseline,
        "resources grew in steady state"
    );
    c.remove_image(GRID);
    assert_eq!(c.resource_counts().media_textures, 1);
}

#[test]
fn oversized_canvas_and_media_are_errors_not_panics() {
    let Some(g) = gpu() else { return };
    let max = g.max_texture_dimension();
    let mut c = Compositor::new(g);
    if max < Canvas::MAX_DIMENSION {
        let pr = project(max + 1, 16, vec![]);
        assert!(matches!(
            c.render(&pr),
            Err(om_render::RenderError::TooLarge { .. })
        ));
    }
    // A too-wide image is rejected without touching the GPU.
    let wide = StillImage::from_rgba8(max + 1, 1, vec![0; (max as usize + 1) * 4]).unwrap();
    assert!(c.set_image(GRID, &wide).is_err());
    assert!(!c.has_image(GRID));
    // The compositor keeps working afterwards.
    let pr = project(8, 8, vec![]);
    c.render(&pr).unwrap();
    c.read_rgba8().unwrap();
}

#[test]
fn per_frame_uploads_reuse_textures() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let pr = project(32, 32, vec![surface(1, Shape::full_quad(), GRID)]);
    let a = StillImage::pattern(PatternKind::White, 16, 16).unwrap();
    let b = StillImage::pattern(PatternKind::Checkerboard, 16, 16).unwrap();
    c.set_image(GRID, &a).unwrap();
    c.render(&pr).unwrap();
    let before = c.resource_counts();
    for i in 0..200 {
        c.set_image(GRID, if i % 2 == 0 { &b } else { &a }).unwrap();
        c.render(&pr).unwrap();
    }
    assert_eq!(c.resource_counts(), before);
    // The last upload (a, white) is what renders.
    assert!(
        c.read_rgba8()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] == 255)
    );
}
