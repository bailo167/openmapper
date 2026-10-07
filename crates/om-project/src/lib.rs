// SPDX-License-Identifier: Apache-2.0
//! The OpenMapper project document (`.omproj`).
//!
//! See `docs/project-format.md` for the format rules. In short: versioned,
//! human-readable, deterministically serialised JSON; unknown `extensions`
//! payloads round-trip; non-finite numbers are rejected; files are replaced
//! atomically.
//!
//! This crate defines *data* only. Mutations go through `om-command`.

mod migrate;
mod schema;
pub mod store;

pub use migrate::{CURRENT_VERSION, FORMAT};
pub use schema::{Extensions, Media, Output, Project, Show, Surface, Timebase};

use std::collections::HashSet;

/// Errors from parsing, validating or migrating a project document.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not an OpenMapper project (format is {found:?}, expected {FORMAT:?})")]
    WrongFormat { found: Option<String> },
    #[error(
        "project version {found} is newer than this build supports (max {CURRENT_VERSION}); upgrade OpenMapper"
    )]
    FutureVersion { found: u64 },
    #[error("project version field is missing or not a positive integer")]
    BadVersion,
    #[error("no migration from project version {0}")]
    NoMigration(u64),
    #[error("invalid project: {0}")]
    Invalid(String),
}

/// Outcome of loading a document.
#[derive(Debug)]
pub struct Loaded {
    pub project: Project,
    /// The on-disk version if a migration ran.
    pub migrated_from: Option<u64>,
}

impl Project {
    /// Creates an empty current-version project.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: CURRENT_VERSION,
            project_id: om_types::ProjectId::new(),
            revision: 0,
            name: name.into(),
            timebase: Timebase::default(),
            media: Vec::new(),
            surfaces: Vec::new(),
            outputs: Vec::new(),
            show: Show::default(),
            extensions: Extensions::new(),
        }
    }

    /// Parses, migrates and validates a document.
    pub fn from_json(text: &str) -> Result<Loaded, ProjectError> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        let (value, migrated_from) = migrate::migrate(value)?;
        let project: Project = serde_json::from_value(value)?;
        project.validate()?;
        Ok(Loaded {
            project,
            migrated_from,
        })
    }

    /// Canonical serialisation: pretty JSON, fixed key order, trailing newline.
    /// The same project always produces the same bytes.
    pub fn to_canonical_json(&self) -> Result<String, ProjectError> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }

    /// Checks invariants that the type system does not.
    pub fn validate(&self) -> Result<(), ProjectError> {
        let invalid = |m: String| Err(ProjectError::Invalid(m));
        if self.format != FORMAT {
            return Err(ProjectError::WrongFormat {
                found: Some(self.format.clone()),
            });
        }
        if self.version != CURRENT_VERSION {
            return invalid(format!("in-memory version {} is not current", self.version));
        }
        if self.name.trim().is_empty() {
            return invalid("project name is empty".into());
        }
        let mut seen = HashSet::new();
        for s in &self.surfaces {
            if !seen.insert(s.id) {
                return invalid(format!("duplicate surface id {}", s.id));
            }
            if s.name.trim().is_empty() {
                return invalid(format!("surface {} has an empty name", s.id));
            }
        }
        let mut seen = HashSet::new();
        for m in &self.media {
            if !seen.insert(m.id) {
                return invalid(format!("duplicate media id {}", m.id));
            }
        }
        let mut seen = HashSet::new();
        for o in &self.outputs {
            if !seen.insert(o.id) {
                return invalid(format!("duplicate output id {}", o.id));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn surface(&self, id: om_types::SurfaceId) -> Option<&Surface> {
        self.surfaces.iter().find(|s| s.id == id)
    }

    pub fn surface_mut(&mut self, id: om_types::SurfaceId) -> Option<&mut Surface> {
        self.surfaces.iter_mut().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests;
