// SPDX-License-Identifier: Apache-2.0
//! Session orchestration.
//!
//! A [`Session`] owns the open [`Document`], its file path and its recovery
//! journal. Every front end (GUI, CLI, and later OSC/MIDI) drives the project
//! through a session so persistence and recovery behave identically
//! everywhere.

mod discovery;
pub mod live;
mod media;
pub mod plugins;
mod publish;
pub mod relink;

use std::path::{Path, PathBuf};

pub use discovery::Discovery;
pub use publish::{PublishRuntime, PublishStatus};

pub use om_media_core::Adapters;

pub use media::{
    AudioSetup, MediaChanges, MediaRuntime, MediaStatus, Transport, audio_clock, media_time,
    path_for_storage, player_time, resolve_media_path, speed_percent,
};

use om_command::{Command, CommandError, CommandResult, Document, HistoryError};
use om_project::Project;
use om_project::store::{self, Journal, JournalEntry, StoreError};

/// Session-level failure.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error(transparent)]
    History(#[from] HistoryError),
    #[error("project has never been saved; choose a file path first")]
    NoPath,
}

/// What happened while opening a project.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpenReport {
    /// Version migrated from, if the file was older than this build.
    pub migrated_from: Option<u64>,
    /// Commands replayed from the recovery journal (unsaved work after a crash).
    pub recovered_commands: usize,
    /// Human-readable warnings, e.g. a journal that did not match the file.
    pub warnings: Vec<String>,
}

/// An open project plus its persistence state.
#[derive(Debug)]
pub struct Session {
    document: Document,
    path: Option<PathBuf>,
    journal: Option<Journal>,
    saved_revision: Option<u64>,
    /// Journal write failures are surfaced here rather than failing edits:
    /// the in-memory edit already succeeded and a save will persist it.
    journal_error: Option<String>,
}

