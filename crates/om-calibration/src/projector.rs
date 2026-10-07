// SPDX-License-Identifier: Apache-2.0
//! Projector (pinhole) calibration from 3-D ↔ 2-D correspondences.
//!
//! A projector is modelled as an inverse camera: world point `X` lands on
//! projector pixel `K (R X + t)`. Calibration fits `K` (focal lengths and
//! principal point; no skew or lens distortion) and the pose `R, t` with a
//! normalised DLT, then refines all ten parameters by Levenberg–Marquardt
//! on the reprojection error.

use serde::{Deserialize, Serialize};

use crate::CalibrationError;
use crate::linalg::{M3, cholesky_solve, det3, inv3, mul3v, null_vector, rodrigues, rodrigues_of};

/// Pinhole intrinsics in projector pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intrinsics {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
}

/// World → projector transform: `x_p = R x_w + t` with `R` from the
/// Rodrigues vector `rotation`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pose {
    pub rotation: [f64; 3],
    pub translation: [f64; 3],
}

/// A calibrated projector.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projector {
    /// Resolution the pixel coordinates refer to.
    pub width: u32,
    pub height: u32,
    pub intrinsics: Intrinsics,
    pub pose: Pose,
}

impl Projector {
    /// Projector pixel of world point `x`, or `None` if it is behind the
    /// projector.
    #[must_use]
    pub fn project(&self, x: [f64; 3]) -> Option<(f64, f64)> {
        let r = rodrigues(self.pose.rotation);
        let c = mul3v(&r, x);
        let c = [
            c[0] + self.pose.translation[0],
            c[1] + self.pose.translation[1],
            c[2] + self.pose.translation[2],
        ];
        if c[2] <= 1e-9 {
            return None;
        }
        let k = &self.intrinsics;
        Some((k.fx * c[0] / c[2] + k.cx, k.fy * c[1] / c[2] + k.cy))
    }

    /// RMS reprojection error over correspondences (pixels).
    #[must_use]
    pub fn rms_error(&self, world: &[[f64; 3]], image: &[(f64, f64)]) -> f64 {
        let mut sum = 0.0;
        for (x, p) in world.iter().zip(image) {
            match self.project(*x) {
                Some(q) => sum += (q.0 - p.0).powi(2) + (q.1 - p.1).powi(2),
                None => return f64::INFINITY,
            }
        }
        #[allow(clippy::cast_precision_loss)]
        (sum / world.len().max(1) as f64).sqrt()
    }

    /// Projector position in world space.
    #[must_use]
    pub fn position(&self) -> [f64; 3] {
        let r = rodrigues(self.pose.rotation);
        let t = self.pose.translation;
        // c = -Rᵀ t
        [
            -(r[0][0] * t[0] + r[1][0] * t[1] + r[2][0] * t[2]),
            -(r[0][1] * t[0] + r[1][1] * t[1] + r[2][1] * t[2]),
            -(r[0][2] * t[0] + r[1][2] * t[1] + r[2][2] * t[2]),
        ]
    }

    fn params(&self) -> [f64; 10] {
        let k = &self.intrinsics;
        let (r, t) = (self.pose.rotation, self.pose.translation);
        [k.fx, k.fy, k.cx, k.cy, r[0], r[1], r[2], t[0], t[1], t[2]]
    }

    fn with_params(&self, p: &[f64; 10]) -> Self {
        Self {
            intrinsics: Intrinsics {
                fx: p[0],
                fy: p[1],
                cx: p[2],
                cy: p[3],
            },
            pose: Pose {
                rotation: [p[4], p[5], p[6]],
                translation: [p[7], p[8], p[9]],
            },
            ..*self
        }
    }
}

/// Result of [`calibrate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub projector: Projector,
    /// RMS reprojection error of the fit (pixels).
    pub rms_error: f64,
}

