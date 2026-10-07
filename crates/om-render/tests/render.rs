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

#[test]
fn ellipse_line_and_mesh_match_reference() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((96, 96));
    let ellipse = Shape::Ellipse {
        corners: [p(0.05, 0.1), p(0.45, 0.05), p(0.5, 0.55), p(0.02, 0.5)],
        uv: Point2::unit_square(),
    };
    let line = Shape::Line {
        ends: [p(0.1, 0.8), p(0.9, 0.65)],
        width: om_types::Finite::new(0.08).unwrap(),
        uv: Point2::unit_square(),
    };
    // A 3x2 mesh with interior points pushed around (still unfolded).
    let mut mesh = Shape::Quad {
        corners: [p(0.55, 0.08), p(0.95, 0.12), p(0.97, 0.55), p(0.52, 0.5)],
        uv: Point2::unit_square(),
    }
    .to_mesh(3, 2)
    .unwrap();
    if let Shape::Mesh { points, .. } = &mut mesh {
        points[5] = p(points[5].x() + 0.03, points[5].y() - 0.04);
        points[6] = p(points[6].x() - 0.02, points[6].y() + 0.05);
    }
    let pr = project(
        150,
        120,
        vec![
            surface(1, ellipse, GRID),
            surface(2, line, GRID),
            surface(3, mesh, GRID),
        ],
    );
    let out = fx.gpu_render(&mut c, &pr);
    let cpu = fx.cpu_render(&pr);
    assert!(cpu.plan.skipped.is_empty(), "{:?}", cpu.plan.skipped);
    assert_eq!(
        cpu.plan.items.len(),
        1 + 1 + 6,
        "mesh draws one item per cell"
    );
    assert_matches_reference(&out, &cpu, "ellipse+line+mesh", SHARP);
}

/// Mesh cells are bilinear (seamless across cells); on a parallelogram
/// bilinear and perspective mappings coincide, so a mesh built from a
/// parallelogram quad must render exactly like the quad.
#[test]
fn mesh_from_parallelogram_renders_like_the_quad() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((80, 60));
    let quad = Shape::Quad {
        corners: [p(0.1, 0.12), p(0.8, 0.2), p(0.9, 0.9), p(0.2, 0.82)],
        uv: Point2::unit_square(),
    };
    let mesh = quad.to_mesh(4, 3).unwrap();
    let quad_project = project(120, 90, vec![surface(1, quad, GRID)]);
    let a = fx.gpu_render(&mut c, &quad_project);
    let b = fx.gpu_render(&mut c, &project(120, 90, vec![surface(1, mesh, GRID)]));
    // Interior (including every internal cell seam) must match; pixels whose
    // centres lie on the outer outline may flip (the 1-px edge tier).
    let cpu = fx.cpu_render(&quad_project);
    let outer_edge = edge_mask(&cpu.coverage, 120, 90, 1);
    let s = diff(&a, &b, |i| !outer_edge[i]);
    eprintln!("quad vs mesh (interior): {s:?}");
    assert!(
        s.max <= 1,
        "mesh from a parallelogram must equal its quad: {s:?}"
    );
}

#[test]
fn blend_modes_match_reference() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((64, 64));
    for blend in [
        om_project::BlendMode::Normal,
        om_project::BlendMode::Add,
        om_project::BlendMode::Screen,
        om_project::BlendMode::Multiply,
    ] {
        let mut top = surface(2, Shape::centred_quad(), GRID);
        top.blend = blend;
        top.opacity = UnitInterval::new(0.7).unwrap();
        // Rotate the top layer's UVs so it differs from the bottom.
        top.shape = Shape::Quad {
            corners: [p(0.2, 0.2), p(0.8, 0.2), p(0.8, 0.8), p(0.2, 0.8)],
            uv: [p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0), p(0.0, 0.0)],
        };
        let pr = project(64, 64, vec![surface(1, Shape::full_quad(), GRID), top]);
        let out = fx.gpu_render(&mut c, &pr);
        assert_matches_reference(&out, &fx.cpu_render(&pr), &format!("{blend:?}"), SHARP);
    }
}

#[test]
fn folded_mesh_cells_are_reported_and_skipped() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((8, 8));
    let mut mesh = Shape::full_quad().to_mesh(2, 1).unwrap();
    if let Shape::Mesh { points, .. } = &mut mesh {
        points[1] = p(1.2, 0.0); // fold the top middle point past the right edge
    }
    let pr = project(16, 16, vec![surface(1, mesh, WHITE)]);
    for (id, img) in &fx.images {
        c.set_image(*id, img).unwrap();
    }
    let report = c.render(&pr).unwrap();
    assert!(
        matches!(
            report.plan.skipped[..],
            [(_, SkipReason::PartlyUnmappable { skipped_cells: 1 })]
        ),
        "{:?}",
        report.plan.skipped
    );
    assert_eq!(report.plan.items.len(), 1);
}

