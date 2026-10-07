// SPDX-License-Identifier: Apache-2.0
//! End-to-end: video file → decode → GPU composite → PNG, frame-exact.
//! Needs a GPU (skips without one unless OM_REQUIRE_GPU=1).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::process::Command;

use om_media_ffmpeg::fixtures::{FixtureCodec, FixtureSpec, read_index, write_index_video};
use om_time::Rate;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openmapper-cli"))
}

#[test]
fn render_at_selects_exact_video_frame() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("clip.mp4");
    write_index_video(
        &video,
        FixtureSpec {
            width: 160,
            height: 96,
            rate: Rate::FPS_25,
            frames: 100,
            codec: FixtureCodec::Mpeg4Mp4,
        },
    )
    .unwrap();
    let proj = dir.path().join("show.omproj");
    assert!(cli().arg("new").arg(&proj).status().unwrap().success());
    let cmds = dir.path().join("cmds.jsonl");
    std::fs::write(
        &cmds,
        [
            r#"{"type":"set_canvas","canvas":{"width":160,"height":96}}"#,
            r#"{"type":"add_media","media":{"id":"00000000000000000000000010","name":"clip","source":{"kind":"video","path":"clip.mp4"},"playback":{"looping":true,"speed":{"num":1,"den":1}}}}"#,
            r#"{"type":"add_surface","surface":{"id":"00000000000000000000000001","name":"full","shape":{"kind":"quad","corners":[[0,0],[1,0],[1,1],[0,1]],"uv":[[0,0],[1,0],[1,1],[0,1]]},"media":"00000000000000000000000010"}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    assert!(
        cli()
            .arg("apply")
            .arg(&proj)
            .arg(&cmds)
            .status()
            .unwrap()
            .success()
    );

    let gpu_required = std::env::var("OM_REQUIRE_GPU").as_deref() == Ok("1");
    // (seconds, expected frame): exact boundaries, mid-frame, and a loop.
    for (at, expected) in [(0.0, 0), (1.24, 31), (1.239, 30), (3.0, 75), (4.04, 1)] {
        let png = dir.path().join(format!("f{at}.png"));
        let out = cli()
            .arg("render")
            .arg(&proj)
            .arg("-o")
            .arg(&png)
            .arg("--at")
            .arg(at.to_string())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() && stderr.contains("GPU adapter") && !gpu_required {
            eprintln!("skipping: no GPU ({stderr})");
            return;
        }
        assert!(out.status.success(), "{stderr}");
        let img = image::open(&png).unwrap().to_rgba8();
        let still =
            om_media_core::StillImage::from_rgba8(img.width(), img.height(), img.into_raw())
                .unwrap();
        assert_eq!(read_index(&still), expected, "at {at}s");
    }
}
