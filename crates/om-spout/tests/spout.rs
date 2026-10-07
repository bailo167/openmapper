// SPDX-License-Identifier: Apache-2.0
//! Spout loopbacks on Windows: OpenMapper sending to OpenMapper through the
//! Spout directory and a shared D3D11 texture, in-process and across
//! processes (Windows CI runs these on WARP).
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use om_media_core::{
    LiveConfig, LiveFeed, LiveOpener, LiveState, PublishFeed, SinkOpener, SinkState, StillImage,
};
use om_project::{LiveInput, Publish};
use om_spout::{SpoutOpener, sender_names};

fn quick() -> LiveConfig {
    LiveConfig {
        min_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(400),
        poll: Duration::from_millis(20),
    }
}

/// A unique sender name per test and process.
fn unique(tag: &str) -> String {
    format!("OpenMapper test {tag} {}", std::process::id())
}

/// A frame whose every pixel depends on `i`, including partial alpha.
fn test_frame(i: u32, w: u32, h: u32) -> Arc<StillImage> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            #[allow(clippy::cast_possible_truncation)]
            px.extend_from_slice(&[
                (x * 7 + i * 11) as u8,
                (y * 13 + i * 3) as u8,
                ((x ^ y) + i * 5) as u8,
                (x * 2 + y + i) as u8,
            ]);
        }
    }
    Arc::new(StillImage::from_rgba8(w, h, px).unwrap())
}

fn sender(name: &str) -> PublishFeed {
    PublishFeed::spawn_with(
        Arc::new(SpoutOpener) as Arc<dyn SinkOpener>,
        Publish::Spout { name: name.into() },
        quick(),
    )
}

fn receiver(name: &str) -> LiveFeed {
    LiveFeed::spawn_with(
        Arc::new(SpoutOpener) as Arc<dyn LiveOpener>,
        LiveInput::Spout {
            sender: name.into(),
        },
        quick(),
    )
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sends frames until `count` received frames have matched a sent frame
/// exactly; returns the number matched.
fn exchange(tx: &PublishFeed, rx: &LiveFeed, w: u32, h: u32, count: u32) -> u32 {
    let mut sent: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut seen = 0;
    let mut matched = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut i = 0;
    while matched < count && Instant::now() < deadline {
        let f = test_frame(i, w, h);
        sent.insert(f.rgba8().to_vec(), i);
        tx.submit(f);
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
        while let Some(f) = rx.newer_than(seen) {
            seen = f.seq;
            assert_eq!((f.image.width(), f.image.height()), (w, h));
            assert!(
                sent.contains_key(f.image.rgba8()),
                "received frame differs from every sent frame"
            );
            matched += 1;
        }
    }
    matched
}

#[test]
fn loopback_is_bit_exact_and_discoverable() {
    let name = unique("loopback");
    let tx = sender(&name);
    tx.submit(test_frame(0, 64, 48));
    wait_until(
        "the sender to be announced",
        Duration::from_secs(10),
        || sender_names().contains(&name),
    );
    assert!(SpoutOpener.discover().contains(&LiveInput::Spout {
        sender: name.clone()
    }));
    let rx = receiver(&name);
    let matched = exchange(&tx, &rx, 64, 48, 20);
    assert!(
        matched >= 20,
        "matched {matched}; rx {:?} tx {:?}",
        rx.state(),
        tx.state()
    );
    assert_eq!(rx.state(), LiveState::Live);
    assert_eq!(tx.state(), SinkState::Sending);

    // A size change reaches the receiver.
    let matched = exchange(&tx, &rx, 33, 17, 5);
    assert!(matched >= 5, "after resize matched {matched}");

    drop(tx);
    wait_until(
        "the sender to leave the directory",
        Duration::from_secs(5),
        || !sender_names().contains(&name),
    );
    wait_until("the receiver to notice", Duration::from_secs(5), || {
        matches!(rx.state(), LiveState::Retrying { .. })
    });
}

#[test]
fn receiver_reconnects_when_the_sender_returns() {
    let name = unique("reconnect");
    let rx = receiver(&name);
    wait_until(
        "a missing sender to be reported",
        Duration::from_secs(5),
        || matches!(rx.state(), LiveState::Retrying { .. }),
    );
    for round in 0..3 {
        let tx = sender(&name);
        let matched = exchange(&tx, &rx, 40, 30, 5);
        assert!(matched >= 5, "round {round}: matched {matched}");
        drop(tx);
        wait_until("the loss to be noticed", Duration::from_secs(5), || {
            matches!(rx.state(), LiveState::Retrying { .. })
        });
    }
    assert!(rx.stats().reconnects >= 2, "{:?}", rx.stats());
}

#[test]
fn duplicate_sender_names_are_refused() {
    let name = unique("duplicate");
    let first = sender(&name);
    first.submit(test_frame(0, 16, 16));
    wait_until("the first sender", Duration::from_secs(10), || {
        sender_names().contains(&name)
    });
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second = SpoutOpener.open_sink(&Publish::Spout { name: name.clone() }, &stop);
    assert!(second.is_err(), "a second sender with the same name opened");
    drop(first);
    wait_until("the name to be released", Duration::from_secs(5), || {
        !sender_names().contains(&name)
    });
    assert!(
        SpoutOpener
            .open_sink(&Publish::Spout { name }, &stop)
            .is_ok()
    );
}

/// Child half of `cross_process_frames_are_bit_exact`: sends frames as the
/// sender named in `OM_TEST_SPOUT_NAME` until killed.
#[test]
fn child_spout_sender() {
    let Ok(name) = std::env::var("OM_TEST_SPOUT_NAME") else {
        return;
    };
    let tx = sender(&name);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut i = 0;
    while Instant::now() < deadline {
        tx.submit(test_frame(i, 96, 64));
        i += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
}

#[test]
fn cross_process_frames_are_bit_exact() {
    let name = unique("cross-process");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "child_spout_sender",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("OM_TEST_SPOUT_NAME", &name)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let sent: HashMap<Vec<u8>, u32> = (0..4000)
        .map(|i| (test_frame(i, 96, 64).rgba8().to_vec(), i))
        .collect();
    let rx = receiver(&name);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = 0;
    let mut matched = 0;
    while matched < 20 && Instant::now() < deadline {
        if let Some(f) = rx.wait_newer_than(seen, Duration::from_millis(200)) {
            seen = f.seq;
            assert!(
                sent.contains_key(f.image.rgba8()),
                "frame from the other process differs from what it sent"
            );
            matched += 1;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(matched >= 20, "matched {matched}; rx {:?}", rx.state());
}
