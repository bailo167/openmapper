// SPDX-License-Identifier: Apache-2.0
//! Controllable parameters.
//!
//! A [`ParamId`] names one live-controllable value in a project. Its string
//! form (`surface/<id>/opacity`, `master/opacity`, …) is stable: it is what
//! cues, timelines, modulators and MIDI bindings store, and it maps directly
//! onto OpenMapper's OSC address space (`/openmapper/<param>`).

use std::fmt;
use std::str::FromStr;

use om_types::{MediaId, SurfaceId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One controllable value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamId {
    /// Overall output level, 0..=1.
    MasterOpacity,
    /// Output forced to black.
    MasterBlackout,
    SurfaceOpacity(SurfaceId),
    SurfaceEnabled(SurfaceId),
    /// Playback speed (ratio, -16..=16).
    MediaSpeed(MediaId),
    MediaVolume(MediaId),
    /// A numeric or bool input of a shader generator.
    ShaderInput(MediaId, String),
}

/// Kind and range of a parameter's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    Float { min: f64, max: f64 },
    Bool,
}

/// A parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Float(f64),
}

impl ParamValue {
    /// Numeric view (bools are 0/1).
    #[must_use]
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Bool(b) => f64::from(u8::from(b)),
            Self::Float(f) => f,
        }
    }

    #[must_use]
    pub fn as_bool(self) -> bool {
        match self {
            Self::Bool(b) => b,
            Self::Float(f) => f >= 0.5,
        }
    }

    /// Converts to `kind`, clamping floats to range. Non-finite floats
    /// become the range minimum.
    #[must_use]
    pub fn coerce(self, kind: ParamKind) -> Self {
        match kind {
            ParamKind::Bool => Self::Bool(self.as_bool()),
            ParamKind::Float { min, max } => {
                let v = self.as_f64();
                Self::Float(if v.is_finite() {
                    v.clamp(min, max)
                } else {
                    min
                })
            }
        }
    }
}

/// Bad parameter string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown parameter {0:?}")]
pub struct ParamParseError(pub String);

impl fmt::Display for ParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MasterOpacity => f.write_str("master/opacity"),
            Self::MasterBlackout => f.write_str("master/blackout"),
            Self::SurfaceOpacity(id) => write!(f, "surface/{id}/opacity"),
            Self::SurfaceEnabled(id) => write!(f, "surface/{id}/enabled"),
            Self::MediaSpeed(id) => write!(f, "media/{id}/speed"),
            Self::MediaVolume(id) => write!(f, "media/{id}/volume"),
            Self::ShaderInput(id, name) => write!(f, "media/{id}/input/{name}"),
        }
    }
}

impl FromStr for ParamId {
    type Err = ParamParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ParamParseError(s.to_owned());
        let parts: Vec<&str> = s.trim_matches('/').split('/').collect();
        Ok(match parts.as_slice() {
            ["master", "opacity"] => Self::MasterOpacity,
            ["master", "blackout"] => Self::MasterBlackout,
            ["surface", id, "opacity"] => Self::SurfaceOpacity(id.parse().map_err(|_| bad())?),
            ["surface", id, "enabled"] => Self::SurfaceEnabled(id.parse().map_err(|_| bad())?),
            ["media", id, "speed"] => Self::MediaSpeed(id.parse().map_err(|_| bad())?),
            ["media", id, "volume"] => Self::MediaVolume(id.parse().map_err(|_| bad())?),
            ["media", id, "input", name] if !name.is_empty() => {
                Self::ShaderInput(id.parse().map_err(|_| bad())?, (*name).to_owned())
            }
            _ => return Err(bad()),
        })
    }
}

impl Serialize for ParamId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ParamId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_form_round_trips() {
        let s = SurfaceId::from_u128(1);
        let m = MediaId::from_u128(2);
        for p in [
            ParamId::MasterOpacity,
            ParamId::MasterBlackout,
            ParamId::SurfaceOpacity(s),
            ParamId::SurfaceEnabled(s),
            ParamId::MediaSpeed(m),
            ParamId::MediaVolume(m),
            ParamId::ShaderInput(m, "scale".into()),
        ] {
            let text = p.to_string();
            assert_eq!(text.parse::<ParamId>().unwrap(), p, "{text}");
            assert_eq!(
                format!("/{text}").parse::<ParamId>().unwrap(),
                p,
                "leading slash"
            );
        }
        for bad in [
            "",
            "master",
            "surface/x/opacity",
            "media/00000000000000000000000002/input/",
            "nope/x",
        ] {
            assert!(bad.parse::<ParamId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn coercion_clamps_and_converts() {
        let k = ParamKind::Float { min: 0.0, max: 1.0 };
        assert_eq!(ParamValue::Float(3.0).coerce(k), ParamValue::Float(1.0));
        assert_eq!(
            ParamValue::Float(f64::NAN).coerce(k),
            ParamValue::Float(0.0)
        );
        assert_eq!(ParamValue::Bool(true).coerce(k), ParamValue::Float(1.0));
        assert_eq!(
            ParamValue::Float(0.7).coerce(ParamKind::Bool),
            ParamValue::Bool(true)
        );
    }
}
