// SPDX-License-Identifier: Apache-2.0
//! Version-1 document schema.
//!
//! Field order in these structs *is* the serialised key order. Maps are
//! `BTreeMap` so their order is deterministic too.

use std::collections::BTreeMap;

use om_geom::Point2;
use om_time::{DEFAULT_TICKS_PER_SECOND, I128Str, Rate, Speed};
use om_types::{Finite, MediaId, OutputId, ProjectId, SurfaceId, UnitInterval};
use serde::{Deserialize, Serialize};

/// Free-form extension payloads, keyed by namespace (e.g. `"org.example.foo"`).
/// Preserved verbatim across load/save even when this build does not
/// understand them.
pub type Extensions = BTreeMap<String, serde_json::Value>;

/// Root document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub format: String,
    pub version: u64,
    pub project_id: ProjectId,
    /// Incremented by every applied command; ties the recovery journal to
    /// the saved file.
    #[serde(default)]
    pub revision: u64,
    pub name: String,
    #[serde(default)]
    pub timebase: Timebase,
    #[serde(default)]
    pub canvas: Canvas,
    #[serde(default)]
    pub media: Vec<Media>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub show: Show,
    #[serde(default)]
    pub extensions: Extensions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timebase {
    pub ticks_per_second: I128Str,
}

impl Default for Timebase {
    fn default() -> Self {
        Self {
            ticks_per_second: I128Str(DEFAULT_TICKS_PER_SECOND),
        }
    }
}

/// The composition canvas every surface is mapped into. Surface coordinates
/// are normalised to it; outputs display all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
}

impl Canvas {
    pub const MAX_DIMENSION: u32 = 16384;
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
        }
    }
}

/// A media item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub id: MediaId,
    pub name: String,
    pub source: MediaSource,
    /// Time-based media only (video, sequences); ignored for stills.
    #[serde(default)]
    pub playback: Playback,
    #[serde(default)]
    pub extensions: Extensions,
}

/// How time-based media plays.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playback {
    /// Restart at the end (or wrap at the start when reversed).
    #[serde(default = "default_true")]
    pub looping: bool,
    /// Exact playback speed; negative plays in reverse. Audio plays only at
    /// normal speed (DECISIONS.md D-015).
    #[serde(default)]
    pub speed: Speed,
    /// Audio gain (linear).
    #[serde(default)]
    pub volume: UnitInterval,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            looping: true,
            speed: Speed::NORMAL,
            volume: UnitInterval::ONE,
        }
    }
}

/// Where a media item's pixels come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaSource {
    /// A still image file. `path` is relative to the project file's
    /// directory when possible (forward slashes); see DECISIONS.md D-009.
    Image { path: String },
    /// A video file decoded by the media adapter (FFmpeg).
    Video { path: String },
    /// A folder of numbered still images played at `rate`.
    Sequence { path: String, rate: Rate },
    /// A procedurally generated pattern; needs no files.
    Pattern { pattern: PatternKind },
}

impl MediaSource {
    /// Stored file or folder path, for file-backed sources.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Image { path } | Self::Video { path } | Self::Sequence { path, .. } => Some(path),
            Self::Pattern { .. } => None,
        }
    }

    /// True for sources with a timeline (video, sequences).
    #[must_use]
    pub fn is_time_based(&self) -> bool {
        matches!(self, Self::Video { .. } | Self::Sequence { .. })
    }
}

/// Built-in generated patterns (used for calibration and tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternKind {
    /// Labelled UV grid: colour encodes position, lines every 1/8.
    UvGrid,
    /// 8×8 black/white checkerboard.
    Checkerboard,
    /// Solid white.
    White,
}

/// A mapping surface. Geometry arrives with the renderer milestone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    pub id: SurfaceId,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub opacity: UnitInterval,
    #[serde(default)]
    pub shape: Shape,
    #[serde(default)]
    pub blend: BlendMode,
    /// Media shown on this surface; `None` renders nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaId>,
    #[serde(default)]
    pub extensions: Extensions,
}

impl Surface {
    #[must_use]
    pub fn new(id: SurfaceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            enabled: true,
            opacity: UnitInterval::ONE,
            shape: Shape::default(),
            blend: BlendMode::Normal,
            media: None,
            extensions: Extensions::new(),
        }
    }
}

