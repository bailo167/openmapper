// SPDX-License-Identifier: Apache-2.0
//! Audio decoding: sample-accurate positions in lossless and lossy tracks.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::Path;

use om_media_core::AudioSource;
use om_media_ffmpeg::FfmpegAudio;
use om_media_ffmpeg::fixtures::{FixtureCodec, FixtureSpec, audio_sample, write_index_video};
use om_time::Rate;

fn make(dir: &Path, codec: FixtureCodec, audio_rate: u32) -> std::path::PathBuf {
    let p = dir.join(format!("a{audio_rate}.{}", codec.extension()));
    write_index_video(
        &p,
        FixtureSpec {
            width: 64,
            height: 32,
            rate: Rate::FPS_25,
            frames: 125, // 5 s
            codec,
            audio_rate: Some(audio_rate),
        },
    )
    .unwrap();
    p
}

/// First sample index at or after `from` with |x| > 0.1.
fn onset(samples: &[f32], from: usize) -> Option<usize> {
    samples[from * 2..]
        .chunks(2)
        .position(|f| f[0].abs() > 0.1)
        .map(|i| i + from)
}

fn decode_all(a: &mut FfmpegAudio) -> Vec<f32> {
    let mut out = Vec::new();
    let mut expected = 0;
    while let Some(b) = a.next_block().unwrap() {
        assert_eq!(b.start, expected, "blocks must be contiguous");
        expected += b.frames() as i64;
        out.extend(b.samples);
    }
    out
}

#[test]
fn burst_onsets_land_on_exact_seconds() {
    let dir = tempfile::tempdir().unwrap();
    // (codec, source rate, output rate, tolerance in samples)
    for (codec, src_rate, out_rate, tol) in [
        (FixtureCodec::Ffv1Mkv, 48_000, 48_000, 0),
        (FixtureCodec::Ffv1Mkv, 44_100, 48_000, 2),
        (FixtureCodec::Mpeg4Mp4, 48_000, 48_000, 2),
    ] {
        let path = make(dir.path(), codec, src_rate);
        let mut a = FfmpegAudio::open(&path, out_rate).unwrap().unwrap();
        let samples = decode_all(&mut a);
        let reference_offset = (0..)
            .position(|n| audio_sample(n, src_rate).abs() > 0.1)
            .unwrap() as f64;
        for second in 1..5usize {
            let boundary = second * out_rate as usize;
            let found = onset(&samples, boundary - out_rate as usize / 4).unwrap();
            let expected =
                boundary as f64 + reference_offset * f64::from(out_rate) / f64::from(src_rate);
            let err = (found as f64 - expected).abs();
            assert!(
                err <= tol as f64 + 1.0,
                "{codec:?} {src_rate}->{out_rate}: second {second} onset {found}, expected {expected}"
            );
        }
    }
}

#[test]
fn seeks_are_sample_exact() {
    let dir = tempfile::tempdir().unwrap();
    let path = make(dir.path(), FixtureCodec::Ffv1Mkv, 48_000);
    let mut a = FfmpegAudio::open(&path, 48_000).unwrap().unwrap();
    let all = decode_all(&mut a);
    for target in [96_000i64, 47_990, 150_123, 10] {
        a.seek(target).unwrap();
        let b = a.next_block().unwrap().unwrap();
        assert_eq!(b.start, target);
        let t = usize::try_from(target).unwrap() * 2;
        assert_eq!(&b.samples[..64], &all[t..t + 64], "after seek to {target}");
    }
}

#[test]
fn files_without_audio_report_none() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("v.mkv");
    write_index_video(
        &p,
        FixtureSpec {
            width: 32,
            height: 16,
            rate: Rate::FPS_25,
            frames: 5,
            codec: FixtureCodec::Ffv1Mkv,
            audio_rate: None,
        },
    )
    .unwrap();
    assert!(FfmpegAudio::open(&p, 48_000).unwrap().is_none());
}
