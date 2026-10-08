// SPDX-License-Identifier: Apache-2.0
//! Live input through FFmpeg: capture devices (AVFoundation, DirectShow,
//! Video4Linux2) and network streams (SRT, UDP, RTP, RTSP, RTMP, TCP, HTTP).
//!
//! Every blocking FFmpeg call runs under an interrupt callback that fires
//! when the feed is stopped or when no data arrived within
//! [`READ_TIMEOUT`], so a vanished camera or a silent stream turns into an
//! error (and a reconnect) rather than a hang.

use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ffmpeg_next as ff;
use om_media_core::{LiveOpener, LiveSource, MediaError, StillImage};
use om_project::LiveInput;

use crate::interrupt::Interrupt;
use crate::video::{FfmpegVideo, LiveStep, WOULD_BLOCK_NAP};

/// Longest wait for a frame before the source counts as lost.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest wait for a stream or device to open.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(8);

/// Capture sizes asked for, best first; `None` lets the device choose.
/// Without a size, devices such as Mac cameras pick a portrait mode.
const CAMERA_SIZES: [Option<&str>; 3] = [Some("1920x1080"), Some("1280x720"), None];

/// Capture rates asked for, best first. Many devices reject FFmpeg's
/// default (29.97), so it comes last.
const CAMERA_RATES: [Option<&str>; 5] = [Some("30"), Some("25"), Some("60"), Some("15"), None];

/// A live FFmpeg input.
pub struct FfmpegLive {
    // Drops before `interrupt`, which the format context points at.
    video: FfmpegVideo,
    interrupt: Box<Interrupt>,
    describe: String,
}

impl std::fmt::Debug for FfmpegLive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FfmpegLive({})", self.describe)
    }
}

fn open_error(what: &str, message: impl std::fmt::Display) -> MediaError {
    MediaError::Open {
        path: what.to_owned(),
        message: message.to_string(),
    }
}

/// Opens `url` with input format `format` (`None`: probe) and `options`.
#[allow(unsafe_code)]
fn open_input(
    format: Option<&str>,
    url: &str,
    options: &[(&str, String)],
    interrupt: &Interrupt,
) -> Result<ff::format::context::Input, String> {
    use ff::ffi;
    let c_url = CString::new(url).map_err(|_| "URL contains a NUL byte".to_owned())?;
    let fmt = match format {
        Some(name) => {
            let c = CString::new(name).map_err(|e| e.to_string())?;
            // SAFETY: `c` is a valid NUL-terminated string; the result is a
            // pointer to a static format description (or null).
            let f = unsafe { ffi::av_find_input_format(c.as_ptr()) };
            if f.is_null() {
                return Err(format!("FFmpeg was built without the {name} input device"));
            }
            f
        }
        None => std::ptr::null(),
    };
    let mut dict = ff::Dictionary::new();
    for (k, v) in options {
        dict.set(k, v);
    }
    interrupt.arm(OPEN_TIMEOUT);
    // SAFETY: standard FFmpeg open sequence. The context is allocated here,
    // its interrupt callback points at `interrupt` (which the caller keeps
    // alive at least as long as the returned `Input`), and on every failure
    // path the context is freed exactly once (avformat_open_input frees it
    // itself when it fails).
    unsafe {
        let mut ps = ffi::avformat_alloc_context();
        if ps.is_null() {
            return Err("out of memory".into());
        }
        (*ps).interrupt_callback = interrupt.callback();
        let mut opts = dict.disown();
        let r = ffi::avformat_open_input(&raw mut ps, c_url.as_ptr(), fmt, &raw mut opts);
        drop(ff::Dictionary::own(opts));
        if r < 0 {
            return Err(interrupt.error_text(r));
        }
        let r = ffi::avformat_find_stream_info(ps, std::ptr::null_mut());
        if r < 0 {
            ffi::avformat_close_input(&raw mut ps);
            return Err(interrupt.error_text(r));
        }
        Ok(ff::format::context::Input::wrap(ps))
    }
}

