// SPDX-License-Identifier: Apache-2.0
//! Project trust: a project file may come from someone else, so the things
//! it can make this computer do *outside* the app — use the camera, send
//! video or DMX to network addresses, accept remote control — stay off
//! until the user allows them for that project (DECISIONS.md D-029).
//!
//! [`external`] lists those connections; [`restrict`] removes them from the
//! project the runtimes see. Allowing a project stores a fingerprint of
//! its id and connection list in a per-user [`TrustStore`], so the same
//! project reopens without asking, but any new connection asks again.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use om_project::{LiveInput, MediaSource, PatternKind, Project, Publish};

/// Human-readable connections the project makes beyond the app, sorted
/// and deduplicated. Empty for a project that needs no permission.
#[must_use]
pub fn external(project: &Project) -> Vec<String> {
    let mut out = BTreeSet::new();
    for m in &project.media {
        if let MediaSource::Live { input } = &m.source {
            match input {
                LiveInput::Camera { device } => {
                    out.insert(format!("camera input {device:?}"));
                }
                LiveInput::Stream { url } => {
                    out.insert(format!("network stream input {url}"));
                }
                LiveInput::Ndi { source } => {
                    out.insert(format!("NDI input {source:?}"));
                }
                // Local inter-app sharing: nothing leaves the computer.
                LiveInput::Syphon { .. } | LiveInput::Spout { .. } => {}
            }
        }
    }
    for o in &project.outputs {
        for p in &o.publish {
            match p {
                Publish::Ndi { name } => {
                    out.insert(format!("NDI output {name:?} (visible on the network)"));
                }
                Publish::Stream { url, .. } => {
                    out.insert(format!("stream output to {url}"));
                }
                Publish::Syphon { .. } | Publish::Spout { .. } => {}
            }
        }
    }
    for n in project.dmx.nodes.iter().filter(|n| n.enabled) {
        out.insert(match &n.protocol {
            om_project::dmx::DmxProtocol::ArtNet { address } => {
                format!("DMX (Art-Net) to {address}")
            }
            om_project::dmx::DmxProtocol::Sacn { address, .. } if address.is_empty() => {
                "DMX (sACN multicast)".to_owned()
            }
            om_project::dmx::DmxProtocol::Sacn { address, .. } => {
                format!("DMX (sACN) to {address}")
            }
        });
    }
    let c = &project.controls;
    if c.network && (c.osc_port != 0 || c.oscquery_port != 0) {
        out.insert("OSC / OSCQuery control from other computers".to_owned());
    }
    if c.dmx_input.enabled {
        out.insert("DMX input (Art-Net / sACN) from the network".to_owned());
    }
    out.into_iter().collect()
}

/// Removes every [`external`] connection: blocked live inputs show a
/// checkerboard, network publish targets are dropped, DMX nodes are
/// disabled and control listens on this computer only.
pub fn restrict(project: &mut Project) {
    for m in &mut project.media {
        if let MediaSource::Live {
            input: LiveInput::Camera { .. } | LiveInput::Stream { .. } | LiveInput::Ndi { .. },
        } = &m.source
        {
            m.source = MediaSource::Pattern {
                pattern: PatternKind::Checkerboard,
            };
        }
    }
    for o in &mut project.outputs {
        o.publish
            .retain(|p| matches!(p, Publish::Syphon { .. } | Publish::Spout { .. }));
    }
    for n in &mut project.dmx.nodes {
        n.enabled = false;
    }
    project.controls.network = false;
    project.controls.dmx_input.enabled = false;
}

