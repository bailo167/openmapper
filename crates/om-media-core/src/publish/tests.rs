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

fn target() -> Publish {
    Publish::Ndi {
        name: "test".into(),
    }
}

/// Records what it sends; fails after `fail_after` sends (per open), and
/// fails the first `fail_opens` opens.
struct Recorder {
    sent: Arc<Mutex<Vec<u8>>>,
    opens: AtomicU32,
    fail_opens: u32,
    fail_after: Option<usize>,
    rate: Option<u32>,
}

struct Sink {
    sent: Arc<Mutex<Vec<u8>>>,
    count: usize,
    fail_after: Option<usize>,
    rate: Option<u32>,
}

impl FrameSink for Sink {
    fn send(&mut self, image: &Arc<StillImage>) -> Result<(), MediaError> {
        if self.fail_after.is_some_and(|n| self.count >= n) {
            return Err(MediaError::Stream("receiver went away".into()));
        }
        self.count += 1;
        self.sent
            .lock()
            .unwrap()
            .push(image.pixel(0, 0).unwrap()[0]);
        Ok(())
    }

    fn describe(&self) -> String {
        "test sink".into()
    }

    fn rate(&self) -> Option<u32> {
        self.rate
    }
}

impl SinkOpener for Recorder {
    fn supports(&self, target: &Publish) -> bool {
        matches!(target, Publish::Ndi { .. })
    }

    fn open_sink(
        &self,
        _target: &Publish,
        _stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        let n = self.opens.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_opens {
            return Err(MediaError::Open {
                path: "test".into(),
                message: "port in use".into(),
            });
        }
        Ok(Box::new(Sink {
            sent: Arc::clone(&self.sent),
            count: 0,
            fail_after: self.fail_after,
            rate: self.rate,
        }))
    }
}

fn recorder(fail_opens: u32, fail_after: Option<usize>, rate: Option<u32>) -> Arc<Recorder> {
    Arc::new(Recorder {
        sent: Arc::new(Mutex::new(Vec::new())),
        opens: AtomicU32::new(0),
        fail_opens,
        fail_after,
        rate,
    })
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
fn sends_each_new_frame_once() {
    let rec = recorder(0, None, None);
    let feed = PublishFeed::spawn_with(rec.clone(), target(), fast());
    for v in 1..=5 {
        feed.submit(frame(v));
        assert!(wait_for(|| rec.sent.lock().unwrap().len() == usize::from(v)));
    }
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(*rec.sent.lock().unwrap(), vec![1, 2, 3, 4, 5]);
    assert_eq!(feed.state(), SinkState::Sending);
    assert_eq!(feed.stats().sent, 5);
}

#[test]
fn newest_frame_wins_when_the_sink_is_busy() {
    let rec = recorder(1, None, None);
    let feed = PublishFeed::spawn_with(rec.clone(), target(), fast());
    // The first open fails, so these queue up while it backs off.
    for v in 1..=4 {
        feed.submit(frame(v));
    }
    assert!(wait_for(|| !rec.sent.lock().unwrap().is_empty()));
    assert_eq!(*rec.sent.lock().unwrap(), vec![4]);
    assert_eq!(feed.stats().skipped, 3);
}

#[test]
fn fixed_rate_sinks_repeat_the_newest_frame() {
    let rec = recorder(0, None, Some(200));
    let feed = PublishFeed::spawn_with(rec.clone(), target(), fast());
    feed.submit(frame(7));
    assert!(wait_for(|| rec.sent.lock().unwrap().len() >= 10));
    assert!(rec.sent.lock().unwrap().iter().all(|v| *v == 7));
    feed.submit(frame(8));
    assert!(wait_for(|| rec.sent.lock().unwrap().last() == Some(&8)));
}

#[test]
fn failed_sink_is_reopened_and_resends() {
    let rec = recorder(0, Some(2), None);
    let feed = PublishFeed::spawn_with(rec.clone(), target(), fast());
    for v in 1..=3 {
        feed.submit(frame(v));
        std::thread::sleep(Duration::from_millis(20));
    }
    // Frame 3 failed on the first sink, then went out on the reopened one.
    assert!(wait_for(|| rec.sent.lock().unwrap().len() == 3));
    assert_eq!(*rec.sent.lock().unwrap(), vec![1, 2, 3]);
    assert!(feed.stats().reconnects >= 1);
}

#[test]
fn reports_open_errors() {
    let rec = recorder(u32::MAX, None, None);
    let feed = PublishFeed::spawn_with(rec, target(), fast());
    assert!(wait_for(|| matches!(
        feed.state(),
        SinkState::Retrying { .. }
    )));
    let SinkState::Retrying { error } = feed.state() else {
        unreachable!()
    };
    assert!(error.contains("port in use"), "{error}");
}

#[test]
fn drop_is_prompt() {
    let rec = recorder(0, None, Some(1));
    let feed = PublishFeed::spawn_with(rec, target(), fast());
    feed.submit(frame(1));
    std::thread::sleep(Duration::from_millis(20));
    let t = Instant::now();
    drop(feed);
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
}