fn mask(points: &[(f64, f64, bool)], feather: f64, invert: bool) -> om_project::Mask {
    om_project::Mask {
        points: points
            .iter()
            .map(|&(x, y, smooth)| om_project::MaskPoint { p: p(x, y), smooth })
            .collect(),
        feather: om_types::Finite::new(feather).unwrap(),
        invert,
    }
}

#[test]
fn masks_match_reference() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((96, 96));
    let cases = [
        (
            "hard triangle",
            mask(
                &[(0.2, 0.1, false), (0.9, 0.5, false), (0.15, 0.9, false)],
                0.0,
                false,
            ),
        ),
        (
            "feathered",
            mask(
                &[
                    (0.2, 0.2, false),
                    (0.8, 0.25, false),
                    (0.75, 0.8, false),
                    (0.25, 0.75, false),
                ],
                0.08,
                false,
            ),
        ),
        (
            "smooth curve",
            mask(
                &[
                    (0.3, 0.2, true),
                    (0.8, 0.3, true),
                    (0.7, 0.8, true),
                    (0.2, 0.7, true),
                ],
                0.02,
                false,
            ),
        ),
        (
            "inverted",
            mask(
                &[
                    (0.3, 0.3, false),
                    (0.7, 0.3, false),
                    (0.7, 0.7, false),
                    (0.3, 0.7, false),
                ],
                0.05,
                true,
            ),
        ),
    ];
    for (label, m) in cases {
        let mut s = surface(1, Shape::full_quad(), GRID);
        s.mask = Some(m);
        let pr = project(120, 100, vec![s]);
        let out = fx.gpu_render(&mut c, &pr);
        assert_matches_reference(&out, &fx.cpu_render(&pr), label, SHARP);
    }
}

#[test]
fn mask_textures_are_cached_and_released() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx = Fixture::new((16, 16));
    for (id, img) in &fx.images {
        c.set_image(*id, img).unwrap();
    }
    let mut s = surface(1, Shape::full_quad(), WHITE);
    s.mask = Some(mask(
        &[(0.1, 0.1, false), (0.9, 0.1, false), (0.5, 0.9, false)],
        0.0,
        false,
    ));
    let mut pr = project(32, 32, vec![s]);
    c.render(&pr).unwrap();
    let with_mask = c.resource_counts();
    assert_eq!(with_mask.mask_textures, 1);
    for _ in 0..50 {
        c.render(&pr).unwrap();
    }
    assert_eq!(
        c.resource_counts(),
        with_mask,
        "unchanged mask is not reallocated"
    );
    // Editing the mask re-rasterises; the visible result follows.
    pr.surfaces[0].mask.as_mut().unwrap().invert = true;
    c.render(&pr).unwrap();
    let px = c.read_rgba8().unwrap();
    assert_eq!(
        &px[(16 * 32 + 16) * 4..(16 * 32 + 16) * 4 + 3],
        &[0, 0, 0],
        "centre hidden when inverted"
    );
    pr.surfaces[0].mask = None;
    c.render(&pr).unwrap();
    assert_eq!(c.resource_counts().mask_textures, 0);
}

fn fx(kind: om_project::EffectKind) -> om_project::Effect {
    om_project::Effect {
        enabled: true,
        kind,
    }
}

fn finite(v: f64) -> om_types::Finite {
    om_types::Finite::new(v).unwrap()
}

#[test]
fn effects_match_reference() {
    use om_project::EffectKind;
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx_ = Fixture::new((64, 48));
    let cases: Vec<(&str, Vec<om_project::Effect>)> = vec![
        (
            "color",
            vec![fx(EffectKind::Color {
                brightness: finite(0.1),
                contrast: finite(1.3),
                saturation: finite(0.6),
                hue: finite(40.0),
                gamma: finite(1.4),
            })],
        ),
        ("invert", vec![fx(EffectKind::Invert {})]),
        (
            "blur",
            vec![fx(EffectKind::Blur {
                radius: finite(5.0),
            })],
        ),
        ("pixelate", vec![fx(EffectKind::Pixelate { size: 6 })]),
        (
            "chain",
            vec![
                fx(EffectKind::Pixelate { size: 4 }),
                fx(EffectKind::Blur {
                    radius: finite(2.5),
                }),
                fx(EffectKind::Invert {}),
            ],
        ),
    ];
    for (label, effects) in cases {
        let mut s = surface(1, Shape::full_quad(), GRID);
        s.effects = effects;
        let pr = project(64, 48, vec![s]);
        let out = fx_.gpu_render(&mut c, &pr);
        assert_matches_reference(&out, &fx_.cpu_render(&pr), label, SHARP);
    }
}

