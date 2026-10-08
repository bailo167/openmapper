// SPDX-License-Identifier: Apache-2.0
//! End-to-end 3-D calibration through the CLI: a project stores measured
//! point pairs; `calibrate` fits and saves the projector; `render-output`
//! shows the model through it. The fitted output matches one using the true
//! projector, and calibrating again from the saved file is byte-identical.
//! Rendering needs a GPU (skipped without one unless OM_REQUIRE_GPU=1).

#![allow(clippy::unwrap_used, clippy::panic, clippy::cast_precision_loss)]

use std::process::Command;

use om_calibration::{Intrinsics, Pose, Projector};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openmapper-cli"))
}

const CUBE: &str = "v -0.5 -0.5 -0.5\nv 0.5 -0.5 -0.5\nv 0.5 0.5 -0.5\nv -0.5 0.5 -0.5\n\
v -0.5 -0.5 0.5\nv 0.5 -0.5 0.5\nv 0.5 0.5 0.5\nv -0.5 0.5 0.5\n\
vt 0 1\nvt 1 1\nvt 1 0\nvt 0 0\n\
f 1/1 2/2 3/3 4/4\nf 5/1 6/2 7/3 8/4\nf 1/1 2/2 6/3 5/4\n\
f 4/1 3/2 7/3 8/4\nf 1/1 4/2 8/3 5/4\nf 2/1 3/2 7/3 6/4\n";

fn truth() -> Projector {
    Projector {
        width: 320,
        height: 240,
        intrinsics: Intrinsics {
            fx: 360.0,
            fy: 360.0,
            cx: 162.0,
            cy: 200.0,
        },
        pose: Pose {
            rotation: [0.3, -0.5, 0.05],
            translation: [0.0, -0.3, 3.0],
        },
    }
}

fn run(args: &[&str], dir: &std::path::Path) -> std::process::Output {
    cli().args(args).current_dir(dir).output().unwrap()
}

#[test]
fn calibrate_then_render_matches_the_true_projector() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cube.obj"), CUBE).unwrap();
    let proj = dir.path().join("show.omproj");
    assert!(cli().arg("new").arg(&proj).status().unwrap().success());

    let t = truth();
    let mut points = Vec::new();
    for x in [-0.5, 0.0, 0.5] {
        for y in [-0.5, 0.0, 0.5] {
            for z in [-0.5, 0.5] {
                let (u, v) = t.project([x, y, z]).unwrap();
                points.push(serde_json::json!({
                    "world": [x, y, z],
                    "pixel": [(u * 8.0).round() / 8.0, (v * 8.0).round() / 8.0],
                }));
            }
        }
    }
    let output =
        |id: &str, name: &str, projector: serde_json::Value, points: &[serde_json::Value]| {
            let mut projection = serde_json::json!({ "model": "cube.obj", "points": points });
            if !projector.is_null() {
                projection["projector"] = projector;
            }
            serde_json::json!({
                "type": "add_output",
                "output": { "id": id, "name": name, "projection": projection },
            })
        };
    let true_params = serde_json::to_value(t.to_params().unwrap()).unwrap();
    let cmds = [
        serde_json::json!({"type": "set_canvas", "canvas": {"width": 64, "height": 64}}),
        serde_json::json!({"type": "add_media", "media": {"id": "00000000000000000000000010", "name": "grid", "source": {"kind": "pattern", "pattern": "uv_grid"}}}),
        serde_json::json!({"type": "add_surface", "surface": {"id": "00000000000000000000000001", "name": "full", "shape": {"kind": "quad", "corners": [[0,0],[1,0],[1,1],[0,1]], "uv": [[0,0],[1,0],[1,1],[0,1]]}, "media": "00000000000000000000000010"}}),
        output("00000000000000000000000020", "true", true_params, &[]),
        output(
            "00000000000000000000000021",
            "fitted",
            serde_json::Value::Null,
            &points,
        ),
    ];
    let lines: Vec<String> = cmds.iter().map(ToString::to_string).collect();
    std::fs::write(dir.path().join("cmds.jsonl"), lines.join("\n")).unwrap();
    let applied = run(&["apply", "show.omproj", "cmds.jsonl"], dir.path());
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );

    let cal = run(
        &[
            "calibrate",
            "show.omproj",
            "--output",
            "fitted",
            "--size",
            "320x240",
        ],
        dir.path(),
    );
    assert!(
        cal.status.success(),
        "{}",
        String::from_utf8_lossy(&cal.stderr)
    );
    let first = std::fs::read(&proj).unwrap();
    let again = run(
        &[
            "calibrate",
            "show.omproj",
            "--output",
            "fitted",
            "--size",
            "320x240",
        ],
        dir.path(),
    );
    assert!(again.status.success());
    let second = std::fs::read(&proj).unwrap();
    // Revision increases; everything else (the fitted projector) is equal.
    let strip = |bytes: &[u8]| {
        let mut v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        v["revision"] = serde_json::Value::Null;
        v
    };
    assert_eq!(strip(&first), strip(&second), "calibration is repeatable");

    let mut images = Vec::new();
    for name in ["true", "fitted"] {
        let png = format!("{name}.png");
        let out = run(
            &[
                "render-output",
                "show.omproj",
                "--output",
                name,
                "-o",
                &png,
                "--size",
                "320x240",
            ],
            dir.path(),
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success()
            && stderr.contains("GPU adapter")
            && std::env::var("OM_REQUIRE_GPU").as_deref() != Ok("1")
        {
            eprintln!("skipping: no GPU ({stderr})");
            return;
        }
        assert!(out.status.success(), "{stderr}");
        images.push(
            image::open(dir.path().join(&png))
                .unwrap()
                .to_rgba8()
                .into_raw(),
        );
    }
    let lit = images[0].chunks(4).filter(|p| p[..3] != [0, 0, 0]).count();
    assert!(lit > 5000, "the cube covers {lit} pixels");
    let diffs: Vec<u8> = images[0]
        .iter()
        .zip(&images[1])
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let mean = diffs.iter().map(|&d| f64::from(d)).sum::<f64>() / diffs.len() as f64;
    assert!(mean < 1.0, "mean difference {mean}");
}
