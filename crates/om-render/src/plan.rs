// SPDX-License-Identifier: Apache-2.0
//! Turns a project into an ordered list of drawable surfaces. Shared by the
//! GPU compositor and the CPU reference renderer so both draw the same thing.

use om_geom::{GeomError, Homography, Point2, quad_map, triangle_map};
use om_project::{BlendMode, Project, Shape, line_quad};
use om_types::{MediaId, SurfaceId};

/// Extra per-pixel clipping for an item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clip {
    None,
    /// Keep pixels inside the ellipse inscribed in the unit square of
    /// `canvas_to_local`'s output space.
    Ellipse {
        canvas_to_local: Homography,
    },
}

/// How canvas positions map to media UVs within an item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mapping {
    /// Perspective-correct (quads, triangles, lines, ellipses).
    Projective(Homography),
    /// Bilinear patch (mesh cells): continuous across shared cell edges.
    /// Corners and UVs are ordered P00, P10, P11, P01.
    Bilinear {
        corners: [(f64, f64); 4],
        uv: [(f64, f64); 4],
    },
}

impl Mapping {
    /// UV for a canvas point, or `None` outside a bilinear patch.
    #[must_use]
    pub fn apply(&self, p: (f64, f64)) -> Option<(f64, f64)> {
        match self {
            Self::Projective(h) => h.apply(p),
            Self::Bilinear { corners, uv } => {
                let (s, t) = om_geom::inverse_bilinear(corners, p)?;
                Some(om_geom::bilinear(uv, s, t))
            }
        }
    }
}

/// One convex piece of a surface, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawItem {
    pub surface: SurfaceId,
    pub media: MediaId,
    /// Convex polygon in canvas space (3 or 4 corners).
    pub polygon: Vec<(f64, f64)>,
    /// Maps canvas space to media UV space.
    pub mapping: Mapping,
    pub clip: Clip,
    pub opacity: f32,
    pub blend: BlendMode,
}

/// Why a surface was not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    Transparent,
    NoMedia,
    MediaNotLoaded(MediaId),
    Unmappable(GeomError),
    /// Some mesh cells could not be mapped (the rest were drawn).
    PartlyUnmappable {
        skipped_cells: u32,
    },
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("disabled"),
            Self::Transparent => f.write_str("opacity is 0"),
            Self::NoMedia => f.write_str("no media assigned"),
            Self::MediaNotLoaded(m) => write!(f, "media {m} is not loaded"),
            Self::Unmappable(e) => write!(f, "shape cannot be mapped: {e}"),
            Self::PartlyUnmappable { skipped_cells } => {
                write!(f, "{skipped_cells} mesh cell(s) are folded or degenerate")
            }
        }
    }
}

/// Draw list for one frame, in back-to-front order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderPlan {
    pub items: Vec<DrawItem>,
    pub skipped: Vec<(SurfaceId, SkipReason)>,
}

/// One mappable piece: polygon corners plus matching UV corners.
enum Piece {
    Quad([Point2; 4], [Point2; 4]),
    Triangle([Point2; 3], [Point2; 3]),
    /// A mesh cell (bilinear).
    Cell([Point2; 4], [Point2; 4]),
}

/// Splits a shape into convex pieces. Ellipses come back as their quad.
fn pieces(shape: &Shape, aspect: f64) -> Vec<Piece> {
    match shape {
        Shape::Quad { corners, uv } | Shape::Ellipse { corners, uv } => {
            vec![Piece::Quad(*corners, *uv)]
        }
        Shape::Triangle { corners, uv } => vec![Piece::Triangle(*corners, *uv)],
        Shape::Line { ends, width, uv } => {
            vec![Piece::Quad(line_quad(ends, width.get(), aspect), *uv)]
        }
        Shape::Mesh {
            columns,
            rows,
            points,
            uv,
        } => {
            let (c, r) = (usize::from(*columns), usize::from(*rows));
            // UV corners of each cell follow the UV quad's own perspective.
            let Ok(uv_map) = Homography::square_to_quad(uv) else {
                return Vec::new();
            };
            let uv_at = |i: usize, j: usize| {
                uv_map
                    .apply((i as f64 / c as f64, j as f64 / r as f64))
                    .and_then(|(x, y)| Point2::new(x, y).ok())
                    .unwrap_or_default()
            };
            let p = |i: usize, j: usize| points.get(j * (c + 1) + i).copied().unwrap_or_default();
            let mut out = Vec::with_capacity(c * r);
            for j in 0..r {
                for i in 0..c {
                    out.push(Piece::Cell(
                        [p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)],
                        [
                            uv_at(i, j),
                            uv_at(i + 1, j),
                            uv_at(i + 1, j + 1),
                            uv_at(i, j + 1),
                        ],
                    ));
                }
            }
            out
        }
    }
}

