// SPDX-License-Identifier: Apache-2.0
//! Syphon loopbacks on macOS: OpenMapper publishing to OpenMapper through
//! the Syphon directory and shared IOSurfaces, in-process and across
//! processes.
//!
//! A custom harness (no libtest): Syphon discovery arrives on the main
//! thread's run loop, so `main` runs that loop while the tests run on a
//! worker thread.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("Syphon tests run on macOS only");
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::HashMap;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use om_media_core::{
        LiveConfig, LiveFeed, LiveOpener, LiveState, PublishFeed, SinkOpener, SinkState, StillImage,
    };
    use om_project::{LiveInput, Publish};
    use om_syphon::{SyphonOpener, run_main_loop, servers};

    fn quick() -> LiveConfig {
        LiveConfig {
            min_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(400),
            poll: Duration::from_millis(20),
        }
    }

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
            Arc::new(SyphonOpener) as Arc<dyn SinkOpener>,
            Publish::Syphon { name: name.into() },
            quick(),
        )
    }

    fn receiver(name: &str) -> LiveFeed {
        LiveFeed::spawn_with(
            Arc::new(SyphonOpener) as Arc<dyn LiveOpener>,
            LiveInput::Syphon {
                server: name.into(),
                app: String::new(),
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

    fn listed(name: &str) -> bool {
        servers().iter().any(|(s, _)| s == name)
    }

    /// Sends frames until `count` received frames have matched a sent
    /// frame exactly; returns the number matched.
    fn exchange(tx: &PublishFeed, rx: &LiveFeed, w: u32, h: u32, count: u32) -> u32 {
        let mut sent: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut seen = rx.newer_than(0).map_or(0, |f| f.seq);
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
                if (f.image.width(), f.image.height()) != (w, h) {
                    continue; // a frame from before a resize
                }
                assert!(
                    sent.contains_key(f.image.rgba8()),
                    "received frame differs from every sent frame"
                );
                matched += 1;
            }
        }
        matched
    }

    fn loopback_is_bit_exact_and_discoverable() {
        let name = unique("loopback");
        let tx = sender(&name);
        tx.submit(test_frame(0, 64, 48));
        wait_until(
            "the server to be announced",
            Duration::from_secs(10),
            || listed(&name),
        );
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

        let matched = exchange(&tx, &rx, 33, 17, 5);
        assert!(matched >= 5, "after resize matched {matched}");

        drop(tx);
        wait_until("the receiver to notice", Duration::from_secs(10), || {
            matches!(rx.state(), LiveState::Retrying { .. })
        });
        wait_until("the server to be retired", Duration::from_secs(10), || {
            !listed(&name)
        });
    }

    fn receiver_reconnects_when_the_server_returns() {
        let name = unique("reconnect");
        let rx = receiver(&name);
        wait_until(
            "a missing server to be reported",
            Duration::from_secs(5),
            || matches!(rx.state(), LiveState::Retrying { .. }),
        );
        for round in 0..3 {
            let tx = sender(&name);
            let matched = exchange(&tx, &rx, 40, 30, 5);
            assert!(matched >= 5, "round {round}: matched {matched}");
            drop(tx);
            wait_until("the loss to be noticed", Duration::from_secs(10), || {
                matches!(rx.state(), LiveState::Retrying { .. })
            });
        }
        assert!(rx.stats().reconnects >= 2, "{:?}", rx.stats());
    }

    fn cross_process_frames_are_bit_exact() {
        let name = unique("cross-process");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .env("OM_TEST_SYPHON_CHILD", &name)
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

    /// Child process: publishes frames as `name` until killed.
    fn child(name: String) {
        let done = Arc::new(AtomicBool::new(false));
        {
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let tx = sender(&name);
                let deadline = Instant::now() + Duration::from_secs(60);
                let mut i = 0;
                while Instant::now() < deadline {
                    tx.submit(test_frame(i, 96, 64));
                    i += 1;
                    std::thread::sleep(Duration::from_millis(16));
                }
                done.store(true, Ordering::Relaxed);
            });
        }
        while !done.load(Ordering::Relaxed) {
            run_main_loop(Duration::from_millis(50));
        }
    }

    type Test = (&'static str, fn());

    const TESTS: &[Test] = &[
        (
            "loopback_is_bit_exact_and_discoverable",
            loopback_is_bit_exact_and_discoverable,
        ),
        (
            "receiver_reconnects_when_the_server_returns",
            receiver_reconnects_when_the_server_returns,
        ),
        (
            "cross_process_frames_are_bit_exact",
            cross_process_frames_are_bit_exact,
        ),
    ];

    pub fn main() {
        if let Ok(name) = std::env::var("OM_TEST_SYPHON_CHILD") {
            child(name);
            return;
        }
        let filter: Vec<String> = std::env::args()
            .skip(1)
            .filter(|a| !a.starts_with('-'))
            .collect();
        let failures = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        {
            let (failures, done) = (Arc::clone(&failures), Arc::clone(&done));
            std::thread::spawn(move || {
                for (name, test) in TESTS {
                    if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
                        continue;
                    }
                    let t = Instant::now();
                    let ok = catch_unwind(AssertUnwindSafe(test)).is_ok();
                    println!(
                        "test {name} ... {} ({:.1} s)",
                        if ok { "ok" } else { "FAILED" },
                        t.elapsed().as_secs_f32()
                    );
                    if !ok {
                        failures.lock().unwrap().push(*name);
                    }
                }
                done.store(true, Ordering::Relaxed);
            });
        }
        while !done.load(Ordering::Relaxed) {
            run_main_loop(Duration::from_millis(50));
        }
        let failures = failures.lock().unwrap();
        if failures.is_empty() {
            println!("test result: ok");
        } else {
            println!("test result: FAILED {failures:?}");
            std::process::exit(1);
        }
    }
}
