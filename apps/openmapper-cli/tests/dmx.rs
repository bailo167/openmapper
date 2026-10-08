// SPDX-License-Identifier: Apache-2.0
//! End-to-end pixel mapping: project → GPU composite → fixture sampling →
//! DMX channel values, through the CLI. Needs a GPU (skips without one
//! unless OM_REQUIRE_GPU=1).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openmapper-cli"))
}

#[test]
fn checkerboard_maps_to_alternating_channels() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("show.omproj");
    assert!(cli().arg("new").arg(&proj).status().unwrap().success());
    let cmds = dir.path().join("cmds.jsonl");
    std::fs::write(
        &cmds,
        [
            r#"{"type":"set_canvas","canvas":{"width":64,"height":64}}"#,
            r#"{"type":"add_media","media":{"id":"00000000000000000000000010","name":"check","source":{"kind":"pattern","pattern":"checkerboard"}}}"#,
            r#"{"type":"add_surface","surface":{"id":"00000000000000000000000001","name":"full","shape":{"kind":"quad","corners":[[0,0],[1,0],[1,1],[0,1]],"uv":[[0,0],[1,0],[1,1],[0,1]]},"media":"00000000000000000000000010"}}"#,
            r#"{"type":"put_dmx_node","node":{"id":"00000000000000000000000020","name":"Rig","enabled":true,"protocol":{"type":"art_net","address":"127.0.0.1"}}}"#,
            r#"{"type":"put_fixture","fixture":{"id":"00000000000000000000000030","name":"Matrix","node":"00000000000000000000000020","universe":2,"address":5,"order":"mono","shape":{"kind":"grid","corners":[[0,0],[1,0],[1,1],[0,1]],"columns":8,"rows":8,"wiring":"rows"}}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    let applied = cli().arg("apply").arg(&proj).arg(&cmds).output().unwrap();
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );

    let out = cli()
        .args(["dmx", "frame"])
        .arg(&proj)
        .arg("--json")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success()
        && stderr.contains("GPU adapter")
        && std::env::var("OM_REQUIRE_GPU").as_deref() != Ok("1")
    {
        eprintln!("skipping: no GPU ({stderr})");
        return;
    }
    assert!(out.status.success(), "{stderr}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let values: Vec<u64> = json["Rig/2"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert_eq!(values.len(), 512);
    assert!(
        values[..4].iter().all(|&v| v == 0),
        "channels before address 5"
    );
    for row in 0..8 {
        for col in 0..8 {
            let expected = if (row + col) % 2 == 0 { 255 } else { 0 };
            assert_eq!(values[4 + row * 8 + col], expected, "cell ({col}, {row})");
        }
    }
    assert!(values[68..].iter().all(|&v| v == 0));
}
