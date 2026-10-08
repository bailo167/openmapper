// SPDX-License-Identifier: Apache-2.0
//! The set of adapters a front end provides.

use std::sync::Arc;

use crate::{AudioOpener, LiveOpener, SinkOpener, VideoOpener};

/// The platform adapters a front end provides (any may be missing).
#[derive(Clone, Default)]
pub struct Adapters {
    pub video: Option<Arc<dyn VideoOpener>>,
    pub audio: Option<Arc<dyn AudioOpener>>,
    pub live: Option<Arc<dyn LiveOpener>>,
    pub sinks: Option<Arc<dyn SinkOpener>>,
}

impl std::fmt::Debug for Adapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adapters")
            .field("video", &self.video.is_some())
            .field("audio", &self.audio.is_some())
            .field("live", &self.live.is_some())
            .field("sinks", &self.sinks.is_some())
            .finish()
    }
}