impl FfmpegLive {
    /// Opens a capture device by name (or by platform id).
    pub fn open_camera(device: &str, stop: Arc<AtomicBool>) -> Result<Self, MediaError> {
        let what = format!("camera \"{device}\"");
        crate::init().map_err(|e| open_error(&what, e))?;
        let id = list_cameras()
            .into_iter()
            .find(|c| c.name == device)
            .map_or_else(|| device.to_owned(), |c| c.id);
        let (format, url, base): (&str, String, Vec<(&str, String)>) = if cfg!(target_os = "macos")
        {
            // NV12 is native to Mac cameras (FFmpeg's yuv420p default is not).
            (
                "avfoundation",
                format!("{id}:none"),
                vec![("pixel_format", "nv12".into())],
            )
        } else if cfg!(windows) {
            (
                "dshow",
                format!("video={id}"),
                vec![("rtbufsize", "64M".into())],
            )
        } else {
            ("v4l2", id, vec![])
        };
        // Devices reject modes they lack (quickly, before capture starts);
        // take the first supported size/rate combination.
        let mut last = String::new();
        for size in CAMERA_SIZES {
            for rate in CAMERA_RATES {
                let mut options = base.clone();
                if let Some(s) = size {
                    options.push(("video_size", s.into()));
                }
                if let Some(r) = rate {
                    options.push(("framerate", r.into()));
                }
                match Self::open(Some(format), &url, &options, Arc::clone(&stop), &what) {
                    Ok(s) => return Ok(s),
                    Err(e) => last = e.to_string(),
                }
                if stop.load(Ordering::Relaxed) {
                    return Err(open_error(&what, "stopped"));
                }
            }
        }
        Err(open_error(&what, last))
    }

    /// Opens a network stream (see [`om_project::STREAM_SCHEMES`]).
    pub fn open_stream(url: &str, stop: Arc<AtomicBool>) -> Result<Self, MediaError> {
        om_project::validate_stream_url(url).map_err(|e| open_error(url, e))?;
        crate::init().map_err(|e| open_error(url, e))?;
        let scheme = url.split_once("://").map_or("", |(s, _)| s);
        // Open quickly and keep latency low: small probe, no demuxer buffering.
        let mut options: Vec<(&str, String)> = vec![
            ("probesize", "1000000".into()),
            ("analyzeduration", "1000000".into()),
            ("fflags", "nobuffer".into()),
        ];
        match scheme.to_ascii_lowercase().as_str() {
            "udp" | "rtp" => {
                options.push(("overrun_nonfatal", "1".into()));
                options.push(("fifo_size", "1000000".into()));
            }
            "rtsp" => options.push(("timeout", READ_TIMEOUT.as_micros().to_string())),
            _ => {}
        }
        Self::open(None, url, &options, stop, url)
    }

    fn open(
        format: Option<&str>,
        url: &str,
        options: &[(&str, String)],
        stop: Arc<AtomicBool>,
        what: &str,
    ) -> Result<Self, MediaError> {
        let interrupt = Interrupt::new(stop);
        let input =
            open_input(format, url, options, &interrupt).map_err(|e| open_error(what, e))?;
        let video = FfmpegVideo::from_input(input, what.to_owned(), true)?;
        let d = om_media_core::MediaSource::descriptor(&video);
        let describe = match d.frame_rate {
            #[allow(clippy::cast_precision_loss)]
            Some(r) if r.den() > 0 => format!(
                "{}×{} {} @ {:.2} fps",
                d.width,
                d.height,
                d.codec,
                r.num() as f64 / r.den() as f64
            ),
            _ => format!("{}×{} {}", d.width, d.height, d.codec),
        };
        // The no-data timer runs from the last frame (here: from opening).
        interrupt.arm(READ_TIMEOUT);
        Ok(Self {
            video,
            interrupt,
            describe,
        })
    }
}

