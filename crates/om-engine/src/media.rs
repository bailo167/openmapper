// SPDX-License-Identifier: Apache-2.0
//! Loads the pixels for a project's media items and keeps them in sync with
//! the document. Front ends hand the loaded images to the renderer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use om_media_core::StillImage;
use om_project::{MediaSource, Project};
use om_types::MediaId;

#[derive(Debug)]
struct Entry {
    key: (MediaSource, Option<PathBuf>, (u32, u32)),
    result: Result<Arc<StillImage>, String>,
}

/// What changed in a [`MediaLibrary::sync`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MediaChanges {
    /// Newly loaded or reloaded images (upload these).
    pub loaded: Vec<MediaId>,
    /// Media that no longer has pixels (removed from the project or failed).
    pub unloaded: Vec<MediaId>,
}

impl MediaChanges {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty() && self.unloaded.is_empty()
    }
}

/// Cache of decoded media keyed by id and source.
#[derive(Debug, Default)]
pub struct MediaLibrary {
    entries: HashMap<MediaId, Entry>,
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

impl MediaLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads anything new or changed and drops media no longer in the project.
    /// Patterns are generated at the canvas size. Failed loads are remembered
    /// (see [`MediaLibrary::error`]) and retried by [`MediaLibrary::retry_failed`].
    pub fn sync(&mut self, project: &Project, project_dir: Option<&Path>) -> MediaChanges {
        let mut changes = MediaChanges::default();
        let canvas = (project.canvas.width, project.canvas.height);
        let present: std::collections::HashSet<MediaId> =
            project.media.iter().map(|m| m.id).collect();
        self.entries.retain(|id, _| {
            let keep = present.contains(id);
            if !keep {
                changes.unloaded.push(*id);
            }
            keep
        });
        for m in &project.media {
            let key = entry_key(&m.source, project_dir, canvas);
            if self.entries.get(&m.id).is_some_and(|e| e.key == key) {
                continue;
            }
            let result = load(&key);
            if result.is_ok() {
                changes.loaded.push(m.id);
            } else {
                changes.unloaded.push(m.id);
            }
            self.entries.insert(m.id, Entry { key, result });
        }
        changes.loaded.sort();
        changes.unloaded.sort();
        changes
    }

    /// Retries every failed load (e.g. after the user reconnects a drive).
    pub fn retry_failed(&mut self) -> MediaChanges {
        let mut changes = MediaChanges::default();
        for (id, e) in &mut self.entries {
            if e.result.is_err() {
                e.result = load(&e.key);
                if e.result.is_ok() {
                    changes.loaded.push(*id);
                }
            }
        }
        changes.loaded.sort();
        changes
    }

    #[must_use]
    pub fn image(&self, id: MediaId) -> Option<&Arc<StillImage>> {
        self.entries.get(&id).and_then(|e| e.result.as_ref().ok())
    }

    /// Why a media item has no pixels, if it failed to load.
    #[must_use]
    pub fn error(&self, id: MediaId) -> Option<&str> {
        self.entries
            .get(&id)
            .and_then(|e| e.result.as_ref().err())
            .map(String::as_str)
    }
}

fn entry_key(
    source: &MediaSource,
    project_dir: Option<&Path>,
    canvas: (u32, u32),
) -> (MediaSource, Option<PathBuf>, (u32, u32)) {
    match source {
        MediaSource::Image { path } => (
            source.clone(),
            Some(resolve_media_path(project_dir, path)),
            (0, 0),
        ),
        MediaSource::Pattern { .. } => (source.clone(), None, canvas),
    }
}

fn load(key: &(MediaSource, Option<PathBuf>, (u32, u32))) -> Result<Arc<StillImage>, String> {
    match (&key.0, &key.1) {
        (MediaSource::Image { .. }, Some(path)) => StillImage::load(path)
            .map(Arc::new)
            .map_err(|e| e.to_string()),
        (MediaSource::Pattern { pattern }, _) => {
            StillImage::pattern(*pattern, key.2.0.max(1), key.2.1.max(1))
                .map(Arc::new)
                .map_err(|e| e.to_string())
        }
        (MediaSource::Image { path }, None) => Err(format!("{path}: unresolved path")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use om_project::{Canvas, Media, PatternKind};

    fn media(id: u128, source: MediaSource) -> Media {
        Media {
            id: MediaId::from_u128(id),
            name: "m".into(),
            source,
            extensions: Default::default(),
        }
    }

    #[test]
    fn loads_patterns_and_reports_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("p");
        p.canvas = Canvas {
            width: 64,
            height: 32,
        };
        p.media.push(media(
            1,
            MediaSource::Pattern {
                pattern: PatternKind::UvGrid,
            },
        ));
        p.media.push(media(
            2,
            MediaSource::Image {
                path: "img/a.png".into(),
            },
        ));
        let mut lib = MediaLibrary::new();
        let ch = lib.sync(&p, Some(dir.path()));
        assert_eq!(ch.loaded, vec![MediaId::from_u128(1)]);
        assert_eq!(ch.unloaded, vec![MediaId::from_u128(2)]);
        assert_eq!(lib.image(MediaId::from_u128(1)).unwrap().width(), 64);
        assert!(
            lib.error(MediaId::from_u128(2))
                .unwrap()
                .contains("not found")
        );
        // Unchanged project: nothing to do.
        assert!(lib.sync(&p, Some(dir.path())).is_empty());

        // File appears (drive reconnected): retry picks it up.
        std::fs::create_dir_all(dir.path().join("img")).unwrap();
        let img = StillImage::pattern(PatternKind::White, 3, 3).unwrap();
        image_save(&dir.path().join("img/a.png"), &img);
        assert_eq!(lib.retry_failed().loaded, vec![MediaId::from_u128(2)]);

        // Canvas resize regenerates patterns; removal unloads.
        p.canvas = Canvas {
            width: 10,
            height: 10,
        };
        p.media.remove(1);
        let ch = lib.sync(&p, Some(dir.path()));
        assert_eq!(ch.loaded, vec![MediaId::from_u128(1)]);
        assert_eq!(ch.unloaded, vec![MediaId::from_u128(2)]);
    }

    fn image_save(path: &Path, img: &StillImage) {
        image::save_buffer(
            path,
            img.rgba8(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    }

    #[test]
    fn storage_paths_are_relative_under_project_dir() {
        let dir = Path::new("/shows/demo");
        assert_eq!(
            path_for_storage(Some(dir), Path::new("/shows/demo/media/a b.png")),
            "media/a b.png"
        );
        assert_eq!(
            path_for_storage(Some(dir), Path::new("/elsewhere/x.png")),
            "/elsewhere/x.png"
        );
        assert_eq!(
            resolve_media_path(Some(dir), "media/a.png"),
            PathBuf::from("/shows/demo/media/a.png")
        );
    }
}
