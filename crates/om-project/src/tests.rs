// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::store::{self, Journal, JournalEntry};
use om_types::{ProjectId, SurfaceId, UnitInterval};
use proptest::prelude::*;

fn sample() -> Project {
    let mut p = Project::new("Sample");
    p.project_id = ProjectId::from_u128(1);
    p.surfaces
        .push(Surface::new(SurfaceId::from_u128(2), "Quad 1"));
    p.extensions.insert(
        "org.example.future".into(),
        serde_json::json!({"z": [1, 2, {"nested": true}], "a": "keep me"}),
    );
    p
}

#[test]
fn canonical_json_round_trips_byte_identically() {
    let p = sample();
    let text = p.to_canonical_json().unwrap();
    let loaded = Project::from_json(&text).unwrap();
    assert_eq!(loaded.migrated_from, None);
    assert_eq!(loaded.project, p);
    assert_eq!(loaded.project.to_canonical_json().unwrap(), text);
}

#[test]
fn golden_v1_document_is_stable() {
    // Guards the on-disk format: changing this requires a deliberate schema
    // decision (and a migration if incompatible).
    let expected = include_str!("../tests/golden/sample-v1.omproj");
    assert_eq!(sample().to_canonical_json().unwrap(), expected);
    assert_eq!(Project::from_json(expected).unwrap().project, sample());
}

#[test]
fn unknown_extension_payloads_survive() {
    let mut text = sample().to_canonical_json().unwrap();
    text = text.replace("\"keep me\"", "\"keep me\", \"b\": 1e3");
    let p = Project::from_json(&text).unwrap().project;
    assert_eq!(
        p.extensions["org.example.future"]["b"],
        serde_json::json!(1000.0)
    );
}