/// Surface geometry. `corners` are in canvas space; `uv` are the matching
/// points in the media (see `om_geom` for conventions). Any finite corners
/// may be stored; shapes that cannot be mapped (concave, degenerate) render
/// nothing and are reported by the renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Quad {
        corners: [Point2; 4],
        uv: [Point2; 4],
    },
    Triangle {
        corners: [Point2; 3],
        uv: [Point2; 3],
    },
    /// The ellipse inscribed in a (possibly perspective) quad: the media is
    /// mapped as for `Quad`, then clipped to the inscribed ellipse.
    Ellipse {
        corners: [Point2; 4],
        uv: [Point2; 4],
    },
    /// A `columns × rows` grid warp. `points` holds `(columns + 1) × (rows +
    /// 1)` control points row-major from the top-left; each cell maps its
    /// share of the `uv` quad with its own perspective map.
    Mesh {
        columns: u16,
        rows: u16,
        points: Vec<Point2>,
        uv: [Point2; 4],
    },
    /// A straight stroke between two points. `width` is a fraction of the
    /// canvas height; the media maps along the stroke (u) and across it (v).
    Line {
        ends: [Point2; 2],
        width: Finite,
        uv: [Point2; 4],
    },
}

/// Largest mesh subdivision per axis.
pub const MAX_MESH_DIVISIONS: u16 = 32;

/// How a surface combines with what is beneath it (premultiplied, linear).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    /// `src + dst × (1 − αs)`
    #[default]
    Normal,
    /// `src + dst`
    Add,
    /// `src + dst × (1 − src)` per channel
    Screen,
    /// `src × dst + dst × (1 − αs)` (exact over opaque backgrounds)
    Multiply,
}

fn pt(x: f64, y: f64) -> Point2 {
    // Callers pass literals in [0, 1]; finite by construction.
    Point2::from_finite(
        Finite::new(x).unwrap_or(Finite::ZERO),
        Finite::new(y).unwrap_or(Finite::ZERO),
    )
}

impl Shape {
    /// The full-canvas quad with full-media UVs.
    #[must_use]
    pub fn full_quad() -> Self {
        Self::Quad {
            corners: Point2::unit_square(),
            uv: Point2::unit_square(),
        }
    }

    /// A centred quad covering the middle half of the canvas.
    #[must_use]
    pub fn centred_quad() -> Self {
        Self::Quad {
            corners: [
                pt(0.25, 0.25),
                pt(0.75, 0.25),
                pt(0.75, 0.75),
                pt(0.25, 0.75),
            ],
            uv: Point2::unit_square(),
        }
    }

    /// A centred ellipse (circle on a square canvas).
    #[must_use]
    pub fn centred_ellipse() -> Self {
        Self::Ellipse {
            corners: [pt(0.3, 0.3), pt(0.7, 0.3), pt(0.7, 0.7), pt(0.3, 0.7)],
            uv: Point2::unit_square(),
        }
    }

    /// A horizontal line across the middle of the canvas.
    #[must_use]
    pub fn centred_line() -> Self {
        Self::Line {
            ends: [pt(0.2, 0.5), pt(0.8, 0.5)],
            width: Finite::new(0.02).unwrap_or(Finite::ZERO),
            uv: Point2::unit_square(),
        }
    }

    /// A centred triangle mapping the top-left half of the media.
    #[must_use]
    pub fn centred_triangle() -> Self {
        Self::Triangle {
            corners: [pt(0.3, 0.3), pt(0.7, 0.3), pt(0.3, 0.7)],
            uv: Point2::unit_triangle(),
        }
    }

    /// Editable control points (quad/ellipse corners, triangle corners,
    /// mesh points, line ends).
    #[must_use]
    pub fn corners(&self) -> &[Point2] {
        match self {
            Self::Quad { corners, .. } | Self::Ellipse { corners, .. } => corners,
            Self::Triangle { corners, .. } => corners,
            Self::Mesh { points, .. } => points,
            Self::Line { ends, .. } => ends,
        }
    }

    fn corners_mut(&mut self) -> &mut [Point2] {
        match self {
            Self::Quad { corners, .. } | Self::Ellipse { corners, .. } => corners,
            Self::Triangle { corners, .. } => corners,
            Self::Mesh { points, .. } => points,
            Self::Line { ends, .. } => ends,
        }
    }

    /// Returns a copy with control point `index` moved to `to` (no-op if out
    /// of range).
    #[must_use]
    pub fn with_corner(mut self, index: usize, to: Point2) -> Self {
        if let Some(c) = self.corners_mut().get_mut(index) {
            *c = to;
        }
        self
    }

    /// Returns a copy with all control points translated by `(dx, dy)`, or
    /// `None` if a result would be non-finite.
    #[must_use]
    pub fn translated(mut self, dx: f64, dy: f64) -> Option<Self> {
        for c in self.corners_mut() {
            *c = Point2::new(c.x() + dx, c.y() + dy).ok()?;
        }
        Some(self)
    }

