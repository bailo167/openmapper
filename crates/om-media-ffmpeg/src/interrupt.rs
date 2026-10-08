// SPDX-License-Identifier: Apache-2.0
//! FFmpeg interrupt callbacks: bound every blocking network/device call by a
//! deadline and a stop flag.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ffmpeg_next as ff;

/// Interrupt state polled by FFmpeg during blocking calls. Must stay at a
/// fixed address (boxed) for as long as the format context using it.
pub(crate) struct Interrupt {
    pub(crate) stop: Arc<AtomicBool>,
    origin: Instant,
    /// Nanoseconds after `origin` at which the current call gives up.
    deadline: AtomicU64,
}

impl Interrupt {
    pub(crate) fn new(stop: Arc<AtomicBool>) -> Box<Self> {
        Box::new(Self {
            stop,
            origin: Instant::now(),
            deadline: AtomicU64::new(0),
        })
    }

    /// Lets the next blocking call run for up to `timeout`.
    pub(crate) fn arm(&self, timeout: Duration) {
        let at = self.origin.elapsed().saturating_add(timeout);
        self.deadline.store(
            u64::try_from(at.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn fired(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
            || u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
                > self.deadline.load(Ordering::Relaxed)
    }

    /// The callback struct to install on a format context.
    pub(crate) fn callback(&self) -> ff::ffi::AVIOInterruptCB {
        ff::ffi::AVIOInterruptCB {
            callback: Some(interrupted),
            opaque: std::ptr::from_ref(self).cast_mut().cast(),
        }
    }

    /// Explains a failed FFmpeg call, preferring stop/timeout over FFmpeg's
    /// generic "Immediate exit requested".
    pub(crate) fn error_text(&self, code: std::ffi::c_int) -> String {
        if self.stop.load(Ordering::Relaxed) {
            "stopped".into()
        } else if self.fired() {
            "timed out".into()
        } else {
            ff::Error::from(code).to_string()
        }
    }
}

#[allow(unsafe_code)]
unsafe extern "C" fn interrupted(opaque: *mut std::ffi::c_void) -> std::ffi::c_int {
    // SAFETY: `opaque` was produced by `Interrupt::callback` from a boxed
    // `Interrupt` that its owner keeps alive (and unmoved) for as long as
    // the format context that calls back.
    let state = unsafe { &*opaque.cast::<Interrupt>() };
    std::ffi::c_int::from(state.fired())
}
