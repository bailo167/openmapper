// SPDX-License-Identifier: Apache-2.0
//! Assembles the adapters this build and operating system provide, so the
//! desktop app and the CLI offer exactly the same inputs and outputs.

use std::sync::Arc;

use om_media_core::{Adapters, LiveOpener, LiveOpeners, SinkOpener, SinkOpeners};

/// Every adapter available here: FFmpeg files, cameras and network
/// streams, plus platform video sharing where supported.
#[must_use]
pub fn adapters() -> Adapters {
    let ffmpeg = Arc::new(om_media_ffmpeg::FfmpegOpener);
    let live: Vec<Arc<dyn LiveOpener>> = vec![Arc::new(om_media_ffmpeg::FfmpegLiveOpener)];
    let sinks: Vec<Arc<dyn SinkOpener>> = vec![Arc::new(om_media_ffmpeg::FfmpegSinkOpener)];
    Adapters {
        video: Some(ffmpeg.clone()),
        audio: Some(ffmpeg),
        live: Some(Arc::new(LiveOpeners(live))),
        sinks: Some(Arc::new(SinkOpeners(sinks))),
    }
}
