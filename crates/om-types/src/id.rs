// SPDX-License-Identifier: Apache-2.0
//! Stable opaque identifiers for persistent objects.
//!
//! IDs are ULIDs serialised as 26-character Crockford base32 strings. Each kind
//! of object gets its own type so a `SurfaceId` can never be passed where an
//! `OutputId` is expected.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

/// Error returned when a string is not a valid ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} id {value:?}: expected a 26-character ULID")]
pub struct IdParseError {
    kind: &'static str,
    value: String,
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Ulid);

        impl $name {
            /// Generates a new unique ID from the current time and system randomness.
            #[must_use]
            pub fn new() -> Self {
                Self(Ulid::generate())
            }

            /// Builds an ID from its raw 128-bit value. Intended for tests and
            /// deterministic fixtures.
            #[must_use]
            pub const fn from_u128(value: u128) -> Self {
                Self(Ulid(value))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($kind, "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_string(s).map(Self).map_err(|_| IdParseError {
                    kind: $kind,
                    value: s.to_owned(),
                })
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id!(
    /// Identifies a project across saves and renames.
    ProjectId,
    "project"
);
define_id!(
    /// Identifies a mapping surface.
    SurfaceId,
    "surface"
);
define_id!(
    /// Identifies a media item.
    MediaId,
    "media"
);
define_id!(
    /// Identifies an output.
    OutputId,
    "output"
);
define_id!(
    /// Identifies a cue.
    CueId,
    "cue"
);
define_id!(
    /// Identifies a timeline.
    TimelineId,
    "timeline"
);
define_id!(
    /// Identifies a modulator.
    ModulatorId,
    "modulator"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_string_and_json() {
        let id = SurfaceId::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let text = id.to_string();
        assert_eq!(text.len(), 26);
        assert_eq!(text.parse::<SurfaceId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<SurfaceId>(&json).unwrap(), id);
    }

    #[test]
    fn rejects_garbage_with_kind_in_message() {
        let err = "not-an-id".parse::<OutputId>().unwrap_err();
        assert!(err.to_string().contains("output"));
        assert!(serde_json::from_str::<OutputId>("\"xyz\"").is_err());
    }

    #[test]
    fn new_ids_are_unique() {
        assert_ne!(MediaId::new(), MediaId::new());
    }
}
