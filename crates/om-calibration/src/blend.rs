// SPDX-License-Identifier: Apache-2.0
//! Soft-edge blending for overlapping projectors.
//!
//! Across an overlap of two projectors, one fades out while the other fades
//! in. The *light* weights must sum to one everywhere in the overlap, so
//! the overlap is as bright as the rest of the image. The weights are
//! symmetric S-curves, `w(t) + w(1 − t) = 1`. Because a projector turns its
//! signal into light through its gamma, the signal multiplier is
//! `w^(1/γ)`.

/// Light weight at position `t` across a ramp (0 = outer edge, fully dark;
/// 1 = inner edge, full brightness). `curve` ≥ 1 sets the S-shape (1 =
/// linear, 2 = smooth, higher = steeper middle).
#[must_use]
pub fn light_weight(t: f64, curve: f64) -> f64 {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    let p = if curve.is_finite() {
        curve.max(1.0)
    } else {
        1.0
    };
    if t < 0.5 {
        0.5 * (2.0 * t).powf(p)
    } else {
        1.0 - 0.5 * (2.0 * (1.0 - t)).powf(p)
    }
}

/// Signal multiplier for a display with `gamma` (≈2.2 for most
/// projectors; 1 for linear devices).
#[must_use]
pub fn signal_weight(t: f64, curve: f64, gamma: f64) -> f64 {
    let g = if gamma.is_finite() && gamma > 0.0 {
        gamma
    } else {
        1.0
    };
    light_weight(t, curve).powf(1.0 / g)
}

/// Soft-edge widths (fractions of the output's width/height, 0 = none)
/// and shape. Ramps run inward from each edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub curve: f64,
    pub gamma: f64,
}

impl Edges {
    /// Signal multiplier at output position `(u, v)` in `[0, 1]²` (top-left
    /// origin): the product of the ramps that cover it.
    #[must_use]
    pub fn signal(&self, u: f64, v: f64) -> f64 {
        let ramp = |pos: f64, width: f64| {
            if width > 0.0 && pos < width {
                signal_weight(pos / width, self.curve, self.gamma)
            } else {
                1.0
            }
        };
        ramp(u, self.left)
            * ramp(1.0 - u, self.right)
            * ramp(v, self.top)
            * ramp(1.0 - v, self.bottom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complementary_ramps_sum_to_one_in_light() {
        for curve in [1.0, 2.0, 3.5] {
            for k in 0..=100 {
                let t = f64::from(k) / 100.0;
                let sum = light_weight(t, curve) + light_weight(1.0 - t, curve);
                assert!((sum - 1.0).abs() < 1e-12, "curve {curve} t {t}");
            }
        }
        assert_eq!(light_weight(0.0, 2.0), 0.0);
        assert_eq!(light_weight(1.0, 2.0), 1.0);
        assert_eq!(light_weight(f64::NAN, 2.0), 0.0);
        assert_eq!(light_weight(-3.0, f64::NAN), 0.0);
    }

    #[test]
    fn gamma_compensated_signals_add_up_on_screen() {
        // Two projectors with gamma 2.2 showing a flat field through
        // complementary edges: the light they add is constant.
        let gamma = 2.2;
        for k in 0..=50 {
            let t = f64::from(k) / 50.0;
            let a = signal_weight(t, 2.0, gamma).powf(gamma);
            let b = signal_weight(1.0 - t, 2.0, gamma).powf(gamma);
            assert!((a + b - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn edges_multiply_and_leave_the_centre_alone() {
        let e = Edges {
            left: 0.2,
            right: 0.1,
            top: 0.0,
            bottom: 0.0,
            curve: 2.0,
            gamma: 1.0,
        };
        assert_eq!(e.signal(0.5, 0.5), 1.0);
        assert_eq!(e.signal(0.0, 0.5), 0.0);
        assert!(
            (e.signal(0.1, 0.5) - 0.5).abs() < 1e-12,
            "middle of the left ramp"
        );
        assert!(
            (e.signal(0.95, 0.5) - 0.5).abs() < 1e-12,
            "middle of the right ramp"
        );
    }
}
