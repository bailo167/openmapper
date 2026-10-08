// SPDX-License-Identifier: Apache-2.0
//! Background discovery of live inputs (cameras, NDI/Syphon/Spout senders).
//!
//! Discovery can take about a second (network announcements), so it runs
//! on its own thread, and only while someone recently asked for the list.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use om_media_core::LiveOpener;
use om_project::LiveInput;

/// How often the list refreshes while wanted.
const REFRESH: Duration = Duration::from_secs(2);
/// Discovery pauses this long after the last request.
const IDLE_AFTER: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    inputs: Vec<LiveInput>,
    requested: Option<Instant>,
    refreshed: Option<Instant>,
}

/// Keeps a recent list of available live inputs.
pub struct Discovery {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Discovery")
    }
}

impl Discovery {
    #[must_use]
    pub fn new(opener: Arc<dyn LiveOpener>) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (state, stop) = (Arc::clone(&state), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("om-discovery".into())
                .spawn(move || run(&*opener, &state, &stop))
                .ok()
        };
        Self {
            state,
            stop,
            thread,
        }
    }

    /// The latest list; also keeps discovery running for a while.
    #[must_use]
    pub fn inputs(&self) -> Vec<LiveInput> {
        let mut s = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = s.requested.is_none();
        s.requested = Some(Instant::now());
        let list = s.inputs.clone();
        drop(s);
        if first && let Some(t) = &self.thread {
            t.thread().unpark();
        }
        list
    }

    /// True once at least one scan finished.
    #[must_use]
    pub fn scanned(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.refreshed.is_some())
            .unwrap_or(false)
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            // Discovery calls are bounded (about a second); wait for them.
            let _ = t.join();
        }
    }
}

fn run(opener: &dyn LiveOpener, state: &Mutex<State>, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let wanted = state
            .lock()
            .ok()
            .and_then(|s| s.requested)
            .is_some_and(|t| t.elapsed() < IDLE_AFTER);
        if wanted {
            let inputs = opener.discover();
            if let Ok(mut s) = state.lock() {
                s.inputs = inputs;
                s.refreshed = Some(Instant::now());
            }
            std::thread::park_timeout(REFRESH);
        } else {
            std::thread::park_timeout(Duration::from_millis(500));
        }
    }
}