/// Fits a projector to at least 6 correspondences between world points
/// and projector pixels. The world points must not all lie on one plane
/// (a single planar view cannot separate focal length from distance).
pub fn calibrate(
    world: &[[f64; 3]],
    image: &[(f64, f64)],
    width: u32,
    height: u32,
) -> Result<Calibration, CalibrationError> {
    if world.len() != image.len() {
        return Err(CalibrationError::Mismatched);
    }
    if world.len() < 6 {
        return Err(CalibrationError::TooFewPoints {
            need: 6,
            got: world.len(),
        });
    }
    if world
        .iter()
        .flatten()
        .chain(image.iter().flat_map(|p| [&p.0, &p.1]))
        .any(|v| !v.is_finite())
    {
        return Err(CalibrationError::NonFinite);
    }
    if coplanar(world) {
        return Err(CalibrationError::Coplanar);
    }
    let initial = dlt(world, image, width, height)?;
    let refined = refine(&initial, world, image);
    let rms_error = refined.rms_error(world, image);
    if !rms_error.is_finite() {
        return Err(CalibrationError::Degenerate);
    }
    Ok(Calibration {
        projector: refined,
        rms_error,
    })
}

/// True if all points lie (nearly) on one plane, relative to their spread.
fn coplanar(world: &[[f64; 3]]) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let n = world.len() as f64;
    let c: [f64; 3] = std::array::from_fn(|k| world.iter().map(|p| p[k]).sum::<f64>() / n);
    let rows: Vec<Vec<f64>> = world
        .iter()
        .map(|p| vec![p[0] - c[0], p[1] - c[1], p[2] - c[2]])
        .collect();
    let (vals, _) = crate::linalg::symmetric_eigen(&crate::linalg::gram(&rows, 3));
    let max = vals.iter().copied().fold(0.0, f64::max);
    let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
    max <= 0.0 || min / max < 1e-8
}

fn normalise3(world: &[[f64; 3]]) -> ([[f64; 4]; 4], Vec<[f64; 3]>) {
    #[allow(clippy::cast_precision_loss)]
    let n = world.len() as f64;
    let c: [f64; 3] = std::array::from_fn(|k| world.iter().map(|p| p[k]).sum::<f64>() / n);
    let mean = world
        .iter()
        .map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    let s = 3f64.sqrt() / mean.max(1e-300);
    let t = [
        [s, 0.0, 0.0, -s * c[0]],
        [0.0, s, 0.0, -s * c[1]],
        [0.0, 0.0, s, -s * c[2]],
        [0.0, 0.0, 0.0, 1.0],
    ];
    (
        t,
        world
            .iter()
            .map(|p| std::array::from_fn(|k| s * (p[k] - c[k])))
            .collect(),
    )
}

fn normalise2(image: &[(f64, f64)]) -> (M3, Vec<(f64, f64)>) {
    #[allow(clippy::cast_precision_loss)]
    let n = image.len() as f64;
    let cx = image.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = image.iter().map(|p| p.1).sum::<f64>() / n;
    let mean = image
        .iter()
        .map(|p| (p.0 - cx).hypot(p.1 - cy))
        .sum::<f64>()
        / n;
    let s = std::f64::consts::SQRT_2 / mean.max(1e-300);
    (
        [[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]],
        image
            .iter()
            .map(|p| (s * (p.0 - cx), s * (p.1 - cy)))
            .collect(),
    )
}

/// Linear estimate: normalised DLT for the 3×4 projection matrix, then
/// decomposition into `K [R | t]`.
fn dlt(
    world: &[[f64; 3]],
    image: &[(f64, f64)],
    width: u32,
    height: u32,
) -> Result<Projector, CalibrationError> {
    let (t3, w) = normalise3(world);
    let (t2, im) = normalise2(image);
    let mut rows = Vec::with_capacity(2 * w.len());
    for (x, &(u, v)) in w.iter().zip(&im) {
        let xh = [x[0], x[1], x[2], 1.0];
        let mut r1 = vec![0.0; 12];
        let mut r2 = vec![0.0; 12];
        for k in 0..4 {
            r1[k] = xh[k];
            r1[8 + k] = -u * xh[k];
            r2[4 + k] = xh[k];
            r2[8 + k] = -v * xh[k];
        }
        rows.push(r1);
        rows.push(r2);
    }
    let p = null_vector(&rows, 12);
    let pn = [
        [p[0], p[1], p[2], p[3]],
        [p[4], p[5], p[6], p[7]],
        [p[8], p[9], p[10], p[11]],
    ];
    // Denormalise: P = T2⁻¹ P̂ T3.
    let t2i = inv3(&t2).ok_or(CalibrationError::Degenerate)?;
    let mut tmp = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            tmp[i][j] = (0..4).map(|k| pn[i][k] * t3[k][j]).sum();
        }
    }
    let mut pm = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            pm[i][j] = (0..3).map(|k| t2i[i][k] * tmp[k][j]).sum();
        }
    }
    let mut m: M3 = [
        [pm[0][0], pm[0][1], pm[0][2]],
        [pm[1][0], pm[1][1], pm[1][2]],
        [pm[2][0], pm[2][1], pm[2][2]],
    ];
    let mut p4 = [pm[0][3], pm[1][3], pm[2][3]];
    if det3(&m) < 0.0 {
        for row in &mut m {
            for v in row {
                *v = -*v;
            }
        }
        for v in &mut p4 {
            *v = -*v;
        }
    }
    let (k, r) = rq(&m).ok_or(CalibrationError::Degenerate)?;
    let lambda = k[2][2];
    if lambda.abs() < 1e-300 {
        return Err(CalibrationError::Degenerate);
    }
    let kn: M3 = std::array::from_fn(|i| std::array::from_fn(|j| k[i][j] / lambda));
    let kinv = inv3(&kn).ok_or(CalibrationError::Degenerate)?;
    let t = mul3v(&kinv, [p4[0] / lambda, p4[1] / lambda, p4[2] / lambda]);
    Ok(Projector {
        width,
        height,
        intrinsics: Intrinsics {
            fx: kn[0][0],
            fy: kn[1][1],
            cx: kn[0][2],
            cy: kn[1][2],
        },
        pose: Pose {
            rotation: rodrigues_of(&r),
            translation: t,
        },
    })
}

