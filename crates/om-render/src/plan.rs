// SPDX-License-Identifier: Apache-2.0
//! Turns a project into an ordered list of drawable surfaces. Shared by the
//! GPU compositor and the CPU reference renderer so both draw the same thing.

use om_geom::{GeomError, Homography, quad_map, triangle_map};
use om_project::{Project, Shape};
use om_types::{MediaId, SurfaceId};

/// One surface ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawItem {
    pub surface: SurfaceId,
    pub media: MediaId,
    /// Convex polygon in canvas space (3 or 4 corners).
    pub polygon: Vec<(f64, f64)>,
    /// Maps canvas space to media UV space.
    pub canvas_to_uv: Homography,
    pub opacity: f32,
}

/// Why a surface was not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    Transparent,
    NoMedia,
    MediaNotLoaded(MediaId),
    Unmappable(GeomError),
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("disabled"),
            Self::Transparent => f.write_str("opacity is 0"),
            Self::NoMedia => f.write_str("no media assigned"),
            Self::MediaNotLoaded(m) => write!(f, "media {m} is not loaded"),
            Self::Unmappable(e) => write!(f, "shape cannot be mapped: {e}"),
        }
    }
}

/// Draw list for one frame, in back-to-front order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderPlan {
    pub items: Vec<DrawItem>,
    pub skipped: Vec<(SurfaceId, SkipReason)>,
}

/// Builds the draw list. `is_loaded` reports whether a media item's pixels
/// are available to the renderer.
pub fn plan(project: &Project, is_loaded: impl Fn(MediaId) -> bool) -> RenderPlan {
    let mut out = RenderPlan::default();
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
        let map = match &s.shape {
            Shape::Quad { corners, uv } => quad_map(corners, uv),
            Shape::Triangle { corners, uv } => triangle_map(corners, uv),
        };
        match map {
            Ok(canvas_to_uv) => out.items.push(DrawItem {
                surface: s.id,
                media,
                polygon: s.shape.corners().iter().map(|p| p.to_tuple()).collect(),
                canvas_to_uv,
                #[allow(clippy::cast_possible_truncation)]
                opacity: s.opacity.get() as f32,
            }),
            Err(e) => out.skipped.push(skip(SkipReason::Unmappable(e))),
        }
    }
    out
}
