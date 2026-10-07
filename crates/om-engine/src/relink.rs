// SPDX-License-Identifier: Apache-2.0
//! Relinking missing media: finds files with the same name in a folder the
//! user chooses and points the media at them, as one undoable command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use om_command::{Command, MediaPath};
use om_project::Project;
use om_types::MediaId;

use crate::media::{path_for_storage, resolve_media_path};

/// Most directory entries scanned (bounds time on huge folders).
const MAX_ENTRIES: usize = 200_000;
/// Deepest folder level scanned.
const MAX_DEPTH: usize = 12;

/// One proposed relink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relink {
    pub media: MediaId,
    pub name: String,
    pub old: String,
    pub new: String,
}

/// Media whose file or folder does not exist: `(id, stored path)`.
#[must_use]
pub fn missing(project: &Project, project_dir: Option<&Path>) -> Vec<(MediaId, String)> {
    project
        .media
        .iter()
        .filter_map(|m| {
            let stored = m.source.path()?;
            (!resolve_media_path(project_dir, stored).exists()).then(|| (m.id, stored.to_owned()))
        })
        .collect()
}

fn file_name(stored: &str) -> Option<String> {
    stored
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .map(str::to_lowercase)
}

/// Components of a stored path, lower-cased, last first.
fn tail(stored: &str) -> Vec<String> {
    stored
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .rev()
        .map(str::to_lowercase)
        .collect()
}

fn index(search: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut out: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![(search.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return out;
            }
            let path = entry.path();
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
                continue;
            };
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            out.entry(name).or_default().push(path.clone());
            if is_dir && depth < MAX_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    out
}

/// Proposes a new location under `search` for every missing media item
/// whose file name is found there. Among several candidates, the one whose
/// parent folders match the old path best wins (then the shortest path).
#[must_use]
pub fn find(project: &Project, project_dir: Option<&Path>, search: &Path) -> Vec<Relink> {
    let missing = missing(project, project_dir);
    if missing.is_empty() {
        return Vec::new();
    }
    let index = index(search);
    let mut out = Vec::new();
    for (id, stored) in missing {
        let Some(name) = file_name(&stored) else {
            continue;
        };
        let Some(candidates) = index.get(&name) else {
            continue;
        };
        let want = tail(&stored);
        let score = |p: &PathBuf| {
            let have = tail(&p.to_string_lossy());
            want.iter().zip(&have).take_while(|(a, b)| a == b).count()
        };
        let Some(best) = candidates.iter().max_by(|a, b| {
            score(a)
                .cmp(&score(b))
                .then_with(|| b.as_os_str().len().cmp(&a.as_os_str().len()))
                .then_with(|| b.cmp(a))
        }) else {
            continue;
        };
        let name = project
            .media
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.name.clone())
            .unwrap_or_default();
        out.push(Relink {
            media: id,
            name,
            old: stored,
            new: path_for_storage(project_dir, best),
        });
    }
    out
}

/// The command applying `relinks` (one undo step), if there are any.
#[must_use]
pub fn command(relinks: &[Relink]) -> Option<Command> {
    (!relinks.is_empty()).then(|| Command::RelinkMedia {
        changes: relinks
            .iter()
            .map(|r| MediaPath {
                id: r.media,
                path: r.new.clone(),
            })
            .collect(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use om_project::{Media, MediaSource};

    use super::*;

    fn media(n: u128, source: MediaSource) -> Media {
        Media {
            id: MediaId::from_u128(n),
            name: format!("m{n}"),
            source,
            playback: Default::default(),
            plugins: Vec::new(),
            extensions: Default::default(),
        }
    }

    #[test]
    fn finds_moved_files_preferring_matching_folders() {
        let dir = tempfile::tempdir().unwrap();
        let proj_dir = dir.path().join("show");
        let lib = dir.path().join("library");
        for p in [
            "clips/intro.mp4",
            "other/intro.mp4",
            "stills/logo.png",
            "seq/frames/a.png",
        ] {
            let f = lib.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, b"x").unwrap();
        }
        std::fs::create_dir_all(proj_dir.join("present")).unwrap();
        std::fs::write(proj_dir.join("present/ok.png"), b"x").unwrap();
        let mut project = Project::new("relink");
        project.media = vec![
            media(
                1,
                MediaSource::Video {
                    path: "media/clips/intro.mp4".into(),
                },
            ),
            media(
                2,
                MediaSource::Image {
                    path: "C:\\old\\Logo.PNG".into(),
                },
            ),
            media(
                3,
                MediaSource::Image {
                    path: "present/ok.png".into(),
                },
            ),
            media(
                4,
                MediaSource::Image {
                    path: "gone/nowhere.png".into(),
                },
            ),
            media(
                5,
                MediaSource::Sequence {
                    path: "frames".into(),
                    rate: om_time::Rate::FPS_30,
                },
            ),
        ];
        let found = find(&project, Some(&proj_dir), &lib);
        let by_id: HashMap<u128, &Relink> = found.iter().map(|r| (r.media.as_u128(), r)).collect();
        // Paths outside the project folder are stored absolute, with the
        // platform's separator.
        assert!(
            by_id[&1].new.replace('\\', "/").ends_with("clips/intro.mp4"),
            "{:?}",
            by_id[&1]
        );
        assert!(
            by_id[&2].new.ends_with("logo.png"),
            "case-insensitive, either separator"
        );
        assert!(!by_id.contains_key(&3), "present media is left alone");
        assert!(!by_id.contains_key(&4), "nothing to find");
        assert!(by_id[&5].new.ends_with("frames"), "folders (sequences) too");

        let mut doc = om_command::Document::new(project.clone());
        doc.execute(command(&found).unwrap()).unwrap();
        assert!(
            missing(doc.project(), Some(&proj_dir))
                .iter()
                .all(|(id, _)| id.as_u128() == 4)
        );
        doc.undo().unwrap();
        let mut back = doc.project().clone();
        back.revision = project.revision;
        assert_eq!(back, project, "one undo step restores everything");
        assert!(command(&[]).is_none());
    }
}
