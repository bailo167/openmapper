// SPDX-License-Identifier: Apache-2.0
//! 3-D mapping on the GPU: models seen through calibrated projectors,
//! checked against a CPU ray-casting reference, depth ordering, and an end-
//! to-end synthetic calibration (calibrate from point pairs, render, compare
//! with the true projector).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::needless_range_loop
)]

use om_calibration::linalg::{mul3v, rodrigues, transpose3};
use om_calibration::{Intrinsics, Pose, Projector, calibrate};
use om_geom::Point2;
use om_geom::obj::{self, Mesh};
use om_gpu::GpuContext;
use om_media_core::StillImage;
use om_project::{
    Canvas, Media, MediaSource, Output, PatternKind, Project, Projection, Shape, Surface,
};
use om_render::Compositor;
use om_types::{MediaId, OutputId, SurfaceId};

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

const MEDIA: MediaId = MediaId::from_u128(7);

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y).unwrap()
}

fn gradient(w: u32, h: u32) -> StillImage {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[
                (x * 255 / (w - 1)) as u8,
                (y * 255 / (h - 1)) as u8,
                128,
                255,
            ]);
        }
    }
    StillImage::from_rgba8(w, h, px).unwrap()
}

/// A compositor that has rendered a full-canvas `image`.
fn canvas(image: &StillImage) -> Option<Compositor> {
    let mut pr = Project::new("projection");
    pr.canvas = Canvas {
        width: image.width(),
        height: image.height(),
    };
    pr.media.push(Media {
        id: MEDIA,
        name: "m".into(),
        source: MediaSource::Pattern {
            pattern: PatternKind::White,
        },
        playback: Default::default(),
        plugins: Vec::new(),
        extensions: Default::default(),
    });
    let mut s = Surface::new(SurfaceId::from_u128(1), "full");
    let q = [p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
    s.shape = Shape::Quad { corners: q, uv: q };
    s.media = Some(MEDIA);
    pr.surfaces.push(s);
    let mut c = Compositor::new(gpu()?);
    c.set_image(MEDIA, image).unwrap();
    c.render(&pr).unwrap();
    Some(c)
}

fn output(model: &str, projector: Option<&Projector>) -> Output {
    Output {
        id: OutputId::from_u128(1),
        name: "proj".into(),
        enabled: false,
        display: None,
        publish: Vec::new(),
        mapping: Default::default(),
        projection: Some(Projection {
            model: model.into(),
            projector: projector.map(|p| p.to_params().unwrap()),
            points: Vec::new(),
        }),
        extensions: Default::default(),
    }
}

/// A 2×1 plane at z = 0 whose UVs cover the whole canvas (canvas x from
/// left to right, canvas y from top (y = −0.5) to bottom (y = +0.5)).
fn plane() -> Mesh {
    obj::parse(
        "v -1 -0.5 0\nv 1 -0.5 0\nv 1 0.5 0\nv -1 0.5 0\n\
         vt 0 1\nvt 1 1\nvt 1 0\nvt 0 0\n\
         f 1/1 2/2 3/3 4/4\n",
    )
    .unwrap()
}

fn projector(rotation: [f64; 3], translation: [f64; 3]) -> Projector {
    Projector {
        width: 160,
        height: 120,
        intrinsics: Intrinsics {
            fx: 180.0,
            fy: 180.0,
            cx: 82.0,
            cy: 64.0,
        },
        pose: Pose {
            rotation,
            translation,
        },
    }
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

/// Where the ray through projector pixel `(u, v)` meets the plane z = 0.
fn hit_plane(pr: &Projector, u: f64, v: f64) -> Option<(f64, f64)> {
    let rt = transpose3(&rodrigues(pr.pose.rotation));
    let t = pr.pose.translation;
    let c = mul3v(&rt, [-t[0], -t[1], -t[2]]);
    let k = &pr.intrinsics;
    let d = mul3v(&rt, [(u - k.cx) / k.fx, (v - k.cy) / k.fy, 1.0]);
    if d[2].abs() < 1e-12 {
        return None;
    }
    let s = -c[2] / d[2];
    (s > 0.0).then(|| (c[0] + s * d[0], c[1] + s * d[1]))
}

#[test]
fn plane_matches_the_ray_cast_reference() {
    let img = gradient(64, 32);
    let Some(mut c) = canvas(&img) else { return };
    let canvas_px = c.read_rgba8().unwrap();
    c.set_model("plane.obj", &plane());
    let pr = projector([0.15, -0.25, 0.05], [0.1, -0.05, 3.0]);
    let out = c
        .read_output_rgba8(&output("plane.obj", Some(&pr)), 160, 120)
        .unwrap();
    let (mut sum, mut n, mut worst) = (0.0, 0u32, 0.0f64);
    let mut lit = 0;
    for y in 0..120u32 {
        for x in 0..160u32 {
            let px = &out[((y * 160 + x) * 4) as usize..][..3];
            let (u, v) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            // Classify with a one-pixel margin around the plane's outline.
            let inside = |m: f64| {
                [(-m, -m), (m, -m), (-m, m), (m, m)].iter().all(|(dx, dy)| {
                    hit_plane(&pr, u + dx, v + dy)
                        .is_some_and(|(px, py)| px.abs() <= 1.0 && py.abs() <= 0.5)
                })
            };
            let outside = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                .iter()
                .all(|(dx, dy)| {
                    hit_plane(&pr, u + dx, v + dy)
                        .is_none_or(|(px, py)| px.abs() > 1.0 || py.abs() > 0.5)
                });
            if outside {
                assert_eq!(px, [0, 0, 0], "background at ({x}, {y})");
                continue;
            }
            if !inside(1.0) {
                continue;
            }
            lit += 1;
            let (wx, wy) = hit_plane(&pr, u, v).unwrap();
            let cpos = ((wx + 1.0) / 2.0, wy + 0.5);
            for ch in 0..3 {
                let d = (f64::from(px[ch]) - sample(&canvas_px, 64, 32, cpos, ch)).abs();
                worst = worst.max(d);
                sum += d;
                n += 1;
            }
        }
    }
    assert!(lit > 3000, "the plane covers {lit} pixels");
    let mean = sum / f64::from(n);
    assert!(mean <= 0.5 && worst <= 3.0, "mean {mean}, max {worst}");
}

#[test]
fn nearer_surfaces_hide_farther_ones() {
    // Left half of the canvas red, right half blue.
    let mut px = Vec::new();
    for _ in 0..8 {
        for x in 0..16 {
            px.extend_from_slice(if x < 8 {
                &[255, 0, 0, 255]
            } else {
                &[0, 0, 255, 255]
            });
        }
    }
    let Some(mut c) = canvas(&StillImage::from_rgba8(16, 8, px).unwrap()) else {
        return;
    };
    // Far plane (z = 1) samples blue, near plane (z = 0) red; the far one
    // is listed first and last so draw order cannot decide.
    let model = "v -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n\
                 v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\n\
                 vt 0.9 0.5\nvt 0.1 0.5\n\
                 f 1/1 2/1 3/1 4/1\nf 5/2 6/2 7/2 8/2\nf 1/1 2/1 3/1 4/1\n";
    c.set_model("planes.obj", &obj::parse(model).unwrap());
    let pr = projector([0.0, 0.0, 0.0], [0.0, 0.0, 4.0]);
    let out = c
        .read_output_rgba8(&output("planes.obj", Some(&pr)), 160, 120)
        .unwrap();
    let centre = &out[((60 * 160 + 80) * 4) as usize..][..3];
    assert_eq!(centre, [255, 0, 0], "the near (red) plane wins");
}

#[test]
fn uncalibrated_or_missing_models_show_black() {
    let Some(mut c) = canvas(&gradient(16, 8)) else {
        return;
    };
    let pr = projector([0.0; 3], [0.0, 0.0, 3.0]);
    for o in [output("missing.obj", Some(&pr)), output("plane.obj", None)] {
        c.set_model("plane.obj", &plane());
        let out = c.read_output_rgba8(&o, 32, 24).unwrap();
        assert!(out.chunks(4).all(|p| p[..3] == [0, 0, 0]));
    }
    assert_eq!(c.resource_counts().models, 1);
    c.remove_model("plane.obj");
    assert_eq!(c.resource_counts().models, 0);
}

/// A unit cube's corners and edge midpoints (non-coplanar calibration
/// points) and a cube model textured with the canvas on every face.
fn cube() -> (Mesh, Vec<[f64; 3]>) {
    let text = "v -0.5 -0.5 -0.5\nv 0.5 -0.5 -0.5\nv 0.5 0.5 -0.5\nv -0.5 0.5 -0.5\n\
                v -0.5 -0.5 0.5\nv 0.5 -0.5 0.5\nv 0.5 0.5 0.5\nv -0.5 0.5 0.5\n\
                vt 0 1\nvt 1 1\nvt 1 0\nvt 0 0\n\
                f 1/1 2/2 3/3 4/4\nf 5/1 6/2 7/3 8/4\nf 1/1 2/2 6/3 5/4\n\
                f 4/1 3/2 7/3 8/4\nf 1/1 4/2 8/3 5/4\nf 2/1 3/2 7/3 6/4\n";
    let mut points = Vec::new();
    for x in [-0.5, 0.0, 0.5] {
        for y in [-0.5, 0.0, 0.5] {
            for z in [-0.5, 0.5] {
                points.push([x, y, z]);
            }
        }
    }
    (obj::parse(text).unwrap(), points)
}

#[test]
fn calibrated_render_matches_the_true_projector() {
    let Some(mut c) = canvas(&gradient(64, 64)) else {
        return;
    };
    let (mesh, points) = cube();
    c.set_model("cube.obj", &mesh);
    let truth = projector([0.35, -0.6, 0.1], [0.05, 0.1, 3.2]);
    // Measured pixels: the true projection, rounded to 1/8 pixel as a
    // careful manual click would be.
    let pixels: Vec<(f64, f64)> = points
        .iter()
        .map(|x| {
            let (u, v) = truth.project(*x).unwrap();
            ((u * 8.0).round() / 8.0, (v * 8.0).round() / 8.0)
        })
        .collect();
    let fitted = calibrate(&points, &pixels, 160, 120).unwrap();
    assert!(fitted.rms_error < 0.1, "rms {}", fitted.rms_error);
    let a = c
        .read_output_rgba8(&output("cube.obj", Some(&truth)), 160, 120)
        .unwrap();
    let b = c
        .read_output_rgba8(&output("cube.obj", Some(&fitted.projector)), 160, 120)
        .unwrap();
    let diffs: Vec<u8> = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).collect();
    let mean = diffs.iter().map(|&d| f64::from(d)).sum::<f64>() / diffs.len() as f64;
    let big = diffs.iter().filter(|&&d| d > 8).count();
    // Silhouette pixels may flip; everything else agrees closely.
    assert!(mean < 1.0, "mean difference {mean}");
    assert!(big < 160 * 2, "{big} channels differ by more than 8 codes");
}
