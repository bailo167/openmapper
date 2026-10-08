// SPDX-License-Identifier: Apache-2.0
//! Decoding acceptance tests on generated frame-index videos.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};

use om_media_core::{FrameCursor, MediaError, MediaSource};
use om_media_ffmpeg::FfmpegVideo;
use om_media_ffmpeg::fixtures::{FixtureCodec, FixtureSpec, read_index, write_index_video};
use om_time::{Rate, RationalTime};

fn fixture(dir: &Path, codec: FixtureCodec, rate: Rate, frames: u32) -> PathBuf {
    let path = dir.join(format!("f-{}-{}.{}", rate.num(), frames, codec.extension()));
    write_index_video(
        &path,
        FixtureSpec {
            width: 160,
            height: 96,
            rate,
            frames,
            codec,
            audio_rate: None,
        },
    )
    .unwrap();
    path
}

fn index_at(c: &mut FrameCursor<FfmpegVideo>, t: RationalTime) -> u32 {
    read_index(&c.frame_at(t).unwrap().unwrap().image)
}

const RATES: [Rate; 4] = [
    Rate::FPS_29_97,
    Rate::FPS_23_976,
    Rate::FPS_25,
    Rate::FPS_60,
];
const CODECS: [FixtureCodec; 2] = [FixtureCodec::Ffv1Mkv, FixtureCodec::Mpeg4Mp4];

#[test]
fn every_frame_boundary_selects_the_exact_frame() {
    let dir = tempfile::tempdir().unwrap();
    for codec in CODECS {
        for rate in RATES {
            let n = 60;
            let path = fixture(dir.path(), codec, rate, n);
            let src = FfmpegVideo::open(&path).unwrap();
            assert_eq!(
                src.descriptor().frame_rate,
                Some(rate),
                "{codec:?} {rate:?}"
            );
            let mut c = FrameCursor::new(src);
            let tiny = RationalTime::new(1, 1_000_000_000).unwrap();
            for i in 0..i64::from(n) {
                let start = RationalTime::from_frame(i, rate).unwrap();
                assert_eq!(
                    index_at(&mut c, start),
                    i as u32,
                    "{codec:?} {rate:?} start of {i}"
                );
                if i + 1 < i64::from(n) {
                    let end = RationalTime::from_frame(i + 1, rate)
                        .unwrap()
                        .checked_sub(tiny)
                        .unwrap();
                    assert_eq!(
                        index_at(&mut c, end),
                        i as u32,
                        "{codec:?} {rate:?} end of {i}"
                    );
                }
            }
            // Sequential playback must not seek after the first lookup.
            assert_eq!(c.stats.seeks, 1, "{codec:?} {rate:?}: {:?}", c.stats);
        }
    }
}

#[test]
fn random_access_seeks_are_exact() {
    let dir = tempfile::tempdir().unwrap();
    for codec in CODECS {
        let rate = Rate::FPS_29_97;
        let path = fixture(dir.path(), codec, rate, 120);
        let mut c = FrameCursor::new(FfmpegVideo::open(&path).unwrap());
        // Backwards, across GOPs, mid-GOP, repeated.
        for i in [119, 0, 77, 76, 13, 12, 11, 100, 50, 51, 49, 3, 118, 1] {
            let mid = RationalTime::from_frame(i, rate)
                .unwrap()
                .checked_add(RationalTime::new(1, 120).unwrap())
                .unwrap();
            assert_eq!(index_at(&mut c, mid), i as u32, "{codec:?} frame {i}");
        }
        assert_eq!(
            index_at(&mut c, RationalTime::new(-5, 1).unwrap()),
            0,
            "before start"
        );
        assert_eq!(
            index_at(&mut c, RationalTime::from_seconds(3600)),
            119,
            "after end"
        );
        assert_eq!(
            index_at(&mut c, RationalTime::from_seconds(3600)),
            119,
            "after end again"
        );
    }
}

