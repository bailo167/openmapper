// SPDX-License-Identifier: Apache-2.0
//! Crash-safe persistence: atomic project replacement and the append-only
//! recovery journal.
//!
//! Save protocol: write `<path>.tmp`, fsync, re-read and validate, rename over
//! `<path>`, fsync the directory. A crash at any point leaves either the old
//! or the new file intact, never a partial one.
//!
//! The journal (`<path>.journal`) is JSON lines: a header naming the project
//! and base revision, then one entry per accepted command. A torn final line
//! (crash mid-append) is ignored.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use om_types::ProjectId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{Loaded, Project, ProjectError};

/// Errors from reading or writing project files.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {source}")]
    Project {
        path: PathBuf,
        #[source]
        source: ProjectError,
    },
    #[error("journal {path} is corrupt at line {line}: {message}")]
    Journal {
        path: PathBuf,
        line: usize,
        message: String,
    },
}

fn io_err(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> StoreError + use<> {
    let path = path.to_owned();
    move |source| StoreError::Io {
        action,
        path,
        source,
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// `<path>.tmp`
#[must_use]
pub fn temp_path(path: &Path) -> PathBuf {
    with_suffix(path, ".tmp")
}

/// `<path>.journal`
#[must_use]
pub fn journal_path(path: &Path) -> PathBuf {
    with_suffix(path, ".journal")
}

/// Reads, migrates and validates a project file.
pub fn load(path: &Path) -> Result<Loaded, StoreError> {
    let text = fs::read_to_string(path).map_err(io_err("reading", path))?;
    Project::from_json(&text).map_err(|source| StoreError::Project {
        path: path.to_owned(),
        source,
    })
}

/// Atomically replaces `path` with `project`.
pub fn save_atomic(path: &Path, project: &Project) -> Result<(), StoreError> {
    let as_project_err = |source| StoreError::Project {
        path: path.to_owned(),
        source,
    };
    project.validate().map_err(as_project_err)?;
    let text = project.to_canonical_json().map_err(as_project_err)?;
    let tmp = temp_path(path);
    {
        let mut file = File::create(&tmp).map_err(io_err("creating", &tmp))?;
        file.write_all(text.as_bytes())
            .map_err(io_err("writing", &tmp))?;
        file.sync_all().map_err(io_err("syncing", &tmp))?;
    }
    // Validate what actually reached the disk before replacing anything.
    let written = fs::read_to_string(&tmp).map_err(io_err("re-reading", &tmp))?;
    if written != text {
        let _ = fs::remove_file(&tmp);
        return Err(StoreError::Io {
            action: "verifying",
            path: tmp,
            source: io::Error::other("written bytes differ from serialised project"),
        });
    }
    if let Err(source) = Project::from_json(&written) {
        let _ = fs::remove_file(&tmp);
        return Err(StoreError::Project { path: tmp, source });
    }
    fs::rename(&tmp, path).map_err(io_err("replacing", path))?;
    sync_parent_dir(path);
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    // Best effort: makes the rename durable on POSIX filesystems.
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

const JOURNAL_MAGIC: &str = "openmapper-journal";
const JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct JournalHeader {
    journal: String,
    version: u32,
    project_id: ProjectId,
    base_revision: u64,
}

/// One accepted change after the base revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry<T> {
    pub revision: u64,
    pub command: T,
}

/// Append-only recovery journal for commands of type `T`.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: File,
    /// When entries were last forced to disk (timed fsync batching, D-004).
    synced: std::time::Instant,
}

/// Longest time journal entries stay only in OS buffers (power-loss
/// window).
pub const JOURNAL_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Journal contents read back after a crash.
#[derive(Debug)]
pub struct JournalContents<T> {
    pub project_id: ProjectId,
    pub base_revision: u64,
    pub entries: Vec<JournalEntry<T>>,
    /// True if a torn trailing line was dropped.
    pub truncated_tail: bool,
}

impl Journal {
    /// Starts a fresh journal for `project` at its current revision,
    /// replacing any existing journal.
    pub fn create(project_path: &Path, project: &Project) -> Result<Self, StoreError> {
        Self::create_with_base(project_path, project.project_id, project.revision)
    }

    /// Starts a fresh journal with an explicit base revision, replacing any
    /// existing journal. Used to rewrite a journal after recovery.
    pub fn create_with_base(
        project_path: &Path,
        project_id: ProjectId,
        base_revision: u64,
    ) -> Result<Self, StoreError> {
        Self::create_with_entries::<()>(project_path, project_id, base_revision, &[])
    }

    /// Atomically replaces the journal with a header and `entries`: written
    /// to a temporary file, synced, then renamed over the old journal, so a
    /// crash at any moment leaves either the old or the new journal, never a
    /// partial one (losing recovered work).
    pub fn create_with_entries<T: Serialize>(
        project_path: &Path,
        project_id: ProjectId,
        base_revision: u64,
        entries: &[JournalEntry<T>],
    ) -> Result<Self, StoreError> {
        let path = journal_path(project_path);
        let tmp = with_suffix(&path, ".tmp");
        let header = JournalHeader {
            journal: JOURNAL_MAGIC.into(),
            version: JOURNAL_VERSION,
            project_id,
            base_revision,
        };
        let json_err = |e: serde_json::Error| StoreError::Journal {
            path: path.clone(),
            line: 1,
            message: e.to_string(),
        };
        let mut text = serde_json::to_string(&header).map_err(json_err)?;
        text.push('\n');
        for e in entries {
            text.push_str(&serde_json::to_string(e).map_err(json_err)?);
            text.push('\n');
        }
        {
            let mut file = File::create(&tmp).map_err(io_err("creating", &tmp))?;
            file.write_all(text.as_bytes())
                .map_err(io_err("writing", &tmp))?;
            file.sync_all().map_err(io_err("syncing", &tmp))?;
        }
        fs::rename(&tmp, &path).map_err(io_err("replacing", &path))?;
        sync_parent_dir(&path);
        Self::open_append(project_path)
    }

    /// Re-opens an existing journal for appending (after recovery).
    pub fn open_append(project_path: &Path) -> Result<Self, StoreError> {
        let path = journal_path(project_path);
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(io_err("opening", &path))?;
        Ok(Self {
            path,
            file,
            synced: std::time::Instant::now(),
        })
    }

    /// Appends one entry. Flushed to the OS immediately and forced to disk
    /// at most [`JOURNAL_SYNC_INTERVAL`] later (DECISIONS.md D-004, D-026).
    pub fn append<T: Serialize>(&mut self, entry: &JournalEntry<T>) -> Result<(), StoreError> {
        let mut line = serde_json::to_string(entry).map_err(|e| StoreError::Journal {
            path: self.path.clone(),
            line: 0,
            message: e.to_string(),
        })?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .map_err(io_err("appending to", &self.path))?;
        self.file.flush().map_err(io_err("flushing", &self.path))?;
        if self.synced.elapsed() >= JOURNAL_SYNC_INTERVAL {
            self.sync()?;
        }
        Ok(())
    }

    /// Forces appended entries to disk.
    pub fn sync(&mut self) -> Result<(), StoreError> {
        self.file
            .sync_data()
            .map_err(io_err("syncing", &self.path))?;
        self.synced = std::time::Instant::now();
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads a journal if one exists next to `project_path`.
    pub fn read<T: DeserializeOwned>(
        project_path: &Path,
    ) -> Result<Option<JournalContents<T>>, StoreError> {
        let path = journal_path(project_path);
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err("opening", &path)(e)),
        };
        let corrupt = |line: usize, message: String| StoreError::Journal {
            path: path.clone(),
            line,
            message,
        };
        let mut lines = Vec::new();
        for line in BufReader::new(file).lines() {
            lines.push(line.map_err(io_err("reading", &path))?);
        }
        let Some(first) = lines.first() else {
            return Err(corrupt(1, "empty journal".into()));
        };
        let header: JournalHeader =
            serde_json::from_str(first).map_err(|e| corrupt(1, e.to_string()))?;
        if header.journal != JOURNAL_MAGIC || header.version != JOURNAL_VERSION {
            return Err(corrupt(1, "unrecognised journal header".into()));
        }
        let mut entries = Vec::new();
        let mut truncated_tail = false;
        let body = &lines[1..];
        for (i, text) in body.iter().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JournalEntry<T>>(text) {
                Ok(e) => entries.push(e),
                // Only the final line may be torn by a crash.
                Err(_) if i + 1 == body.len() => truncated_tail = true,
                Err(e) => return Err(corrupt(i + 2, e.to_string())),
            }
        }
        Ok(Some(JournalContents {
            project_id: header.project_id,
            base_revision: header.base_revision,
            entries,
            truncated_tail,
        }))
    }

    /// Moves an unusable journal aside to `<path>.journal.stale` so it is not
    /// lost, replacing any older stale journal.
    pub fn set_aside(project_path: &Path) -> Result<PathBuf, StoreError> {
        let path = journal_path(project_path);
        let stale = with_suffix(&path, ".stale");
        fs::rename(&path, &stale).map_err(io_err("moving aside", &path))?;
        Ok(stale)
    }

    /// Deletes the journal next to `project_path`, if any.
    pub fn remove(project_path: &Path) -> Result<(), StoreError> {
        let path = journal_path(project_path);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("removing", &path)(e)),
        }
    }
}
