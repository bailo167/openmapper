// SPDX-License-Identifier: Apache-2.0
//! macOS implementation over the C shim in `shim.m`.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use om_media_core::{FrameSink, LiveOpener, LiveSource, MediaError, SinkOpener, StillImage};
use om_project::{LiveInput, Publish};

type ListCb = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char);

unsafe extern "C" {
    fn om_syphon_server_new(name: *const c_char) -> *mut c_void;
    fn om_syphon_server_publish(s: *mut c_void, bgra: *const u8, w: u32, h: u32) -> c_int;
    fn om_syphon_server_free(s: *mut c_void);
    fn om_syphon_run_loop(seconds: f64);
    fn om_syphon_is_main_thread() -> c_int;
    fn om_syphon_list(cb: ListCb, ctx: *mut c_void);
    fn om_syphon_client_new(name: *const c_char, app: *const c_char) -> *mut c_void;
    fn om_syphon_client_next(
        c: *mut c_void,
        data: *mut *const u8,
        w: *mut u32,
        h: *mut u32,
    ) -> c_int;
    fn om_syphon_client_free(c: *mut c_void);
}

fn cstring(s: &str) -> Result<CString, MediaError> {
    CString::new(s).map_err(|_| MediaError::Stream("Syphon: name contains a NUL byte".into()))
}

/// Runs the main thread's run loop for `d` (call from the main thread in
/// command-line tools and tests; the desktop app's event loop does this).
/// Does nothing on other threads.
pub fn run_main_loop(d: Duration) {
    // SAFETY: no preconditions.
    if unsafe { om_syphon_is_main_thread() } == 1 {
        // SAFETY: runs the current (main) thread's run loop.
        unsafe { om_syphon_run_loop(d.as_secs_f64()) };
    }
}

unsafe extern "C" fn collect(ctx: *mut c_void, name: *const c_char, app: *const c_char) {
    let text = |p: *const c_char| {
        if p.is_null() {
            String::new()
        } else {
            // SAFETY: non-null, NUL-terminated and valid during the callback.
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    if ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the `Vec` passed by `servers`, alive for the call.
    let out = unsafe { &mut *ctx.cast::<Vec<(String, String)>>() };
    out.push((text(name), text(app)));
}

/// Announced Syphon servers as `(server name, application name)`.
#[must_use]
pub fn servers() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    // SAFETY: `collect` matches the callback signature; `out` outlives the
    // synchronous call.
    unsafe { om_syphon_list(collect, (&raw mut out).cast::<c_void>()) };
    out
}

fn rgba_to_bgra(src: &[u8], dst: &mut Vec<u8>) {
    dst.clear();
    dst.reserve(src.len());
    for px in src.as_chunks::<4>().0 {
        dst.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
}

/// Publishes frames as a Syphon server.
pub struct SyphonSender {
    handle: *mut c_void,
    name: String,
    scratch: Vec<u8>,
    size: Option<(u32, u32)>,
}

// SAFETY: the Syphon server and its Metal objects may be used from any
// thread; this one is used by one thread at a time (its publisher thread).
unsafe impl Send for SyphonSender {}

impl std::fmt::Debug for SyphonSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SyphonSender({})", self.name)
    }
}

impl SyphonSender {
    pub fn new(name: &str) -> Result<Self, MediaError> {
        let c = cstring(name)?;
        // SAFETY: valid NUL-terminated string; NULL result handled.
        let handle = unsafe { om_syphon_server_new(c.as_ptr()) };
        if handle.is_null() {
            return Err(MediaError::Open {
                path: format!("Syphon \"{name}\""),
                message: "could not start a Syphon server (no Metal device?)".into(),
            });
        }
        Ok(Self {
            handle,
            name: name.to_string(),
            scratch: Vec::new(),
            size: None,
        })
    }
}

impl Drop for SyphonSender {
    fn drop(&mut self) {
        // SAFETY: `handle` came from om_syphon_server_new and is freed once.
        unsafe { om_syphon_server_free(self.handle) };
    }
}

impl FrameSink for SyphonSender {
    fn send(&mut self, image: &Arc<StillImage>) -> Result<(), MediaError> {
        rgba_to_bgra(image.rgba8(), &mut self.scratch);
        let (w, h) = (image.width(), image.height());
        // SAFETY: `scratch` holds w*h*4 bytes; the handle is live.
        let r = unsafe { om_syphon_server_publish(self.handle, self.scratch.as_ptr(), w, h) };
        if r != 0 {
            return Err(MediaError::Stream(format!(
                "Syphon: publishing failed ({r})"
            )));
        }
        self.size = Some((w, h));
        Ok(())
    }

