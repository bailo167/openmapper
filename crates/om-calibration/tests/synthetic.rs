// SPDX-License-Identifier: Apache-2.0
//! Synthetic calibration error bounds: known projectors observe known 3-D
//! points; calibration from the (optionally noisy) observations must
//! recover the projector within stated bounds.
//!
//! Bounds (documented in docs/calibration.md):
//! - noise-free: reprojection RMS < 1e-6 px; focal length and principal
//!   point within 1e-6 relative / 1e-4 px;
//! - Gaussian noise σ = 0.5 px per axis, 40 points: RMS point distance
//!   within [0.5, 0.8] px (expected σ·√2·√(1 − 10/80) ≈ 0.66: two axes,
//!   ten fitted parameters), focal within 1.5 %, principal point within 15 px, and held-out
//!   points reproject within 1.5 px of their true position.
#![allow(clippy::unwrap_used, clippy::cast_precision_loss)]

use om_calibration::{Intrinsics, Pose, Projector, calibrate};

/// Deterministic xorshift generator (tests must not depend on a seed source).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }

    /// Standard normal (Box–Muller).
    fn gauss(&mut self) -> f64 {
        let u = self.next().max(1e-12);
        let v = self.next();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

fn truth(rng: &mut Rng) -> Projector {
    Projector {
        width: 1920,
        height: 1080,
        intrinsics: Intrinsics {
            fx: rng.range(1400.0, 2600.0),
            fy: 0.0,
            cx: rng.range(860.0, 1060.0),
            // Projectors usually have an offset lens: principal point low.
            cy: rng.range(700.0, 1080.0),
        },
        pose: Pose {
            rotation: [
                rng.range(-0.3, 0.3),
                rng.range(-0.5, 0.5),
                rng.range(-0.1, 0.1),
            ],
            translation: [
                rng.range(-0.5, 0.5),
                rng.range(-0.5, 0.5),
                rng.range(2.5, 4.0),
            ],
        },
    }
}

/// Points spread over a 2 m box around the origin that the projector
/// actually lights (inside its image).
fn scene(p: &Projector, rng: &mut Rng, n: usize) -> (Vec<[f64; 3]>, Vec<(f64, f64)>) {
    let mut world = Vec::new();
    let mut image = Vec::new();
    while world.len() < n {
        let x = [
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
        ];
        if let Some(q) = p.project(x)
            && (0.0..1920.0).contains(&q.0)
            && (0.0..1080.0).contains(&q.1)
        {
            world.push(x);
            image.push(q);
        }
    }
    (world, image)
}

#[test]
fn noise_free_calibration_is_exact() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    for case in 0..20 {
        let mut t = truth(&mut rng);
        t.intrinsics.fy = t.intrinsics.fx * rng.range(0.98, 1.02);
        let (world, image) = scene(&t, &mut rng, 12);
        let c = calibrate(&world, &image, 1920, 1080).unwrap();
        assert!(c.rms_error < 1e-6, "case {case}: rms {}", c.rms_error);
        let (k, kt) = (c.projector.intrinsics, t.intrinsics);
        assert!(
            (k.fx / kt.fx - 1.0).abs() < 1e-6,
            "case {case}: {k:?} vs {kt:?}"
        );
        assert!((k.fy / kt.fy - 1.0).abs() < 1e-6);
        assert!((k.cx - kt.cx).abs() < 1e-4 && (k.cy - kt.cy).abs() < 1e-4);
        let (a, b) = (c.projector.position(), t.position());
        let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        assert!(d < 1e-6, "case {case}: position off by {d}");
    }
}

#[test]
fn noisy_calibration_stays_within_bounds() {
    let mut rng = Rng(0xfeed_beef_cafe_f00d);
    let sigma = 0.5;
    for case in 0..20 {
        let mut t = truth(&mut rng);
        t.intrinsics.fy = t.intrinsics.fx;
        let (world, image) = scene(&t, &mut rng, 40);
        let noisy: Vec<(f64, f64)> = image
            .iter()
            .map(|p| (p.0 + sigma * rng.gauss(), p.1 + sigma * rng.gauss()))
            .collect();
        let c = calibrate(&world, &noisy, 1920, 1080).unwrap();
        assert!(
            (0.5..=0.8).contains(&c.rms_error),
            "case {case}: rms {} for σ {sigma}",
            c.rms_error
        );
        let (k, kt) = (c.projector.intrinsics, t.intrinsics);
        assert!(
            (k.fx / kt.fx - 1.0).abs() < 0.015,
            "case {case}: fx {} vs {}",
            k.fx,
            kt.fx
        );
        assert!(
            (k.cx - kt.cx).abs() < 15.0 && (k.cy - kt.cy).abs() < 15.0,
            "case {case}: {k:?} vs {kt:?}"
        );
        // Held-out points reproject close to the truth.
        let (held, truth_px) = scene(&t, &mut rng, 50);
        let worst = held
            .iter()
            .zip(&truth_px)
            .map(|(x, q)| {
                let p = c.projector.project(*x).unwrap();
                (p.0 - q.0).hypot(p.1 - q.1)
            })
            .fold(0.0, f64::max);
        assert!(worst < 1.5, "case {case}: held-out error {worst} px");
    }
}

#[test]
fn saved_calibration_is_repeatable() {
    let mut rng = Rng(42);
    let mut t = truth(&mut rng);
    t.intrinsics.fy = t.intrinsics.fx;
    let (world, image) = scene(&t, &mut rng, 20);
    let a = calibrate(&world, &image, 1920, 1080).unwrap();
    let b = calibrate(&world, &image, 1920, 1080).unwrap();
    assert_eq!(a, b, "calibration is deterministic");
    // A JSON round trip reproduces the projector exactly.
    let json = serde_json::to_string(&a.projector).unwrap();
    let back: Projector = serde_json::from_str(&json).unwrap();
    assert_eq!(back, a.projector);
    for x in &world {
        assert_eq!(back.project(*x), a.projector.project(*x));
    }
}
