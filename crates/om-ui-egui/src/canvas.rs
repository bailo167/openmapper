// SPDX-License-Identifier: Apache-2.0
//! Mapping-canvas interaction: hit testing and drag-to-command translation.
//! Pure functions so the behaviour is unit-testable without a window.

use eframe::egui::{Pos2, Rect, Vec2};
use om_geom::Point2;
use om_project::{Project, Shape};
use om_types::SurfaceId;

/// Handle grab radius in screen points.
pub const HANDLE_RADIUS: f32 = 9.0;

/// What a drag is moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragKind {
    Corner(usize),
    /// Moving the whole surface; `origin` is the shape when the drag began.
    Body {
        origin: Shape,
        start: Pos2,
    },
}

/// An in-progress drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub surface: SurfaceId,
    pub kind: DragKind,
}

/// The largest rect with the canvas aspect ratio centred in `avail`.
#[must_use]
pub fn fit_rect(avail: Rect, canvas: (u32, u32)) -> Rect {
    let aspect = canvas.0 as f32 / canvas.1.max(1) as f32;
    let mut size = avail.size();
    if size.x / size.y.max(1.0) > aspect {
        size.x = size.y * aspect;
    } else {
        size.y = size.x / aspect;
    }
    Rect::from_center_size(avail.center(), size)
}

/// Canvas-space point to screen position inside `rect`.
#[must_use]
pub fn to_screen(rect: Rect, p: Point2) -> Pos2 {
    rect.min + Vec2::new(p.x() as f32 * rect.width(), p.y() as f32 * rect.height())
}

/// Screen position to canvas space (may be outside 0..1).
#[must_use]
pub fn to_canvas(rect: Rect, pos: Pos2) -> Option<Point2> {
    let x = f64::from((pos.x - rect.min.x) / rect.width());
    let y = f64::from((pos.y - rect.min.y) / rect.height());
    Point2::new(x, y).ok()
}

/// Corner of `shape` under `pos`, nearest first.
#[must_use]
pub fn corner_at(rect: Rect, shape: &Shape, pos: Pos2) -> Option<usize> {
    shape
        .corners()
        .iter()
        .enumerate()
        .map(|(i, c)| (i, to_screen(rect, *c).distance(pos)))
        .filter(|(_, d)| *d <= HANDLE_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Even-odd point-in-polygon in screen space (handles any shape the user
/// drags into, including concave ones).
#[must_use]
pub fn polygon_contains(rect: Rect, shape: &Shape, pos: Pos2) -> bool {
    let pts: Vec<Pos2> = shape
        .corners()
        .iter()
        .map(|c| to_screen(rect, *c))
        .collect();
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + n - 1) % n]);
        if (a.y > pos.y) != (b.y > pos.y) {
            let x = (b.x - a.x) * (pos.y - a.y) / (b.y - a.y) + a.x;
            if pos.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Decides what a press at `pos` grabs: a corner of the selected surface
/// first, then the topmost surface body under the pointer.
#[must_use]
pub fn begin_drag(
    project: &Project,
    rect: Rect,
    selected: Option<SurfaceId>,
    pos: Pos2,
) -> Option<Drag> {
    if let Some(s) = selected.and_then(|id| project.surface(id))
        && let Some(i) = corner_at(rect, &s.shape, pos)
    {
        return Some(Drag {
            surface: s.id,
            kind: DragKind::Corner(i),
        });
    }
    project
        .surfaces
        .iter()
        .rev()
        .find(|s| polygon_contains(rect, &s.shape, pos))
        .map(|s| Drag {
            surface: s.id,
            kind: DragKind::Body {
                origin: s.shape,
                start: pos,
            },
        })
}

/// The shape a drag produces with the pointer at `pos`.
#[must_use]
pub fn dragged_shape(rect: Rect, current: &Shape, drag: &Drag, pos: Pos2) -> Option<Shape> {
    match drag.kind {
        DragKind::Corner(i) => Some(current.with_corner(i, to_canvas(rect, pos)?)),
        DragKind::Body { origin, start } => {
            let d = pos - start;
            origin.translated(
                f64::from(d.x / rect.width()),
                f64::from(d.y / rect.height()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use om_project::Surface;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(400.0, 200.0))
    }

    #[test]
    fn fit_rect_keeps_aspect() {
        let r = fit_rect(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 1000.0)),
            (1920, 1080),
        );
        assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 1e-4);
        assert_eq!(r.width(), 1000.0);
    }

    #[test]
    fn screen_canvas_round_trip() {
        let p = Point2::new(0.25, 0.75).unwrap();
        let s = to_screen(rect(), p);
        assert_eq!(s, Pos2::new(200.0, 200.0));
        let back = to_canvas(rect(), s).unwrap();
        assert!((back.x() - 0.25).abs() < 1e-6 && (back.y() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn corner_drag_and_body_drag() {
        let mut p = Project::new("t");
        let id = SurfaceId::from_u128(1);
        p.surfaces.push(Surface::new(id, "s")); // centred quad 0.25..0.75
        let r = rect();
        let tl = to_screen(r, p.surfaces[0].shape.corners()[0]);

        // Corner handles only grab on the selected surface.
        let d = begin_drag(&p, r, Some(id), tl + Vec2::new(3.0, 3.0)).unwrap();
        assert_eq!(d.kind, DragKind::Corner(0));
        let moved = dragged_shape(r, &p.surfaces[0].shape, &d, Pos2::new(100.0, 50.0)).unwrap();
        assert_eq!(moved.corners()[0].to_tuple(), (0.0, 0.0));

        // Pressing inside an unselected surface grabs its body.
        let centre = Pos2::new(300.0, 150.0);
        let d = begin_drag(&p, r, None, centre).unwrap();
        assert!(matches!(d.kind, DragKind::Body { .. }));
        let moved =
            dragged_shape(r, &p.surfaces[0].shape, &d, centre + Vec2::new(40.0, 20.0)).unwrap();
        let c0 = moved.corners()[0].to_tuple();
        assert!(
            (c0.0 - 0.35).abs() < 1e-6 && (c0.1 - 0.35).abs() < 1e-6,
            "{c0:?}"
        );

        assert!(begin_drag(&p, r, None, Pos2::new(105.0, 55.0)).is_none());
    }
}
