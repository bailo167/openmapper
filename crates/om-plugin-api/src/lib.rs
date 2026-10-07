// SPDX-License-Identifier: Apache-2.0
//! The OpenMapper plugin ABI, version 1 (`openmapper:plugin@1.0.0`).
//!
//! A plugin is a core WebAssembly module. It is versioned independently of
//! the project format. The host gives it **no** filesystem, network,
//! environment or clock access beyond the capabilities below, and it must
//! declare those capabilities in its manifest.
//!
//! # Guest exports
//!
//! | Export | Signature | Meaning |
//! |---|---|---|
//! | `memory` | memory | linear memory shared with the host |
//! | `om_abi_version` | `() -> i32` | must return [`ABI_VERSION`] |
//! | `om_manifest` | `() -> i64` | `ptr << 32 \| len` of the UTF-8 JSON [`Manifest`] |
//! | `om_alloc` | `(size: i32) -> i32` | guest buffer of `size` bytes (0 = failure) |
//! | `om_process` | `(in, width, height, params, n_params, out) -> i32` | effect: reads `width·height·4` bytes of sRGB RGBA8 (straight alpha) at `in` and `n_params` `f32`s at `params`, writes the same size at `out`; returns 0 on success |
//!
//! # Host imports (module `openmapper`)
//!
//! | Import | Signature | Capability |
//! |---|---|---|
//! | `log` | `(ptr: i32, len: i32)` | [`Capability::Log`] |
//! | `time` | `() -> f64` (show seconds) | [`Capability::Time`] |
//!
//! Any other import makes the plugin fail to load.

use serde::{Deserialize, Serialize};

/// The ABI version this host implements.
pub const ABI_VERSION: i32 = 1;

/// The import module name.
pub const HOST_MODULE: &str = "openmapper";

/// Required exports.
pub const EXPORTS: [&str; 5] = [
    "memory",
    "om_abi_version",
    "om_manifest",
    "om_alloc",
    "om_process",
];

/// Most parameters a plugin may declare.
pub const MAX_PARAMS: usize = 64;

/// What a plugin may ask of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Write messages to OpenMapper's log.
    Log,
    /// Read the show clock.
    Time,
}

impl Capability {
    /// The host import that requires this capability.
    #[must_use]
    pub const fn import(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Time => "time",
        }
    }

    /// The capability an import requires, if the host offers it.
    #[must_use]
    pub fn for_import(name: &str) -> Option<Self> {
        match name {
            "log" => Some(Self::Log),
            "time" => Some(Self::Time),
            _ => None,
        }
    }
}

/// One numeric parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamDecl {
    pub name: String,
    #[serde(default)]
    pub min: f32,
    #[serde(default = "one")]
    pub max: f32,
    #[serde(default)]
    pub default: f32,
}

fn one() -> f32 {
    1.0
}

/// What a plugin says about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    pub abi: i32,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
}

/// Why a manifest is unacceptable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest is not valid JSON: {0}")]
    Json(String),
    #[error("plugin targets ABI {0}; this OpenMapper supports ABI {ABI_VERSION}")]
    Abi(i32),
    #[error("manifest: {0}")]
    Invalid(String),
}

impl Manifest {
    /// Parses and validates a manifest.
    pub fn parse(json: &[u8]) -> Result<Self, ManifestError> {
        let m: Self =
            serde_json::from_slice(json).map_err(|e| ManifestError::Json(e.to_string()))?;
        if m.abi != ABI_VERSION {
            return Err(ManifestError::Abi(m.abi));
        }
        if m.name.trim().is_empty() || m.name.len() > 128 {
            return Err(ManifestError::Invalid("name must be 1–128 bytes".into()));
        }
        if m.params.len() > MAX_PARAMS {
            return Err(ManifestError::Invalid(format!(
                "more than {MAX_PARAMS} parameters"
            )));
        }
        for p in &m.params {
            let finite = [p.min, p.max, p.default].iter().all(|v| v.is_finite());
            if !finite || p.min > p.max || !(p.min..=p.max).contains(&p.default) {
                return Err(ManifestError::Invalid(format!(
                    "parameter {:?} has an invalid range",
                    p.name
                )));
            }
        }
        Ok(m)
    }

    /// Parameter values in declaration order: `given` where provided
    /// (clamped to range), defaults elsewhere.
    #[must_use]
    pub fn resolve_params(&self, given: &std::collections::BTreeMap<String, f32>) -> Vec<f32> {
        self.params
            .iter()
            .map(|p| {
                given
                    .get(&p.name)
                    .filter(|v| v.is_finite())
                    .map_or(p.default, |v| v.clamp(p.min, p.max))
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn manifests_parse_and_validate() {
        let m = Manifest::parse(
            br#"{"name":"Invert","version":"1.0.0","abi":1,"capabilities":["log"],
                 "params":[{"name":"amount","min":0,"max":1,"default":1}]}"#,
        )
        .unwrap();
        assert_eq!(m.capabilities, vec![Capability::Log]);
        let mut given = std::collections::BTreeMap::new();
        assert_eq!(m.resolve_params(&given), vec![1.0]);
        given.insert("amount".into(), 7.0);
        assert_eq!(m.resolve_params(&given), vec![1.0], "clamped");
        given.insert("amount".into(), f32::NAN);
        assert_eq!(
            m.resolve_params(&given),
            vec![1.0],
            "NaN falls back to the default"
        );

        assert_eq!(
            Manifest::parse(br#"{"name":"x","abi":2}"#),
            Err(ManifestError::Abi(2))
        );
        assert!(Manifest::parse(br#"{"name":"","abi":1}"#).is_err());
        assert!(
            Manifest::parse(br#"{"name":"x","abi":1,"params":[{"name":"p","min":1,"max":0}]}"#)
                .is_err()
        );
        assert!(Manifest::parse(br#"{"name":"x","abi":1,"capabilities":["network"]}"#).is_err());
        assert!(Manifest::parse(b"not json").is_err());
    }

    #[test]
    fn capabilities_map_to_imports() {
        for c in [Capability::Log, Capability::Time] {
            assert_eq!(Capability::for_import(c.import()), Some(c));
        }
        assert_eq!(Capability::for_import("fd_write"), None);
    }
}
