// SPDX-License-Identifier: Apache-2.0

use om_project::{
    BlendMode, Canvas, DisplayTarget, Effect, Mask, Media, Output, Playback, Project, Shape,
    Surface,
};
use om_types::{MediaId, OutputId, SurfaceId, UnitInterval};
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
    /// Replaces a surface's geometry (corners and UVs).
    SetSurfaceShape {
        id: SurfaceId,
        shape: Shape,
    },
    SetSurfaceBlend {
        id: SurfaceId,
        blend: BlendMode,
    },
    /// Replaces a surface's whole effect chain.
    SetSurfaceEffects {
        id: SurfaceId,
        effects: Vec<Effect>,
    },
    /// Sets (`Some`) or removes (`None`) a surface's mask.
    SetSurfaceMask {
        id: SurfaceId,
        mask: Option<Mask>,
    },
    /// Assigns (`Some`) or clears (`None`) a surface's media.
    SetSurfaceMedia {
        id: SurfaceId,
        media: Option<MediaId>,
    },
    AddMedia {
        media: Media,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// Changes how a time-based media item plays.
    SetMediaPlayback {
        id: MediaId,
        playback: Playback,
    },
    /// Removes a media item. Rejected while any surface uses it.
    RemoveMedia {
        id: MediaId,
    },
    SetCanvas {
        canvas: Canvas,
    },
    AddOutput {
        output: Output,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    RemoveOutput {
        id: OutputId,
    },
    UpdateOutput {
        id: OutputId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
    },
    /// Sets (`Some`) or clears (`None`) the output's preferred display.
    SetOutputDisplay {
        id: OutputId,
        display: Option<DisplayTarget>,
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
    MediaAdded { id: MediaId },
    MediaRemoved { id: MediaId },
    MediaChanged { id: MediaId },
    CanvasChanged,
    OutputAdded { id: OutputId },
    OutputRemoved { id: OutputId },
    OutputChanged { id: OutputId },
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
    #[error("media {0} does not exist")]
    UnknownMedia(MediaId),
    #[error("media {0} already exists")]
    DuplicateMedia(MediaId),
    #[error("media {media} is still used by surface {surface}")]
    MediaInUse { media: MediaId, surface: SurfaceId },
    #[error("output {0} does not exist")]
    UnknownOutput(OutputId),
    #[error("output {0} already exists")]
    DuplicateOutput(OutputId),
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

fn insert_at<T>(items: &mut Vec<T>, item: T, index: Option<usize>) -> Result<(), CommandError> {
    let max = items.len();
    let at = index.unwrap_or(max);
    if at > max {
        return Err(CommandError::IndexOutOfRange { index: at, max });
    }
    items.insert(at, item);
    Ok(())
}

fn media_index(project: &Project, id: MediaId) -> Result<usize, CommandError> {
    project
        .media
        .iter()
        .position(|m| m.id == id)
        .ok_or(CommandError::UnknownMedia(id))
}

fn output_index(project: &Project, id: OutputId) -> Result<usize, CommandError> {
    project
        .outputs
        .iter()
        .position(|o| o.id == id)
        .ok_or(CommandError::UnknownOutput(id))
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
                if let Some(m) = surface.media {
                    media_index(project, m)?;
                }
                insert_at(&mut project.surfaces, surface.clone(), *index)?;
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
            Self::SetSurfaceShape { id, shape } => {
                let at = surface_index(project, *id)?;
                shape.validate().map_err(CommandError::Invalid)?;
                let old = std::mem::replace(&mut project.surfaces[at].shape, shape.clone());
                Ok(Applied {
                    inverse: Self::SetSurfaceShape {
                        id: *id,
                        shape: old,
                    },
                    events: vec![Event::SurfaceChanged { id: *id }],
                })
            }
            Self::SetSurfaceBlend { id, blend } => {
                let at = surface_index(project, *id)?;
                let old = std::mem::replace(&mut project.surfaces[at].blend, *blend);
                Ok(Applied {
                    inverse: Self::SetSurfaceBlend {
                        id: *id,
                        blend: old,
                    },
                    events: vec![Event::SurfaceChanged { id: *id }],
                })
            }
            Self::SetSurfaceEffects { id, effects } => {
                if effects.len() > om_project::MAX_EFFECTS {
                    return Err(CommandError::Invalid(format!(
                        "at most {} effects per surface",
                        om_project::MAX_EFFECTS
                    )));
                }
                for e in effects {
                    e.kind.validate().map_err(CommandError::Invalid)?;
                }
                let at = surface_index(project, *id)?;
                let old = std::mem::replace(&mut project.surfaces[at].effects, effects.clone());
                Ok(Applied {
                    inverse: Self::SetSurfaceEffects {
                        id: *id,
                        effects: old,
                    },
                    events: vec![Event::SurfaceChanged { id: *id }],
                })
            }
            Self::SetSurfaceMask { id, mask } => {
                if let Some(m) = mask {
                    m.validate().map_err(CommandError::Invalid)?;
                }
                let at = surface_index(project, *id)?;
                let old = std::mem::replace(&mut project.surfaces[at].mask, mask.clone());
                Ok(Applied {
                    inverse: Self::SetSurfaceMask { id: *id, mask: old },
                    events: vec![Event::SurfaceChanged { id: *id }],
                })
            }
            Self::SetSurfaceMedia { id, media } => {
                let at = surface_index(project, *id)?;
                if let Some(m) = media {
                    media_index(project, *m)?;
                }
                let old = std::mem::replace(&mut project.surfaces[at].media, *media);
                Ok(Applied {
                    inverse: Self::SetSurfaceMedia {
                        id: *id,
                        media: old,
                    },
                    events: vec![Event::SurfaceChanged { id: *id }],
                })
            }
            Self::AddMedia { media, index } => {
                check_name(&media.name)?;
                if project.media_item(media.id).is_some() {
                    return Err(CommandError::DuplicateMedia(media.id));
                }
                insert_at(&mut project.media, media.clone(), *index)?;
                Ok(Applied {
                    inverse: Self::RemoveMedia { id: media.id },
                    events: vec![Event::MediaAdded { id: media.id }],
                })
            }
            Self::SetMediaPlayback { id, playback } => {
                let at = media_index(project, *id)?;
                let old = std::mem::replace(&mut project.media[at].playback, *playback);
                Ok(Applied {
                    inverse: Self::SetMediaPlayback {
                        id: *id,
                        playback: old,
                    },
                    events: vec![Event::MediaChanged { id: *id }],
                })
            }
            Self::RemoveMedia { id } => {
                let at = media_index(project, *id)?;
                if let Some(s) = project.surfaces.iter().find(|s| s.media == Some(*id)) {
                    return Err(CommandError::MediaInUse {
                        media: *id,
                        surface: s.id,
                    });
                }
                let media = project.media.remove(at);
                Ok(Applied {
                    inverse: Self::AddMedia {
                        media,
                        index: Some(at),
                    },
                    events: vec![Event::MediaRemoved { id: *id }],
                })
            }
            Self::SetCanvas { canvas } => {
                let max = Canvas::MAX_DIMENSION;
                if canvas.width == 0
                    || canvas.height == 0
                    || canvas.width > max
                    || canvas.height > max
                {
                    return Err(CommandError::Invalid(format!(
                        "canvas {}x{} is outside 1..={max}",
                        canvas.width, canvas.height
                    )));
                }
                let old = std::mem::replace(&mut project.canvas, *canvas);
                Ok(Applied {
                    inverse: Self::SetCanvas { canvas: old },
                    events: vec![Event::CanvasChanged],
                })
            }
            Self::AddOutput { output, index } => {
                check_name(&output.name)?;
                if project.output(output.id).is_some() {
                    return Err(CommandError::DuplicateOutput(output.id));
                }
                insert_at(&mut project.outputs, output.clone(), *index)?;
                Ok(Applied {
                    inverse: Self::RemoveOutput { id: output.id },
                    events: vec![Event::OutputAdded { id: output.id }],
                })
            }
            Self::RemoveOutput { id } => {
                let at = output_index(project, *id)?;
                let output = project.outputs.remove(at);
                Ok(Applied {
                    inverse: Self::AddOutput {
                        output,
                        index: Some(at),
                    },
                    events: vec![Event::OutputRemoved { id: *id }],
                })
            }
            Self::UpdateOutput { id, name, enabled } => {
                if let Some(n) = name {
                    check_name(n)?;
                }
                let at = output_index(project, *id)?;
                let o = &mut project.outputs[at];
                let inverse = Self::UpdateOutput {
                    id: *id,
                    name: name.as_ref().map(|_| o.name.clone()),
                    enabled: enabled.map(|_| o.enabled),
                };
                if let Some(n) = name {
                    o.name.clone_from(n);
                }
                if let Some(e) = enabled {
                    o.enabled = *e;
                }
                Ok(Applied {
                    inverse,
                    events: vec![Event::OutputChanged { id: *id }],
                })
            }
            Self::SetOutputDisplay { id, display } => {
                let at = output_index(project, *id)?;
                let old = std::mem::replace(&mut project.outputs[at].display, display.clone());
                Ok(Applied {
                    inverse: Self::SetOutputDisplay {
                        id: *id,
                        display: old,
                    },
                    events: vec![Event::OutputChanged { id: *id }],
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
            Self::SetSurfaceShape { .. } => "Edit Shape",
            Self::SetSurfaceBlend { .. } => "Blend Mode",
            Self::SetSurfaceMask { .. } => "Edit Mask",
            Self::SetSurfaceEffects { .. } => "Edit Effects",
            Self::SetSurfaceMedia { .. } => "Assign Media",
            Self::AddMedia { .. } => "Add Media",
            Self::SetMediaPlayback { .. } => "Playback Settings",
            Self::RemoveMedia { .. } => "Remove Media",
            Self::SetCanvas { .. } => "Canvas Size",
            Self::AddOutput { .. } => "Add Output",
            Self::RemoveOutput { .. } => "Remove Output",
            Self::UpdateOutput { .. } => "Edit Output",
            Self::SetOutputDisplay { .. } => "Assign Display",
            Self::SetExtension { .. } => "Edit Extension",
        }
    }
}
