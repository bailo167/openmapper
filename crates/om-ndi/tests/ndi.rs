// SPDX-License-Identifier: Apache-2.0
//! NDI loopback through an installed NDI runtime. Skipped (with a note)
//! when no runtime is installed, unless `OM_REQUIRE_NDI=1`, which makes a
//! missing runtime a failure (for machines that have one).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashSet;
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

fn frame(i: u32) -> Arc<StillImage> {
    let (w, h) = (64u32, 36u32);
    #[allow(clippy::cast_possible_truncation)]
    let px = (0..w * h)
        .flat_map(|p| [(p + i) as u8, (p * 3) as u8, i as u8, 255])
        .collect();
    Arc::new(StillImage::from_rgba8(w, h, px).unwrap())
}

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
    let sent: HashSet<Vec<u8>> = (0..2000).map(|i| frame(i).rgba8().to_vec()).collect();
    let mut seen = 0;
    let mut matched = 0;
    let mut i = 0;
    while matched < 10 && Instant::now() < deadline + Duration::from_secs(20) {
        tx.submit(frame(i));
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
        while let Some(f) = rx.newer_than(seen) {
            seen = f.seq;
            assert!(
                sent.contains(f.image.rgba8()),
                "NDI frame differs from what was sent"
            );
            matched += 1;
        }
    }
    assert!(matched >= 10, "matched {matched}; rx {:?}", rx.state());
}
