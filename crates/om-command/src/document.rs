// SPDX-License-Identifier: Apache-2.0

use om_project::Project;

use crate::{Command, CommandError, Event};

/// Outcome of a successful execute/undo/redo.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandResult {
    /// Project revision after the change.
    pub revision: u64,
    /// The command that was actually applied (for undo, the inverse). This is
    /// what gets journaled: replaying applied commands reproduces the state.
    pub applied: Command,
    pub events: Vec<Event>,
}

/// Undo/redo failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HistoryError {
    #[error("nothing to undo")]
    NothingToUndo,
    #[error("nothing to redo")]
    NothingToRedo,
    #[error(transparent)]
    Command(#[from] CommandError),
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    /// Re-applying this redoes the change.
    forward: Command,
    /// Applying this undoes the change.
    inverse: Command,
    /// Consecutive commands with the same key merge into one undo step
    /// (e.g. every frame of a slider drag).
    coalesce_key: Option<String>,
}

/// A project plus its undo/redo history. The only way to change the project
/// is through [`Document::execute`], [`Document::undo`] and [`Document::redo`].
#[derive(Debug, Clone)]
pub struct Document {
    project: Project,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    history_limit: usize,
}

impl Document {
    pub const DEFAULT_HISTORY_LIMIT: usize = 1000;

    #[must_use]
    pub fn new(project: Project) -> Self {
        Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            history_limit: Self::DEFAULT_HISTORY_LIMIT,
        }
    }

    #[must_use]
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Applies a command as a new undo step.
    pub fn execute(&mut self, command: Command) -> Result<CommandResult, CommandError> {
        self.execute_coalescing(command, None)
    }

    /// Applies a command; if `coalesce_key` matches the previous undo step's
    /// key, both become a single undo step.
    pub fn execute_coalescing(
        &mut self,
        command: Command,
        coalesce_key: Option<String>,
    ) -> Result<CommandResult, CommandError> {
        let result = self.apply(&command)?;
        let merge = coalesce_key.is_some()
            && self
                .undo
                .last()
                .is_some_and(|e| e.coalesce_key == coalesce_key);
        if merge {
            if let Some(last) = self.undo.last_mut() {
                // Keep the oldest inverse so one undo returns to the state
                // before the first merged command.
                last.forward = command;
            }
        } else {
            self.undo.push(HistoryEntry {
                forward: command,
                inverse: result.1,
                coalesce_key,
            });
            if self.undo.len() > self.history_limit {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        Ok(result.0)
    }

    /// Ends the current coalescing run so the next command starts a new step.
    pub fn break_coalescing(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.coalesce_key = None;
        }
    }

    pub fn undo(&mut self) -> Result<CommandResult, HistoryError> {
        let entry = self.undo.pop().ok_or(HistoryError::NothingToUndo)?;
        match self.apply(&entry.inverse) {
            Ok((result, _)) => {
                self.redo.push(entry);
                Ok(result)
            }
            Err(e) => {
                // An inverse failing means history is inconsistent with the
                // project; drop history rather than corrupt state.
                self.undo.clear();
                self.redo.clear();
                Err(e.into())
            }
        }
    }

    pub fn redo(&mut self) -> Result<CommandResult, HistoryError> {
        let entry = self.redo.pop().ok_or(HistoryError::NothingToRedo)?;
        match self.apply(&entry.forward) {
            Ok((result, inverse)) => {
                self.undo.push(HistoryEntry {
                    forward: entry.forward,
                    inverse,
                    coalesce_key: None,
                });
                Ok(result)
            }
            Err(e) => {
                self.undo.clear();
                self.redo.clear();
                Err(e.into())
            }
        }
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|e| e.forward.label())
    }

    #[must_use]
    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|e| e.forward.label())
    }

    /// Applies, validates the whole project, bumps the revision. Rolls back
    /// if validation fails.
    fn apply(&mut self, command: &Command) -> Result<(CommandResult, Command), CommandError> {
        let applied = command.apply(&mut self.project)?;
        if let Err(e) = self.project.validate() {
            // Commands validate their own inputs; this is a safety net.
            let _ = applied.inverse.apply(&mut self.project);
            return Err(CommandError::Invalid(e.to_string()));
        }
        self.project.revision += 1;
        Ok((
            CommandResult {
                revision: self.project.revision,
                applied: command.clone(),
                events: applied.events,
            },
            applied.inverse,
        ))
    }
}
