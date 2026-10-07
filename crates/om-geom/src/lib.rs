// SPDX-License-Identifier: Apache-2.0
//! Geometry for mapping.
//!
//! Coordinate conventions used throughout OpenMapper:
//! - **Canvas space**: normalised `[0, 1] × [0, 1]`, origin top-left, +y down.
//!   Points may lie outside the unit square (surfaces can extend off-canvas).
//! - **UV space**: normalised source-image coordinates, origin top-left, +y down.
//! - Quad corners are ordered top-left, top-right, bottom-right, bottom-left
//!   of the *source*; either winding is accepted in canvas space (mirroring).
//!
//! All maths is `f64`. Mappings are 3×3 projective matrices acting on
//! homogeneous column vectors `(x, y, 1)`.

mod homography;
mod point;

pub use homography::{GeomError, Homography, quad_map, triangle_map};
pub use point::Point2;

/// Twice the signed area of polygon `pts` (shoelace). Positive for
/// clockwise order in a y-down space.
#[must_use]
pub fn signed_area2(pts: &[Point2]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x() * b.y() - b.x() * a.y()
        })
        .sum()
}

/// True if `pts` form a strictly convex polygon with non-negligible area
/// (either winding).
#[must_use]
pub fn is_strictly_convex(pts: &[Point2]) -> bool {
    let n = pts.len();
    if n < 3 || signed_area2(pts).abs() < 1e-12 {
        return false;
    }
    let mut sign = 0.0_f64;
    for i in 0..n {
        let (a, b, c) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        let cross = (b.x() - a.x()) * (c.y() - b.y()) - (b.y() - a.y()) * (c.x() - b.x());
        if cross.abs() < 1e-15 {
            return false;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Point-in-convex-polygon test, either winding. Points exactly on an edge
/// count as inside.
#[must_use]
pub fn convex_contains(pts: &[Point2], p: (f64, f64)) -> bool {
    let n = pts.len();
    let mut sign = 0.0_f64;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let cross = (b.x() - a.x()) * (p.1 - a.y()) - (b.y() - a.y()) * (p.0 - a.x());
        if cross != 0.0 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests;