/// `M = K R` with `K` upper triangular (positive diagonal) and `R`
/// orthonormal, by Gram–Schmidt on the rows from the bottom.
fn rq(m: &M3) -> Option<(M3, M3)> {
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |a: [f64; 3]| dot(a, a).sqrt();
    let scale = |a: [f64; 3], s: f64| [a[0] * s, a[1] * s, a[2] * s];
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let (m1, m2, m3) = (m[0], m[1], m[2]);
    let k22 = norm(m3);
    if k22 < 1e-300 {
        return None;
    }
    let r3 = scale(m3, 1.0 / k22);
    let k12 = dot(m2, r3);
    let m2p = sub(m2, scale(r3, k12));
    let k11 = norm(m2p);
    if k11 < 1e-300 {
        return None;
    }
    let r2 = scale(m2p, 1.0 / k11);
    let k02 = dot(m1, r3);
    let k01 = dot(m1, r2);
    let m1p = sub(sub(m1, scale(r3, k02)), scale(r2, k01));
    let k00 = norm(m1p);
    if k00 < 1e-300 {
        return None;
    }
    let r1 = scale(m1p, 1.0 / k00);
    Some((
        [[k00, k01, k02], [0.0, k11, k12], [0.0, 0.0, k22]],
        [r1, r2, r3],
    ))
}

/// Reprojection residuals (x and y per point).
fn residuals(p: &Projector, world: &[[f64; 3]], image: &[(f64, f64)]) -> Option<Vec<f64>> {
    let mut out = Vec::with_capacity(2 * world.len());
    for (x, q) in world.iter().zip(image) {
        let r = p.project(*x)?;
        out.push(r.0 - q.0);
        out.push(r.1 - q.1);
    }
    Some(out)
}

