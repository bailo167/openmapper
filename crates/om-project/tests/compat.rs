// SPDX-License-Identifier: Apache-2.0
//! Compatibility fixtures: project files written by earlier OpenMapper
//! versions must keep loading. `v1-compat.omproj` exercises every major
//! feature of format version 1 (milestones 1–8); it must load, validate,
//! and serialise back to exactly the same bytes for as long as version 1
//! is current, and migrate forward once it is not.

#![allow(clippy::unwrap_used)]

use om_project::{CURRENT_VERSION, Project};

const V1: &str = include_str!("fixtures/v1-compat.omproj");

#[test]
fn version_1_fixture_loads_and_round_trips() {
    let loaded = Project::from_json(V1).unwrap();
    let p = &loaded.project;
    assert_eq!(p.version, CURRENT_VERSION);
    assert_eq!(p.media.len(), 6);
    assert_eq!(p.surfaces.len(), 2);
    assert_eq!(p.outputs.len(), 2);
    assert!(p.outputs[1].projection.is_some());
    assert_eq!(p.dmx.fixtures.len(), 1);
    assert!(p.extensions.contains_key("com.example.plugin"));
    if CURRENT_VERSION == 1 {
        assert_eq!(loaded.migrated_from, None);
        assert_eq!(
            p.to_canonical_json().unwrap(),
            V1,
            "byte-identical round trip"
        );
    } else {
        assert_eq!(loaded.migrated_from, Some(1));
    }
}
