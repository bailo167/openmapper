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
pub mod param;
mod schema;
pub mod show;
pub mod store;

pub use migrate::{CURRENT_VERSION, FORMAT};
pub use param::{ParamId, ParamKind, ParamValue};
pub use schema::{
    BlendMode, Canvas, DisplayTarget, Effect, EffectKind, Extensions, LiveInput, MAX_BLUR_RADIUS,
    MAX_EFFECTS, MAX_MASK_POINTS, MAX_MESH_DIVISIONS, MAX_PUBLISH, Mask, MaskPoint, Media,
    MediaSource, Output, PUBLISH_SCHEMES, PatternKind, Playback, Project, Publish, STREAM_FPS,
    STREAM_SCHEMES, ShaderValue, Shape, StreamCodec, Surface, Timebase, line_quad,
    validate_stream_url,
};
pub use show::{
    AudioBand, Controls, Cue, CueValue, Ease, Keyframe, LfoShape, Marker, Master, MidiBinding,
    MidiMessageKind, MidiTarget, ModSource, Modulator, Show, Timeline, Track,
};

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
            canvas: Canvas::default(),
            media: Vec::new(),
            surfaces: Vec::new(),
            outputs: Vec::new(),
            show: Show::default(),
            master: Master::default(),
            controls: Controls::default(),
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
        let Canvas { width, height } = self.canvas;
        if width == 0
            || height == 0
            || width > Canvas::MAX_DIMENSION
            || height > Canvas::MAX_DIMENSION
        {
            return invalid(format!(
                "canvas {width}x{height} is outside 1..={}",
                Canvas::MAX_DIMENSION
            ));
        }
        let mut seen = HashSet::new();
        for s in &self.surfaces {
            if !seen.insert(s.id) {
                return invalid(format!("duplicate surface id {}", s.id));
            }
            if s.name.trim().is_empty() {
                return invalid(format!("surface {} has an empty name", s.id));
            }
            if let Err(e) = s.shape.validate() {
                return invalid(format!("surface {}: {e}", s.id));
            }
            if let Some(Err(e)) = s.mask.as_ref().map(Mask::validate) {
                return invalid(format!("surface {} mask: {e}", s.id));
            }
            if s.effects.len() > MAX_EFFECTS {
                return invalid(format!(
                    "surface {} has more than {MAX_EFFECTS} effects",
                    s.id
                ));
            }
            for e in &s.effects {
                if let Err(msg) = e.kind.validate() {
                    return invalid(format!("surface {} effect: {msg}", s.id));
                }
            }
        }
        let mut media_ids = HashSet::new();
        for m in &self.media {
            if !media_ids.insert(m.id) {
                return invalid(format!("duplicate media id {}", m.id));
            }
            if m.name.trim().is_empty() {
                return invalid(format!("media {} has an empty name", m.id));
            }
            if let Some(path) = m.source.path()
                && path.trim().is_empty()
            {
                return invalid(format!("media {} has an empty path", m.id));
            }
            if let MediaSource::Live { input } = &m.source
                && let Err(e) = input.validate()
            {
                return invalid(format!("media {}: {e}", m.id));
            }
        }
        for s in &self.surfaces {
            if let Some(mid) = s.media
                && !media_ids.contains(&mid)
            {
                return invalid(format!("surface {} uses missing media {mid}", s.id));
            }
        }
        let mut seen = HashSet::new();
        for o in &self.outputs {
            if !seen.insert(o.id) {
                return invalid(format!("duplicate output id {}", o.id));
            }
            if o.publish.len() > MAX_PUBLISH {
                return invalid(format!(
                    "output {} has more than {MAX_PUBLISH} publish targets",
                    o.id
                ));
            }
            for p in &o.publish {
                if let Err(e) = p.validate() {
                    return invalid(format!("output {}: {e}", o.id));
                }
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

    #[must_use]
    pub fn media_item(&self, id: om_types::MediaId) -> Option<&Media> {
        self.media.iter().find(|m| m.id == id)
    }

    #[must_use]
    pub fn output(&self, id: om_types::OutputId) -> Option<&Output> {
        self.outputs.iter().find(|o| o.id == id)
    }
}

#[cfg(test)]
mod tests;
