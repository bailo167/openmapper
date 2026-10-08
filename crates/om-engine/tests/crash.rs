// SPDX-License-Identifier: Apache-2.0
//! Crash injection: a child process edits and saves a project in a loop
//! and is killed at random moments (no clean shutdown). After every kill
//! the project must open, validate, contain an unbroken prefix of the
//! edits, and never lose an edit that a previous run had recovered.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::{Duration, Instant};

use om_command::Command;
use om_engine::Session;
use om_project::Surface;
use om_types::SurfaceId;

/// The edits are numbered surfaces 1, 2, 3, …
fn edit_count(path: &Path) -> usize {
    let (session, report) = Session::open(path).unwrap_or_else(|e| panic!("reopen failed: {e}"));
    session.project().validate().unwrap();
    let names: Vec<String> = session
        .project()
        .surfaces
        .iter()
        .map(|s| s.name.clone())
        .collect();
    for (i, name) in names.iter().enumerate() {
        assert_eq!(*name, format!("S{}", i + 1), "edits are an unbroken prefix");
    }
    // Leave recovery state as it is for the next child (no close/save).
    drop(report);
    drop(session);
    names.len()
}

/// Child half: edits forever (saving every few edits) until killed.
#[test]
fn child_editor() {
    let Ok(path) = std::env::var("OM_TEST_CRASH_PROJECT") else {
        return;
    };
    let path = Path::new(&path);
    let (mut session, _) = Session::open(path).unwrap();
    let mut n = session.project().surfaces.len();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        n += 1;
        let id = SurfaceId::from_u128(n as u128);
        session
            .execute(Command::AddSurface {
                surface: Surface::new(id, format!("S{n}")),
                index: None,
            })
            .unwrap();
        if n % 7 == 0 {
            session.save().unwrap();
        }
    }
}

#[test]
fn random_kills_never_corrupt_or_lose_recovered_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crash.omproj");
    let mut s = Session::new("crash");
    s.save_as(&path).unwrap();
    s.close().unwrap();

    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut last = 0;
    for round in 0..40 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["child_editor", "--exact", "--nocapture", "--test-threads=1"])
            .env("OM_TEST_CRASH_PROJECT", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        std::thread::sleep(Duration::from_millis(20 + seed % 180));
        let _ = child.kill(); // SIGKILL / TerminateProcess: no cleanup runs
        let _ = child.wait();
        let n = edit_count(&path);
        assert!(
            n >= last,
            "round {round}: {n} edits after kill, {last} before"
        );
        last = n;
    }
    assert!(last > 0, "the child made progress");
}
