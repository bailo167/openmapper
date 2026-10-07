// SPDX-License-Identifier: Apache-2.0
//! Live video feeds: cameras, network streams and other applications.
//!
//! An adapter implements [`LiveOpener`] / [`LiveSource`]. A [`LiveFeed`] runs
//! one source on its own thread and supervises it: it keeps only the newest
//! frame (live video never queues — a late frame is worthless), treats a
//! source that stops delivering as disconnected, and reopens it with
//! exponential back-off until the feed is dropped. The last good frame stays
//! visible while reconnecting.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use om_project::LiveInput;

use crate::{MediaError, StillImage};

/// One open live source.
pub trait LiveSource: Send {
    /// Waits up to `timeout` for the next frame. `Ok(None)` means none
    /// arrived yet (still connected); `Err` means the source is gone and
    /// will be reopened.
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Arc<StillImage>>, MediaError>;

    /// Format summary for status display (`1280×720 h264`).
    fn describe(&self) -> String;

    /// How long without a frame counts as a lost connection. `None` for
    /// sources that legitimately go quiet (senders that publish only on
    /// change); those report disconnection through `next_frame` instead.
    fn stall_timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(5))
    }
}

/// Opens live sources of the kinds an adapter supports.
pub trait LiveOpener: Send + Sync {
    /// Whether this opener handles `input`.
    fn supports(&self, input: &LiveInput) -> bool;

    /// Opens `input`. Slow opens should give up soon after `stop` is set.
    fn open_live(
        &self,
        input: &LiveInput,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError>;

    /// Inputs currently available (connected cameras, announced senders).
    /// May take up to about a second; call it off the UI thread.
    fn discover(&self) -> Vec<LiveInput> {
        Vec::new()
    }
}

/// Several adapters behind one opener; the first that supports an input
/// opens it.
#[derive(Clone, Default)]
pub struct LiveOpeners(pub Vec<Arc<dyn LiveOpener>>);

impl std::fmt::Debug for LiveOpeners {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LiveOpeners({})", self.0.len())
    }
}

impl LiveOpener for LiveOpeners {
    fn supports(&self, input: &LiveInput) -> bool {
        self.0.iter().any(|o| o.supports(input))
    }

    fn open_live(
        &self,
        input: &LiveInput,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn LiveSource>, MediaError> {
        match self.0.iter().find(|o| o.supports(input)) {
            Some(o) => o.open_live(input, stop),
            None => Err(MediaError::Open {
                path: input.label(),
                message: "this input type is not available in this build or on this platform"
                    .into(),
            }),
        }
    }

    fn discover(&self) -> Vec<LiveInput> {
        let mut all: Vec<LiveInput> = self.0.iter().flat_map(|o| o.discover()).collect();
        all.dedup();
        all
    }
}

/// Connection state of a [`LiveFeed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveState {
    /// Opening, or open but no frame yet.
    Connecting,
    /// Frames are arriving.
    Live,
    /// Could not open, or the connection was lost; retrying.
    Retrying { error: String },
}

/// The newest frame of a feed. `seq` increases by one per received frame
/// (across reconnects), so consumers can tell new frames from old ones.
#[derive(Debug, Clone)]
pub struct LiveFrame {
    pub image: Arc<StillImage>,
    pub seq: u64,
}

/// Counters for status display and tests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveStats {
    /// Frames received in total.
    pub frames: u64,
    /// Frames replaced by a newer one before anyone took them.
    pub dropped: u64,
    /// Successful opens after the first.
    pub reconnects: u64,
    /// Smoothed receive rate (frames per second).
    pub fps: f64,
    /// What the source reported when opened.
    pub description: String,
}

/// Timing of the supervisor (defaults suit real devices; tests shorten them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveConfig {
    /// First retry delay; doubles after each failure.
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    /// Longest single wait inside the source (bounds shutdown latency).
    pub poll: Duration,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            min_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(4),
            poll: Duration::from_millis(100),
        }
    }
}

#[derive(Default)]
struct Inner {
    /// The supervisor thread has finished.
    done: bool,
    state: Option<LiveState>,
    latest: Option<LiveFrame>,
    taken: u64,
    stats: LiveStats,
}

