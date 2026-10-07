// SPDX-License-Identifier: Apache-2.0
//! Keeps one [`PublishFeed`] running per publish target of every output.

use std::collections::HashMap;
use std::sync::Arc;

use om_media_core::{PublishFeed, SinkOpener, SinkState, SinkStats, StillImage};
use om_project::{Project, Publish};
use om_types::OutputId;

/// Status of one publish target, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishStatus {
    pub state: SinkState,
    pub stats: SinkStats,
}

/// Publishing runtime: follows the project's outputs and hands each target
/// the newest output frame.
#[derive(Default)]
pub struct PublishRuntime {
    opener: Option<Arc<dyn SinkOpener>>,
    feeds: HashMap<(OutputId, Publish), PublishFeed>,
}

impl std::fmt::Debug for PublishRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublishRuntime({} feeds)", self.feeds.len())
    }
}

impl PublishRuntime {
    #[must_use]
    pub fn new(opener: Option<Arc<dyn SinkOpener>>) -> Self {
        Self {
            opener,
            feeds: HashMap::new(),
        }
    }

    /// Whether this build can publish `target`.
    #[must_use]
    pub fn supports(&self, target: &Publish) -> bool {
        self.opener.as_ref().is_some_and(|o| o.supports(target))
    }

    /// Starts and stops feeds to match `project`.
    pub fn sync(&mut self, project: &Project) {
        let Some(opener) = &self.opener else { return };
        let wanted: Vec<(OutputId, Publish)> = project
            .outputs
            .iter()
            .flat_map(|o| o.publish.iter().map(move |p| (o.id, p.clone())))
            .collect();
        self.feeds.retain(|k, _| wanted.contains(k));
        for key in wanted {
            let target = key.1.clone();
            self.feeds
                .entry(key)
                .or_insert_with(|| PublishFeed::spawn(Arc::clone(opener), target));
        }
    }

    /// True if any target wants frames (so the caller can skip readback).
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.feeds.is_empty()
    }

    /// Offers the newest frame of `output` to its targets.
    pub fn submit(&self, output: OutputId, frame: &Arc<StillImage>) {
        for ((o, _), feed) in &self.feeds {
            if *o == output {
                feed.submit(Arc::clone(frame));
            }
        }
    }

    /// Offers the same frame to every output's targets.
    pub fn submit_all(&self, frame: &Arc<StillImage>) {
        for feed in self.feeds.values() {
            feed.submit(Arc::clone(frame));
        }
    }

    #[must_use]
    pub fn status(&self, output: OutputId, target: &Publish) -> Option<PublishStatus> {
        let feed = self.feeds.get(&(output, target.clone()))?;
        Some(PublishStatus {
            state: feed.state(),
            stats: feed.stats(),
        })
    }
}
