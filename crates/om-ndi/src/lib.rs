// SPDX-License-Identifier: Apache-2.0
//! NDI adapter. OpenMapper never ships NDI: it uses an NDI runtime the user
//! has installed (NDI Tools or a vendor installer), loaded at run time.
//! Without one, NDI inputs and outputs report that the runtime is missing
//! and everything else keeps working. See docs/live-io.md and DECISIONS.md.
//!
//! Search order for the runtime: `OM_NDI_LIBRARY` (a file path), the
//! directories in `NDI_RUNTIME_DIR_V6` / `NDI_RUNTIME_DIR_V5`, then the
//! platform's usual install locations and library search path.
#![allow(unsafe_code)]

pub mod ffi;

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use libloading::Library;
use om_media_core::{FrameSink, LiveOpener, LiveSource, MediaError, SinkOpener, StillImage};
use om_project::{LiveInput, Publish};

#[cfg(windows)]
const LIBRARY_NAMES: &[&str] = &["Processing.NDI.Lib.x64.dll"];
#[cfg(target_os = "macos")]
const LIBRARY_NAMES: &[&str] = &["libndi.dylib"];
#[cfg(not(any(windows, target_os = "macos")))]
const LIBRARY_NAMES: &[&str] = &["libndi.so.6", "libndi.so.5", "libndi.so"];

#[cfg(target_os = "macos")]
const SYSTEM_DIRS: &[&str] = &[
    "/Library/NDI SDK for Apple/lib/macOS",
    "/usr/local/lib",
    "/opt/homebrew/lib",
];
#[cfg(not(target_os = "macos"))]
const SYSTEM_DIRS: &[&str] = &[];

/// Largest received frame (8192 × 8192).
pub const MAX_RECEIVE_PIXELS: usize = 8192 * 8192;

/// Where to look for the runtime, most specific first. On Linux and macOS
/// bare file names fall back to the platform's library search path; on
/// Windows only absolute paths are tried, because the default DLL search
/// includes the current directory and `PATH` (a planted DLL would run).
#[must_use]
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os("OM_NDI_LIBRARY") {
        out.push(PathBuf::from(p));
    }
    for var in ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"] {
        if let Some(dir) = std::env::var_os(var) {
            for name in LIBRARY_NAMES {
                out.push(PathBuf::from(&dir).join(name));
            }
        }
    }
    for dir in SYSTEM_DIRS {
        for name in LIBRARY_NAMES {
            out.push(PathBuf::from(dir).join(name));
        }
    }
    if cfg!(windows) {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            for sub in [r"NDI\NDI 6 Runtime\v6", r"NDI\NDI 5 Runtime\v5"] {
                for name in LIBRARY_NAMES {
                    out.push(PathBuf::from(&pf).join(sub).join(name));
                }
            }
        }
        out.retain(|p| p.is_absolute());
    } else {
        out.extend(LIBRARY_NAMES.iter().map(PathBuf::from));
    }
    out
}

/// Opens a library without searching the current directory or `PATH` for
/// it or its dependencies (Windows), or with the default loader rules.
fn open_library(path: &Path) -> Result<Library, libloading::Error> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{
            LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        // SAFETY: as for `Library::new`; see `Runtime::load_from`.
        unsafe {
            libloading::os::windows::Library::load_with_flags(
                path,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map(Library::from)
    }
    #[cfg(not(windows))]
    {
        // SAFETY: see `Runtime::load_from`.
        unsafe { Library::new(path) }
    }
}

/// The loaded runtime's entry points.
pub struct Runtime {
    find_create: ffi::FnFindCreate,
    find_destroy: ffi::FnFindDestroy,
    find_wait: ffi::FnFindWait,
    find_sources: ffi::FnFindSources,
    recv_create: ffi::FnRecvCreate,
    recv_destroy: ffi::FnRecvDestroy,
    recv_capture: ffi::FnRecvCapture,
    recv_free_video: ffi::FnRecvFreeVideo,
    send_create: ffi::FnSendCreate,
    send_destroy: ffi::FnSendDestroy,
    send_video: ffi::FnSendVideo,
    /// Shared source finder for discovery (created on first use).
    finder: Mutex<Option<Finder>>,
    path: PathBuf,
    // Keeps the function pointers above valid; never unloaded.
    _lib: Library,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NdiRuntime({})", self.path.display())
    }
}