/// Identity of a project together with its connections (FNV-1a 64; stable
/// across builds, unlike std's hasher).
#[must_use]
pub fn fingerprint(project: &Project) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for item in external(project) {
        for b in item.bytes().chain([0]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{}:{h:016x}", project.project_id)
}

/// Most fingerprints remembered (oldest are forgotten).
pub const MAX_TRUSTED: usize = 1000;

/// Allowed project fingerprints, persisted per user.
#[derive(Debug, Default)]
pub struct TrustStore {
    path: Option<PathBuf>,
    /// Oldest first.
    allowed: Vec<String>,
}

impl TrustStore {
    /// The store at [`default_path`] (empty if missing or unreadable).
    #[must_use]
    pub fn load_default() -> Self {
        default_path().map_or_else(Self::default, |p| Self::load(&p))
    }

    /// The store at `path` (empty if missing or unreadable).
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let allowed = om_project::store::read_limited(path, 1 << 20)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default();
        Self {
            path: Some(path.to_owned()),
            allowed,
        }
    }

    /// True if `project` needs no permission or was allowed as it is now.
    #[must_use]
    pub fn is_trusted(&self, project: &Project) -> bool {
        external(project).is_empty() || self.allowed.contains(&fingerprint(project))
    }

    /// Allows `project` as it is now and saves the store.
    pub fn allow(&mut self, project: &Project) -> Result<(), String> {
        let f = fingerprint(project);
        self.allowed.retain(|a| *a != f);
        self.allowed.push(f);
        let excess = self.allowed.len().saturating_sub(MAX_TRUSTED);
        self.allowed.drain(..excess);
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&self.allowed).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

/// `<config dir>/OpenMapper/trusted-projects.json`: `%APPDATA%` on
/// Windows, `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME`
/// or `~/.config` elsewhere.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let env = |k| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        env("APPDATA")?
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?
    };
    Some(base.join("OpenMapper").join("trusted-projects.json"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use om_project::dmx::{DmxNode, DmxProtocol};
    use om_project::{Media, Output, StreamCodec};
    use om_types::{DmxNodeId, MediaId, OutputId};

    use super::*;

    fn hostile() -> Project {
        let mut p = Project::new("shared");
        let live = |id, input| Media {
            id: MediaId::from_u128(id),
            name: "in".into(),
            source: MediaSource::Live { input },
            playback: Default::default(),
            plugins: Vec::new(),
            extensions: Default::default(),
        };
        p.media.push(live(
            1,
            LiveInput::Camera {
                device: "FaceTime".into(),
            },
        ));
        p.media.push(live(
            2,
            LiveInput::Spout {
                sender: "local".into(),
            },
        ));
        p.outputs.push(Output {
            id: OutputId::from_u128(1),
            name: "out".into(),
            enabled: false,
            display: None,
            publish: vec![
                Publish::Stream {
                    url: "srt://203.0.113.9:9000".into(),
                    codec: StreamCodec::default(),
                    fps: 30,
                },
                Publish::Syphon {
                    name: "local".into(),
                },
            ],
            mapping: Default::default(),
            projection: None,
            extensions: Default::default(),
        });
        p.dmx.nodes.push(DmxNode {
            id: DmxNodeId::from_u128(1),
            name: "n".into(),
            enabled: true,
            protocol: DmxProtocol::ArtNet {
                address: "255.255.255.255".into(),
            },
        });
        p.controls.network = true;
        p.validate().unwrap();
        p
    }

    #[test]
    fn external_connections_are_listed_and_restricted() {
        assert!(external(&Project::new("plain")).is_empty());
        let p = hostile();
        assert_eq!(
            external(&p),
            vec![
                "DMX (Art-Net) to 255.255.255.255",
                "OSC / OSCQuery control from other computers",
                "camera input \"FaceTime\"",
                "stream output to srt://203.0.113.9:9000",
            ]
        );
        let mut r = p.clone();
        restrict(&mut r);
        assert!(external(&r).is_empty(), "{:?}", external(&r));
        r.validate().unwrap();
        assert!(
            matches!(r.media[1].source, MediaSource::Live { .. }),
            "local kept"
        );
        assert_eq!(r.outputs[0].publish.len(), 1, "Syphon kept");
    }

    #[test]
    fn allowing_is_remembered_until_connections_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/trusted.json");
        let mut p = hostile();
        let mut store = TrustStore::load(&path);
        assert!(!store.is_trusted(&p));
        assert!(store.is_trusted(&Project::new("plain")));
        store.allow(&p).unwrap();
        assert!(TrustStore::load(&path).is_trusted(&p), "persisted");
        // A new destination asks again.
        p.controls.dmx_input.enabled = true;
        assert!(!TrustStore::load(&path).is_trusted(&p));
        // Same contents under another project id: not trusted.
        let mut other = hostile();
        other.project_id = om_types::ProjectId::new();
        assert!(!store.is_trusted(&other));
        // Unreadable store: empty, not an error.
        std::fs::write(&path, "not json").unwrap();
        assert!(!TrustStore::load(&path).is_trusted(&hostile()));
    }

    #[test]
    fn a_blocked_live_session_keeps_control_local_and_output_restricted() {
        let mut session = crate::Session::from_project(hostile());
        let mut transport = crate::Transport::default();
        let mut live = crate::live::Live::without_devices();
        live.external_blocked = true;
        let effective = live.frame(&mut session, &mut transport, std::time::Instant::now());
        assert!(external(&effective).is_empty());
        assert!(
            live.servers.status.contains("this computer only"),
            "{}",
            live.servers.status
        );
        live.external_blocked = false;
        let effective = live.frame(&mut session, &mut transport, std::time::Instant::now());
        assert_eq!(external(&effective).len(), 4);
    }
}
