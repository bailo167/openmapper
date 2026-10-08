// SPDX-License-Identifier: Apache-2.0
//! Plane-to-plane homographies fitted to point correspondences
//! (normalised direct linear transform).

use crate::CalibrationError;
use crate::linalg::{M3, inv3, mul3, null_vector};

/// A 3×3 projective map `p ↦ H p` (homogeneous, row-major).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography(pub M3);

impl Homography {
    pub const IDENTITY: Self = Self([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// Maps a point; `None` at the line at infinity.
    #[must_use]
    pub fn apply(&self, (x, y): (f64, f64)) -> Option<(f64, f64)> {
        let h = &self.0;
        let w = h[2][0] * x + h[2][1] * y + h[2][2];
        if w.abs() < 1e-12 || !w.is_finite() {
            return None;
        }
        Some((
            (h[0][0] * x + h[0][1] * y + h[0][2]) / w,
            (h[1][0] * x + h[1][1] * y + h[1][2]) / w,
        ))
    }

    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        inv3(&self.0).map(Self)
    }

    /// `self` after `first`.
    #[must_use]
    pub fn then(&self, first: &Self) -> Self {
        Self(mul3(&self.0, &first.0))
    }

    /// Fits the homography taking each `src` point to its `dst` point (at
    /// least 4 pairs, no three of the first four collinear). Least squares
    /// for more than 4.
    pub fn fit(src: &[(f64, f64)], dst: &[(f64, f64)]) -> Result<Self, CalibrationError> {
        if src.len() != dst.len() {
            return Err(CalibrationError::Mismatched);
        }
        if src.len() < 4 {
            return Err(CalibrationError::TooFewPoints {
                need: 4,
                got: src.len(),
            });
        }
        if src
            .iter()
            .chain(dst)
            .any(|p| !p.0.is_finite() || !p.1.is_finite())
        {
            return Err(CalibrationError::NonFinite);
        }
        let (ts, s) = normalise(src)?;
        let (td, d) = normalise(dst)?;
        let mut rows = Vec::with_capacity(2 * s.len());
        for (&(x, y), &(u, v)) in s.iter().zip(&d) {
            rows.push(vec![-x, -y, -1.0, 0.0, 0.0, 0.0, u * x, u * y, u]);
            rows.push(vec![0.0, 0.0, 0.0, -x, -y, -1.0, v * x, v * y, v]);
        }
        let h = null_vector(&rows, 9);
        let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]];
        let td_inv = inv3(&td).ok_or(CalibrationError::Degenerate)?;
        let mut m = mul3(&mul3(&td_inv, &hn), &ts);
        let scale = m[2][2];
        if scale.abs() > 1e-12 {
            for row in &mut m {
                for v in row {
                    *v /= scale;
                }
            }
        }
        let hom = Self(m);
        // Reject degenerate fits (collinear points give a rank-deficient map).
        if crate::linalg::det3(&hom.0).abs() < 1e-12 {
            return Err(CalibrationError::Degenerate);
        }
        Ok(hom)
    }

    /// Least-squares fit that repeatedly drops pairs mapped further than
    /// `threshold` from their target (decoding outliers), then refits.
    pub fn fit_robust(
        src: &[(f64, f64)],
        dst: &[(f64, f64)],
        threshold: f64,
    ) -> Result<Self, CalibrationError> {
        let mut h = Self::fit(src, dst)?;
        for _ in 0..4 {
            let (s, d): (Vec<_>, Vec<_>) = src
                .iter()
                .zip(dst)
                .filter(|(a, b)| {
                    h.apply(**a)
                        .is_some_and(|p| (p.0 - b.0).hypot(p.1 - b.1) <= threshold)
                })
                .map(|(a, b)| (*a, *b))
                .unzip();
            if s.len() < 4 {
                break;
            }
            h = Self::fit(&s, &d)?;
        }
        Ok(h)
    }

    /// Root-mean-square distance between mapped `src` and `dst`.
    #[must_use]
    pub fn rms_error(&self, src: &[(f64, f64)], dst: &[(f64, f64)]) -> f64 {
        let mut sum = 0.0;
        for (s, d) in src.iter().zip(dst) {
            let Some(p) = self.apply(*s) else {
                return f64::INFINITY;
            };
            sum += (p.0 - d.0).powi(2) + (p.1 - d.1).powi(2);
        }
        #[allow(clippy::cast_precision_loss)]
        (sum / src.len().max(1) as f64).sqrt()
    }
}

/// Similarity transform moving the centroid to the origin with mean
/// distance √2 (Hartley normalisation), and the transformed points.
fn normalise(pts: &[(f64, f64)]) -> Result<(M3, Vec<(f64, f64)>), CalibrationError> {
    #[allow(clippy::cast_precision_loss)]
    let n = pts.len() as f64;
    let cx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let mean = pts.iter().map(|p| (p.0 - cx).hypot(p.1 - cy)).sum::<f64>() / n;
    if mean < 1e-12 {
        return Err(CalibrationError::Degenerate);
    }
    let s = std::f64::consts::SQRT_2 / mean;
    let t = [[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]];
    Ok((
        t,
        pts.iter()
            .map(|p| (s * (p.0 - cx), s * (p.1 - cy)))
            .collect(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit_from_four_points() {
        let src = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let dst = [(0.1, 0.2), (0.9, 0.1), (0.8, 0.95), (0.05, 0.8)];
        let h = Homography::fit(&src, &dst).unwrap();
        assert!(h.rms_error(&src, &dst) < 1e-10);
        let back = h.inverse().unwrap();
        let p = h.apply((0.3, 0.6)).unwrap();
        let q = back.apply(p).unwrap();
        assert!((q.0 - 0.3).abs() < 1e-10 && (q.1 - 0.6).abs() < 1e-10);
    }

    #[test]
    fn degenerate_inputs_are_errors() {
        let line = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        assert!(Homography::fit(&line, &line).is_err());
        assert!(Homography::fit(&line[..3], &line[..3]).is_err());
        assert!(Homography::fit(&line, &line[..3]).is_err());
        let nan = [(f64::NAN, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert_eq!(
            Homography::fit(&nan, &nan),
            Err(CalibrationError::NonFinite)
        );
    }
}
