// SPDX-License-Identifier: Apache-2.0
//! Output routing.
//!
//! Enumerates connected displays and resolves each project output's
//! [`DisplayTarget`] to a currently connected display. Resolution prefers the
//! display *name* (so a projector moved to another port is found again) and
//! falls back to the stored index only when the name is unknown.
//!
//! Display indices follow the operating system's enumeration order, which is
//! also the order the windowing layer uses for monitor selection
//! (see DECISIONS.md D-008).

use om_project::DisplayTarget;

/// A connected display.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// Position in the OS enumeration order.
    pub index: u32,
    /// Human-readable name (e.g. the projector model).
    pub name: String,
    /// Desktop position and size in logical pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Physical pixels per logical pixel.
    pub scale: f32,
    /// Refresh rate in Hz (0 if unknown).
    pub refresh_hz: f32,
    pub is_primary: bool,
    pub is_builtin: bool,
}

impl Display {
    /// Native resolution in physical pixels.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn native_size(&self) -> (u32, u32) {
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        (
            (self.width as f32 * s).round() as u32,
            (self.height as f32 * s).round() as u32,
        )
    }

    #[must_use]
    pub fn target(&self) -> DisplayTarget {
        DisplayTarget {
            name: self.name.clone(),
            index: self.index,
        }
    }
}

impl std::fmt::Display for Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.native_size();
        write!(f, "#{} {} — {w}×{h}", self.index, self.name)?;
        if self.refresh_hz > 0.0 {
            write!(f, " @ {:.2} Hz", self.refresh_hz)?;
        }
        if self.is_primary {
            f.write_str(" (primary)")?;
        }
        if self.is_builtin {
            f.write_str(" (built-in)")?;
        }
        Ok(())
    }
}

/// Display enumeration failure.
#[derive(Debug, thiserror::Error)]
#[error("could not list displays: {0}")]
pub struct DisplayError(String);

/// Lists connected displays in OS enumeration order.
pub fn list_displays() -> Result<Vec<Display>, DisplayError> {
    let all = display_info::DisplayInfo::all().map_err(|e| DisplayError(e.to_string()))?;
    Ok(all
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            let name = if d.friendly_name.trim().is_empty() {
                d.name
            } else {
                d.friendly_name
            };
            Display {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                name,
                x: d.x,
                y: d.y,
                width: d.width,
                height: d.height,
                scale: d.scale_factor,
                refresh_hz: d.frequency,
                is_primary: d.is_primary,
                is_builtin: d.is_builtin,
            }
        })
        .collect())
}

/// Finds the connected display for `target`: an exact name match first (the
/// lowest index if several share the name, preferring the stored index), then
/// the stored index only if no connected display has that name.
#[must_use]
pub fn resolve<'a>(target: &DisplayTarget, displays: &'a [Display]) -> Option<&'a Display> {
    let named: Vec<&Display> = displays.iter().filter(|d| d.name == target.name).collect();
    if let Some(d) = named.iter().find(|d| d.index == target.index) {
        return Some(d);
    }
    if let Some(d) = named.first() {
        return Some(d);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(index: u32, name: &str) -> Display {
        Display {
            index,
            name: name.into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            refresh_hz: 60.0,
            is_primary: index == 0,
            is_builtin: false,
        }
    }

    fn t(index: u32, name: &str) -> DisplayTarget {
        DisplayTarget {
            name: name.into(),
            index,
        }
    }

    #[test]
    fn resolves_by_name_when_port_changes() {
        let displays = [d(0, "Built-in"), d(1, "Monitor"), d(2, "Projector")];
        assert_eq!(
            resolve(&t(1, "Projector"), &displays).map(|d| d.index),
            Some(2)
        );
    }

    #[test]
    fn unplugged_display_resolves_to_none_not_to_another_screen() {
        // Sending a show to the operator's monitor because the projector was
        // unplugged would be worse than showing nothing.
        let displays = [d(0, "Built-in"), d(1, "Monitor")];
        assert_eq!(resolve(&t(1, "Projector"), &displays), None);
    }

    #[test]
    fn identical_names_prefer_stored_index() {
        let displays = [d(0, "Built-in"), d(1, "Projector"), d(2, "Projector")];
        assert_eq!(
            resolve(&t(2, "Projector"), &displays).map(|d| d.index),
            Some(2)
        );
        assert_eq!(
            resolve(&t(5, "Projector"), &displays).map(|d| d.index),
            Some(1)
        );
    }

    #[test]
    fn native_size_applies_scale() {
        let mut m = d(0, "Retina");
        m.width = 1470;
        m.height = 956;
        m.scale = 2.0;
        assert_eq!(m.native_size(), (2940, 1912));
    }
}
