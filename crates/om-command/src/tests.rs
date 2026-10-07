// SPDX-License-Identifier: Apache-2.0

use crate::Event;
use crate::{Command, Document, HistoryError};
use om_geom::Point2;
use om_project::{
    Canvas, DisplayTarget, Media, MediaSource, Output, PatternKind, Project, Shape, Surface,
};
use om_types::{MediaId, OutputId, ProjectId, SurfaceId, UnitInterval};
use proptest::prelude::*;

fn sid(n: u128) -> SurfaceId {
    SurfaceId::from_u128(n)
}

fn project() -> Project {
    let mut p = Project::new("Show");
    p.project_id = ProjectId::from_u128(1);
    p
}

fn add(n: u128) -> Command {
    Command::AddSurface {
        surface: Surface::new(sid(n), format!("S{n}")),
        index: None,
    }
}

#[test]
fn execute_bumps_revision_and_emits_events() {
    let mut d = Document::new(project());
    let r = d.execute(add(1)).unwrap();
    assert_eq!(r.revision, 1);
    assert_eq!(r.events, vec![Event::SurfaceAdded { id: sid(1) }]);
    assert_eq!(d.project().surfaces.len(), 1);
}

#[test]
fn rejected_commands_leave_project_and_revision_untouched() {
    let mut d = Document::new(project());
    d.execute(add(1)).unwrap();
    let before = d.project().clone();
    let bad = [
        add(1),
        Command::RemoveSurface { id: sid(9) },
        Command::SetProjectName { name: " ".into() },
        Command::UpdateSurface {
            id: sid(1),
            name: Some(String::new()),
            enabled: Some(false),
            opacity: None,
        },
        Command::MoveSurface {
            id: sid(1),
            to_index: 5,
        },
        Command::AddSurface {
            surface: Surface::new(sid(2), "x"),
            index: Some(7),
        },
        Command::SetExtension {
            key: String::new(),
            value: None,
        },
    ];
    for c in bad {
        assert!(d.execute(c.clone()).is_err(), "{c:?} should fail");
        assert_eq!(d.project(), &before);
    }
}

#[test]
fn undo_redo_restore_exact_states() {
    let mut d = Document::new(project());
    let s0 = d.project().clone();
    d.execute(add(1)).unwrap();
    d.execute(add(2)).unwrap();
    d.execute(Command::RemoveSurface { id: sid(1) }).unwrap();
    let s3 = d.project().clone();
    d.undo().unwrap();
    assert_eq!(
        d.project().surfaces[0].id,
        sid(1),
        "re-inserted at original index"
    );
    d.undo().unwrap();
    d.undo().unwrap();
    assert_eq!(d.project().surfaces, s0.surfaces);
    assert!(matches!(d.undo(), Err(HistoryError::NothingToUndo)));
    d.redo().unwrap();
    d.redo().unwrap();
    d.redo().unwrap();
    assert_eq!(d.project().surfaces, s3.surfaces);
    assert!(matches!(d.redo(), Err(HistoryError::NothingToRedo)));
}

#[test]
fn new_command_clears_redo() {
    let mut d = Document::new(project());
    d.execute(add(1)).unwrap();
    d.undo().unwrap();
    assert!(d.can_redo());
    d.execute(add(2)).unwrap();
    assert!(!d.can_redo());
}

#[test]
fn coalescing_merges_drag_into_one_undo_step() {
    let mut d = Document::new(project());
    d.execute(add(1)).unwrap();
    for i in 1..=10 {
        let opacity = UnitInterval::new(f64::from(i) / 10.0).unwrap();
        d.execute_coalescing(
            Command::UpdateSurface {
                id: sid(1),
                name: None,
                enabled: None,
                opacity: Some(opacity),
            },
            Some("opacity".into()),
        )
        .unwrap();
    }
    assert_eq!(d.project().surfaces[0].opacity.get(), 1.0);
    d.undo().unwrap();
    assert_eq!(
        d.project().surfaces[0].opacity.get(),
        1.0,
        "back to pre-drag value"
    );
    assert_eq!(d.undo_label(), Some("Add Surface"));
    d.redo().unwrap();
    assert_eq!(d.project().surfaces[0].opacity.get(), 1.0);
}