impl Session {
    /// A new, unsaved, empty project.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self::from_project(Project::new(name))
    }

    /// An unsaved session around an existing in-memory project.
    #[must_use]
    pub fn from_project(project: Project) -> Self {
        Self {
            document: Document::new(project),
            path: None,
            journal: None,
            saved_revision: None,
            journal_error: None,
        }
    }

    /// Opens a project file, replaying its recovery journal if it holds
    /// unsaved work for exactly this file's revision.
    pub fn open(path: &Path) -> Result<(Self, OpenReport), SessionError> {
        let loaded = store::load(path)?;
        let mut report = OpenReport {
            migrated_from: loaded.migrated_from,
            ..OpenReport::default()
        };
        let saved_revision = loaded.project.revision;
        let mut document = Document::new(loaded.project);

        // Commands to keep in the rewritten journal: replayed unsaved work.
        let mut replayed = Vec::new();
        match Journal::read::<Command>(path) {
            Ok(None) => {}
            Ok(Some(contents)) => {
                let project = document.project();
                if contents.project_id != project.project_id
                    || contents.base_revision != project.revision
                {
                    report.warnings.push(format!(
                        "recovery journal is for revision {} but the file is at revision {}; {}",
                        contents.base_revision,
                        project.revision,
                        set_aside_message(path)
                    ));
                } else {
                    for entry in contents.entries {
                        match document.execute(entry.command) {
                            Ok(r) => replayed.push(r),
                            Err(e) => {
                                report.warnings.push(format!("stopped journal replay: {e}"));
                                break;
                            }
                        }
                    }
                    if contents.truncated_tail {
                        report
                            .warnings
                            .push("dropped an incomplete final journal entry".into());
                    }
                }
            }
            Err(e) => report.warnings.push(format!(
                "recovery journal is unreadable ({e}); {}",
                set_aside_message(path)
            )),
        }
        report.recovered_commands = replayed.len();

        // Rewrite the journal from the saved revision: it then holds exactly
        // the replayed work (no torn lines), so a second crash before saving
        // still recovers it.
        let project_id = document.project().project_id;
        let entries: Vec<JournalEntry<&Command>> = replayed
            .iter()
            .map(|r| JournalEntry {
                revision: r.revision,
                command: &r.applied,
            })
            .collect();
        let journal = match Journal::create_with_entries(path, project_id, saved_revision, &entries)
        {
            Ok(j) => Some(j),
            Err(e) => {
                report
                    .warnings
                    .push(format!("recovery journal disabled: {e}"));
                None
            }
        };
        let session = Self {
            document,
            path: Some(path.to_owned()),
            journal,
            saved_revision: Some(saved_revision),
            journal_error: None,
        };
        Ok((session, report))
    }

    #[must_use]
    pub fn project(&self) -> &Project {
        self.document.project()
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Directory relative media paths resolve against (the project file's
    /// directory), if the project has been saved.
    #[must_use]
    pub fn project_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// True if there are changes since the last save (or it was never saved).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.saved_revision != Some(self.project().revision)
    }

    /// Most recent journal write failure, if any.
    #[must_use]
    pub fn journal_error(&self) -> Option<&str> {
        self.journal_error.as_deref()
    }

    pub fn execute(&mut self, command: Command) -> Result<CommandResult, SessionError> {
        let r = self.document.execute(command)?;
        self.journal(&r);
        Ok(r)
    }

    /// See [`Document::execute_coalescing`].
    pub fn execute_coalescing(
        &mut self,
        command: Command,
        key: impl Into<String>,
    ) -> Result<CommandResult, SessionError> {
        let r = self
            .document
            .execute_coalescing(command, Some(key.into()))?;
        self.journal(&r);
        Ok(r)
    }

    pub fn break_coalescing(&mut self) {
        self.document.break_coalescing();
    }

    pub fn undo(&mut self) -> Result<CommandResult, SessionError> {
        let r = self.document.undo()?;
        self.journal(&r);
        Ok(r)
    }

    pub fn redo(&mut self) -> Result<CommandResult, SessionError> {
        let r = self.document.redo()?;
        self.journal(&r);
        Ok(r)
    }

    /// Saves to the current path.
    pub fn save(&mut self) -> Result<(), SessionError> {
        let path = self.path.clone().ok_or(SessionError::NoPath)?;
        self.save_as(&path)
    }

    /// Saves to `path` and makes it the session's path. The old journal is
    /// replaced by a fresh one based on the saved revision.
    pub fn save_as(&mut self, path: &Path) -> Result<(), SessionError> {
        store::save_atomic(path, self.project())?;
        if let Some(old) = self.path.as_deref()
            && old != path
        {
            let _ = Journal::remove(old);
        }
        self.path = Some(path.to_owned());
        self.saved_revision = Some(self.project().revision);
        match Journal::create(path, self.project()) {
            Ok(j) => {
                self.journal = Some(j);
                self.journal_error = None;
            }
            Err(e) => {
                self.journal = None;
                self.journal_error = Some(e.to_string());
            }
        }
        Ok(())
    }

    /// Closes the session cleanly, removing the journal. Unsaved changes
    /// are discarded; callers must check [`Session::is_dirty`] first.
    pub fn close(self) -> Result<(), SessionError> {
        if let Some(path) = &self.path {
            Journal::remove(path)?;
        }
        Ok(())
    }

    fn journal(&mut self, r: &CommandResult) {
        if let Some(j) = &mut self.journal {
            let entry = JournalEntry {
                revision: r.revision,
                command: &r.applied,
            };
            if let Err(e) = j.append(&entry) {
                self.journal_error = Some(e.to_string());
            }
        }
    }
}

fn set_aside_message(path: &Path) -> String {
    match Journal::set_aside(path) {
        Ok(stale) => format!("kept it as {}", stale.display()),
        Err(e) => format!("could not keep it ({e})"),
    }
}

#[cfg(test)]
mod tests;
