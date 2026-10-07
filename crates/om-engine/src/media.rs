// SPDX-License-Identifier: Apache-2.0
//! Media runtime: loads stills and patterns, runs video/sequence players,
//! and works out which pixels each media item shows at a given show time.
//! Front ends hand the resulting images to the renderer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use om_media_core::{DEFAULT_QUEUE, ImageSequence, StillImage, VideoOpener, VideoPlayer};
use om_project::{MediaSource, Playback, Project};
use om_time::{RationalTime, Speed};
use om_types::MediaId;

/// The show clock. Time is computed from the total elapsed monotonic time
/// since the last play/seek, so it never accumulates rounding drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Playing { since: Instant, base: RationalTime },
    Paused { at: RationalTime },
}

impl Default for Transport {
    fn default() -> Self {
        Self::Paused {
            at: RationalTime::ZERO,
        }
    }
}

impl Transport {
    /// Show time at `now`.
    #[must_use]
    pub fn time(&self, now: Instant) -> RationalTime {
        match *self {
            Self::Paused { at } => at,
            Self::Playing { since, base } => {
                let elapsed =
                    RationalTime::from_nanos(now.saturating_duration_since(since).as_nanos());
                base.checked_add(elapsed).unwrap_or(base)
            }
        }
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        matches!(self, Self::Playing { .. })
    }

    pub fn play(&mut self, now: Instant) {
        if let Self::Paused { at } = *self {
            *self = Self::Playing {
                since: now,
                base: at,
            };
        }
    }

    pub fn pause(&mut self, now: Instant) {
        let at = self.time(now);
        *self = Self::Paused { at };
    }

    /// Jumps to `t`, keeping the play/pause state.
    pub fn seek(&mut self, t: RationalTime, now: Instant) {
        *self = match self {
            Self::Playing { .. } => Self::Playing {
                since: now,
                base: t,
            },
            Self::Paused { .. } => Self::Paused { at: t },
        };
    }
}

/// Maps show time to a media item's own time: `(show - restart) × speed`,
/// wrapped (looping) or clamped to `[0, duration]`.
#[must_use]
pub fn media_time(
    show: RationalTime,
    restart_at: RationalTime,
    playback: Playback,
    duration: Option<RationalTime>,
) -> RationalTime {
    let local = show
        .checked_sub(restart_at)
        .and_then(|d| d.checked_mul_speed(playback.speed))
        .unwrap_or(RationalTime::ZERO);
    match duration.filter(|d| d.ticks() > 0) {
        Some(d) if playback.looping => local.rem_euclid(d).unwrap_or(RationalTime::ZERO),
        Some(d) => {
            if local.ticks() < 0 {
                RationalTime::ZERO
            } else if local
                .checked_cmp(d)
                .is_ok_and(|o| o != std::cmp::Ordering::Less)
            {
                d
            } else {
                local
            }
        }
        None => {
            if local.ticks() < 0 {
                RationalTime::ZERO
            } else {
                local
            }
        }
    }
}

/// Resolves a stored media path against the project directory.
#[must_use]
pub fn resolve_media_path(project_dir: Option<&Path>, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    match project_dir {
        Some(dir) if p.is_relative() => dir.join(p),
        _ => p.to_path_buf(),
    }
}

/// Converts a user-chosen file path to the form stored in the project:
/// relative (with `/` separators) when it lies under the project directory.
#[must_use]
pub fn path_for_storage(project_dir: Option<&Path>, chosen: &Path) -> String {
    let rel = project_dir
        .and_then(|dir| chosen.strip_prefix(dir).ok())
        .map(Path::to_path_buf);
    match rel {
        Some(r) => r
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        None => chosen.to_string_lossy().into_owned(),
    }
}

/// What to do with the renderer after an update.
#[derive(Debug, Default, Clone)]
pub struct MediaChanges {
    /// New pixels to upload.
    pub upload: Vec<(MediaId, Arc<StillImage>)>,
    /// Media that no longer has pixels.
    pub unload: Vec<MediaId>,
}

