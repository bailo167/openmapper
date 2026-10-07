// SPDX-License-Identifier: Apache-2.0
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::AtomicU32;

use super::*;

fn frame(v: u8) -> Arc<StillImage> {
    Arc::new(StillImage::from_rgba8(1, 1, vec![v, v, v, 255]).unwrap())
}

fn fast() -> LiveConfig {
    LiveConfig {
        min_backoff: Duration::from_millis(5),
        max_backoff: Duration::from_millis(40),
        poll: Duration::from_millis(5),
    }
}

fn camera() -> LiveInput {
    LiveInput::Camera {
        device: "test".into(),
    }
}

/// What each successive open does.
#[derive(Clone, Copy)]
enum Plan {
    Fail,
    /// Delivers `n` frames then disconnects.
    Frames(u32),
    /// Delivers `n` frames then goes silent.
    Stall(u32),
}

struct Scripted {
    plans: Mutex<Vec<Plan>>,
    opens: AtomicU32,
    stall: Option<Duration>,
}

impl Scripted {
    fn new(plans: &[Plan], stall: Option<Duration>) -> Arc<Self> {
        let mut plans = plans.to_vec();
        plans.reverse();
        Arc::new(Self {
            plans: Mutex::new(plans),
            opens: AtomicU32::new(0),
            stall,
        })
    }
}

struct Source {
    plan: Plan,
    sent: u32,
    stall: Option<Duration>,
}

impl LiveSource for Source {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<StillImage>>, MediaError> {
        let (n, then_fail) = match self.plan {
            Plan::Fail => unreachable!(),
            Plan::Frames(n) => (n, true),
            Plan::Stall(n) => (n, false),
        };
        if self.sent < n {
            self.sent += 1;
            std::thread::sleep(Duration::from_millis(1));
            return Ok(Some(frame(u8::try_from(self.sent).unwrap_or(255))));
        }
        if then_fail {
            return Err(MediaError::Stream("unplugged".into()));
        }
        std::thread::sleep(timeout);
        Ok(None)
    }

    fn describe(&self) -> String {
        "1×1 test".into()
    }

    fn stall_timeout(&self) -> Option<Duration> {
        self.stall
    }
}

impl LiveOpener for Scripted {
    fn supports(&self, input: &LiveInput) -> bool {
        matches!(input, LiveInput::Camera { .. })
    }

    fn open_live(
        &self,
        _input: &LiveInput,
        _stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        // After the script, stay silent forever (no stall limit).
        let plan = self.plans.lock().unwrap().pop().unwrap_or(Plan::Stall(0));
        match plan {
            Plan::Fail => Err(MediaError::Open {
                path: "test".into(),
                message: "busy".into(),
            }),
            plan => Ok(Box::new(Source {
                plan,
                sent: 0,
                stall: self.stall,
            })),
        }
    }
}

fn wait_for(cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

#[test]
fn retries_failed_opens_then_goes_live() {
    let opener = Scripted::new(&[Plan::Fail, Plan::Fail, Plan::Stall(3)], None);
    let feed = LiveFeed::spawn_with(opener.clone(), camera(), fast());
    assert!(wait_for(|| feed.stats().frames == 3));
    assert_eq!(feed.state(), LiveState::Live);
    assert_eq!(opener.opens.load(Ordering::SeqCst), 3);
    // Failed opens are not reconnects; only re-opens after a success are.
    assert_eq!(feed.stats().reconnects, 0);
    assert_eq!(feed.stats().description, "1×1 test");
}

#[test]
fn reconnects_after_disconnect_and_keeps_counting() {
    let opener = Scripted::new(
        &[Plan::Frames(2), Plan::Fail, Plan::Frames(2), Plan::Stall(1)],
        None,
    );
    let feed = LiveFeed::spawn_with(opener.clone(), camera(), fast());
    assert!(wait_for(|| feed.stats().frames == 5));
    let stats = feed.stats();
    assert_eq!(stats.reconnects, 2);
    // Sequence numbers continue across reconnects.
    assert_eq!(feed.newer_than(0).unwrap().seq, 5);
}

#[test]
fn reports_errors_while_retrying() {
    let opener = Scripted::new(&[Plan::Fail; 100], None);
    let feed = LiveFeed::spawn_with(opener, camera(), fast());
    assert!(wait_for(|| matches!(
        feed.state(),
        LiveState::Retrying { .. }
    )));
    let LiveState::Retrying { error } = feed.state() else {
        unreachable!()
    };
    assert!(error.contains("busy"), "{error}");
}

#[test]
fn stalled_source_is_reopened() {
    let opener = Scripted::new(
        &[Plan::Stall(1), Plan::Stall(1)],
        Some(Duration::from_millis(30)),
    );
    let feed = LiveFeed::spawn_with(opener.clone(), camera(), fast());
    assert!(wait_for(|| feed.stats().frames == 2));
    assert!(opener.opens.load(Ordering::SeqCst) >= 2);
    assert_eq!(feed.stats().reconnects, 1);
}

#[test]
fn quiet_source_without_stall_limit_stays_open() {
    let opener = Scripted::new(&[Plan::Stall(1)], None);
    let feed = LiveFeed::spawn_with(opener.clone(), camera(), fast());
    assert!(wait_for(|| feed.stats().frames == 1));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(opener.opens.load(Ordering::SeqCst), 1);
    assert_eq!(feed.state(), LiveState::Live);
}

#[test]
fn only_the_newest_frame_is_kept() {
    let opener = Scripted::new(&[Plan::Stall(10)], None);
    let feed = LiveFeed::spawn_with(opener, camera(), fast());
    assert!(wait_for(|| feed.stats().frames == 10));
    let f = feed.newer_than(0).unwrap();
    assert_eq!(f.seq, 10);
    assert_eq!(f.image.pixel(0, 0), Some([10, 10, 10, 255]));
    assert!(feed.newer_than(10).is_none());
    // Nine frames were replaced before being taken.
    assert_eq!(feed.stats().dropped, 9);
}

#[test]
fn drop_stops_promptly_even_while_backing_off() {
    let opener = Scripted::new(&[Plan::Fail; 100], None);
    let config = LiveConfig {
        min_backoff: Duration::from_secs(30),
        max_backoff: Duration::from_secs(30),
        poll: Duration::from_millis(5),
    };
    let feed = LiveFeed::spawn_with(opener.clone(), camera(), config);
    assert!(wait_for(|| opener.opens.load(Ordering::SeqCst) >= 1));
    let t = Instant::now();
    drop(feed);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn openers_route_by_support() {
    let openers = LiveOpeners(vec![Scripted::new(&[], None)]);
    assert!(openers.supports(&camera()));
    let ndi = LiveInput::Ndi {
        source: "X (Y)".into(),
    };
    assert!(!openers.supports(&ndi));
    let stop = Arc::new(AtomicBool::new(false));
    let err = openers.open_live(&ndi, &stop).err().unwrap();
    assert!(err.to_string().contains("not available"), "{err}");
}
