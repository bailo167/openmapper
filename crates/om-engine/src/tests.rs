// SPDX-License-Identifier: Apache-2.0

use std::fs;

use om_command::Command;
use om_project::Surface;
use om_project::store::{self, Journal};
use om_types::SurfaceId;

use crate::{Session, SessionError};

fn add(n: u128) -> Command {
    Command::AddSurface {
        surface: Surface::new(SurfaceId::from_u128(n), format!("S{n}")),
        index: None,
    }
}

#[test]
fn new_session_is_dirty_and_needs_a_path() {
    let mut s = Session::new("Show");
    assert!(s.is_dirty());
    assert!(matches!(s.save(), Err(SessionError::NoPath)));
}

#[test]
fn save_open_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.omproj");
    let mut s = Session::new("Show");
    s.execute(add(1)).unwrap();
    s.save_as(&path).unwrap();
    assert!(!s.is_dirty());
    let expected = s.project().clone();
    s.close().unwrap();
    assert!(!store::journal_path(&path).exists());

    let (s2, report) = Session::open(&path).unwrap();
    assert_eq!(s2.project(), &expected);
    assert_eq!(report.recovered_commands, 0);
    assert!(report.warnings.is_empty());
    assert!(!s2.is_dirty());
}

#[test]
fn crash_before_save_recovers_unsaved_commands() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.omproj");
    let mut s = Session::new("Show");
    s.save_as(&path).unwrap();
    s.execute(add(1)).unwrap();
    s.execute(add(2)).unwrap();
    s.undo().unwrap();
    s.execute(Command::SetProjectName {
        name: "Renamed".into(),
    })
    .unwrap();
    let expected = s.project().clone();
    drop(s); // crash: no save, no close

    let (s2, report) = Session::open(&path).unwrap();
    assert_eq!(report.recovered_commands, 4);
    assert_eq!(s2.project(), &expected);
    assert!(s2.is_dirty(), "recovered work is unsaved");
    drop(s2); // crash again before saving

    let (s3, report) = Session::open(&path).unwrap();
    assert_eq!(
        report.recovered_commands, 4,
        "second crash keeps recovered work"
    );
    assert_eq!(s3.project(), &expected);
}

#[test]
fn torn_journal_tail_recovers_complete_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.omproj");
    let mut s = Session::new("Show");
    s.save_as(&path).unwrap();
    s.execute(add(1)).unwrap();
    drop(s);
    let jp = store::journal_path(&path);
    let mut text = fs::read_to_string(&jp).unwrap();
    text.push_str("{\"revision\":2,\"command\":{\"type\":\"add_surf");
    fs::write(&jp, text).unwrap();

    let (mut s2, report) = Session::open(&path).unwrap();
    assert_eq!(report.recovered_commands, 1);
    assert_eq!(report.warnings.len(), 1);
    // Appending after recovery must not corrupt the journal.
    s2.execute(add(2)).unwrap();
    drop(s2);
    let (s3, report) = Session::open(&path).unwrap();
    assert_eq!(report.recovered_commands, 2);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(s3.project().surfaces.len(), 2);
}

#[test]
fn mismatched_journal_is_set_aside_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.omproj");
    let mut s = Session::new("Show");
    s.execute(add(1)).unwrap();
    s.save_as(&path).unwrap();
    s.execute(add(2)).unwrap();
    // Simulate the file being replaced by another save at a different revision.
    let mut other = s.project().clone();
    other.revision = 99;
    store::save_atomic(&path, &other).unwrap();
    drop(s);

    let (s2, report) = Session::open(&path).unwrap();
    assert_eq!(report.recovered_commands, 0);
    assert_eq!(report.warnings.len(), 1);
    assert_eq!(s2.project().revision, 99);
    let stale = dir.path().join("a.omproj.journal.stale");
    assert!(stale.exists());
}

#[test]
fn corrupt_project_file_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.omproj");
    fs::write(&path, "{\"format\":\"openmapper-project\",\"version\":1,").unwrap();
    assert!(Session::open(&path).is_err());
    assert!(Session::open(&dir.path().join("missing.omproj")).is_err());
}

#[test]
fn save_as_moves_journal() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.omproj");
    let b = dir.path().join("b.omproj");
    let mut s = Session::new("Show");
    s.save_as(&a).unwrap();
    s.save_as(&b).unwrap();
    assert!(!store::journal_path(&a).exists());
    assert!(store::journal_path(&b).exists());
    assert!(Journal::read::<Command>(&b).unwrap().is_some());
}
