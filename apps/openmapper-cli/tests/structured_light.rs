// SPDX-License-Identifier: Apache-2.0
//! Structured-light CLI round trip: decoding the generated patterns
//! themselves (a "camera" that sees exactly the projector image) yields
//! every projector pixel and an identity homography.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openmapper-cli"))
}

#[test]
fn patterns_decode_to_the_identity() {
    let dir = tempfile::tempdir().unwrap();
    let pat = dir.path().join("pat");
    let out = cli()
        .args(["structured-light", "patterns", "--size", "48x20", "--out"])
        .arg(&pat)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_dir(&pat).unwrap().count(), 2 * (6 + 5));
    let json = dir.path().join("pairs.json");
    let out = cli()
        .args(["structured-light", "decode", "--size", "48x20"])
        .arg(&pat)
        .arg("--out")
        .arg(&json)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
    assert_eq!(v["pairs"].as_array().unwrap().len(), 48 * 20);
    let h = &v["camera_to_projector"];
    for i in 0..3 {
        for j in 0..3 {
            let want = if i == j { 1.0 } else { 0.0 };
            assert!((h[i][j].as_f64().unwrap() - want).abs() < 1e-9, "{h}");
        }
    }
    // Wrong count of photographs is an error, not a panic.
    std::fs::remove_file(pat.join("001.png")).unwrap();
    let out = cli()
        .args(["structured-light", "decode", "--size", "48x20"])
        .arg(&pat)
        .arg("--out")
        .arg(&json)
        .output()
        .unwrap();
    assert!(!out.status.success());
}
