// SPDX-License-Identifier: Apache-2.0
//! Assembles the adapters this build and operating system provide, so the
//! desktop app and the CLI offer exactly the same inputs and outputs.

use std::sync::Arc;

use om_media_core::{Adapters, LiveOpener, LiveOpeners, SinkOpener, SinkOpeners};

/// Every adapter available here: FFmpeg files, cameras and network
/// streams, NDI (when its runtime is installed), plus platform video
/// sharing where supported (Syphon on macOS, Spout on Windows).
#[must_use]
pub fn adapters() -> Adapters {
    let ffmpeg = Arc::new(om_media_ffmpeg::FfmpegOpener);
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(unused_mut))]
    let mut live: Vec<Arc<dyn LiveOpener>> = vec![
        Arc::new(om_media_ffmpeg::FfmpegLiveOpener),
        Arc::new(om_ndi::NdiOpener),
    ];
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(unused_mut))]
    let mut sinks: Vec<Arc<dyn SinkOpener>> = vec![
        Arc::new(om_media_ffmpeg::FfmpegSinkOpener),
        Arc::new(om_ndi::NdiOpener),
    ];
    #[cfg(target_os = "macos")]
    {
        live.push(Arc::new(om_syphon::SyphonOpener));
        sinks.push(Arc::new(om_syphon::SyphonOpener));
    }
    #[cfg(windows)]
    {
        live.push(Arc::new(om_spout::SpoutOpener));
        sinks.push(Arc::new(om_spout::SpoutOpener));
    }
    Adapters {
        video: Some(ffmpeg.clone()),
        audio: Some(ffmpeg),
        live: Some(Arc::new(LiveOpeners(live))),
        sinks: Some(Arc::new(SinkOpeners(sinks))),
    }
}

/// Waits `d` while delivering the platform's main-thread events that
/// adapters depend on (Syphon discovery on macOS). Command-line tools call
/// this from the main thread instead of sleeping; GUI event loops already
/// deliver these events.
pub fn pump_events(d: std::time::Duration) {
    #[cfg(target_os = "macos")]
    om_syphon::run_main_loop(d);
    #[cfg(not(target_os = "macos"))]
    std::thread::sleep(d);
}
