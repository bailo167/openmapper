// SPDX-License-Identifier: Apache-2.0

use super::*;
use proptest::prelude::*;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y).unwrap()
}

fn close(a: (f64, f64), b: (f64, f64), tol: f64) -> bool {
    (a.0 - b.0).abs() <= tol && (a.1 - b.1).abs() <= tol
}

#[test]
fn identity_quad_is_identity_map() {
    let sq = Point2::unit_square();
    let h = quad_map(&sq, &sq).unwrap();
    for q in [(0.0, 0.0), (0.3, 0.7), (1.0, 1.0), (0.5, 0.25)] {
        assert!(close(h.apply(q).unwrap(), q, 1e-15), "{q:?}");
    }
}

#[test]
fn corners_map_exactly_for_perspective_quad() {
    let quad = [p(0.10, 0.10), p(0.82, 0.18), p(0.91, 0.86), p(0.06, 0.92)];
    let h = Homography::square_to_quad(&quad).unwrap();
    for (s, q) in Point2::unit_square().iter().zip(&quad) {
        assert!(close(h.apply(s.to_tuple()).unwrap(), q.to_tuple(), 1e-14));
    }
    let m = quad_map(&quad, &Point2::unit_square()).unwrap();
    for (q, s) in quad.iter().zip(Point2::unit_square()) {
        assert!(close(m.apply(q.to_tuple()).unwrap(), s.to_tuple(), 1e-13));
    }
}

#[test]
fn perspective_preserves_straight_lines_and_cross_ratio_midpoint() {
    // The image of the square's centre is the intersection of the quad's
    // diagonals (projective invariant).
    let quad = [p(0.0, 0.0), p(1.0, 0.1), p(0.8, 0.9), p(0.1, 1.0)];
    let h = Homography::square_to_quad(&quad).unwrap();
    let c = h.apply((0.5, 0.5)).unwrap();
    // Intersection of diagonals q0-q2 and q1-q3.
    let (a, b, cc, d) = (quad[0], quad[2], quad[1], quad[3]);
    let den = (a.x() - b.x()) * (cc.y() - d.y()) - (a.y() - b.y()) * (cc.x() - d.x());
    let t = ((a.x() - cc.x()) * (cc.y() - d.y()) - (a.y() - cc.y()) * (cc.x() - d.x())) / den;
    let ix = (a.x() + t * (b.x() - a.x()), a.y() + t * (b.y() - a.y()));
    assert!(close(c, ix, 1e-13), "{c:?} vs {ix:?}");
}

#[test]
fn degenerate_and_concave_quads_are_rejected() {
    let collinear = [p(0.0, 0.0), p(0.5, 0.0), p(1.0, 0.0), p(0.2, 0.0)];
    assert_eq!(
        Homography::square_to_quad(&collinear),
        Err(GeomError::Degenerate)
    );
    let concave = [p(0.0, 0.0), p(1.0, 0.0), p(0.3, 0.3), p(0.0, 1.0)];
    assert_eq!(
        Homography::square_to_quad(&concave),
        Err(GeomError::NotConvex)
    );
    let bowtie = [p(0.0, 0.0), p(1.0, 1.0), p(1.0, 0.0), p(0.0, 1.0)];
    assert!(Homography::square_to_quad(&bowtie).is_err());
    let flat_tri = [p(0.0, 0.0), p(1.0, 1.0), p(2.0, 2.0)];
    assert_eq!(
        Homography::unit_triangle_to(&flat_tri),
        Err(GeomError::Degenerate)
    );
}

#[test]
fn mirrored_quad_is_accepted() {
    let mirrored = [p(1.0, 0.0), p(0.0, 0.0), p(0.0, 1.0), p(1.0, 1.0)];
    let h = quad_map(&mirrored, &Point2::unit_square()).unwrap();
    assert!(close(h.apply((0.25, 0.5)).unwrap(), (0.75, 0.5), 1e-15));
}