#[test]
fn commands_have_stable_json_form() {
    let c = Command::UpdateSurface {
        id: sid(1),
        name: None,
        enabled: Some(false),
        opacity: None,
    };
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(
        json,
        r#"{"type":"update_surface","id":"00000000000000000000000001","enabled":false}"#
    );
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), c);
    assert!(serde_json::from_str::<Command>(r#"{"type":"explode"}"#).is_err());
    assert!(
        serde_json::from_str::<Command>(
            r#"{"type":"update_surface","id":"00000000000000000000000001","opacity":3}"#
        )
        .is_err(),
        "out-of-range opacity rejected at parse time"
    );
}

fn arb_shape() -> impl Strategy<Value = Shape> {
    let pt = (-0.5f64..1.5, -0.5f64..1.5).prop_map(|(x, y)| Point2::new(x, y).unwrap());
    prop_oneof![
        proptest::array::uniform4(pt.clone()).prop_map(|corners| Shape::Quad {
            corners,
            uv: Point2::unit_square()
        }),
        proptest::array::uniform3(pt).prop_map(|corners| Shape::Triangle {
            corners,
            uv: Point2::unit_triangle()
        }),
    ]
}

fn arb_new_command() -> impl Strategy<Value = Command> {
    let sid_s = (1u128..6).prop_map(sid);
    let mid = (1u128..4).prop_map(MediaId::from_u128);
    let oid = (1u128..3).prop_map(OutputId::from_u128);
    prop_oneof![
        (sid_s.clone(), arb_shape()).prop_map(|(id, shape)| Command::SetSurfaceShape { id, shape }),
        (sid_s, proptest::option::of(mid.clone()))
            .prop_map(|(id, media)| Command::SetSurfaceMedia { id, media }),
        (mid.clone(), proptest::option::of(0usize..4)).prop_map(|(id, index)| Command::AddMedia {
            media: Media {
                id,
                name: "m".into(),
                source: MediaSource::Pattern {
                    pattern: PatternKind::UvGrid
                },
                extensions: Default::default(),
            },
            index,
        }),
        mid.prop_map(|id| Command::RemoveMedia { id }),
        (0u32..5000, 0u32..5000).prop_map(|(width, height)| Command::SetCanvas {
            canvas: Canvas { width, height }
        }),
        oid.clone().prop_map(|id| Command::AddOutput {
            output: Output {
                id,
                name: "o".into(),
                enabled: false,
                display: None,
                extensions: Default::default(),
            },
            index: None,
        }),
        oid.clone().prop_map(|id| Command::RemoveOutput { id }),
        (oid.clone(), proptest::option::of(any::<bool>())).prop_map(|(id, enabled)| {
            Command::UpdateOutput {
                id,
                name: None,
                enabled,
            }
        }),
        (oid, proptest::option::of(0u32..3)).prop_map(|(id, i)| Command::SetOutputDisplay {
            id,
            display: i.map(|index| DisplayTarget {
                name: format!("D{index}"),
                index
            }),
        }),
    ]
}

fn arb_command() -> impl Strategy<Value = Command> {
    prop_oneof![arb_old_command(), arb_new_command()]
}

fn arb_old_command() -> impl Strategy<Value = Command> {
    let id = (1u128..6).prop_map(sid);
    prop_oneof![
        "[a-z]{0,4}".prop_map(|name| Command::SetProjectName { name }),
        (id.clone(), "[a-z]{0,3}", proptest::option::of(0usize..6)).prop_map(
            |(id, name, index)| {
                Command::AddSurface {
                    surface: Surface::new(id, name),
                    index,
                }
            }
        ),
        id.clone().prop_map(|id| Command::RemoveSurface { id }),
        (
            id.clone(),
            proptest::option::of("[a-z]{0,3}"),
            proptest::option::of(any::<bool>()),
            proptest::option::of(0.0f64..=1.0)
        )
            .prop_map(|(id, name, enabled, o)| Command::UpdateSurface {
                id,
                name,
                enabled,
                opacity: o.map(|v| UnitInterval::new(v).unwrap()),
            }),
        (id, 0usize..6).prop_map(|(id, to_index)| Command::MoveSurface { id, to_index }),
        ("[ab]{0,1}", proptest::option::of(any::<i32>())).prop_map(|(key, v)| {
            Command::SetExtension {
                key,
                value: v.map(serde_json::Value::from),
            }
        }),
    ]
}

proptest! {
    /// For any command sequence: undoing everything restores the start state,
    /// redoing everything restores the end state, and replaying the applied
    /// commands (the journal) on the start state reproduces every step.
    #[test]
    fn history_and_replay_are_exact(cmds in proptest::collection::vec(arb_command(), 0..60)) {
        let start = project();
        let mut d = Document::new(start.clone());
        let mut applied = Vec::new();
        for c in cmds {
            if let Ok(r) = d.execute(c) {
                applied.push(r.applied);
            }
        }
        let end = d.project().clone();

        let mut replay = Document::new(start.clone());
        for c in &applied {
            replay.execute(c.clone()).unwrap();
        }
        prop_assert_eq!(replay.project(), &end);

        let mut undone = 0;
        while d.can_undo() {
            d.undo().unwrap();
            undone += 1;
        }
        prop_assert_eq!(undone, applied.len());
        let mut back = d.project().clone();
        back.revision = start.revision;
        prop_assert_eq!(&back, &start);

        while d.can_redo() {
            d.redo().unwrap();
        }
        let mut fwd = d.project().clone();
        fwd.revision = end.revision;
        prop_assert_eq!(&fwd, &end);
    }
}

#[test]
fn media_in_use_cannot_be_removed() {
    let mut d = Document::new(project());
    let m = MediaId::from_u128(7);
    d.execute(Command::AddMedia {
        media: Media {
            id: m,
            name: "grid".into(),
            source: MediaSource::Pattern {
                pattern: PatternKind::UvGrid,
            },
            extensions: Default::default(),
        },
        index: None,
    })
    .unwrap();
    d.execute(add(1)).unwrap();
    d.execute(Command::SetSurfaceMedia {
        id: sid(1),
        media: Some(m),
    })
    .unwrap();
    assert!(d.execute(Command::RemoveMedia { id: m }).is_err());
    assert!(
        d.execute(Command::SetSurfaceMedia {
            id: sid(1),
            media: Some(MediaId::from_u128(8))
        })
        .is_err()
    );
    d.execute(Command::SetSurfaceMedia {
        id: sid(1),
        media: None,
    })
    .unwrap();
    d.execute(Command::RemoveMedia { id: m }).unwrap();
}