    fn describe(&self) -> String {
        match self.size {
            Some((w, h)) => format!("Syphon \"{}\" {w}×{h}", self.name),
            None => format!("Syphon \"{}\"", self.name),
        }
    }
}

/// Receives frames from a Syphon server.
pub struct SyphonReceiver {
    handle: *mut c_void,
    label: String,
    size: Option<(u32, u32)>,
}

// SAFETY: Syphon clients may be used from any thread; this one is used by
// one thread at a time (its feed thread).
unsafe impl Send for SyphonReceiver {}

impl std::fmt::Debug for SyphonReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SyphonReceiver({})", self.label)
    }
}

impl SyphonReceiver {
    /// Connects to the first server matching `server` and `app` (empty
    /// matches any).
    pub fn new(server: &str, app: &str) -> Result<Self, MediaError> {
        let label = LiveInput::Syphon {
            server: server.into(),
            app: app.into(),
        }
        .label();
        let (s, a) = (cstring(server)?, cstring(app)?);
        // SAFETY: valid NUL-terminated strings; NULL result handled.
        let handle = unsafe { om_syphon_client_new(s.as_ptr(), a.as_ptr()) };
        if handle.is_null() {
            return Err(MediaError::Open {
                path: label,
                message: "no matching Syphon server is running".into(),
            });
        }
        Ok(Self {
            handle,
            label,
            size: None,
        })
    }
}

impl Drop for SyphonReceiver {
    fn drop(&mut self) {
        // SAFETY: `handle` came from om_syphon_client_new and is freed once.
        unsafe { om_syphon_client_free(self.handle) };
    }
}

impl LiveSource for SyphonReceiver {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<StillImage>>, MediaError> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut data: *const u8 = std::ptr::null();
            let (mut w, mut h) = (0u32, 0u32);
            // SAFETY: out-pointers are valid; the handle is live.
            let r = unsafe {
                om_syphon_client_next(self.handle, &raw mut data, &raw mut w, &raw mut h)
            };
            match r {
                1 if !data.is_null() => {
                    let len = w as usize * h as usize * 4;
                    // SAFETY: the shim returns `len` bytes valid until the
                    // next call on this client.
                    let bgra = unsafe { std::slice::from_raw_parts(data, len) };
                    let mut rgba = Vec::with_capacity(len);
                    rgba_to_bgra(bgra, &mut rgba); // the swap is symmetric
                    self.size = Some((w, h));
                    return StillImage::from_rgba8(w, h, rgba).map(|i| Some(Arc::new(i)));
                }
                -1 => return Err(MediaError::Stream("Syphon: the server closed".into())),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn describe(&self) -> String {
        match self.size {
            Some((w, h)) => format!("{w}×{h} Syphon"),
            None => "Syphon".into(),
        }
    }

    /// Syphon servers publish only when their picture changes; a server
    /// that quits is reported by `next_frame`.
    fn stall_timeout(&self) -> Option<Duration> {
        None
    }
}

/// Syphon inputs and outputs.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyphonOpener;

impl LiveOpener for SyphonOpener {
    fn supports(&self, input: &LiveInput) -> bool {
        matches!(input, LiveInput::Syphon { .. })
    }

    fn open_live(
        &self,
        input: &LiveInput,
        _stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError> {
        match input {
            LiveInput::Syphon { server, app } => Ok(Box::new(SyphonReceiver::new(server, app)?)),
            other => Err(MediaError::Open {
                path: other.label(),
                message: "not a Syphon input".into(),
            }),
        }
    }

    fn discover(&self) -> Vec<LiveInput> {
        // From the main thread (command-line tools), let announcements
        // arrive first; elsewhere the app's event loop delivers them.
        run_main_loop(Duration::from_millis(500));
        servers()
            .into_iter()
            .map(|(server, app)| LiveInput::Syphon { server, app })
            .collect()
    }
}

impl SinkOpener for SyphonOpener {
    fn supports(&self, target: &Publish) -> bool {
        matches!(target, Publish::Syphon { .. })
    }

    fn open_sink(
        &self,
        target: &Publish,
        _stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        match target {
            Publish::Syphon { name } => Ok(Box::new(SyphonSender::new(name)?)),
            other => Err(MediaError::Open {
                path: other.label(),
                message: "not a Syphon output".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_swap_is_symmetric() {
        let mut once = Vec::new();
        rgba_to_bgra(&[1, 2, 3, 4, 5, 6, 7, 8], &mut once);
        assert_eq!(once, [3, 2, 1, 4, 7, 6, 5, 8]);
        let mut twice = Vec::new();
        rgba_to_bgra(&once, &mut twice);
        assert_eq!(twice, [1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
