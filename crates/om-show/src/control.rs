// SPDX-License-Identifier: Apache-2.0
//! Control messages from external protocols (OSC, MIDI) and the UI.
//!
//! Protocol crates translate their input into [`ControlMessage`]s; the
//! engine applies them: `Set` becomes an undoable document command,
//! actions drive the transport and show runtime.

use om_project::{ParamId, ParamValue};
use om_types::{CueId, TimelineId};

/// One control instruction.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlMessage {
    /// Set a parameter in the document.
    Set {
        param: ParamId,
        value: ParamValue,
    },
    Action(Action),
}

/// Runtime actions.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Play,
    Pause,
    /// Show clock back to zero.
    Restart,
    CueGoNext,
    CueGo(CueId),
    /// Fade cue-held values back to the document (seconds).
    CueRelease {
        fade: f64,
    },
    TimelinePlay(TimelineId),
    TimelinePause(TimelineId),
    TimelineStop(TimelineId),
    /// Jump a timeline to a position in seconds.
    TimelineSeek(TimelineId, f64),
}
