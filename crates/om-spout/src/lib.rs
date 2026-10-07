// SPDX-License-Identifier: Apache-2.0
//! Spout adapter: shares video frames with other Windows applications.
//!
//! Implements the Spout 2 conventions (sender-name directory in shared
//! memory, per-sender description, texture access mutex, frame-count
//! semaphore, legacy DXGI shared textures), so OpenMapper interoperates
//! with any Spout 2 application without linking the Spout SDK. See
//! docs/live-io.md.
//!
//! On other platforms only the platform-independent [`format`] module is
//! built.

pub mod format;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{SpoutOpener, SpoutReceiver, SpoutSender, sender_names};