impl MediaChanges {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.upload.is_empty() && self.unload.is_empty()
    }
}

type Key = (MediaSource, Option<PathBuf>, (u32, u32));

enum Content {
    Still(Arc<StillImage>),
    Player {
        player: Box<VideoPlayer>,
        duration: Option<RationalTime>,
        shown: Option<RationalTime>,
    },
}

struct Entry {
    key: Key,
    content: Result<Content, String>,
    /// Show time at which this item last restarted.
    restart_at: RationalTime,
}

/// Status of one media item, for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaStatus {
    pub error: Option<String>,
    /// Size, codec and duration summary, once opened.
    pub summary: Option<String>,
    /// Current media time (time-based media).
    pub position: Option<RationalTime>,
    pub duration: Option<RationalTime>,
}

/// Keeps decoded media in sync with the project and the show clock.
pub struct MediaRuntime {
    opener: Option<Arc<dyn VideoOpener>>,
    entries: HashMap<MediaId, Entry>,
    last_retry: Option<Instant>,
}

impl std::fmt::Debug for MediaRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaRuntime")
            .field("entries", &self.entries.len())
            .field("video", &self.opener.is_some())
            .finish()
    }
}

/// How often failed loads (missing files) are retried.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

impl MediaRuntime {
    /// `opener` decodes video files; without one, video media reports an
    /// error and everything else still works.
    #[must_use]
    pub fn new(opener: Option<Arc<dyn VideoOpener>>) -> Self {
        Self {
            opener,
            entries: HashMap::new(),
            last_retry: None,
        }
    }

    /// Brings media in line with `project` and returns the pixels to show at
    /// show time `show`. Never blocks on video decoding.
    pub fn update(
        &mut self,
        project: &Project,
        project_dir: Option<&Path>,
        show: RationalTime,
    ) -> MediaChanges {
        self.update_inner(project, project_dir, show, None)
    }

    /// Like [`MediaRuntime::update`] but waits (up to `timeout` per item) for
    /// exact video frames. For offline rendering and tests.
    pub fn update_blocking(
        &mut self,
        project: &Project,
        project_dir: Option<&Path>,
        show: RationalTime,
        timeout: Duration,
    ) -> MediaChanges {
        self.update_inner(project, project_dir, show, Some(timeout))
    }

    /// Restarts a time-based item from its beginning at show time `show`.
    pub fn restart(&mut self, id: MediaId, show: RationalTime) {
        if let Some(e) = self.entries.get_mut(&id) {
            e.restart_at = show;
        }
    }

    #[must_use]
    pub fn status(
        &self,
        project: &Project,
        id: MediaId,
        show: RationalTime,
    ) -> Option<MediaStatus> {
        let e = self.entries.get(&id)?;
        let playback = project
            .media_item(id)
            .map(|m| m.playback)
            .unwrap_or_default();
        Some(match &e.content {
            Err(err) => MediaStatus {
                error: Some(err.clone()),
                summary: None,
                position: None,
                duration: None,
            },
            Ok(Content::Still(img)) => MediaStatus {
                error: None,
                summary: Some(format!("{}×{}", img.width(), img.height())),
                position: None,
                duration: None,
            },
            Ok(Content::Player {
                player, duration, ..
            }) => {
                let d = player.descriptor();
                MediaStatus {
                    error: player.error(),
                    summary: Some(format!("{}×{} {}", d.width, d.height, d.codec)),
                    position: Some(media_time(show, e.restart_at, playback, *duration)),
                    duration: *duration,
                }
            }
        })
    }

