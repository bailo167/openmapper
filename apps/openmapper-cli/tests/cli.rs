// SPDX-License-Identifier: Apache-2.0
//! End-to-end tests of the CLI binary.

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openmapper-cli"))
}

#[test]
fn new_apply_inspect_validate() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("show.omproj");
    let cmds = dir.path().join("cmds.jsonl");

    let out = cli()
        .args(["new"])
        .arg(&proj)
        .args(["--name", "Demo"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Refuses to clobber.
    assert!(!cli().arg("new").arg(&proj).status().unwrap().success());

    std::fs::write(
        &cmds,
        concat!(
            r#"{"type":"add_surface","surface":{"id":"00000000000000000000000001","name":"Quad"}}"#,
            "\n",
            r#"{"type":"update_surface","id":"00000000000000000000000001","opacity":0.5}"#,
            "\n"
        ),
    )
    .unwrap();
    let out = cli().arg("apply").arg(&proj).arg(&cmds).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "revision 2");

    let out = cli().arg("inspect").arg(&proj).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Demo") && text.contains("Quad") && text.contains("opacity=0.5"));

    let out = cli()
        .arg("inspect")
        .arg(&proj)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(
        out.stdout,
        std::fs::read(&proj).unwrap(),
        "--json is the canonical file"
    );

    assert!(cli().arg("validate").arg(&proj).status().unwrap().success());
    assert!(!dir.path().join("show.omproj.journal").exists());
}

#[test]
fn rejected_command_saves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("show.omproj");
    let cmds = dir.path().join("cmds.json");
    assert!(cli().arg("new").arg(&proj).status().unwrap().success());
    let before = std::fs::read(&proj).unwrap();
    std::fs::write(
        &cmds,
        r#"[{"type":"set_project_name","name":"ok"},{"type":"remove_surface","id":"00000000000000000000000009"}]"#,
    )
    .unwrap();
    let out = cli().arg("apply").arg(&proj).arg(&cmds).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("command 2 rejected"));
    assert_eq!(std::fs::read(&proj).unwrap(), before);
    assert!(
        !dir.path().join("show.omproj.journal").exists(),
        "partial batch must not be recoverable"
    );
}

#[test]
fn invalid_files_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.omproj");
    std::fs::write(&bad, r#"{"format":"openmapper-project","version":42}"#).unwrap();
    let out = cli().arg("validate").arg(&bad).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("newer than this build"));
}

/// A show machine set up over SSH has no screen to click "Allow for this
/// project" on; `trust --allow` records the same per-user permission, so
/// network control is not held back when the app starts the show.
#[test]
fn trust_allows_a_project_without_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let user = |c: &mut Command| {
        c.env("HOME", &config)
            .env("XDG_CONFIG_HOME", &config)
            .env("APPDATA", &config);
    };
    let proj = dir.path().join("show.omproj");
    let cmds = dir.path().join("cmds.jsonl");
    assert!(cli().arg("new").arg(&proj).status().unwrap().success());

    let mut c = cli();
    user(&mut c);
    let out = c.arg("trust").arg(&proj).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("needs no permission"));

    std::fs::write(
        &cmds,
        r#"{"type":"set_controls","controls":{"osc_port":8010,"oscquery_port":8011,"network":true,"midi":[]}}"#,
    )
    .unwrap();
    let applied = cli().arg("apply").arg(&proj).arg(&cmds).status().unwrap();
    assert!(applied.success());

    let mut c = cli();
    user(&mut c);
    let out = c.arg("trust").arg(&proj).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success());
    assert!(
        text.contains("OSC / OSCQuery control from other computers"),
        "{text}"
    );
    assert!(text.contains("held back until allowed"), "{text}");

    let mut c = cli();
    user(&mut c);
    let out = c.args(["trust", "--allow"]).arg(&proj).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("allowed for this user"));

    // Persisted where the app looks for it.
    let project = om_project::store::load(&proj).unwrap().project;
    let store_file = std::fs::read_dir(&config)
        .unwrap()
        .flatten()
        .flat_map(|e| walk(&e.path()))
        .find(|p| p.ends_with("OpenMapper/trusted-projects.json"))
        .unwrap();
    let store = om_engine::trust::TrustStore::load(&store_file);
    assert!(store.is_trusted(&project));

    let mut c = cli();
    user(&mut c);
    let out = c.arg("trust").arg(&proj).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("allowed for this user"));
}

fn walk(p: &std::path::Path) -> Vec<std::path::PathBuf> {
    if p.is_dir() {
        std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|e| walk(&e.path()))
            .collect()
    } else {
        vec![p.to_owned()]
    }
}
