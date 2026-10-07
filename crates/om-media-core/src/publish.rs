// SPDX-License-Identifier: Apache-2.0
//! Publishing output frames to other applications and the network.
//!
//! An adapter implements [`SinkOpener`] / [`FrameSink`]. A [`PublishFeed`]
//! runs one sink on its own thread: the renderer hands it the newest frame
//! ([`PublishFeed::submit`] never blocks), the sink sends either every new
//! frame or — for fixed-rate sinks such as encoded streams — the newest
//! frame at each tick, and a failed sink is reopened with back-off.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use om_project::Publish;

use crate::{LiveConfig, MediaError, StillImage};

/// One open publishing target.
pub trait FrameSink: Send {
    /// Sends one frame (sRGB RGBA8, straight alpha).
    fn send(&mut self, image: &Arc<StillImage>) -> Result<(), MediaError>;

    /// Summary for status display.
    fn describe(&self) -> String;

    /// `Some(fps)`: send the newest frame at this fixed rate (repeating it
    /// when nothing new arrived). `None`: send each new frame once.
    fn rate(&self) -> Option<u32> {
        None
    }
}

/// Opens publishing targets of the kinds an adapter supports.
pub trait SinkOpener: Send + Sync {
    fn supports(&self, target: &Publish) -> bool;

    /// Opens `target`. Opens that wait for a receiver (listen mode) should
    /// give up soon after `stop` is set.
    fn open_sink(
        &self,
        target: &Publish,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError>;
}

/// Several adapters behind one opener.
#[derive(Clone, Default)]
pub struct SinkOpeners(pub Vec<Arc<dyn SinkOpener>>);

impl std::fmt::Debug for SinkOpeners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SinkOpeners({})", self.0.len())
    }
}

impl SinkOpener for SinkOpeners {
    fn supports(&self, target: &Publish) -> bool {
        self.0.iter().any(|o| o.supports(target))
    }

    fn open_sink(
        &self,
        target: &Publish,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        match self.0.iter().find(|o| o.supports(target)) {
            Some(o) => o.open_sink(target, stop),
            None => Err(MediaError::Open {
                path: target.label(),
                message: "this publish type is not available in this build or on this platform"
                    .into(),
            }),
        }
    }
}

/// Connection state of a [`PublishFeed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkState {
    /// Opening (or waiting for a receiver to connect).
    Connecting,
    Sending,
    Retrying {
        error: String,
    },
}

/// Counters for status display and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SinkStats {
    /// Frames sent (including repeats at fixed rate).
    pub sent: u64,
    /// Submitted frames replaced before they were sent.
    pub skipped: u64,
    /// Successful opens after the first.
    pub reconnects: u64,
    pub description: String,
}

#[derive(Default)]
struct Inner {
    done: bool,
    state: Option<SinkState>,
    pending: Option<Arc<StillImage>>,
    stats: SinkStats,
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A supervised publishing target running on its own thread.
pub struct PublishFeed {
    target: Publish,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for PublishFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublishFeed({})", self.target.label())
    }
}

impl PublishFeed {
    #[must_use]
    pub fn spawn(opener: Arc<dyn SinkOpener>, target: Publish) -> Self {
        Self::spawn_with(opener, target, LiveConfig::default())
    }

    #[must_use]
    pub fn spawn_with(opener: Arc<dyn SinkOpener>, target: Publish, config: LiveConfig) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                state: Some(SinkState::Connecting),
                ..Inner::default()
            }),
            wake: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop, target) = (Arc::clone(&shared), Arc::clone(&stop), target.clone());
            std::thread::Builder::new()
                .name("om-publish".into())
                .spawn(move || {
                    run(&*opener, &target, &shared, &stop, config);
                    shared.lock().done = true;
                })
                .ok()
        };
        Self {
            target,
            shared,
            stop,
            thread,
        }
    }

    #[must_use]
    pub fn target(&self) -> &Publish {
        &self.target
    }

    /// Offers the newest output frame. Never blocks on the sink.
    pub fn submit(&self, image: Arc<StillImage>) {
        let mut inner = self.shared.lock();
        if inner.pending.replace(image).is_some() {
            inner.stats.skipped += 1;
        }
        drop(inner);
        self.shared.wake.notify_one();
    }

    #[must_use]
    pub fn state(&self) -> SinkState {
        self.shared
            .lock()
            .state
            .clone()
            .unwrap_or(SinkState::Connecting)
    }

    #[must_use]
    pub fn stats(&self) -> SinkStats {
        self.shared.lock().stats.clone()
    }
}