struct Finder(ffi::Instance);

// SAFETY: NDI finder instances may be used from any thread; access is
// serialised by the Mutex in `Runtime`.
unsafe impl Send for Finder {}

/// Message shown when no runtime is installed.
pub const MISSING: &str = "the NDI runtime is not installed (install NDI Tools from ndi.video, \
     or set OM_NDI_LIBRARY to the NDI library)";

impl Runtime {
    /// Loads the first runtime found in `paths`.
    pub fn load_from(paths: &[PathBuf]) -> Result<Self, String> {
        let mut last = None;
        for path in paths {
            // Loading a library runs its initialisers. Only NDI runtime file
            // names in known locations (or the user's explicit override)
            // are tried, and the runtime is designed to be loaded this way.
            match open_library(path) {
                Ok(lib) => return Self::bind(lib, path.clone()),
                Err(e) => last = Some(e.to_string()),
            }
        }
        Err(match last {
            Some(e) if std::env::var_os("OM_NDI_LIBRARY").is_some() => format!("{MISSING} ({e})"),
            _ => MISSING.to_string(),
        })
    }

    fn bind(lib: Library, path: PathBuf) -> Result<Self, String> {
        macro_rules! sym {
            ($name:literal) => {
                // SAFETY: the symbol is declared in ffi.rs with the
                // signature NDI documents; the library outlives the pointer
                // (it is stored in the same struct and never unloaded).
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| format!("{}: missing {}: {e}", path.display(), $name))?
            };
        }
        let initialize: ffi::FnInitialize = sym!("NDIlib_initialize");
        // SAFETY: documented to be callable once before other calls; it
        // returns false on unsupported CPUs.
        if !unsafe { initialize() } {
            return Err("the NDI runtime does not support this CPU".into());
        }
        Ok(Self {
            find_create: sym!("NDIlib_find_create_v2"),
            find_destroy: sym!("NDIlib_find_destroy"),
            find_wait: sym!("NDIlib_find_wait_for_sources"),
            find_sources: sym!("NDIlib_find_get_current_sources"),
            recv_create: sym!("NDIlib_recv_create_v3"),
            recv_destroy: sym!("NDIlib_recv_destroy"),
            recv_capture: sym!("NDIlib_recv_capture_v2"),
            recv_free_video: sym!("NDIlib_recv_free_video_v2"),
            send_create: sym!("NDIlib_send_create"),
            send_destroy: sym!("NDIlib_send_destroy"),
            send_video: sym!("NDIlib_send_send_video_v2"),
            finder: Mutex::new(None),
            path,
            _lib: lib,
        })
    }

    /// Path of the loaded library.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Names of the NDI sources visible now, waiting up to `wait` for the
    /// first announcements.
    #[must_use]
    pub fn sources(&self, wait: Duration) -> Vec<String> {
        let mut guard = self
            .finder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            let settings = ffi::FindCreate {
                show_local_sources: true,
                p_groups: std::ptr::null(),
                p_extra_ips: std::ptr::null(),
            };
            // SAFETY: valid settings; a null result is handled.
            let f = unsafe { (self.find_create)(&raw const settings) };
            if f.is_null() {
                return Vec::new();
            }
            *guard = Some(Finder(f));
            let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: `f` is a live finder.
            unsafe { (self.find_wait)(f, ms) };
        }
        let Some(finder) = guard.as_ref() else {
            return Vec::new();
        };
        let mut n = 0u32;
        // SAFETY: `finder` is live; the returned array has `n` entries and
        // stays valid until the next call on this finder (we hold the lock).
        let list = unsafe { (self.find_sources)(finder.0, &raw mut n) };
        if list.is_null() {
            return Vec::new();
        }
        // SAFETY: as above.
        let sources = unsafe { std::slice::from_raw_parts(list, n as usize) };
        sources
            .iter()
            .filter(|s| !s.p_ndi_name.is_null())
            // SAFETY: names are NUL-terminated strings owned by the finder.
            .map(|s| {
                unsafe { CStr::from_ptr(s.p_ndi_name) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(f) = self
            .finder
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            // SAFETY: the finder is live and destroyed once.
            unsafe { (self.find_destroy)(f.0) };
        }
    }
}

