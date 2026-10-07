// SPDX-License-Identifier: Apache-2.0
//! NDI loopback through an installed NDI runtime. Skipped (with a note)
//! when no runtime is installed, unless `OM_REQUIRE_NDI=1`, which makes a
//! missing runtime a failure (for machines that have one).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use om_media_core::{LiveConfig, LiveFeed, LiveOpener, PublishFeed, SinkOpener, StillImage};
use om_ndi::NdiOpener;
use om_project::{LiveInput, Publish};

fn runtime_or_skip() -> bool {
    match om_ndi::runtime() {
        Ok(rt) => {
            eprintln!("using NDI runtime {}", rt.path().display());
            true
        }
        Err(e) => {
            assert!(
                std::env::var("OM_REQUIRE_NDI").as_deref() != Ok("1"),
                "OM_REQUIRE_NDI=1 but {e}"
            );
            eprintln!("skipping NDI loopback: {e}");
            false
        }
    }
}

/// Smooth content identifying frame `i` by its blue level. NDI's video
/// codec is lossy (and subsamples colour), so fine detail would not
/// survive; smooth gradients come back within a few levels.
fn frame(i: u32) -> Arc<StillImage> {
    let (w, h) = (64u32, 36u32);
    #[allow(clippy::cast_possible_truncation)]
    let px = (0..w * h)
        .flat_map(|p| {
            let (x, y) = (p % w, p / w);
            [
                (x * 4) as u8,
                (64 + y * 3) as u8,
                ((i % 20) * 12) as u8,
                255,
            ]
        })
        .collect();
    Arc::new(StillImage::from_rgba8(w, h, px).unwrap())
}

/// Mean and largest per-channel difference between two equal-size images.
fn difference(a: &StillImage, b: &StillImage) -> (f64, u8) {
    let mut sum = 0u64;
    let mut max = 0u8;
    for (x, y) in a.rgba8().iter().zip(b.rgba8()) {
        let d = x.abs_diff(*y);
        sum += u64::from(d);
        max = max.max(d);
    }
    #[allow(clippy::cast_precision_loss)]
    let mean = sum as f64 / a.rgba8().len() as f64;
    (mean, max)
}

/// Lossy-codec tolerance (8-bit levels), measured with the macOS NDI 6
/// runtime: mean ≈ 1.5, max ≤ 7 on this content.
const MEAN_TOLERANCE: f64 = 3.0;
const MAX_TOLERANCE: u8 = 12;

#[test]
fn opener_reports_a_missing_runtime_cleanly() {
    if om_ndi::runtime().is_ok() {
        return;
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let err = NdiOpener
        .open_live(
            &LiveInput::Ndi {
                source: "X (Y)".into(),
            },
            &stop,
        )
        .err()
        .expect("opening without a runtime fails");
    assert!(err.to_string().contains("NDI runtime"), "{err}");
    assert!(
        NdiOpener
            .open_sink(&Publish::Ndi { name: "x".into() }, &stop)
            .is_err()
    );
    assert!(NdiOpener.discover().is_empty());
}

#[test]
fn loopback_through_the_installed_runtime() {
    if !runtime_or_skip() {
        return;
    }
    let name = format!("OpenMapper test {}", std::process::id());
    let config = LiveConfig {
        min_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(500),
        poll: Duration::from_millis(50),
    };
    let tx = PublishFeed::spawn_with(
        Arc::new(NdiOpener) as Arc<dyn SinkOpener>,
        Publish::Ndi { name: name.clone() },
        config,
    );
    // Full source names are "MACHINE (name)"; find ours.
    let deadline = Instant::now() + Duration::from_secs(20);
    let source = loop {
        tx.submit(frame(0));
        if let Some(LiveInput::Ndi { source }) = NdiOpener.discover().into_iter().find(
            |i| matches!(i, LiveInput::Ndi { source } if source.ends_with(&format!("({name})"))),
        ) {
            break source;
        }
        assert!(Instant::now() < deadline, "our NDI source never appeared");
        std::thread::sleep(Duration::from_millis(100));
    };
    let rx = LiveFeed::spawn_with(
        Arc::new(NdiOpener) as Arc<dyn LiveOpener>,
        LiveInput::Ndi { source },
        config,
    );
    let mut seen = 0;
    let mut matched = 0;
    let mut i = 0;
    while matched < 10 && Instant::now() < deadline + Duration::from_secs(20) {
        tx.submit(frame(i));
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
        while let Some(f) = rx.newer_than(seen) {
            seen = f.seq;
            let img = &f.image;
            assert_eq!((img.width(), img.height()), (64, 36));
            // The closest of the distinct frames sent must be within the
            // codec's tolerance (wrong channel order or layout would not be).
            let (mean, max) = (0..20)
                .map(|k| difference(img, &frame(k)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .unwrap();
            assert!(
                mean <= MEAN_TOLERANCE && max <= MAX_TOLERANCE,
                "NDI frame differs from every sent frame (mean {mean:.2}, max {max})"
            );
            assert!(
                img.rgba8().as_chunks::<4>().0.iter().all(|p| p[3] == 255),
                "alpha must survive"
            );
            matched += 1;
        }
    }
    assert!(matched >= 10, "matched {matched}; rx {:?}", rx.state());
}
