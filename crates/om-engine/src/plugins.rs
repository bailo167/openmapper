// SPDX-License-Identifier: Apache-2.0
//! Plugin filters on media: each media item's enabled plugins run in order
//! on its frames, each on its own [`PluginRunner`] thread. Callers upload
//! the processed frames that [`PluginStage::poll`] returns. The original
//! frame is shown until a chain has produced output, and whenever a chain
//! cannot run (load error, plugin disabled after faults), so a bad plugin
//! never blanks the show or blocks rendering.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use om_media_core::StillImage;
use om_plugin_host::runner::{Job, PluginRunner, RunnerState};
use om_plugin_host::{Plugin, PluginHost};
use om_project::{PluginUse, Project};
use om_types::MediaId;

use crate::media::resolve_media_path;

struct StageRun {
    name: String,
    runner: PluginRunner,
    params: Vec<f32>,
    /// Sequence of the previous stage's output last fed to this stage.
    fed: u64,
}

struct Chain {
    config: Vec<PluginUse>,
    stages: Vec<StageRun>,
    /// Load errors, one per plugin that could not be used.
    errors: Vec<String>,
    /// Last final output sequence handed to the caller.
    delivered: u64,
    produced: bool,
}

/// Status of one plugin on a media item, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginStatus {
    pub name: String,
    pub state: String,
    pub ok: bool,
}

/// Runs plugin filters for every media item that has them.
#[derive(Default)]
pub struct PluginStage {
    host: Option<Arc<PluginHost>>,
    host_error: Option<String>,
    modules: HashMap<PathBuf, Result<Plugin, String>>,
    chains: HashMap<MediaId, Chain>,
}

impl std::fmt::Debug for PluginStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PluginStage({} chains)", self.chains.len())
    }
}

impl PluginStage {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn host(&mut self) -> Option<Arc<PluginHost>> {
        if self.host.is_none() && self.host_error.is_none() {
            match PluginHost::new() {
                Ok(h) => self.host = Some(Arc::new(h)),
                Err(e) => self.host_error = Some(e.to_string()),
            }
        }
        self.host.clone()
    }

    fn module(&mut self, path: &Path) -> Result<Plugin, String> {
        if let Some(m) = self.modules.get(path) {
            return m.clone();
        }
        let loaded = match self.host() {
            None => Err(self
                .host_error
                .clone()
                .unwrap_or_else(|| "plugin host unavailable".into())),
            Some(h) => std::fs::read(path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|bytes| h.load(&bytes).map_err(|e| e.to_string())),
        };
        self.modules.insert(path.to_owned(), loaded.clone());
        loaded
    }

    /// The manifest of the plugin file at `path` (resolved), once loaded.
    #[must_use]
    pub fn manifest(&self, path: &Path) -> Option<&om_plugin_api::Manifest> {
        self.modules.get(path)?.as_ref().ok().map(|p| &p.manifest)
    }

    /// Forgets loaded plugin files so changed files are read again.
    pub fn reload(&mut self) {
        self.modules.clear();
        self.chains.clear();
    }

    /// Starts and stops chains to match the project.
    pub fn sync(&mut self, project: &Project, project_dir: Option<&Path>) {
        self.chains.retain(|id, chain| {
            project
                .media
                .iter()
                .any(|m| m.id == *id && m.plugins == chain.config)
        });
        for m in &project.media {
            if m.plugins.iter().all(|p| !p.enabled) || self.chains.contains_key(&m.id) {
                continue;
            }
            let mut stages = Vec::new();
            let mut errors = Vec::new();
            for use_ in m.plugins.iter().filter(|p| p.enabled) {
                let path = resolve_media_path(project_dir, &use_.path);
                match (self.module(&path), self.host()) {
                    (Ok(plugin), Some(host)) => {
                        let given: BTreeMap<String, f32> = use_
                            .params
                            .iter()
                            .map(|(k, v)| {
                                #[allow(clippy::cast_possible_truncation)]
                                (k.clone(), v.get() as f32)
                            })
                            .collect();
                        let params = plugin.manifest.resolve_params(&given);
                        stages.push(StageRun {
                            name: plugin.manifest.name.clone(),
                            runner: PluginRunner::spawn(host, plugin),
                            params,
                            fed: 0,
                        });
                    }
                    (Err(e), _) => errors.push(e),
                    (Ok(_), None) => errors.push("plugin host unavailable".into()),
                }
            }
            self.chains.insert(
                m.id,
                Chain {
                    config: m.plugins.clone(),
                    stages,
                    errors,
                    delivered: 0,
                    produced: false,
                },
            );
        }
    }

    /// Whether frames of `id` go through plugins (callers then upload what
    /// [`Self::poll`] returns instead of the original, once
    /// [`Self::shows_original`] is false).
    #[must_use]
    pub fn has_chain(&self, id: MediaId) -> bool {
        self.chains.get(&id).is_some_and(|c| !c.stages.is_empty())
    }

    /// True while the original frame should be shown: before the chain's
    /// first output, or when it cannot run.
    #[must_use]
    pub fn shows_original(&self, id: MediaId) -> bool {
        self.chains.get(&id).is_none_or(|c| {
            !c.produced
                || !c.errors.is_empty()
                || c.stages.is_empty()
                || c.stages
                    .iter()
                    .any(|s| matches!(s.runner.state(), RunnerState::Disabled { .. }))
        })
    }

