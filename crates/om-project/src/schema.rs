// SPDX-License-Identifier: Apache-2.0
//! Version-1 document schema.
//!
//! Field order in these structs *is* the serialised key order. Maps are
//! `BTreeMap` so their order is deterministic too.

use std::collections::BTreeMap;

use om_time::{DEFAULT_TICKS_PER_SECOND, I128Str};
use om_types::{MediaId, OutputId, ProjectId, SurfaceId, UnitInterval};
use serde::{Deserialize, Serialize};

/// Free-form extension payloads, keyed by namespace (e.g. `"org.example.foo"`).
/// Preserved verbatim across load/save even when this build does not
/// understand them.
pub type Extensions = BTreeMap<String, serde_json::Value>;

/// Root document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub format: String,
    pub version: u64,
    pub project_id: ProjectId,
    /// Incremented by every applied command; ties the recovery journal to
    /// the saved file.
    #[serde(default)]
    pub revision: u64,
    pub name: String,
    #[serde(default)]
    pub timebase: Timebase,
    #[serde(default)]
    pub media: Vec<Media>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    #[serde(default)]
    pub outputs: Vec<Output>,
    #[serde(default)]
    pub show: Show,
    #[serde(default)]
    pub extensions: Extensions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timebase {
    pub ticks_per_second: I128Str,
}

impl Default for Timebase {
    fn default() -> Self {
        Self {
            ticks_per_second: I128Str(DEFAULT_TICKS_PER_SECOND),
        }
    }
}

/// A media item. Source references arrive with the media milestone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub id: MediaId,
    pub name: String,
    #[serde(default)]
    pub extensions: Extensions,
}

/// A mapping surface. Geometry arrives with the renderer milestone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    pub id: SurfaceId,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub opacity: UnitInterval,
    #[serde(default)]
    pub extensions: Extensions,
}

impl Surface {
    #[must_use]
    pub fn new(id: SurfaceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            enabled: true,
            opacity: UnitInterval::ONE,
            extensions: Extensions::new(),
        }
    }
}

/// A display/virtual output. Configuration arrives with the renderer milestone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub id: OutputId,
    pub name: String,
    #[serde(default)]
    pub extensions: Extensions,
}

/// Show-control state. Cues and timelines arrive with the show milestone;
/// until then they are kept as opaque JSON so nothing is lost.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show {
    #[serde(default)]
    pub cues: Vec<serde_json::Value>,
    #[serde(default)]
    pub timelines: Vec<serde_json::Value>,
}

fn default_true() -> bool {
    true
}