/// The process-wide runtime, loaded on first use.
pub fn runtime() -> Result<&'static Runtime, String> {
    static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| Runtime::load_from(&candidates()))
        .as_ref()
        .map_err(Clone::clone)
}

fn open_err(what: &str, message: impl Into<String>) -> MediaError {
    MediaError::Open {
        path: what.to_string(),
        message: message.into(),
    }
}

/// Receives one NDI source.
pub struct NdiReceiver {
    rt: &'static Runtime,
    recv: ffi::Instance,
    source: String,
    size: Option<(u32, u32)>,
}

impl std::fmt::Debug for NdiReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NdiReceiver({})", self.source)
    }
}

// SAFETY: an NDI receiver instance may be used from any thread; this one is
// used by one thread at a time (its feed's thread).
unsafe impl Send for NdiReceiver {}

impl NdiReceiver {
    /// Connects to `source` (its full name, `MACHINE (Name)`).
    pub fn new(source: &str, stop: &AtomicBool) -> Result<Self, MediaError> {
        let label = format!("NDI \"{source}\"");
        let rt = runtime().map_err(|e| open_err(&label, e))?;
        // Wait (bounded, interruptible) for the source to be announced.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !rt
            .sources(Duration::from_millis(500))
            .iter()
            .any(|s| s == source)
        {
            if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
                return Err(open_err(&label, "no NDI source with this name is visible"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let name = CString::new(source).map_err(|_| open_err(&label, "name contains NUL"))?;
        let recv_name = c"OpenMapper";
        let settings = ffi::RecvCreateV3 {
            source_to_connect_to: ffi::Source {
                p_ndi_name: name.as_ptr(),
                p_url_address: std::ptr::null(),
            },
            color_format: ffi::RECV_COLOR_RGBX_RGBA,
            bandwidth: ffi::RECV_BANDWIDTH_HIGHEST,
            allow_video_fields: false,
            p_ndi_recv_name: recv_name.as_ptr(),
        };
        // SAFETY: settings and the strings it points to outlive the call
        // (the runtime copies them).
        let recv = unsafe { (rt.recv_create)(&raw const settings) };
        if recv.is_null() {
            return Err(open_err(
                &label,
                "the NDI runtime could not create a receiver",
            ));
        }
        Ok(Self {
            rt,
            recv,
            source: source.to_string(),
            size: None,
        })
    }
}

impl Drop for NdiReceiver {
    fn drop(&mut self) {
        // SAFETY: `recv` is live and destroyed once.
        unsafe { (self.rt.recv_destroy)(self.recv) };
    }
}

/// Copies an RGBA/RGBX/BGRA/BGRX frame into straight RGBA8.
fn frame_to_image(f: &ffi::VideoFrameV2) -> Result<StillImage, MediaError> {
    let (w, h) = (
        usize::try_from(f.xres).unwrap_or(0),
        usize::try_from(f.yres).unwrap_or(0),
    );
    let stride = usize::try_from(f.line_stride_in_bytes).unwrap_or(0);
    if w == 0 || h == 0 || f.p_data.is_null() || stride < w * 4 {
        return Err(MediaError::Stream("NDI: malformed video frame".into()));
    }
    // Sized by the sender: refuse frames that would force huge allocations.
    if w > 16_384 || h > 16_384 || w * h > MAX_RECEIVE_PIXELS {
        return Err(MediaError::Stream(format!(
            "NDI: frame {w}x{h} is too large"
        )));
    }
    let (swap, opaque) = match f.four_cc {
        ffi::FOURCC_RGBA => (false, false),
        ffi::FOURCC_RGBX => (false, true),
        ffi::FOURCC_BGRA => (true, false),
        ffi::FOURCC_BGRX => (true, true),
        other => {
            return Err(MediaError::Stream(format!(
                "NDI: unexpected pixel format {:?}",
                other.to_le_bytes()
            )));
        }
    };
    let mut rgba = vec![0u8; w * h * 4];
    for (y, out) in rgba.chunks_exact_mut(w * 4).enumerate() {
        // SAFETY: the runtime provides `yres` rows of `line_stride` bytes.
        let row = unsafe { std::slice::from_raw_parts(f.p_data.add(y * stride), w * 4) };
        out.copy_from_slice(row);
        for px in out.as_chunks_mut::<4>().0 {
            if swap {
                px.swap(0, 2);
            }
            if opaque {
                px[3] = 255;
            }
        }
    }
    StillImage::from_rgba8(
        u32::try_from(w).unwrap_or(0),
        u32::try_from(h).unwrap_or(0),
        rgba,
    )
}

impl LiveSource for NdiReceiver {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<StillImage>>, MediaError> {
        let mut frame = ffi::VideoFrameV2::default();
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: `recv` is live; only the video out-pointer is requested.
        let kind = unsafe {
            (self.rt.recv_capture)(
                self.recv,
                &raw mut frame,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                ms,
            )
        };
        match kind {
            ffi::FRAME_TYPE_VIDEO => {
                let image = frame_to_image(&frame);
                // SAFETY: the frame was filled by recv_capture and is freed once.
                unsafe { (self.rt.recv_free_video)(self.recv, &raw const frame) };
                let image = image?;
                self.size = Some((image.width(), image.height()));
                Ok(Some(Arc::new(image)))
            }
            ffi::FRAME_TYPE_ERROR => Err(MediaError::Stream("NDI: connection lost".into())),
            _ => Ok(None),
        }
    }

    fn describe(&self) -> String {
        match self.size {
            Some((w, h)) => format!("{w}×{h} NDI"),
            None => "NDI".into(),
        }
    }
}

/// Publishes frames as an NDI source.
pub struct NdiSender {
    rt: &'static Runtime,
    send: ffi::Instance,
    name: String,
}

impl std::fmt::Debug for NdiSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NdiSender({})", self.name)
    }
}

// SAFETY: an NDI sender instance may be used from any thread; this one is
// used by one thread at a time (its publisher's thread).
unsafe impl Send for NdiSender {}

impl NdiSender {
    pub fn new(name: &str) -> Result<Self, MediaError> {
        let label = format!("NDI \"{name}\"");
        let rt = runtime().map_err(|e| open_err(&label, e))?;
        let c = CString::new(name).map_err(|_| open_err(&label, "name contains NUL"))?;
        let settings = ffi::SendCreate {
            p_ndi_name: c.as_ptr(),
            p_groups: std::ptr::null(),
            clock_video: false,
            clock_audio: false,
        };
        // SAFETY: settings and its strings outlive the call.
        let send = unsafe { (rt.send_create)(&raw const settings) };
        if send.is_null() {
            return Err(open_err(
                &label,
                "the NDI runtime could not create a sender",
            ));
        }
        Ok(Self {
            rt,
            send,
            name: name.to_string(),
        })
    }
}

impl Drop for NdiSender {
    fn drop(&mut self) {
        // SAFETY: `send` is live and destroyed once; no asynchronous send
        // is outstanding (only the synchronous call is used).
        unsafe { (self.rt.send_destroy)(self.send) };
    }
}

impl FrameSink for NdiSender {
    fn send(&mut self, image: &Arc<StillImage>) -> Result<(), MediaError> {
        let w = i32::try_from(image.width()).map_err(|_| MediaError::Empty)?;
        let h = i32::try_from(image.height()).map_err(|_| MediaError::Empty)?;
        let stride = w.checked_mul(4).ok_or(MediaError::Empty)?;
        let frame = ffi::VideoFrameV2 {
            xres: w,
            yres: h,
            four_cc: ffi::FOURCC_RGBA,
            frame_rate_n: 60,
            frame_rate_d: 1,
            picture_aspect_ratio: 0.0,
            frame_format_type: ffi::FRAME_FORMAT_PROGRESSIVE,
            timecode: ffi::TIMECODE_SYNTHESIZE,
            // The runtime only reads the pixels during this call.
            p_data: image.rgba8().as_ptr().cast_mut(),
            line_stride_in_bytes: stride,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: the synchronous send reads `w*h*4` bytes from p_data
        // before returning; `image` outlives the call.
        unsafe { (self.rt.send_video)(self.send, &raw const frame) };
        Ok(())
    }

    fn describe(&self) -> String {
        format!("NDI \"{}\"", self.name)
    }
}

/// NDI inputs and outputs (when a runtime is installed).
#[derive(Debug, Clone, Copy, Default)]
pub struct NdiOpener;

impl LiveOpener for NdiOpener {
    fn supports(&self, input: &LiveInput) -> bool {
        matches!(input, LiveInput::Ndi { .. })
    }