#[test]
fn sequential_decode_yields_every_frame_on_the_grid() {
    let dir = tempfile::tempdir().unwrap();
    for codec in CODECS {
        let rate = Rate::FPS_23_976;
        let path = fixture(dir.path(), codec, rate, 48);
        let mut v = FfmpegVideo::open(&path).unwrap();
        let mut i = 0i64;
        while let Some(f) = v.next_frame().unwrap() {
            assert_eq!(
                f.pts,
                RationalTime::from_frame(i, rate).unwrap(),
                "{codec:?} pts of {i}"
            );
            assert_eq!(read_index(&f.image), i as u32);
            i += 1;
        }
        assert_eq!(i, 48, "{codec:?}");
        let d = v.descriptor().duration.unwrap();
        let expected = RationalTime::from_frame(48, rate).unwrap().as_seconds_f64();
        assert!(
            (d.as_seconds_f64() - expected).abs() < 0.05,
            "{codec:?} duration {d}"
        );
    }
}

#[test]
fn corrupt_and_missing_files_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.mp4");
    assert!(matches!(
        FfmpegVideo::open(&missing),
        Err(MediaError::NotFound { .. })
    ));

    let empty = dir.path().join("empty.mp4");
    std::fs::write(&empty, b"").unwrap();
    assert!(FfmpegVideo::open(&empty).is_err());

    let garbage = dir.path().join("garbage.mkv");
    std::fs::write(
        &garbage,
        (0..50_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(FfmpegVideo::open(&garbage).is_err());

    // Truncated mid-stream: whatever decodes must be correct and the end must
    // be reported as end or an error — never a panic or a wrong frame.
    for codec in CODECS {
        let path = fixture(dir.path(), codec, Rate::FPS_25, 50);
        let bytes = std::fs::read(&path).unwrap();
        let cut = dir.path().join(format!("cut.{}", codec.extension()));
        std::fs::write(&cut, &bytes[..bytes.len() * 6 / 10]).unwrap();
        let Ok(mut v) = FfmpegVideo::open(&cut) else {
            continue;
        };
        let mut seen = Vec::new();
        while let Ok(Some(f)) = v.next_frame() {
            seen.push(read_index(&f.image));
        }
        for (i, idx) in seen.iter().enumerate() {
            assert_eq!(*idx, i as u32, "{codec:?} truncated stream frame order");
        }
    }
}

/// Plan acceptance: 10 minutes at 29.97 fps, sampled across the whole run,
/// shows zero accumulated drift. ~18k frames: slow in debug builds, so it
/// runs in release as its own CI step on every OS.
#[test]
#[ignore = "long-running in debug; CI runs it in release"]
fn ten_minutes_without_drift() {
    let dir = tempfile::tempdir().unwrap();
    let rate = Rate::FPS_29_97;
    let frames = 30000 * 600 / 1001; // 10 minutes
    let path = dir.path().join("long.mp4");
    write_index_video(
        &path,
        FixtureSpec {
            width: 64,
            height: 32,
            rate,
            frames,
            codec: FixtureCodec::Mpeg4Mp4,
            audio_rate: None,
        },
    )
    .unwrap();
    let mut c = FrameCursor::new(FfmpegVideo::open(&path).unwrap());
    // Play through in real-time-sized steps (one 60 Hz display tick each),
    // checking the frame against exact arithmetic at every tick.
    let tick = RationalTime::new(1, 60).unwrap();
    let mut t = RationalTime::ZERO;
    let mut checked = 0u64;
    while t.as_seconds_f64() < 599.9 {
        let expected = t.to_frame_floor(rate).unwrap() as u32 & 0xffff;
        assert_eq!(index_at(&mut c, t), expected, "at {t}");
        t = t.checked_add(tick).unwrap();
        checked += 1;
    }
    assert_eq!(
        c.stats.seeks, 1,
        "playback must be sequential: {:?}",
        c.stats
    );
    eprintln!(
        "checked {checked} ticks, decoded {} frames",
        c.stats.decoded
    );
}
