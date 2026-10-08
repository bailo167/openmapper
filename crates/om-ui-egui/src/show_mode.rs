// SPDX-License-Identifier: Apache-2.0
//! Show mode: the main window itself becomes the first output, fullscreen
//! on that output's display, instead of opening a separate output window
//! and minimising the control window.
//!
//! Output windows are egui immediate viewports, painted inside the main
//! window's frame. A minimised main window stops that frame: on Wayland the
//! compositor sends it no frame callbacks (the projector stays black or
//! frozen), elsewhere eframe repaints it only every 100 ms. So an
//! unattended show must keep the main window visible — as the show.

use om_output::Display;
use om_project::Output;
use om_types::OutputId;

/// What the main window must do after [`ShowMode::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    None,
    /// Go borderless fullscreen on this monitor and draw `output`.
    Enter {
        output: OutputId,
        monitor: usize,
    },
    /// Back to the windowed editor.
    Leave,
}

/// Whether the main window shows an output instead of the editor.
#[derive(Debug, Default)]
pub(crate) struct ShowMode {
    requested: bool,
    output: Option<OutputId>,
}

impl ShowMode {
    /// Enter show mode as soon as an enabled output's display is connected.
    pub(crate) fn request(&mut self) {
        self.requested = true;
    }

    /// The output the main window draws, while in show mode.
    pub(crate) fn output(&self) -> Option<OutputId> {
        self.output
    }

    /// Leaves (or cancels a pending) show mode. True if the window must
    /// return to the editor.
    pub(crate) fn leave(&mut self) -> bool {
        self.requested = false;
        self.output.take().is_some()
    }

    /// Follows the project and the connected displays. Leaves when the
    /// shown output is removed or disabled; a disconnected display keeps
    /// the show where it is (the window stays fullscreen).
    pub(crate) fn update(&mut self, outputs: &[Output], displays: &[Display]) -> Change {
        if let Some(id) = self.output {
            if outputs.iter().any(|o| o.id == id && o.enabled) {
                return Change::None;
            }
            self.output = None;
            self.requested = false;
            return Change::Leave;
        }
        if !self.requested {
            return Change::None;
        }
        let target = outputs.iter().filter(|o| o.enabled).find_map(|o| {
            let d = om_output::resolve(o.display.as_ref()?, displays)?;
            Some((o.id, d.index as usize))
        });
        match target {
            Some((output, monitor)) => {
                self.requested = false;
                self.output = Some(output);
                Change::Enter { output, monitor }
            }
            None => Change::None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn display(index: u32, name: &str) -> Display {
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

    fn output(id: &str, enabled: bool, display: Option<&str>) -> Output {
        let mut v = serde_json::json!({ "id": id, "name": "Projector", "enabled": enabled });
        if let Some(d) = display {
            v["display"] = serde_json::json!({ "name": d, "index": 0 });
        }
        serde_json::from_value(v).unwrap()
    }

    const A: &str = "0000000000000000000000000A";
    const B: &str = "0000000000000000000000000B";

    #[test]
    fn idle_until_requested() {
        let mut m = ShowMode::default();
        let outs = [output(A, true, Some("XGIMI TV"))];
        assert_eq!(m.update(&outs, &[display(0, "XGIMI TV")]), Change::None);
        assert_eq!(m.output(), None);
    }

    /// Regression: `--play` minimised the main window, which stops output
    /// windows from repainting on Wayland. It now takes over the first
    /// enabled, connected output's display.
    #[test]
    fn enters_on_first_enabled_connected_output() {
        let mut m = ShowMode::default();
        m.request();
        let outs = [
            output(A, false, Some("Left")),
            output(B, true, Some("XGIMI TV")),
        ];
        let displays = [display(0, "Laptop"), display(1, "XGIMI TV")];
        let id: OutputId = B.parse().unwrap();
        assert_eq!(
            m.update(&outs, &displays),
            Change::Enter {
                output: id,
                monitor: 1
            }
        );
        assert_eq!(m.output(), Some(id));
        assert_eq!(m.update(&outs, &displays), Change::None);
    }

    #[test]
    fn waits_for_the_projector_to_appear() {
        let mut m = ShowMode::default();
        m.request();
        let outs = [output(A, true, Some("XGIMI TV"))];
        assert_eq!(m.update(&outs, &[display(0, "Laptop")]), Change::None);
        assert!(matches!(
            m.update(&outs, &[display(0, "Laptop"), display(1, "XGIMI TV")]),
            Change::Enter { monitor: 1, .. }
        ));
    }

    #[test]
    fn stays_when_the_display_disconnects() {
        let mut m = ShowMode::default();
        m.request();
        let outs = [output(A, true, Some("XGIMI TV"))];
        m.update(&outs, &[display(0, "XGIMI TV")]);
        assert_eq!(m.update(&outs, &[]), Change::None);
        assert!(m.output().is_some());
    }

    #[test]
    fn leaves_when_the_output_is_disabled_or_removed() {
        let mut m = ShowMode::default();
        m.request();
        let displays = [display(0, "XGIMI TV")];
        m.update(&[output(A, true, Some("XGIMI TV"))], &displays);
        assert_eq!(
            m.update(&[output(A, false, Some("XGIMI TV"))], &displays),
            Change::Leave
        );
        // Not re-entered by itself.
        assert_eq!(
            m.update(&[output(A, true, Some("XGIMI TV"))], &displays),
            Change::None
        );

        m.request();
        m.update(&[output(A, true, Some("XGIMI TV"))], &displays);
        assert_eq!(m.update(&[], &displays), Change::Leave);
    }

    #[test]
    fn leave_cancels_a_pending_request() {
        let mut m = ShowMode::default();
        m.request();
        assert!(!m.leave());
        let outs = [output(A, true, Some("XGIMI TV"))];
        assert_eq!(m.update(&outs, &[display(0, "XGIMI TV")]), Change::None);

        m.request();
        m.update(&outs, &[display(0, "XGIMI TV")]);
        assert!(m.leave());
        assert_eq!(m.output(), None);
    }
}