#[test]
fn triangle_map_hits_corners() {
    let tri = [p(0.2, 0.1), p(0.9, 0.3), p(0.4, 0.8)];
    let uv = [p(0.0, 0.0), p(1.0, 0.0), p(0.5, 1.0)];
    let m = triangle_map(&tri, &uv).unwrap();
    for (t, u) in tri.iter().zip(&uv) {
        assert!(close(m.apply(t.to_tuple()).unwrap(), u.to_tuple(), 1e-14));
    }
}

#[test]
fn point_serialises_as_pair_and_rejects_non_finite() {
    assert_eq!(serde_json::to_string(&p(0.5, 1.0)).unwrap(), "[0.5,1.0]");
    assert!(serde_json::from_str::<Point2>("[1.0]").is_err());
    assert!(Point2::new(f64::NAN, 0.0).is_err());
}

#[test]
fn containment_includes_edges() {
    let sq = Point2::unit_square();
    assert!(convex_contains(&sq, (0.5, 0.5)));
    assert!(convex_contains(&sq, (0.0, 0.5)));
    assert!(!convex_contains(&sq, (1.01, 0.5)));
}

fn arb_convex_quad() -> impl Strategy<Value = [Point2; 4]> {
    // Jitter each corner of a square inside its own quadrant: always convex.
    (
        0.0..0.35f64,
        0.0..0.35f64,
        0.0..0.35f64,
        0.0..0.35f64,
        0.0..0.35f64,
        0.0..0.35f64,
        0.0..0.35f64,
        0.0..0.35f64,
    )
        .prop_map(|(a, b, c, d, e, f, g, h)| {
            [p(a, b), p(1.0 - c, d), p(1.0 - e, 1.0 - f), p(g, 1.0 - h)]
        })
}

proptest! {
    #[test]
    fn map_round_trips(quad in arb_convex_quad(), u in 0.0..1.0f64, v in 0.0..1.0f64) {
        let h = Homography::square_to_quad(&quad).unwrap();
        let inv = h.inverse().unwrap();
        let back = inv.apply(h.apply((u, v)).unwrap()).unwrap();
        prop_assert!(close(back, (u, v), 1e-12), "{:?} -> {:?}", (u, v), back);
    }

    #[test]
    fn interior_maps_to_interior(quad in arb_convex_quad(), u in 0.01..0.99f64, v in 0.01..0.99f64) {
        let h = Homography::square_to_quad(&quad).unwrap();
        let q = h.apply((u, v)).unwrap();
        prop_assert!(convex_contains(&quad, q));
    }
}

#[test]
fn inverse_bilinear_round_trips_and_rejects_outside() {
    let q = [(0.1, 0.1), (0.9, 0.2), (0.8, 0.95), (0.05, 0.8)];
    for (s, t) in [(0.0, 0.0), (1.0, 1.0), (0.3, 0.7), (0.5, 0.5), (0.99, 0.01)] {
        let p = bilinear(&q, s, t);
        let (s2, t2) = inverse_bilinear(&q, p).unwrap();
        assert!(
            (s - s2).abs() < 1e-9 && (t - t2).abs() < 1e-9,
            "{s},{t} -> {s2},{t2}"
        );
    }
    assert!(inverse_bilinear(&q, (0.0, 0.0)).is_none());
    // Parallelogram (k2 = 0) branch.
    let par = [(0.0, 0.0), (1.0, 0.0), (1.5, 1.0), (0.5, 1.0)];
    let p = bilinear(&par, 0.25, 0.5);
    let (s, t) = inverse_bilinear(&par, p).unwrap();
    assert!((s - 0.25).abs() < 1e-12 && (t - 0.5).abs() < 1e-12);
}

proptest! {
    #[test]
    fn shared_edges_agree_between_patches(t in 0.0..1.0f64) {
        // Two patches sharing the edge (b, c): the edge maps identically.
        let left = [(0.0, 0.0), (0.5, 0.1), (0.55, 0.9), (0.0, 1.0)];
        let right = [(0.5, 0.1), (1.0, 0.0), (1.0, 1.0), (0.55, 0.9)];
        let a = bilinear(&left, 1.0, t);
        let b = bilinear(&right, 0.0, t);
        prop_assert!((a.0 - b.0).abs() < 1e-15 && (a.1 - b.1).abs() < 1e-15);
    }
}