/// A mesh cell's bilinear mapping; folded (non-convex) cells cannot be drawn.
fn bilinear_cell(c: &[Point2; 4], uv: &[Point2; 4]) -> Result<Mapping, GeomError> {
    if om_geom::signed_area2(c).abs() < 1e-12 {
        return Err(GeomError::Degenerate);
    }
    if !om_geom::is_strictly_convex(c) {
        return Err(GeomError::NotConvex);
    }
    Ok(Mapping::Bilinear {
        corners: c.map(Point2::to_tuple),
        uv: uv.map(Point2::to_tuple),
    })
}

/// Builds the draw list. `is_loaded` reports whether a media item's pixels
/// are available to the renderer.
pub fn plan(project: &Project, is_loaded: impl Fn(MediaId) -> bool) -> RenderPlan {
    let mut out = RenderPlan::default();
    let aspect = f64::from(project.canvas.width) / f64::from(project.canvas.height.max(1));
    for s in &project.surfaces {
        let skip = |r| (s.id, r);
        if !s.enabled {
            out.skipped.push(skip(SkipReason::Disabled));
            continue;
        }
        if s.opacity.get() <= 0.0 {
            out.skipped.push(skip(SkipReason::Transparent));
            continue;
        }
        let Some(media) = s.media else {
            out.skipped.push(skip(SkipReason::NoMedia));
            continue;
        };
        if !is_loaded(media) {
            out.skipped.push(skip(SkipReason::MediaNotLoaded(media)));
            continue;
        }
        let clip = match &s.shape {
            Shape::Ellipse { corners, .. } => {
                match Homography::square_to_quad(corners).and_then(|h| h.inverse()) {
                    Ok(canvas_to_local) => Clip::Ellipse { canvas_to_local },
                    Err(e) => {
                        out.skipped.push(skip(SkipReason::Unmappable(e)));
                        continue;
                    }
                }
            }
            _ => Clip::None,
        };
        let parts = pieces(&s.shape, aspect);
        let total = parts.len();
        let mut failed = 0u32;
        let mut last_err = GeomError::Degenerate;
        for piece in parts {
            let (polygon, map) = match &piece {
                Piece::Quad(c, uv) => (c.as_slice(), quad_map(c, uv).map(Mapping::Projective)),
                Piece::Triangle(c, uv) => {
                    (c.as_slice(), triangle_map(c, uv).map(Mapping::Projective))
                }
                Piece::Cell(c, uv) => (c.as_slice(), bilinear_cell(c, uv)),
            };
            match map {
                Ok(mapping) => out.items.push(DrawItem {
                    surface: s.id,
                    media,
                    polygon: polygon.iter().map(|p| p.to_tuple()).collect(),
                    mapping,
                    clip,
                    #[allow(clippy::cast_possible_truncation)]
                    opacity: s.opacity.get() as f32,
                    blend: s.blend,
                }),
                Err(e) => {
                    failed += 1;
                    last_err = e;
                }
            }
        }
        if total == 0 || failed as usize == total {
            out.skipped.push(skip(SkipReason::Unmappable(last_err)));
        } else if failed > 0 {
            out.skipped.push(skip(SkipReason::PartlyUnmappable {
                skipped_cells: failed,
            }));
        }
    }
    out
}