    fn open_live(
        &self,
        input: &LiveInput,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError> {
        match input {
            LiveInput::Ndi { source } => Ok(Box::new(NdiReceiver::new(source, stop)?)),
            other => Err(open_err(&other.label(), "not an NDI input")),
        }
    }

    fn discover(&self) -> Vec<LiveInput> {
        runtime()
            .map(|rt| rt.sources(Duration::from_millis(1000)))
            .unwrap_or_default()
            .into_iter()
            .map(|source| LiveInput::Ndi { source })
            .collect()
    }
}

impl SinkOpener for NdiOpener {
    fn supports(&self, target: &Publish) -> bool {
        matches!(target, Publish::Ndi { .. })
    }

    fn open_sink(
        &self,
        target: &Publish,
        _stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        match target {
            Publish::Ndi { name } => Ok(Box::new(NdiSender::new(name)?)),
            other => Err(open_err(&other.label(), "not an NDI output")),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_runtime_is_reported_not_fatal() {
        let err = Runtime::load_from(&[PathBuf::from("/nonexistent/libndi-test.so")]).unwrap_err();
        assert!(err.contains("NDI runtime is not installed"), "{err}");
        assert!(Runtime::load_from(&[]).is_err());
    }

    #[test]
    fn candidates_include_overrides_first() {
        let c = candidates();
        assert!(!c.is_empty());
        assert!(c.iter().any(|p| p.ends_with(LIBRARY_NAMES[0])));
    }

    #[test]
    fn frames_convert_from_every_rgb_layout() {
        let mut px = [10u8, 20, 30, 40, 50, 60, 70, 80, 0, 0, 0, 0];
        let mut f = ffi::VideoFrameV2 {
            xres: 2,
            yres: 1,
            four_cc: ffi::FOURCC_BGRX,
            line_stride_in_bytes: 12,
            p_data: px.as_mut_ptr(),
            ..ffi::VideoFrameV2::default()
        };
        assert_eq!(
            frame_to_image(&f).unwrap().rgba8(),
            &[30, 20, 10, 255, 70, 60, 50, 255]
        );
        f.four_cc = ffi::FOURCC_RGBA;
        assert_eq!(frame_to_image(&f).unwrap().rgba8(), &px[..8]);
        f.four_cc = ffi::FOURCC_BGRA;
        assert_eq!(
            frame_to_image(&f).unwrap().rgba8(),
            &[30, 20, 10, 40, 70, 60, 50, 80]
        );
        f.four_cc = ffi::FOURCC_RGBX;
        assert_eq!(
            frame_to_image(&f).unwrap().rgba8(),
            &[10, 20, 30, 255, 50, 60, 70, 255]
        );
        f.four_cc = u32::from_le_bytes(*b"UYVY");
        assert!(frame_to_image(&f).is_err());
        f.four_cc = ffi::FOURCC_RGBA;
        f.line_stride_in_bytes = 4;
        assert!(frame_to_image(&f).is_err(), "stride shorter than a row");
        f.p_data = std::ptr::null_mut();
        f.line_stride_in_bytes = 12;
        assert!(frame_to_image(&f).is_err());
    }
}
