// SPDX-License-Identifier: Apache-2.0
//! Syphon adapter: shares video frames with other macOS applications.
//!
//! Builds the Syphon framework from vendored source (BSD; see
//! `vendor/syphon` and THIRD_PARTY.yml) behind a small C shim. Frames cross
//! the CPU for now (BGRA8 upload on send, readback on receive).
//!
//! Syphon discovery uses distributed notifications, which macOS delivers on
//! the main thread's run loop. The desktop app runs that loop; command-line
//! tools and tests must call [`run_main_loop`] on the main thread.
//!
//! On other platforms the crate is empty.

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
pub use mac::{SyphonOpener, SyphonReceiver, SyphonSender, run_main_loop, servers};