#[test]
fn rejects_wrong_format_future_version_and_garbage() {
    assert!(matches!(
        Project::from_json(r#"{"format":"other","version":1}"#),
        Err(ProjectError::WrongFormat { .. })
    ));
    assert!(matches!(
        Project::from_json(r#"{"format":"openmapper-project","version":99}"#),
        Err(ProjectError::FutureVersion { found: 99 })
    ));
    assert!(matches!(
        Project::from_json(r#"{"format":"openmapper-project","version":0}"#),
        Err(ProjectError::BadVersion)
    ));
    assert!(matches!(
        Project::from_json("{not json"),
        Err(ProjectError::Json(_))
    ));
    assert!(Project::from_json("[]").is_err());
}

#[test]
fn rejects_invalid_content() {
    let base = sample().to_canonical_json().unwrap();
    // Out-of-range opacity.
    let bad = base.replace("\"opacity\": 1.0", "\"opacity\": 2.0");
    assert!(Project::from_json(&bad).is_err());
    // Duplicate surface id.
    let mut p = sample();
    p.surfaces.push(p.surfaces[0].clone());
    assert!(matches!(p.validate(), Err(ProjectError::Invalid(_))));
    // Empty name.
    let mut p = sample();
    p.name = "  ".into();
    assert!(p.validate().is_err());
    // Unknown root field (typo protection).
    let bad = base.replacen('{', "{\"surfacez\": [],", 1);
    assert!(Project::from_json(&bad).is_err());
}

#[test]
fn atomic_save_writes_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("show.omproj");
    let mut p = sample();
    store::save_atomic(&path, &p).unwrap();
    assert!(!store::temp_path(&path).exists());
    p.name = "Renamed".into();
    store::save_atomic(&path, &p).unwrap();
    assert_eq!(store::load(&path).unwrap().project.name, "Renamed");
}

#[test]
fn invalid_project_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("show.omproj");
    store::save_atomic(&path, &sample()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut bad = sample();
    bad.name.clear();
    assert!(store::save_atomic(&path, &bad).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn leftover_temp_file_does_not_affect_load() {
    // Simulates a crash after writing the temp file but before rename.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("show.omproj");
    store::save_atomic(&path, &sample()).unwrap();
    std::fs::write(store::temp_path(&path), "{ torn").unwrap();
    assert_eq!(store::load(&path).unwrap().project, sample());
    // And the next save overwrites the stale temp file.
    store::save_atomic(&path, &sample()).unwrap();
    assert!(!store::temp_path(&path).exists());
}

#[test]
fn journal_round_trip_and_torn_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("show.omproj");
    let p = sample();
    let mut j = Journal::create(&path, &p).unwrap();
    for rev in 1..=3u64 {
        j.append(&JournalEntry {
            revision: rev,
            command: format!("cmd{rev}"),
        })
        .unwrap();
    }
    drop(j);
    // Torn final line from a crash mid-append.
    let jp = store::journal_path(&path);
    let mut text = std::fs::read_to_string(&jp).unwrap();
    text.push_str("{\"revision\":4,\"comm");
    std::fs::write(&jp, text).unwrap();

    let c = Journal::read::<String>(&path).unwrap().unwrap();
    assert_eq!(c.project_id, p.project_id);
    assert_eq!(c.base_revision, 0);
    assert_eq!(c.entries.len(), 3);
    assert!(c.truncated_tail);

    Journal::remove(&path).unwrap();
    assert!(Journal::read::<String>(&path).unwrap().is_none());
}

#[test]
fn corrupt_journal_middle_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("show.omproj");
    let jp = store::journal_path(&path);
    Journal::create(&path, &sample()).unwrap();
    let mut text = std::fs::read_to_string(&jp).unwrap();
    text.push_str("garbage\n{\"revision\":2,\"command\":\"x\"}\n");
    std::fs::write(&jp, text).unwrap();
    assert!(Journal::read::<String>(&path).is_err());
}

proptest! {
    #[test]
    fn arbitrary_surfaces_round_trip(
        names in proptest::collection::vec("[a-zA-Z0-9 ]{1,12}[a-zA-Z0-9]", 0..8),
        opacities in proptest::collection::vec(0.0f64..=1.0, 8),
    ) {
        let mut p = Project::new("P");
        for (i, name) in names.iter().enumerate() {
            let mut s = Surface::new(SurfaceId::from_u128(i as u128 + 10), name.clone());
            s.opacity = UnitInterval::new(opacities[i]).unwrap();
            s.enabled = i % 2 == 0;
            p.surfaces.push(s);
        }
        let text = p.to_canonical_json().unwrap();
        let back = Project::from_json(&text).unwrap().project;
        prop_assert_eq!(&back, &p);
        prop_assert_eq!(back.to_canonical_json().unwrap(), text);
    }
}

#[test]
fn mask_flattening() {
    use crate::{Mask, MaskPoint};
    let pt = |x: f64, y: f64, smooth: bool| MaskPoint {
        p: om_geom::Point2::new(x, y).unwrap(),
        smooth,
    };
    let corners = Mask {
        points: vec![
            pt(0.1, 0.1, false),
            pt(0.9, 0.1, false),
            pt(0.5, 0.9, false),
        ],
        feather: om_types::Finite::ZERO,
        invert: false,
    };
    assert_eq!(
        corners.flatten(0.001, 1.0),
        vec![(0.1, 0.1), (0.9, 0.1), (0.5, 0.9)]
    );
    // All-smooth square becomes a rounded closed curve through its points.
    let round = Mask {
        points: vec![
            pt(0.2, 0.2, true),
            pt(0.8, 0.2, true),
            pt(0.8, 0.8, true),
            pt(0.2, 0.8, true),
        ],
        feather: om_types::Finite::ZERO,
        invert: false,
    };
    let poly = round.flatten(0.0005, 1.0);
    assert!(poly.len() > 20, "curves are subdivided: {}", poly.len());
    for p in [(0.2, 0.2), (0.8, 0.2), (0.8, 0.8), (0.2, 0.8)] {
        assert!(poly.contains(&p), "passes through control point {p:?}");
    }
    // Finer tolerance never gives fewer points.
    assert!(round.flatten(0.0001, 1.0).len() >= poly.len());
    assert!(
        Mask {
            points: vec![pt(0.0, 0.0, false); 2],
            ..corners.clone()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn effects_round_trip_and_validate() {
    use crate::{Effect, EffectKind};
    let mut p = sample();
    p.surfaces[0].effects = vec![
        Effect {
            enabled: true,
            kind: EffectKind::neutral_color(),
        },
        Effect {
            enabled: false,
            kind: EffectKind::Blur {
                radius: om_types::Finite::new(4.0).unwrap(),
            },
        },
        Effect {
            enabled: true,
            kind: EffectKind::Pixelate { size: 8 },
        },
        Effect {
            enabled: true,
            kind: EffectKind::Invert {},
        },
    ];
    let text = p.to_canonical_json().unwrap();
    assert!(text.contains(r#""effect": "blur""#), "{text}");
    let back = Project::from_json(&text).unwrap().project;
    assert_eq!(back, p);
    let bad = text.replace(r#""size": 8"#, r#""size": 0"#);
    assert!(Project::from_json(&bad).is_err());
    let unknown = text.replace(r#""effect": "invert""#, r#""effect": "invert", "bogus": 1"#);
    assert!(
        Project::from_json(&unknown).is_err(),
        "unknown effect fields are rejected"
    );
}
