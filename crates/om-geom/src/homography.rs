// SPDX-License-Identifier: Apache-2.0

use glam::{DMat3, DVec3};

use crate::{Point2, is_strictly_convex, signed_area2};

/// Why a mapping could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GeomError {
    #[error("shape is degenerate (zero area or collinear corners)")]
    Degenerate,
    #[error("quad is not convex")]
    NotConvex,
    #[error("mapping is not invertible")]
    Singular,
}

/// A projective map of the plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography(DMat3);

impl Homography {
    pub const IDENTITY: Self = Self(DMat3::IDENTITY);

    /// Row-major constructor: `[a b c; d e f; g h i]`.
    #[must_use]
    pub fn from_rows(rows: [[f64; 3]; 3]) -> Self {
        Self(DMat3::from_cols_array_2d(&rows).transpose())
    }

    /// Row-major elements.
    #[must_use]
    pub fn to_rows(&self) -> [[f64; 3]; 3] {
        self.0.transpose().to_cols_array_2d()
    }

    /// Maps the unit square (TL, TR, BR, BL) onto `quad` (Heckbert's
    /// closed form). `quad` must be strictly convex.
    pub fn square_to_quad(quad: &[Point2; 4]) -> Result<Self, GeomError> {
        if signed_area2(quad).abs() < 1e-12 {
            return Err(GeomError::Degenerate);
        }
        if !is_strictly_convex(quad) {
            return Err(GeomError::NotConvex);
        }
        let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = quad.map(Point2::to_tuple);
        let sx = x0 - x1 + x2 - x3;
        let sy = y0 - y1 + y2 - y3;
        let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
        let det = dx1 * dy2 - dx2 * dy1;
        if det.abs() < 1e-15 {
            return Err(GeomError::Degenerate);
        }
        let g = (sx * dy2 - dx2 * sy) / det;
        let h = (dx1 * sy - sx * dy1) / det;
        Ok(Self::from_rows([
            [x1 - x0 + g * x1, x3 - x0 + h * x3, x0],
            [y1 - y0 + g * y1, y3 - y0 + h * y3, y0],
            [g, h, 1.0],
        ]))
    }

    /// Affine map sending (0,0), (1,0), (0,1) to the triangle's corners.
    pub fn unit_triangle_to(tri: &[Point2; 3]) -> Result<Self, GeomError> {
        if signed_area2(tri).abs() < 1e-12 {
            return Err(GeomError::Degenerate);
        }
        let [(x0, y0), (x1, y1), (x2, y2)] = tri.map(Point2::to_tuple);
        Ok(Self::from_rows([
            [x1 - x0, x2 - x0, x0],
            [y1 - y0, y2 - y0, y0],
            [0.0, 0.0, 1.0],
        ]))
    }

    pub fn inverse(&self) -> Result<Self, GeomError> {
        let det = self.0.determinant();
        if !det.is_finite() || det.abs() < 1e-18 {
            return Err(GeomError::Singular);
        }
        Ok(Self(self.0.inverse()))
    }

    /// `self ∘ other`: applies `other` first.
    #[must_use]
    pub fn then_after(&self, other: &Self) -> Self {
        Self(self.0 * other.0)
    }

    /// Applies the map. Returns `None` for points on the line at infinity.
    #[must_use]
    pub fn apply(&self, p: (f64, f64)) -> Option<(f64, f64)> {
        let v = self.0 * DVec3::new(p.0, p.1, 1.0);
        if v.z.abs() < 1e-300 {
            return None;
        }
        Some((v.x / v.z, v.y / v.z))
    }

    /// Column-major `f32` elements, for GPU uniforms (as three padded vec4s).
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn to_gpu_cols(&self) -> [[f32; 4]; 3] {
        let c = self.0.to_cols_array_2d();
        c.map(|col| [col[0] as f32, col[1] as f32, col[2] as f32, 0.0])
    }
}

/// The map from canvas space to UV space for a quad surface: canvas
/// `corners[i]` lands on `uv[i]`, perspective-correct.
pub fn quad_map(corners: &[Point2; 4], uv: &[Point2; 4]) -> Result<Homography, GeomError> {
    let to_canvas = Homography::square_to_quad(corners)?;
    let to_uv = Homography::square_to_quad(uv)?;
    Ok(to_uv.then_after(&to_canvas.inverse()?))
}

/// The affine map from canvas space to UV space for a triangle surface.
pub fn triangle_map(corners: &[Point2; 3], uv: &[Point2; 3]) -> Result<Homography, GeomError> {
    let to_canvas = Homography::unit_triangle_to(corners)?;
    let to_uv = Homography::unit_triangle_to(uv)?;
    Ok(to_uv.then_after(&to_canvas.inverse()?))
}