    fn update_inner(
        &mut self,
        project: &Project,
        project_dir: Option<&Path>,
        show: RationalTime,
        wait: Option<Duration>,
    ) -> MediaChanges {
        let mut changes = MediaChanges::default();
        let canvas = (project.canvas.width, project.canvas.height);
        let present: HashSet<MediaId> = project.media.iter().map(|m| m.id).collect();
        self.entries.retain(|id, _| {
            let keep = present.contains(id);
            if !keep {
                changes.unload.push(*id);
            }
            keep
        });
        let retry = self
            .last_retry
            .is_none_or(|t| t.elapsed() >= RETRY_INTERVAL);
        if retry {
            self.last_retry = Some(Instant::now());
        }
        for m in &project.media {
            let key = entry_key(&m.source, project_dir, canvas);
            let stale = self
                .entries
                .get(&m.id)
                .is_none_or(|e| e.key != key || (retry && e.content.is_err()));
            if stale {
                let content = self.load(&key);
                if content.is_err() {
                    changes.unload.push(m.id);
                }
                let restart_at = self
                    .entries
                    .get(&m.id)
                    .map_or(RationalTime::ZERO, |e| e.restart_at);
                self.entries.insert(
                    m.id,
                    Entry {
                        key,
                        content,
                        restart_at,
                    },
                );
                if let Some(Entry {
                    content: Ok(Content::Still(img)),
                    ..
                }) = self.entries.get(&m.id)
                {
                    changes.upload.push((m.id, Arc::clone(img)));
                }
            }
            let Some(entry) = self.entries.get_mut(&m.id) else {
                continue;
            };
            if let Ok(Content::Player {
                player,
                duration,
                shown,
            }) = &mut entry.content
            {
                let t = media_time(show, entry.restart_at, m.playback, *duration);
                let frame = match wait {
                    Some(timeout) => player.frame_at_blocking(t, timeout),
                    None => player.frame_at(t),
                };
                if let Some(f) = frame
                    && *shown != Some(f.pts)
                {
                    *shown = Some(f.pts);
                    changes.upload.push((m.id, Arc::clone(&f.image)));
                }
            }
        }
        changes.upload.sort_by_key(|(id, _)| *id);
        changes.unload.sort();
        changes.unload.dedup();
        changes
    }

    fn load(&self, key: &Key) -> Result<Content, String> {
        match (&key.0, &key.1) {
            (MediaSource::Image { .. }, Some(path)) => StillImage::load(path)
                .map(|i| Content::Still(Arc::new(i)))
                .map_err(|e| e.to_string()),
            (MediaSource::Pattern { pattern }, _) => {
                StillImage::pattern(*pattern, key.2.0.max(1), key.2.1.max(1))
                    .map(|i| Content::Still(Arc::new(i)))
                    .map_err(|e| e.to_string())
            }
            (MediaSource::Sequence { rate, .. }, Some(path)) => {
                let seq = ImageSequence::open(path, *rate).map_err(|e| e.to_string())?;
                Ok(player_content(seq))
            }
            (MediaSource::Video { .. }, Some(path)) => {
                let opener = self
                    .opener
                    .as_ref()
                    .ok_or("video playback is not available in this build")?;
                if !path.is_file() {
                    return Err(format!("{}: file not found", path.display()));
                }
                let src = opener.open_video(path).map_err(|e| e.to_string())?;
                Ok(player_content(src))
            }
            (src, None) => Err(format!("{src:?}: unresolved path")),
        }
    }
}

fn player_content<S: om_media_core::MediaSource + 'static>(source: S) -> Content {
    let duration = source.descriptor().duration;
    Content::Player {
        player: Box::new(VideoPlayer::spawn(
            source,
            RationalTime::ZERO,
            DEFAULT_QUEUE,
        )),
        duration,
        shown: None,
    }
}

fn entry_key(source: &MediaSource, project_dir: Option<&Path>, canvas: (u32, u32)) -> Key {
    match source {
        MediaSource::Pattern { .. } => (source.clone(), None, canvas),
        _ => (
            source.clone(),
            source.path().map(|p| resolve_media_path(project_dir, p)),
            (0, 0),
        ),
    }
}

/// Speed helper re-exported for front ends.
#[must_use]
pub fn speed_percent(speed: Speed) -> f64 {
    speed.as_f64() * 100.0
}

#[cfg(test)]
mod tests;