    /// True if `id`'s chain cannot produce output (load error or a plugin
    /// disabled after repeated faults).
    #[must_use]
    pub fn failed(&self, id: MediaId) -> bool {
        self.chains.get(&id).is_some_and(|c| {
            !c.errors.is_empty()
                || c.stages
                    .iter()
                    .any(|s| matches!(s.runner.state(), RunnerState::Disabled { .. }))
        })
    }

    /// Feeds a new frame of `id` into its chain (never blocks).
    pub fn submit(&mut self, id: MediaId, image: &StillImage, time: f64) {
        let Some(chain) = self.chains.get_mut(&id) else {
            return;
        };
        let Some(first) = chain.stages.first_mut() else {
            return;
        };
        first.runner.submit(Job {
            rgba: Arc::new(image.rgba8().to_vec()),
            width: image.width(),
            height: image.height(),
            params: first.params.clone(),
            time,
        });
    }

    /// Advances chains and returns finished frames to upload.
    pub fn poll(&mut self) -> Vec<(MediaId, StillImage)> {
        let mut out = Vec::new();
        for (id, chain) in &mut self.chains {
            // Hand each stage's newest output to the next stage.
            for k in 1..chain.stages.len() {
                let (before, after) = chain.stages.split_at_mut(k);
                let (prev, next) = (&before[k - 1], &mut after[0]);
                if let Some(o) = prev.runner.output().filter(|o| o.seq > next.fed) {
                    next.fed = o.seq;
                    next.runner.submit(Job {
                        rgba: o.rgba,
                        width: o.width,
                        height: o.height,
                        params: next.params.clone(),
                        time: 0.0,
                    });
                }
            }
            let Some(last) = chain.stages.last() else {
                continue;
            };
            if let Some(o) = last.runner.output().filter(|o| o.seq > chain.delivered) {
                chain.delivered = o.seq;
                if let Ok(img) = StillImage::from_rgba8(o.width, o.height, o.rgba.to_vec()) {
                    chain.produced = true;
                    out.push((*id, img));
                }
            }
        }
        out
    }

    /// Per-plugin status of `id`'s chain.
    #[must_use]
    pub fn status(&self, id: MediaId) -> Vec<PluginStatus> {
        let Some(chain) = self.chains.get(&id) else {
            return Vec::new();
        };
        let mut v: Vec<PluginStatus> = chain
            .errors
            .iter()
            .map(|e| PluginStatus {
                name: "plugin".into(),
                state: e.clone(),
                ok: false,
            })
            .collect();
        for s in &chain.stages {
            let (state, ok) = match s.runner.state() {
                RunnerState::Running => {
                    (format!("running ({} frames)", s.runner.processed()), true)
                }
                RunnerState::Faulted { error, count } => {
                    (format!("restarting after: {error} ({count})"), false)
                }
                RunnerState::Disabled { error } => (format!("disabled: {error}"), false),
            };
            v.push(PluginStatus {
                name: s.name.clone(),
                state,
                ok,
            });
        }
        v
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::{Duration, Instant};

    use om_project::{Media, MediaSource, PatternKind};

    use super::*;

    const INVERT: &str = include_str!("../../om-plugin-host/tests/plugins/invert.wat");

    fn project(plugins: Vec<PluginUse>) -> Project {
        let mut p = Project::new("plugins");
        p.media.push(Media {
            id: MediaId::from_u128(1),
            name: "m".into(),
            source: MediaSource::Pattern {
                pattern: PatternKind::White,
            },
            playback: Default::default(),
            plugins,
            extensions: Default::default(),
        });
        p
    }

    fn use_(path: &str) -> PluginUse {
        PluginUse {
            path: path.into(),
            enabled: true,
            params: BTreeMap::new(),
        }
    }

    fn wait_output(stage: &mut PluginStage) -> Vec<(MediaId, StillImage)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = stage.poll();
            if !out.is_empty() || Instant::now() > deadline {
                return out;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn chains_process_in_order_and_fall_back_on_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("invert.wat"), INVERT).unwrap();
        let id = MediaId::from_u128(1);
        let img = StillImage::from_rgba8(2, 1, vec![10, 20, 30, 255, 200, 100, 0, 128]).unwrap();

        // One plugin: inverted.
        let mut stage = PluginStage::new();
        stage.sync(&project(vec![use_("invert.wat")]), Some(dir.path()));
        assert!(stage.has_chain(id) && stage.shows_original(id));
        stage.submit(id, &img, 0.0);
        let out = wait_output(&mut stage);
        assert_eq!(out[0].1.rgba8(), &[245, 235, 225, 255, 55, 155, 255, 128]);
        assert!(!stage.shows_original(id));
        assert!(stage.status(id)[0].ok);

        // Two inverts cancel out.
        stage.sync(
            &project(vec![use_("invert.wat"), use_("invert.wat")]),
            Some(dir.path()),
        );
        stage.submit(id, &img, 0.0);
        let out = wait_output(&mut stage);
        assert_eq!(out[0].1.rgba8(), img.rgba8());

        // A missing file: the original keeps showing and the error is reported.
        stage.sync(&project(vec![use_("missing.wasm")]), Some(dir.path()));
        assert!(!stage.has_chain(id));
        assert!(stage.shows_original(id));
        assert!(!stage.status(id)[0].ok);

        // Removing plugins removes the chain.
        stage.sync(&project(Vec::new()), Some(dir.path()));
        assert!(stage.status(id).is_empty());
    }
}
