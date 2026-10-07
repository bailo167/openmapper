// SPDX-License-Identifier: Apache-2.0

use om_project::{Project, Surface};
use om_types::{SurfaceId, UnitInterval};
use serde::{Deserialize, Serialize};

/// A validated, serialisable project mutation.
///
/// The JSON form is `{"type": "<snake_case_name>", ...fields}`; this is the
/// format used by the journal, the CLI `apply` command and test scripts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    SetProjectName {
        name: String,
    },
    AddSurface {
        surface: Surface,
        /// Insert position; `None` appends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    RemoveSurface {
        id: SurfaceId,
    },
    /// Changes any subset of a surface's scalar properties.
    UpdateSurface {
        id: SurfaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<UnitInterval>,
    },
    MoveSurface {
        id: SurfaceId,
        to_index: usize,
    },
    /// Sets (`Some`) or removes (`None`) a project-level extension payload.
    SetExtension {
        key: String,
        value: Option<serde_json::Value>,
    },
}

/// What changed. Emitted to observers (UI, OSCQuery, …) after each command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ProjectRenamed,
    SurfaceAdded { id: SurfaceId },
    SurfaceRemoved { id: SurfaceId },
    SurfaceChanged { id: SurfaceId },
    SurfacesReordered,
    ExtensionChanged { key: String },
}

/// Why a command was rejected. The project is unchanged when this is returned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("name must not be empty")]
    EmptyName,
    #[error("surface {0} does not exist")]
    UnknownSurface(SurfaceId),
    #[error("surface {0} already exists")]
    DuplicateSurface(SurfaceId),
    #[error("index {index} is out of range (0..={max})")]
    IndexOutOfRange { index: usize, max: usize },
    #[error("extension key must be non-empty")]
    EmptyExtensionKey,
    #[error("command would make the project invalid: {0}")]
    Invalid(String),
}

/// Result of applying a command.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// Applying this restores the previous state exactly.
    pub inverse: Command,
    pub events: Vec<Event>,
}

fn check_name(name: &str) -> Result<(), CommandError> {
    if name.trim().is_empty() {
        Err(CommandError::EmptyName)
    } else {
        Ok(())
    }
}

fn surface_index(project: &Project, id: SurfaceId) -> Result<usize, CommandError> {
    project
        .surfaces
        .iter()
        .position(|s| s.id == id)
        .ok_or(CommandError::UnknownSurface(id))
}

impl Command {
    /// Validates and applies this command. All checks happen before any
    /// mutation, so an error leaves `project` untouched.
    pub fn apply(&self, project: &mut Project) -> Result<Applied, CommandError> {
        match self {
            Self::SetProjectName { name } => {
                check_name(name)?;
                let old = std::mem::replace(&mut project.name, name.clone());
                Ok(Applied {
                    inverse: Self::SetProjectName { name: old },
                    events: vec![Event::ProjectRenamed],
                })
            }
            Self::AddSurface { surface, index } => {
                check_name(&surface.name)?;
                if project.surface(surface.id).is_some() {
                    return Err(CommandError::DuplicateSurface(surface.id));
                }
                let max = project.surfaces.len();
                let at = index.unwrap_or(max);
                if at > max {
                    return Err(CommandError::IndexOutOfRange { index: at, max });
                }
                project.surfaces.insert(at, surface.clone());
                Ok(Applied {
                    inverse: Self::RemoveSurface { id: surface.id },
                    events: vec![Event::SurfaceAdded { id: surface.id }],
                })
            }
            Self::RemoveSurface { id } => {
                let at = surface_index(project, *id)?;
                let surface = project.surfaces.remove(at);
                Ok(Applied {
                    inverse: Self::AddSurface {
                        surface,
                        index: Some(at),
                    },
                    events: vec![Event::SurfaceRemoved { id: *id }],
                })
            }
            Self::UpdateSurface {
                id,
                name,
                enabled,
                opacity,
            } => {
                if let Some(n) = name {
                    check_name(n)?;
                }
                let at = surface_index(project, *id)?;
                let s = &mut project.surfaces[at];
                let inverse = Self::UpdateSurface {
                    id: *id,
                    name: name.as_ref().map(|_| s.name.clone()),
                    enabled: enabled.map(|_| s.enabled),
                    opacity: opacity.map(|_| s.opacity),
                };
                if let Some(n) = name {
                    s.name.clone_from(n);
                }
                if let Some(e) = enabled {
                    s.enabled = *e;
                }
                if let Some(o) = opacity {
                    s.opacity = *o;
                }
                Ok(Applied {
                    inverse,
                    events: vec![Event::SurfaceChanged { id: *id }],
                })
            }
            Self::MoveSurface { id, to_index } => {
                let from = surface_index(project, *id)?;
                let max = project.surfaces.len() - 1;
                if *to_index > max {
                    return Err(CommandError::IndexOutOfRange {
                        index: *to_index,
                        max,
                    });
                }
                let s = project.surfaces.remove(from);
                project.surfaces.insert(*to_index, s);
                Ok(Applied {
                    inverse: Self::MoveSurface {
                        id: *id,
                        to_index: from,
                    },
                    events: vec![Event::SurfacesReordered],
                })
            }
            Self::SetExtension { key, value } => {
                if key.is_empty() {
                    return Err(CommandError::EmptyExtensionKey);
                }
                let old = match value {
                    Some(v) => project.extensions.insert(key.clone(), v.clone()),
                    None => project.extensions.remove(key),
                };
                Ok(Applied {
                    inverse: Self::SetExtension {
                        key: key.clone(),
                        value: old,
                    },
                    events: vec![Event::ExtensionChanged { key: key.clone() }],
                })
            }
        }
    }

    /// Short human-readable label for undo menus and logs.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::SetProjectName { .. } => "Rename Project",
            Self::AddSurface { .. } => "Add Surface",
            Self::RemoveSurface { .. } => "Remove Surface",
            Self::UpdateSurface { .. } => "Edit Surface",
            Self::MoveSurface { .. } => "Reorder Surfaces",
            Self::SetExtension { .. } => "Edit Extension",
        }
    }
}
