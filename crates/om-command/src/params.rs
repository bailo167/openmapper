// SPDX-License-Identifier: Apache-2.0
//! The parameter registry: what can be controlled live, its type and range,
//! how to read it, and how to change it.
//!
//! Direct control (OSC, MIDI, UI) changes parameters through ordinary
//! [`Command`]s ([`set_command`]), so they are undoable and saved. Cues,
//! timelines and modulators instead produce per-frame *overrides*, applied
//! to a derived copy of the project for rendering ([`apply_overrides`]),
//! so they never touch the document or its undo history.

use std::collections::BTreeMap;

use om_project::{ParamId, ParamKind, ParamValue, Project, ShaderValue};
use om_time::Speed;
use om_types::{Finite, UnitInterval};

use crate::Command;

/// A parameter available in a project.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    pub id: ParamId,
    pub kind: ParamKind,
    /// Human-readable name ("Wall opacity").
    pub label: String,
}

const UNIT: ParamKind = ParamKind::Float { min: 0.0, max: 1.0 };
const SPEED: ParamKind = ParamKind::Float {
    min: -16.0,
    max: 16.0,
};

fn shader_input_kind(project: &Project, media: om_types::MediaId, name: &str) -> Option<ParamKind> {
    let m = project.media_item(media)?;
    let om_project::MediaSource::Shader { inputs, .. } = &m.source else {
        return None;
    };
    // Range comes from the shader header, which the document does not hold;
    // without it, floats are unbounded and bools are bools.
    Some(match inputs.get(name) {
        Some(ShaderValue::Bool(_)) => ParamKind::Bool,
        _ => ParamKind::Float {
            min: f64::MIN,
            max: f64::MAX,
        },
    })
}

/// Type and range of `id` in `project`, or `None` if its target is gone.
#[must_use]
pub fn kind(project: &Project, id: &ParamId) -> Option<ParamKind> {
    match id {
        ParamId::MasterOpacity => Some(UNIT),
        ParamId::MasterBlackout => Some(ParamKind::Bool),
        ParamId::SurfaceOpacity(s) => project.surface(*s).map(|_| UNIT),
        ParamId::SurfaceEnabled(s) => project.surface(*s).map(|_| ParamKind::Bool),
        ParamId::MediaSpeed(m) => project.media_item(*m).map(|_| SPEED),
        ParamId::MediaVolume(m) => project.media_item(*m).map(|_| UNIT),
        ParamId::ShaderInput(m, name) => shader_input_kind(project, *m, name),
    }
}

/// Current document value.
#[must_use]
pub fn get(project: &Project, id: &ParamId) -> Option<ParamValue> {
    Some(match id {
        ParamId::MasterOpacity => ParamValue::Float(project.master.opacity.get()),
        ParamId::MasterBlackout => ParamValue::Bool(project.master.blackout),
        ParamId::SurfaceOpacity(s) => ParamValue::Float(project.surface(*s)?.opacity.get()),
        ParamId::SurfaceEnabled(s) => ParamValue::Bool(project.surface(*s)?.enabled),
        ParamId::MediaSpeed(m) => {
            ParamValue::Float(project.media_item(*m)?.playback.speed.as_f64())
        }
        ParamId::MediaVolume(m) => ParamValue::Float(project.media_item(*m)?.playback.volume.get()),
        ParamId::ShaderInput(m, name) => {
            let om_project::MediaSource::Shader { inputs, .. } = &project.media_item(*m)?.source
            else {
                return None;
            };
            match inputs.get(name) {
                Some(ShaderValue::Bool(b)) => ParamValue::Bool(*b),
                Some(ShaderValue::Number(n)) => ParamValue::Float(n.get()),
                Some(ShaderValue::Vector(v)) => {
                    ParamValue::Float(v.first().map_or(0.0, |f| f.get()))
                }
                None => return None,
            }
        }
    })
}

/// Speed from a float, as an exact ratio with denominator 1000.
#[allow(clippy::cast_possible_truncation)]
fn speed_of(v: f64) -> Speed {
    Speed::new((v.clamp(-16.0, 16.0) * 1000.0).round() as i32, 1000).unwrap_or(Speed::NORMAL)
}