    /// Boundary polygon in canvas space for hit testing and outlines.
    /// `aspect` is canvas width / height (lines are sized by height).
    #[must_use]
    pub fn outline(&self, aspect: f64) -> Vec<Point2> {
        match self {
            Self::Quad { corners, .. } | Self::Ellipse { corners, .. } => corners.to_vec(),
            Self::Triangle { corners, .. } => corners.to_vec(),
            Self::Mesh {
                columns,
                rows,
                points,
                ..
            } => {
                let (c, r) = (usize::from(*columns), usize::from(*rows));
                let at = |i: usize, j: usize| points.get(j * (c + 1) + i).copied();
                let mut out = Vec::new();
                out.extend((0..=c).filter_map(|i| at(i, 0)));
                out.extend((1..=r).filter_map(|j| at(c, j)));
                out.extend((0..c).rev().filter_map(|i| at(i, r)));
                out.extend((1..r).rev().filter_map(|j| at(0, j)));
                out
            }
            Self::Line { ends, width, .. } => line_quad(ends, width.get(), aspect).to_vec(),
        }
    }

    /// A rows × columns mesh following this quad's perspective (for
    /// converting a quad into a warpable mesh). `None` for non-quads or a
    /// quad that cannot be mapped.
    #[must_use]
    pub fn to_mesh(&self, columns: u16, rows: u16) -> Option<Self> {
        let Self::Quad { corners, uv } = self else {
            return None;
        };
        let (columns, rows) = (
            columns.clamp(1, MAX_MESH_DIVISIONS),
            rows.clamp(1, MAX_MESH_DIVISIONS),
        );
        let h = om_geom::Homography::square_to_quad(corners).ok()?;
        let mut points = Vec::new();
        for j in 0..=rows {
            for i in 0..=columns {
                let (x, y) = h.apply((
                    f64::from(i) / f64::from(columns),
                    f64::from(j) / f64::from(rows),
                ))?;
                points.push(Point2::new(x, y).ok()?);
            }
        }
        Some(Self::Mesh {
            columns,
            rows,
            points,
            uv: *uv,
        })
    }

    /// Checks structural invariants (mesh point count, divisions, line width).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Mesh {
                columns,
                rows,
                points,
                ..
            } => {
                if *columns == 0
                    || *rows == 0
                    || *columns > MAX_MESH_DIVISIONS
                    || *rows > MAX_MESH_DIVISIONS
                {
                    return Err(format!(
                        "mesh is {columns}x{rows}; each side must be 1..={MAX_MESH_DIVISIONS}"
                    ));
                }
                let expected = (usize::from(*columns) + 1) * (usize::from(*rows) + 1);
                if points.len() != expected {
                    return Err(format!(
                        "mesh has {} points; {columns}x{rows} needs {expected}",
                        points.len()
                    ));
                }
                Ok(())
            }
            Self::Line { width, .. } if width.get() < 0.0 => Err("line width is negative".into()),
            _ => Ok(()),
        }
    }
}

/// The quad covered by a line stroke: ends offset by ± half the width,
/// perpendicular in pixel-proportional space. Order: start-left, end-left,
/// end-right, start-right (u along the line, v across).
#[must_use]
pub fn line_quad(ends: &[Point2; 2], width: f64, aspect: f64) -> [Point2; 4] {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let (a, b) = (ends[0].to_tuple(), ends[1].to_tuple());
    // Work in units of canvas height so the stroke width is isotropic.
    let (dx, dy) = ((b.0 - a.0) * aspect, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (nx, ny) = if len > 0.0 {
        (-dy / len, dx / len)
    } else {
        (0.0, 0.0)
    };
    let (ox, oy) = (nx * width * 0.5 / aspect, ny * width * 0.5);
    let p = |x: f64, y: f64| Point2::new(x, y).unwrap_or_default();
    [
        p(a.0 - ox, a.1 - oy),
        p(b.0 - ox, b.1 - oy),
        p(b.0 + ox, b.1 + oy),
        p(a.0 + ox, a.1 + oy),
    ]
}

impl Default for Shape {
    fn default() -> Self {
        Self::centred_quad()
    }
}

/// A physical display output (projector/monitor) showing the whole canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub id: OutputId,
    pub name: String,
    /// Whether the output window is open.
    #[serde(default)]
    pub enabled: bool,
    /// Preferred display. Matched by name first, index second, so a
    /// projector re-plugged into another port is found again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplayTarget>,
    #[serde(default)]
    pub extensions: Extensions,
}

/// How an output identifies its display across sessions and hot-plugs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayTarget {
    pub name: String,
    pub index: u32,
}

/// Show-control state. Cues and timelines arrive with the show milestone;
/// until then they are kept as opaque JSON so nothing is lost.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show {
    #[serde(default)]
    pub cues: Vec<serde_json::Value>,
    #[serde(default)]
    pub timelines: Vec<serde_json::Value>,
}

fn default_true() -> bool {
    true
}