/// Levenberg–Marquardt on all ten parameters (numeric Jacobian).
fn refine(initial: &Projector, world: &[[f64; 3]], image: &[(f64, f64)]) -> Projector {
    let mut best = *initial;
    let Some(mut r) = residuals(&best, world, image) else {
        return best;
    };
    let mut cost: f64 = r.iter().map(|v| v * v).sum();
    let mut lambda = 1e-3;
    for _ in 0..200 {
        let p0 = best.params();
        // Jacobian by central differences, step relative to each parameter.
        let mut jac = vec![vec![0.0; 10]; r.len()];
        let mut ok = true;
        for k in 0..10 {
            let h = 1e-6 * p0[k].abs().max(1e-3);
            let (mut a, mut b) = (p0, p0);
            a[k] += h;
            b[k] -= h;
            let (Some(ra), Some(rb)) = (
                residuals(&best.with_params(&a), world, image),
                residuals(&best.with_params(&b), world, image),
            ) else {
                ok = false;
                break;
            };
            for i in 0..r.len() {
                jac[i][k] = (ra[i] - rb[i]) / (2.0 * h);
            }
        }
        if !ok {
            break;
        }
        let jtj = crate::linalg::gram(&jac, 10);
        let jtr: Vec<f64> = (0..10)
            .map(|k| (0..r.len()).map(|i| jac[i][k] * r[i]).sum())
            .collect();
        let mut improved = false;
        for _ in 0..10 {
            let mut a = jtj.clone();
            for k in 0..10 {
                a[k][k] += lambda * jtj[k][k].max(1e-12);
            }
            let neg: Vec<f64> = jtr.iter().map(|v| -v).collect();
            let Some(step) = cholesky_solve(&a, &neg) else {
                lambda *= 10.0;
                continue;
            };
            let p1: [f64; 10] = std::array::from_fn(|k| p0[k] + step[k]);
            let cand = best.with_params(&p1);
            if let Some(rc) = residuals(&cand, world, image) {
                let c: f64 = rc.iter().map(|v| v * v).sum();
                if c < cost {
                    let gain = cost - c;
                    best = cand;
                    r = rc;
                    cost = c;
                    lambda = (lambda / 3.0).max(1e-12);
                    improved = gain > 1e-14 * cost.max(1e-300);
                    break;
                }
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    best
}

impl Projector {
    /// From the parameters stored in a project.
    #[must_use]
    pub fn from_params(p: &om_project::ProjectorParams) -> Self {
        Self {
            width: p.width,
            height: p.height,
            intrinsics: Intrinsics {
                fx: p.fx.get(),
                fy: p.fy.get(),
                cx: p.cx.get(),
                cy: p.cy.get(),
            },
            pose: Pose {
                rotation: p.rotation.map(om_types::Finite::get),
                translation: p.translation.map(om_types::Finite::get),
            },
        }
    }

    /// The parameters to store in a project (`None` if any is not finite).
    #[must_use]
    pub fn to_params(&self) -> Option<om_project::ProjectorParams> {
        let f = |v: f64| om_types::Finite::new(v).ok();
        let k = &self.intrinsics;
        let (r, t) = (self.pose.rotation, self.pose.translation);
        Some(om_project::ProjectorParams {
            width: self.width,
            height: self.height,
            fx: f(k.fx)?,
            fy: f(k.fy)?,
            cx: f(k.cx)?,
            cy: f(k.cy)?,
            rotation: [f(r[0])?, f(r[1])?, f(r[2])?],
            translation: [f(t[0])?, f(t[1])?, f(t[2])?],
        })
    }
}

/// Calibrates from the point pairs stored in a projection, at the given
/// projector resolution.
pub fn calibrate_projection(
    projection: &om_project::Projection,
    width: u32,
    height: u32,
) -> Result<Calibration, CalibrationError> {
    let world: Vec<[f64; 3]> = projection
        .points
        .iter()
        .map(|p| p.world.map(om_types::Finite::get))
        .collect();
    let image: Vec<(f64, f64)> = projection
        .points
        .iter()
        .map(|p| (p.pixel[0].get(), p.pixel[1].get()))
        .collect();
    calibrate(&world, &image, width, height)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rq_reconstructs_the_matrix() {
        let k: M3 = [[1500.0, 2.0, 640.0], [0.0, 1400.0, 360.0], [0.0, 0.0, 1.0]];
        let r = rodrigues([0.2, -0.1, 0.4]);
        let m = crate::linalg::mul3(&k, &r);
        let (k2, r2) = rq(&m).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!((k2[i][j] - k[i][j]).abs() < 1e-9);
                assert!((r2[i][j] - r[i][j]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn invalid_inputs_are_errors() {
        let w = [[0.0, 0.0, 1.0]; 5];
        let i = [(0.0, 0.0); 5];
        assert!(matches!(
            calibrate(&w, &i, 10, 10),
            Err(CalibrationError::TooFewPoints { .. })
        ));
        let plane: Vec<[f64; 3]> = (0..10)
            .map(|k| [f64::from(k % 3), f64::from(k / 3), 2.0])
            .collect();
        let img: Vec<(f64, f64)> = (0..10).map(|k| (f64::from(k), 0.0)).collect();
        assert_eq!(
            calibrate(&plane, &img, 10, 10),
            Err(CalibrationError::Coplanar)
        );
        let mut bad = plane.clone();
        bad[0][0] = f64::NAN;
        assert_eq!(
            calibrate(&bad, &img, 10, 10),
            Err(CalibrationError::NonFinite)
        );
    }
}
