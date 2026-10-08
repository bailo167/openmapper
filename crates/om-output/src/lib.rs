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
    /// Port the display is connected to (`HDMI-A-1`), when the platform
    /// reports it separately from the name (Linux).
    pub connector: Option<String>,
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
    let all = display_info::DisplayInfo::all().map_err(|e| {
        DisplayError(explain_display_error(
            &e.to_string(),
            cfg!(all(unix, not(target_os = "macos"))),
            |k| std::env::var_os(k).is_some_and(|v| !v.is_empty()),
        ))
    })?;
    Ok(all
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            // Linux reports only the connector (as both names); the
            // monitor's own name comes from its EDID, so a projector is
            // found again on another port.
            let (name, connector) = match edid_name_for_connector(&d.name) {
                Some(model) => (model, Some(d.name)),
                None if d.friendly_name.trim().is_empty() => (d.name, None),
                None => (d.friendly_name, None),
            };
            Display {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                name,
                connector,
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

/// On Linux/BSD, display enumeration needs the graphical session; from an
/// SSH shell or a system service `WAYLAND_DISPLAY` and `DISPLAY` are unset
/// and the library's own message ("error during parsing display string")
/// does not say so.
fn explain_display_error(raw: &str, unix_desktop: bool, is_set: impl Fn(&str) -> bool) -> String {
    if unix_desktop && !is_set("WAYLAND_DISPLAY") && !is_set("DISPLAY") {
        format!(
            "{raw} (no graphical session: WAYLAND_DISPLAY and DISPLAY are unset; run this \
             from the desktop, or set them to the session's, e.g. WAYLAND_DISPLAY=wayland-0 \
             XDG_RUNTIME_DIR=/run/user/$(id -u))"
        )
    } else {
        raw.to_owned()
    }
}

/// Finds the connected display for `target`: an exact name match first (the
/// lowest index if several share the name, preferring the stored index), then
/// the stored index only if no connected display has that name.
#[must_use]
pub fn resolve<'a>(target: &DisplayTarget, displays: &'a [Display]) -> Option<&'a Display> {
    // Projects saved before EDID names were read on Linux stored the
    // connector name; it still matches.
    let named: Vec<&Display> = displays
        .iter()
        .filter(|d| d.name == target.name || d.connector.as_deref() == Some(&target.name))
        .collect();
    if let Some(d) = named.iter().find(|d| d.index == target.index) {
        return Some(d);
    }
    if let Some(d) = named.first() {
        return Some(d);
    }
    None
}

/// The monitor name (EDID descriptor `0xFC`) of the display on DRM
/// connector `connector` (Linux sysfs; `None` elsewhere or if unreadable).
fn edid_name_for_connector(connector: &str) -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let suffix = format!("-{connector}");
    let dir = std::fs::read_dir("/sys/class/drm").ok()?;
    dir.filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(&suffix))
        .find_map(|e| std::fs::read(e.path().join("edid")).ok())
        .and_then(|edid| edid_monitor_name(&edid))
}

/// Parses the monitor name from an EDID base block.
fn edid_monitor_name(edid: &[u8]) -> Option<String> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    (0..4).find_map(|k| {
        let d = &edid[54 + 18 * k..72 + 18 * k];
        if d[..3] != [0, 0, 0] || d[3] != 0xFC {
            return None;
        }
        let text: String = d[5..18]
            .iter()
            .take_while(|b| **b != b'\n')
            .map(|b| char::from(*b))
            .filter(|c| c.is_ascii_graphic() || *c == ' ')
            .collect();
        let text = text.trim().to_owned();
        (!text.is_empty()).then_some(text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: over SSH `openmapper-cli displays` failed with only
    /// "error during parsing display string".
    #[test]
    fn display_error_names_the_missing_session() {
        let raw = "Connection closed, error during parsing display string";
        let none = explain_display_error(raw, true, |_| false);
        assert!(none.starts_with(raw));
        assert!(none.contains("no graphical session"), "{none}");
        assert_eq!(explain_display_error(raw, true, |k| k == "DISPLAY"), raw);
        assert_eq!(
            explain_display_error(raw, true, |k| k == "WAYLAND_DISPLAY"),
            raw
        );
        assert_eq!(explain_display_error(raw, false, |_| false), raw);
    }

    fn d(index: u32, name: &str) -> Display {
        Display {
            index,
            name: name.into(),
            connector: None,
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
    fn connector_names_from_older_projects_still_match() {
        let mut projector = d(1, "XGIMI TV");
        projector.connector = Some("HDMI-A-1".into());
        let displays = [d(0, "Built-in"), projector];
        assert_eq!(
            resolve(&t(0, "HDMI-A-1"), &displays).map(|d| d.index),
            Some(1)
        );
        assert_eq!(
            resolve(&t(0, "XGIMI TV"), &displays).map(|d| d.index),
            Some(1)
        );
    }

    /// A synthetic EDID base block with the given descriptors.
    fn edid(descriptors: &[[u8; 18]]) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        for (k, d) in descriptors.iter().enumerate() {
            e[54 + 18 * k..72 + 18 * k].copy_from_slice(d);
        }
        e
    }

    fn text_descriptor(tag: u8, text: &str) -> [u8; 18] {
        let mut d = [0u8; 18];
        d[3] = tag;
        let mut body = [b' '; 13];
        body[..text.len()].copy_from_slice(text.as_bytes());
        if text.len() < 13 {
            body[text.len()] = b'\n';
        }
        d[5..].copy_from_slice(&body);
        d
    }

    #[test]
    fn reads_the_monitor_name_from_edid() {
        let timing = [1u8; 18]; // a detailed timing, not a descriptor
        let e = edid(&[
            timing,
            text_descriptor(0xFF, "SERIAL123"),
            text_descriptor(0xFC, "Projector X"),
        ]);
        assert_eq!(edid_monitor_name(&e).as_deref(), Some("Projector X"));
        // Thirteen characters fill the field with no terminator.
        let e = edid(&[text_descriptor(0xFC, "ABCDEFGHIJKLM")]);
        assert_eq!(edid_monitor_name(&e).as_deref(), Some("ABCDEFGHIJKLM"));
    }

    #[test]
    fn rejects_edid_without_a_name_or_header() {
        assert_eq!(edid_monitor_name(&edid(&[[1u8; 18]])), None);
        assert_eq!(edid_monitor_name(&[0u8; 128]), None);
        assert_eq!(edid_monitor_name(&[0u8; 10]), None);
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