impl LiveSource for FfmpegLive {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<StillImage>>, MediaError> {
        // Network reads block inside FFmpeg, bounded by the interrupt
        // (READ_TIMEOUT since the last frame, or stop). Capture devices
        // instead answer "try again", and some ignore the interrupt, so
        // those waits happen here, returning within `timeout`.
        let start = Instant::now();
        loop {
            let lost = || {
                if self.interrupt.stop.load(Ordering::Relaxed) {
                    "stopped".to_owned()
                } else {
                    format!("no data for {} s", READ_TIMEOUT.as_secs())
                }
            };
            match self.video.step() {
                Ok(LiveStep::Frame(f)) => {
                    self.interrupt.arm(READ_TIMEOUT);
                    return Ok(Some(f.image));
                }
                Ok(LiveStep::End) => return Err(MediaError::Stream("stream ended".into())),
                Ok(LiveStep::WouldBlock) => {
                    if self.interrupt.fired() {
                        return Err(MediaError::Stream(lost()));
                    }
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(WOULD_BLOCK_NAP);
                }
                Err(e) if self.interrupt.fired() => {
                    return Err(MediaError::Stream(format!("{} ({e})", lost())));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn describe(&self) -> String {
        self.describe.clone()
    }

    fn stall_timeout(&self) -> Option<Duration> {
        // Enforced inside next_frame via the interrupt.
        None
    }
}

/// A capture device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camera {
    /// Human-readable name (stored in projects).
    pub name: String,
    /// What the platform input device opens (name, moniker or path).
    pub id: String,
}

/// Lists capture devices. Never prompts for camera permission.
#[must_use]
pub fn list_cameras() -> Vec<Camera> {
    #[cfg(target_os = "macos")]
    {
        mac::list_cameras()
    }
    #[cfg(not(target_os = "macos"))]
    {
        list_ffmpeg_devices(if cfg!(windows) { "dshow" } else { "v4l2" })
    }
}

/// Lists video capture devices of an FFmpeg input device.
#[cfg(not(target_os = "macos"))]
#[allow(unsafe_code)]
fn list_ffmpeg_devices(format: &str) -> Vec<Camera> {
    use ff::ffi;
    if crate::init().is_err() {
        return Vec::new();
    }
    let Ok(c) = CString::new(format) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: the format pointer is static (or null, checked); the list is
    // allocated by FFmpeg, only read while alive, and freed exactly once.
    unsafe {
        let fmt = ffi::av_find_input_format(c.as_ptr());
        if fmt.is_null() {
            return out;
        }
        let mut list: *mut ffi::AVDeviceInfoList = std::ptr::null_mut();
        if ffi::avdevice_list_input_sources(
            fmt,
            std::ptr::null(),
            std::ptr::null_mut(),
            &raw mut list,
        ) < 0
            || list.is_null()
        {
            if !list.is_null() {
                ffi::avdevice_free_list_devices(&raw mut list);
            }
            return out;
        }
        let n = usize::try_from((*list).nb_devices).unwrap_or(0);
        for i in 0..n {
            let info = *(*list).devices.add(i);
            if info.is_null() {
                continue;
            }
            let types = usize::try_from((*info).nb_media_types).unwrap_or(0);
            let video = types == 0
                || (0..types)
                    .any(|k| *(*info).media_types.add(k) == ffi::AVMediaType::AVMEDIA_TYPE_VIDEO);
            let text = |p: *const std::ffi::c_char| {
                if p.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            };
            let id = text((*info).device_name);
            let desc = text((*info).device_description);
            if video && !id.is_empty() {
                out.push(Camera {
                    name: if desc.is_empty() { id.clone() } else { desc },
                    id,
                });
            }
        }
        ffi::avdevice_free_list_devices(&raw mut list);
    }
    // DirectShow opens by friendly name; keep monikers only for duplicates.
    if cfg!(windows) {
        let names: Vec<String> = out.iter().map(|c| c.name.clone()).collect();
        for c in &mut out {
            if names.iter().filter(|n| **n == c.name).count() == 1 {
                c.id.clone_from(&c.name);
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Camera;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeVideo};

    /// Video capture devices in AVFoundation's order (the order FFmpeg's
    /// avfoundation device numbers them).
    #[allow(unsafe_code, deprecated)]
    pub(super) fn list_cameras() -> Vec<Camera> {
        // SAFETY: AVMediaTypeVideo is an immutable framework constant;
        // devicesWithMediaType only enumerates (no capture permission).
        let Some(kind) = (unsafe { AVMediaTypeVideo }) else {
            return Vec::new();
        };
        let devices = unsafe { AVCaptureDevice::devicesWithMediaType(kind) };
        devices
            .iter()
            .map(|d| {
                let name = unsafe { d.localizedName() }.to_string();
                Camera {
                    id: name.clone(),
                    name,
                }
            })
            .collect()
    }
}

/// [`LiveOpener`] for cameras and network streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct FfmpegLiveOpener;

impl LiveOpener for FfmpegLiveOpener {
    fn supports(&self, input: &LiveInput) -> bool {
        matches!(input, LiveInput::Camera { .. } | LiveInput::Stream { .. })
    }

    fn open_live(
        &self,
        input: &LiveInput,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError> {
        let src = match input {
            LiveInput::Camera { device } => FfmpegLive::open_camera(device, Arc::clone(stop))?,
            LiveInput::Stream { url } => FfmpegLive::open_stream(url, Arc::clone(stop))?,
            other => return Err(open_error(&other.label(), "not an FFmpeg input")),
        };
        Ok(Box::new(src))
    }

    fn discover(&self) -> Vec<LiveInput> {
        list_cameras()
            .into_iter()
            .map(|c| LiveInput::Camera { device: c.name })
            .collect()
    }
}