#[test]
fn disabled_effects_are_skipped_and_targets_released() {
    use om_project::EffectKind;
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx_ = Fixture::new((32, 32));
    let mut s = surface(1, Shape::full_quad(), GRID);
    s.effects = vec![om_project::Effect {
        enabled: false,
        kind: EffectKind::Invert {},
    }];
    let pr_disabled = project(32, 32, vec![s.clone()]);
    let plain = fx_.gpu_render(
        &mut c,
        &project(32, 32, vec![surface(1, Shape::full_quad(), GRID)]),
    );
    assert_eq!(
        fx_.gpu_render(&mut c, &pr_disabled),
        plain,
        "disabled effect is a no-op"
    );
    s.effects[0].enabled = true;
    fx_.gpu_render(&mut c, &project(32, 32, vec![s]));
    assert_eq!(c.resource_counts().effect_targets, 1);
    fx_.gpu_render(&mut c, &pr_disabled);
    assert_eq!(c.resource_counts().effect_targets, 0);
}

fn corpus_shader(name: &str) -> (String, om_isf::Compiled) {
    let path = format!(
        "{}/../om-isf/tests/corpus/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let src = std::fs::read_to_string(&path).unwrap();
    (
        path,
        om_isf::compile(&om_isf::parse(&src).unwrap()).unwrap(),
    )
}

#[test]
fn shader_generator_media_renders() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let (path, compiled) = corpus_shader("solid.fs");
    let shader_id = MediaId::from_u128(200);
    let mut pr = project(16, 16, vec![]);
    let mut inputs = std::collections::BTreeMap::new();
    let f = |v: f64| om_types::Finite::new(v).unwrap();
    inputs.insert(
        "tint".to_owned(),
        om_project::ShaderValue::Vector(vec![f(1.0), f(0.5), f(0.0), f(1.0)]),
    );
    pr.media.push(Media {
        id: shader_id,
        name: "solid".into(),
        source: MediaSource::Shader {
            path: path.clone(),
            inputs,
        },
        playback: Default::default(),
        extensions: Default::default(),
    });
    pr.surfaces.push(surface(1, Shape::full_quad(), shader_id));
    // Not loaded yet: surface reports it, nothing crashes.
    let report = c.render(&pr).unwrap();
    assert!(matches!(
        report.plan.skipped[..],
        [(_, SkipReason::MediaNotLoaded(_))]
    ));
    c.set_shader(&path, &compiled);
    c.render(&pr).unwrap();
    let px = c.read_rgba8().unwrap();
    // ISF output is sRGB-encoded: 0.5 is code 127.5, so 127 or 128 after the
    // half-float round trip.
    assert_eq!((px[0], px[2]), (255, 0), "{:?}", &px[..4]);
    assert!(px[1].abs_diff(128) <= 1, "{:?}", &px[..4]);
}

#[test]
fn shader_effect_filters_the_chain() {
    let Some(g) = gpu() else { return };
    let mut c = Compositor::new(g);
    let fx_ = Fixture::new((32, 24));
    let (path, compiled) = corpus_shader("passthrough.fs");
    let plain = fx_.gpu_render(
        &mut c,
        &project(32, 24, vec![surface(1, Shape::full_quad(), GRID)]),
    );
    let shader_effect = |swap: bool| om_project::Effect {
        enabled: true,
        kind: om_project::EffectKind::Shader {
            path: path.clone(),
            inputs: std::collections::BTreeMap::from([(
                "swap".to_owned(),
                om_project::ShaderValue::Bool(swap),
            )]),
        },
    };
    let mut s = surface(1, Shape::full_quad(), GRID);
    s.effects = vec![shader_effect(false)];
    let pr = project(32, 24, vec![s.clone()]);
    // Shader not loaded: the effect is skipped (identity).
    assert_eq!(fx_.gpu_render(&mut c, &pr), plain);
    c.set_shader(&path, &compiled);
    let out = fx_.gpu_render(&mut c, &pr);
    let d = diff(&out, &plain, |_| true);
    assert!(d.max <= 1, "passthrough ISF ~ identity: {d:?}");
    s.effects = vec![shader_effect(true)];
    let out = fx_.gpu_render(&mut c, &project(32, 24, vec![s]));
    for (a, b) in out.as_chunks::<4>().0.iter().zip(plain.as_chunks::<4>().0) {
        assert!(
            a[0].abs_diff(b[2]) <= 1 && a[2].abs_diff(b[0]) <= 1,
            "swap: {a:?} vs {b:?}"
        );
    }
}
