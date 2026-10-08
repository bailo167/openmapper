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
pub mod obj;
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

/// Inverse of the bilinear patch with corners `q` = (P00, P10, P11, P01):
/// returns `(s, t)` with `p = bilinear(q, s, t)`, or `None` if `p` is outside
/// the patch. Bilinear maps are linear along every edge, so neighbouring
/// patches that share an edge agree on it exactly (no seams).
#[must_use]
pub fn inverse_bilinear(q: &[(f64, f64); 4], p: (f64, f64)) -> Option<(f64, f64)> {
    let cross = |a: (f64, f64), b: (f64, f64)| a.0 * b.1 - a.1 * b.0;
    let sub = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0, a.1 - b.1);
    let (a, b, c, d) = (q[0], q[1], q[2], q[3]);
    let e = sub(b, a);
    let f = sub(d, a);
    let g = (a.0 - b.0 + c.0 - d.0, a.1 - b.1 + c.1 - d.1);
    let h = sub(p, a);
    let k2 = cross(g, f);
    let k1 = cross(e, f) + cross(h, g);
    let k0 = cross(h, e);
    // u from v using the better-conditioned axis.
    let u_of = |v: f64| {
        let (dx, dy) = (e.0 + g.0 * v, e.1 + g.1 * v);
        if dx.abs() >= dy.abs() {
            (h.0 - f.0 * v) / dx
        } else {
            (h.1 - f.1 * v) / dy
        }
    };
    let inside = |s: f64, t: f64| {
        const EPS: f64 = 1e-9;
        (-EPS..=1.0 + EPS).contains(&s) && (-EPS..=1.0 + EPS).contains(&t)
    };
    // Numerically stable roots of k2·v² + k1·v + k0 = 0: with
    // q = −(k1 + sign(k1)·√disc)/2 the roots are q/k2 and k0/q, which stays
    // accurate as k2 → 0 (near-parallelogram patches).
    let disc = k1 * k1 - 4.0 * k0 * k2;
    if disc < 0.0 {
        return None;
    }
    let q = -0.5 * (k1 + k1.signum() * disc.sqrt());
    let mut roots = [f64::NAN; 2];
    if q != 0.0 {
        roots[0] = k0 / q;
    }
    if k2 != 0.0 {
        roots[1] = q / k2;
    }
    for v in roots {
        if !v.is_finite() {
            continue;
        }
        let u = u_of(v);
        if inside(u, v) {
            return Some((u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)));
        }
    }
    None
}

/// Forward bilinear interpolation of four corner values (P00, P10, P11, P01).
#[must_use]
pub fn bilinear(q: &[(f64, f64); 4], s: f64, t: f64) -> (f64, f64) {
    let lerp =
        |a: (f64, f64), b: (f64, f64), k: f64| (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k);
    lerp(lerp(q[0], q[1], s), lerp(q[3], q[2], s), t)
}

#[cfg(test)]
mod tests;