/// The command that sets `id` to `value` in the document.
#[must_use]
pub fn set_command(project: &Project, id: &ParamId, value: ParamValue) -> Option<Command> {
    let value = value.coerce(kind(project, id)?);
    Some(match id {
        ParamId::MasterOpacity => Command::SetMaster {
            master: om_project::Master {
                opacity: UnitInterval::saturating(value.as_f64()),
                ..project.master
            },
        },
        ParamId::MasterBlackout => Command::SetMaster {
            master: om_project::Master {
                blackout: value.as_bool(),
                ..project.master
            },
        },
        ParamId::SurfaceOpacity(s) => Command::UpdateSurface {
            id: *s,
            name: None,
            enabled: None,
            opacity: Some(UnitInterval::saturating(value.as_f64())),
        },
        ParamId::SurfaceEnabled(s) => Command::UpdateSurface {
            id: *s,
            name: None,
            enabled: Some(value.as_bool()),
            opacity: None,
        },
        ParamId::MediaSpeed(m) => {
            let mut playback = project.media_item(*m)?.playback;
            playback.speed = speed_of(value.as_f64());
            Command::SetMediaPlayback { id: *m, playback }
        }
        ParamId::MediaVolume(m) => {
            let mut playback = project.media_item(*m)?.playback;
            playback.volume = UnitInterval::saturating(value.as_f64());
            Command::SetMediaPlayback { id: *m, playback }
        }
        ParamId::ShaderInput(m, name) => {
            let mut source = project.media_item(*m)?.source.clone();
            if let om_project::MediaSource::Shader { inputs, .. } = &mut source {
                inputs.insert(name.clone(), shader_value(value)?);
            }
            Command::SetMediaSource { id: *m, source }
        }
    })
}

fn shader_value(v: ParamValue) -> Option<ShaderValue> {
    Some(match v {
        ParamValue::Bool(b) => ShaderValue::Bool(b),
        ParamValue::Float(f) => ShaderValue::Number(Finite::new(f).ok()?),
    })
}

/// Every parameter the project currently offers, in a stable order.
#[must_use]
pub fn list(project: &Project) -> Vec<ParamInfo> {
    let mut out = vec![
        ParamInfo {
            id: ParamId::MasterOpacity,
            kind: UNIT,
            label: "Master opacity".into(),
        },
        ParamInfo {
            id: ParamId::MasterBlackout,
            kind: ParamKind::Bool,
            label: "Blackout".into(),
        },
    ];
    for s in &project.surfaces {
        out.push(ParamInfo {
            id: ParamId::SurfaceOpacity(s.id),
            kind: UNIT,
            label: format!("{} opacity", s.name),
        });
        out.push(ParamInfo {
            id: ParamId::SurfaceEnabled(s.id),
            kind: ParamKind::Bool,
            label: format!("{} enabled", s.name),
        });
    }
    for m in &project.media {
        if m.source.is_time_based() {
            out.push(ParamInfo {
                id: ParamId::MediaSpeed(m.id),
                kind: SPEED,
                label: format!("{} speed", m.name),
            });
        }
        if matches!(m.source, om_project::MediaSource::Video { .. }) {
            out.push(ParamInfo {
                id: ParamId::MediaVolume(m.id),
                kind: UNIT,
                label: format!("{} volume", m.name),
            });
        }
        if let om_project::MediaSource::Shader { inputs, .. } = &m.source {
            for name in inputs.keys() {
                let id = ParamId::ShaderInput(m.id, name.clone());
                if let Some(kind) = kind(project, &id) {
                    out.push(ParamInfo {
                        id,
                        kind,
                        label: format!("{} {name}", m.name),
                    });
                }
            }
        }
    }
    out
}

/// Applies live overrides to a *derived copy* of a project for rendering.
/// Unknown targets (deleted surfaces, …) are ignored.
pub fn apply_overrides(project: &mut Project, overrides: &BTreeMap<ParamId, ParamValue>) {
    for (id, value) in overrides {
        let Some(k) = kind(project, id) else { continue };
        let v = value.coerce(k);
        match id {
            ParamId::MasterOpacity => project.master.opacity = UnitInterval::saturating(v.as_f64()),
            ParamId::MasterBlackout => project.master.blackout = v.as_bool(),
            ParamId::SurfaceOpacity(s) => {
                if let Some(x) = project.surface_mut(*s) {
                    x.opacity = UnitInterval::saturating(v.as_f64());
                }
            }
            ParamId::SurfaceEnabled(s) => {
                if let Some(x) = project.surface_mut(*s) {
                    x.enabled = v.as_bool();
                }
            }
            ParamId::MediaSpeed(m) | ParamId::MediaVolume(m) | ParamId::ShaderInput(m, _) => {
                if let Some(item) = project.media.iter_mut().find(|x| x.id == *m) {
                    match id {
                        ParamId::MediaSpeed(_) => item.playback.speed = speed_of(v.as_f64()),
                        ParamId::MediaVolume(_) => {
                            item.playback.volume = UnitInterval::saturating(v.as_f64())
                        }
                        _ => {
                            if let (
                                ParamId::ShaderInput(_, name),
                                om_project::MediaSource::Shader { inputs, .. },
                                Some(sv),
                            ) = (id, &mut item.source, shader_value(v))
                            {
                                inputs.insert(name.clone(), sv);
                            }
                        }
                    }
                }
            }
        }
    }
}
