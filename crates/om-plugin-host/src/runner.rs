// SPDX-License-Identifier: Apache-2.0
//! Runs a plugin on its own thread so render and control threads never
//! wait on plugin code. Frames are newest-wins; the newest result is kept.
//! A faulted instance (crash, timeout, memory) is replaced after a back-off;
//! after repeated faults the runner gives up until reset.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{Plugin, PluginError, PluginHost};

/// Consecutive faults before the runner stops retrying.
pub const MAX_FAULTS: u32 = 3;

/// One frame to process.
#[derive(Debug, Clone)]
pub struct Job {
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub params: Vec<f32>,
    pub time: f64,
}

/// A processed frame: the output for the job with sequence number `seq`.
#[derive(Debug, Clone)]
pub struct Output {
    pub rgba: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub seq: u64,
}

/// What the runner is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerState {
    Running,
    /// Faulted; retrying after a pause.
    Faulted {
        error: PluginError,
        count: u32,
    },
    /// Gave up after [`MAX_FAULTS`] consecutive faults.
    Disabled {
        error: PluginError,
    },
}

/// Log lines kept between [`PluginRunner::take_log`] calls (newest win).
pub const MAX_LOG_LINES: usize = 256;

#[derive(Debug, Default)]
struct Inner {
    pending: Option<(u64, Job)>,
    output: Option<Output>,
    state: Option<RunnerState>,
    log: Vec<String>,
    processed: u64,
    done: bool,
}

impl Inner {
    fn log(&mut self, lines: impl IntoIterator<Item = String>) {
        self.log.extend(lines);
        let excess = self.log.len().saturating_sub(MAX_LOG_LINES);
        self.log.drain(..excess);
    }
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

/// A plugin running on a worker thread.
pub struct PluginRunner {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    next_seq: u64,
}

impl std::fmt::Debug for PluginRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PluginRunner")
    }
}

impl PluginRunner {
    #[must_use]
    pub fn spawn(host: Arc<PluginHost>, plugin: Plugin) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                state: Some(RunnerState::Running),
                ..Inner::default()
            }),
            wake: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("om-plugin".into())
                .spawn(move || {
                    run(&host, &plugin, &shared, &stop);
                    shared.lock().done = true;
                })
                .ok()
        };
        if thread.is_none() {
            shared.lock().state = Some(RunnerState::Disabled {
                error: PluginError::Invalid("could not start a plugin thread".into()),
            });
        }
        Self {
            shared,
            stop,
            thread,
            next_seq: 0,
        }
    }

    /// Queues a frame (replacing any frame not yet started). Never blocks
    /// on plugin code. Returns the job's sequence number.
    pub fn submit(&mut self, job: Job) -> u64 {
        self.next_seq += 1;
        let seq = self.next_seq;
        self.shared.lock().pending = Some((seq, job));
        self.shared.wake.notify_one();
        seq
    }

    /// The newest processed frame.
    #[must_use]
    pub fn output(&self) -> Option<Output> {
        self.shared.lock().output.clone()
    }

    #[must_use]
    pub fn state(&self) -> RunnerState {
        self.shared
            .lock()
            .state
            .clone()
            .unwrap_or(RunnerState::Running)
    }

    /// Frames processed so far.
    #[must_use]
    pub fn processed(&self) -> u64 {
        self.shared.lock().processed
    }

    /// Plugin log messages since the last call.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().log)
    }

    /// Waits up to `timeout` for the output of job `seq` or later (tests).
    #[must_use]
    pub fn wait_for(&self, seq: u64, timeout: Duration) -> Option<Output> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(o) = self.output().filter(|o| o.seq >= seq) {
                return Some(o);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for PluginRunner {
    fn drop(&mut self) {
        // Never waits on plugin code: the thread finishes any call in
        // progress (bounded by the epoch deadline), sees `stop` and exits.
        // Joined only if already finished, otherwise detached.
        self.stop.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take()
            && self.shared.lock().done
        {
            let _ = t.join();
        }
    }
}

fn run(host: &PluginHost, plugin: &Plugin, shared: &Shared, stop: &AtomicBool) {
    let mut instance = None;
    let mut faults = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let (seq, job) = {
            let mut inner = shared.lock();
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(j) = inner.pending.take() {
                    break j;
                }
                inner = shared
                    .wake
                    .wait_timeout(inner, Duration::from_millis(100))
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
        };
        if instance.is_none() {
            match host.instantiate(plugin) {
                Ok(i) => instance = Some(i),
                Err(error) => {
                    faults += 1;
                    if !record_fault(shared, error, faults, stop) {
                        return;
                    }
                    continue;
                }
            }
        }
        let Some(inst) = instance.as_mut() else {
            continue;
        };
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let result = inst.process(&job.rgba, job.width, job.height, &job.params, job.time);
        let log = inst.take_log();
        match result {
            Ok(rgba) => {
                faults = 0;
                let mut inner = shared.lock();
                inner.output = Some(Output {
                    rgba: Arc::new(rgba),
                    width: job.width,
                    height: job.height,
                    seq,
                });
                inner.processed += 1;
                inner.state = Some(RunnerState::Running);
                inner.log(log);
            }
            Err(PluginError::Failed(code)) => {
                let mut inner = shared.lock();
                inner.log(log);
                inner.log([format!("plugin returned error {code}")]);
            }
            Err(error) => {
                instance = None;
                faults += 1;
                shared.lock().log(log);
                if !record_fault(shared, error, faults, stop) {
                    return;
                }
            }
        }
    }
}

/// Records a fault; returns false (and disables the runner) after too many.
fn record_fault(shared: &Shared, error: PluginError, count: u32, stop: &AtomicBool) -> bool {
    if count >= MAX_FAULTS {
        shared.lock().state = Some(RunnerState::Disabled { error });
        return false;
    }
    shared.lock().state = Some(RunnerState::Faulted { error, count });
    // Back off before trying a fresh instance (interruptible).
    let deadline = Instant::now() + Duration::from_millis(200 * u64::from(count));
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}