/// A supervised live source running on its own thread.
pub struct LiveFeed {
    input: LiveInput,
    shared: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for LiveFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LiveFeed({})", self.input.label())
    }
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl LiveFeed {
    /// Starts receiving `input` through `opener`.
    #[must_use]
    pub fn spawn(opener: Arc<dyn LiveOpener>, input: LiveInput) -> Self {
        Self::spawn_with(opener, input, LiveConfig::default())
    }

    #[must_use]
    pub fn spawn_with(opener: Arc<dyn LiveOpener>, input: LiveInput, config: LiveConfig) -> Self {
        let shared = Arc::new(Mutex::new(Inner {
            state: Some(LiveState::Connecting),
            ..Inner::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop, input) = (Arc::clone(&shared), Arc::clone(&stop), input.clone());
            std::thread::Builder::new()
                .name("om-live".into())
                .spawn(move || {
                    supervise(&*opener, &input, &shared, &stop, config);
                    lock(&shared).done = true;
                })
                .ok()
        };
        Self {
            input,
            shared,
            stop,
            thread,
        }
    }

    #[must_use]
    pub fn input(&self) -> &LiveInput {
        &self.input
    }

    #[must_use]
    pub fn state(&self) -> LiveState {
        lock(&self.shared)
            .state
            .clone()
            .unwrap_or(LiveState::Connecting)
    }

    #[must_use]
    pub fn stats(&self) -> LiveStats {
        lock(&self.shared).stats.clone()
    }

    /// The newest frame if it is newer than `seen` (a previously taken
    /// `seq`; pass 0 initially).
    #[must_use]
    pub fn newer_than(&self, seen: u64) -> Option<LiveFrame> {
        let mut inner = lock(&self.shared);
        let frame = inner.latest.as_ref().filter(|f| f.seq > seen)?.clone();
        inner.taken = inner.taken.max(frame.seq);
        Some(frame)
    }

    /// Waits up to `timeout` for a frame newer than `seen` (tests, offline).
    #[must_use]
    pub fn wait_newer_than(&self, seen: u64, timeout: Duration) -> Option<LiveFrame> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(f) = self.newer_than(seen) {
                return Some(f);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// How long dropping a feed waits for its thread. A driver stuck in a
/// blocking call (some capture APIs never time out) is detached instead of
/// freezing the caller.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

impl Drop for LiveFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let Some(t) = self.thread.take() else { return };
        t.thread().unpark();
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !lock(&self.shared).done && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        if lock(&self.shared).done {
            let _ = t.join();
        }
    }
}

/// Sleeps up to `d`, waking early when `stop` is set (Drop unparks).
fn nap(d: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::park_timeout(deadline - now);
    }
}

fn supervise(
    opener: &dyn LiveOpener,
    input: &LiveInput,
    shared: &Mutex<Inner>,
    stop: &Arc<AtomicBool>,
    config: LiveConfig,
) {
    let mut backoff = config.min_backoff;
    let mut opened_before = false;
    let set_state = |s: LiveState| lock(shared).state = Some(s);
    while !stop.load(Ordering::Relaxed) {
        let mut source = match opener.open_live(input, stop) {
            Ok(s) => s,
            Err(e) => {
                set_state(LiveState::Retrying {
                    error: e.to_string(),
                });
                nap(backoff, stop);
                backoff = (backoff * 2).min(config.max_backoff);
                continue;
            }
        };
        {
            let mut inner = lock(shared);
            if opened_before {
                inner.stats.reconnects += 1;
            }
            inner.stats.description = source.describe();
            inner.state = Some(LiveState::Connecting);
        }
        opened_before = true;
        let stall = source.stall_timeout();
        let mut last_frame = Instant::now();
        let error = loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match source.next_frame(config.poll) {
                Ok(Some(image)) => {
                    let now = Instant::now();
                    let dt = now.duration_since(last_frame).as_secs_f64();
                    last_frame = now;
                    backoff = config.min_backoff;
                    let mut inner = lock(shared);
                    let seq = inner.stats.frames + 1;
                    if inner.latest.as_ref().is_some_and(|f| f.seq > inner.taken) {
                        inner.stats.dropped += 1;
                    }
                    inner.latest = Some(LiveFrame { image, seq });
                    inner.stats.frames = seq;
                    if dt > 0.0 {
                        let fps = 1.0 / dt;
                        inner.stats.fps = if inner.stats.fps > 0.0 {
                            inner.stats.fps * 0.9 + fps * 0.1
                        } else {
                            fps
                        };
                    }
                    inner.state = Some(LiveState::Live);
                }
                Ok(None) => {
                    if let Some(limit) = stall
                        && last_frame.elapsed() > limit
                    {
                        break format!("no frames for {} s", limit.as_secs_f32());
                    }
                }
                Err(e) => break e.to_string(),
            }
        };
        drop(source);
        {
            let mut inner = lock(shared);
            inner.state = Some(LiveState::Retrying { error });
            inner.stats.fps = 0.0;
        }
        nap(backoff, stop);
        backoff = (backoff * 2).min(config.max_backoff);
    }
}

#[cfg(test)]
mod tests;
