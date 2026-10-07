// SPDX-License-Identifier: Apache-2.0
//! A/V sync through the real stack: FFmpeg decode → MediaRuntime → mixer.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use om_audio::Mixer;
use om_engine::{AudioSetup, MediaRuntime};
use om_media_ffmpeg::FfmpegOpener;
use om_media_ffmpeg::fixtures::{
    FixtureCodec, FixtureSpec, audio_sample, read_index, write_index_video,
};
use om_project::{Media, MediaSource, Playback, Project};
use om_time::{Rate, RationalTime};
use om_types::MediaId;

#[test]
fn video_frames_and_audio_samples_line_up_on_the_show_clock() {
    let dir = tempfile::tempdir().unwrap();
    let clip = dir.path().join("av.mkv");
    write_index_video(
        &clip,
        FixtureSpec {
            width: 64,
            height: 32,
            rate: Rate::FPS_25,
            frames: 100, // 4 s
            codec: FixtureCodec::Ffv1Mkv,
            audio_rate: Some(48_000),
        },
    )
    .unwrap();
    let id = MediaId::from_u128(1);
    let mut p = Project::new("av");
    p.media.push(Media {
        id,
        name: "av".into(),
        source: MediaSource::Video {
            path: "av.mkv".into(),
        },
        playback: Playback::default(), // looping
        plugins: Vec::new(),
        extensions: Default::default(),
    });
    let mixer = Mixer::new(48_000);
    let opener = Arc::new(FfmpegOpener);
    let mut rt = MediaRuntime::with_audio(
        Some(opener.clone()),
        Some(AudioSetup {
            opener,
            mixer: mixer.clone(),
        }),
    );
    let first_loud = (0..)
        .position(|n| audio_sample(n, 48_000).abs() > 0.1)
        .unwrap() as i64;

    // Seconds 1..3 of the first pass, then second 1 of the third pass.
    for second in [1i64, 2, 3, 9] {
        let show = RationalTime::from_seconds(second);
        let changes = rt.update_blocking(&p, Some(dir.path()), show, Duration::from_secs(10));
        let frame = changes
            .upload
            .iter()
            .find(|(m, _)| *m == id)
            .map(|(_, img)| read_index(img));
        let media_second = second % 4;
        assert_eq!(
            frame,
            Some(25 * media_second as u32),
            "video at show {second}s"
        );

        // Audio: render 30 ms around the boundary and find the burst onset.
        let boundary = second * 48_000;
        let start = boundary - 480;
        let mut out = vec![0.0f32; 1440 * 2];
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let onset = loop {
            mixer.render_at(start, &mut out);
            if let Some(i) = out.chunks(2).position(|f| f[0].abs() > 0.1) {
                break Some(start + i as i64);
            }
            if std::time::Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            onset,
            Some(boundary + first_loud),
            "audio at show {second}s"
        );
    }
}