const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl Drop for PublishFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
        let Some(t) = self.thread.take() else { return };
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !self.shared.lock().done && Instant::now() < deadline {
            self.shared.wake.notify_all();
            std::thread::sleep(Duration::from_millis(2));
        }
        if self.shared.lock().done {
            let _ = t.join();
        }
    }
}

/// Waits until `deadline` (or a stop), without taking frames.
fn wait_until(shared: &Shared, stop: &AtomicBool, deadline: Instant) {
    let mut inner = shared.lock();
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        inner = shared
            .wake
            .wait_timeout(inner, deadline - now)
            .map_or_else(|e| e.into_inner().0, |(g, _)| g);
    }
}

/// Takes the pending frame, waiting up to `timeout` for one.
fn take_frame(shared: &Shared, stop: &AtomicBool, timeout: Duration) -> Option<Arc<StillImage>> {
    let deadline = Instant::now() + timeout;
    let mut inner = shared.lock();
    loop {
        if let Some(f) = inner.pending.take() {
            return Some(f);
        }
        let now = Instant::now();
        if stop.load(Ordering::Relaxed) || now >= deadline {
            return None;
        }
        inner = shared
            .wake
            .wait_timeout(inner, deadline - now)
            .map_or_else(|e| e.into_inner().0, |(g, _)| g);
    }
}

fn run(
    opener: &dyn SinkOpener,
    target: &Publish,
    shared: &Shared,
    stop: &Arc<AtomicBool>,
    config: LiveConfig,
) {
    let mut backoff = config.min_backoff;
    let mut opened_before = false;
    let mut current: Option<Arc<StillImage>> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut sink = match opener.open_sink(target, stop) {
            Ok(s) => s,
            Err(e) => {
                shared.lock().state = Some(SinkState::Retrying {
                    error: e.to_string(),
                });
                wait_until(shared, stop, Instant::now() + backoff);
                backoff = (backoff * 2).min(config.max_backoff);
                continue;
            }
        };
        {
            let mut inner = shared.lock();
            if opened_before {
                inner.stats.reconnects += 1;
            }
            inner.stats.description = sink.describe();
            inner.state = Some(SinkState::Connecting);
        }
        opened_before = true;
        let period = sink
            .rate()
            .filter(|r| *r > 0)
            .map(|r| Duration::from_secs(1) / r);
        let start = Instant::now();
        let mut tick: u32 = 0;
        // A (re)connected receiver gets the current picture straight away.
        let mut resend = current.is_some();
        let error = loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let frame = match period {
                Some(p) => {
                    // Absolute schedule: no drift from send durations.
                    tick = tick.saturating_add(1);
                    wait_until(shared, stop, start + p * tick);
                    if let Some(f) = shared.lock().pending.take() {
                        current = Some(f);
                    }
                    let behind = start.elapsed().saturating_sub(p * tick);
                    if behind > p * 4 {
                        // Sending is slower than the rate: skip ticks.
                        tick = u32::try_from(start.elapsed().as_nanos() / p.as_nanos().max(1))
                            .unwrap_or(u32::MAX);
                    }
                    current.clone()
                }
                None if resend => {
                    resend = false;
                    current.clone()
                }
                None => take_frame(shared, stop, config.poll).inspect(|f| {
                    current = Some(Arc::clone(f));
                }),
            };
            let Some(frame) = frame else { continue };
            match sink.send(&frame) {
                Ok(()) => {
                    backoff = config.min_backoff;
                    let mut inner = shared.lock();
                    inner.stats.sent += 1;
                    inner.state = Some(SinkState::Sending);
                }
                Err(e) => break e.to_string(),
            }
        };
        drop(sink);
        shared.lock().state = Some(SinkState::Retrying { error });
        wait_until(shared, stop, Instant::now() + backoff);
        backoff = (backoff * 2).min(config.max_backoff);
    }
}

#[cfg(test)]
mod tests;
