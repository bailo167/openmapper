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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playback {
    /// Restart at the end (or wrap at the start when reversed).
    #[serde(default = "default_true")]
    pub looping: bool,
    /// Exact playback speed; negative plays in reverse.
    #[serde(default)]
    pub speed: Speed,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            looping: true,
            speed: Speed::NORMAL,
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
            media: None,
            extensions: Extensions::new(),
        }
    }
}

/// Surface geometry. `corners` are in canvas space; `uv` are the matching
/// points in the media (see `om_geom` for conventions). Any finite corners
/// may be stored; shapes that cannot be mapped (concave, degenerate) render
/// nothing and are reported by the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

    /// A centred triangle mapping the top-left half of the media.
    #[must_use]
    pub fn centred_triangle() -> Self {
        Self::Triangle {
            corners: [pt(0.3, 0.3), pt(0.7, 0.3), pt(0.3, 0.7)],
            uv: Point2::unit_triangle(),
        }
    }

    #[must_use]
    pub fn corners(&self) -> &[Point2] {
        match self {
            Self::Quad { corners, .. } => corners,
            Self::Triangle { corners, .. } => corners,
        }
    }

    /// Returns a copy with corner `index` moved to `to` (no-op if out of range).
    #[must_use]
    pub fn with_corner(mut self, index: usize, to: Point2) -> Self {
        match &mut self {
            Self::Quad { corners, .. } => {
                if let Some(c) = corners.get_mut(index) {
                    *c = to;
                }
            }
            Self::Triangle { corners, .. } => {
                if let Some(c) = corners.get_mut(index) {
                    *c = to;
                }
            }
        }
        self
    }

    /// Returns a copy with all corners translated by `(dx, dy)`, or `None`
    /// if a result would be non-finite.
    #[must_use]
    pub fn translated(mut self, dx: f64, dy: f64) -> Option<Self> {
        let mv = |c: &mut Point2| -> Option<()> {
            *c = Point2::new(c.x() + dx, c.y() + dy).ok()?;
            Some(())
        };
        match &mut self {
            Self::Quad { corners, .. } => corners.iter_mut().try_for_each(mv)?,
            Self::Triangle { corners, .. } => corners.iter_mut().try_for_each(mv)?,
        }
        Some(self)
    }
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
