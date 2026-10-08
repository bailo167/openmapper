// SPDX-License-Identifier: Apache-2.0
//! The typed command bus.
//!
//! Every mutation of a [`Project`] — from the UI, CLI, OSC, MIDI or a test —
//! is a serialisable [`Command`]. Applying a command validates it first and
//! then mutates atomically: on error the project is untouched. Each applied
//! command yields its exact inverse, which is what undo replays, and a list of
//! [`Event`]s describing what changed.
//!
//! [`Document`] wraps a project with revision tracking and undo/redo history.

mod command;
mod document;
pub mod params;

pub use command::{Applied, Command, CommandError, Event, MediaPath};
pub use document::{CommandResult, Document, HistoryError};

pub use om_project::Project;

#[cfg(test)]
mod tests;
