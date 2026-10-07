// SPDX-License-Identifier: Apache-2.0
//! Threaded decode-ahead.
//!
//! A [`VideoPlayer`] owns a decoder thread that keeps a bounded queue of
//! upcoming frames. The render thread calls [`VideoPlayer::frame_at`], which
//! never blocks on decoding: it returns the frame covering the requested
//! time if it is queued, otherwise the last frame shown (and asks the
//! decoder to jump). Decoder errors are reported, never panicked.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use om_time::RationalTime;

use crate::source::{FrameCursor, MediaDescriptor, MediaSource, VideoFrame};

/// Frames decoded ahead of the playhead.
pub const DEFAULT_QUEUE: usize = 6;

#[derive(Debug)]
struct State {
    /// Where the render side wants frames from.
    target: RationalTime,
    /// Bumped when the decoder must reposition (seek/discontinuity).
    generation: u64,
    queue: VecDeque<VideoFrame>,
    capacity: usize,
    end_of_stream: bool,
    error: Option<String>,
    shutdown: bool,
    stats: PlayerStats,
}

/// Counters for diagnostics and tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlayerStats {
    /// Requests served from the queue.
    pub hits: u64,
    /// Requests that had to hold the previous frame.
    pub misses: u64,
    /// Repositionings requested.
    pub jumps: u64,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock means the decoder thread panicked mid-update; the
        // state is still plain data, so keep going rather than propagate.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A media source decoded on its own thread.
pub struct VideoPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    descriptor: MediaDescriptor,
    last: Option<VideoFrame>,
}

impl std::fmt::Debug for VideoPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayer")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

/// True if `frame` is displayed at `t`. `last` means it is the final frame
/// of an ended stream, which holds for all later times; otherwise a frame
/// whose end is unknown (successor not decoded yet) does not cover `t`.
fn covers(frame: &VideoFrame, next: Option<&VideoFrame>, t: RationalTime, eos: bool) -> bool {
    use std::cmp::Ordering::Greater;
    let starts = frame.pts.checked_cmp(t).is_ok_and(|o| o != Greater);
    let end = next
        .map(|n| n.pts)
        .or_else(|| frame.duration.and_then(|d| frame.pts.checked_add(d).ok()));
    starts
        && match end {
            Some(e) => eos || e.checked_cmp(t).is_ok_and(|o| o == Greater),
            None => eos,
        }
}

impl VideoPlayer {
    /// Starts a decoder thread for `source`, positioned at `start`.
    pub fn spawn<S: MediaSource + 'static>(
        source: S,
        start: RationalTime,
        capacity: usize,
    ) -> Self {
        let descriptor = source.descriptor().clone();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                target: start,
                generation: 1,
                queue: VecDeque::new(),
                capacity: capacity.max(2),
                end_of_stream: false,
                error: None,
                shutdown: false,
                stats: PlayerStats::default(),
            }),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("om-decode".into())
            .spawn(move || decode_loop(source, &worker))
            .ok();
        if thread.is_none() {
            shared.lock().error = Some("could not start decoder thread".into());
        }
        Self {
            shared,
            thread,
            descriptor,
            last: None,
        }
    }

    #[must_use]
    pub fn descriptor(&self) -> &MediaDescriptor {
        &self.descriptor
    }

    /// The frame to show at `t`, without blocking on decode. Returns the
    /// previously shown frame while the decoder catches up after a jump.
    pub fn frame_at(&mut self, t: RationalTime) -> Option<&VideoFrame> {
        self.lookup(t);
        self.last.as_ref()
    }

    /// Updates `last` for `t`; true if the queue held the frame for `t`.
    fn lookup(&mut self, t: RationalTime) -> bool {
        use std::cmp::Ordering::{Greater, Less};
        let mut st = self.shared.lock();
        st.target = t;
        // Drop frames that ended before t.
        while st.queue.len() >= 2 && st.queue[1].pts.checked_cmp(t).is_ok_and(|o| o != Greater) {
            st.queue.pop_front();
        }
        let eos = st.end_of_stream && st.queue.len() == 1;
        let hit = st
            .queue
            .front()
            .filter(|f| covers(f, st.queue.get(1), t, eos))
            .cloned();
        match hit {
            Some(f) => {
                st.stats.hits += 1;
                drop(st);
                self.shared.wake.notify_all();
                self.last = Some(f);
                true
            }
            None => {
                st.stats.misses += 1;
                // Behind the queue, or beyond what sequential decoding will
                // reach soon: reposition.
                let behind = st
                    .queue
                    .front()
                    .is_some_and(|f| f.pts.checked_cmp(t).is_ok_and(|o| o == Greater));
                let far = st.queue.back().is_some_and(|b| {
                    t.checked_sub(b.pts)
                        .and_then(|d| d.checked_cmp(RationalTime::from_seconds(1)))
                        .is_ok_and(|o| o != Less)
                });
                let stalled_empty = st.queue.is_empty() && st.end_of_stream;
                if behind || far || stalled_empty {
                    st.generation += 1;
                    st.queue.clear();
                    st.end_of_stream = false;
                    st.stats.jumps += 1;
                }
                drop(st);
                self.shared.wake.notify_all();
                false
            }
        }
    }

    /// Blocks until the frame for `t` is available (offline rendering,
    /// tests). Gives up after `timeout`.
    pub fn frame_at_blocking(
        &mut self,
        t: RationalTime,
        timeout: std::time::Duration,
    ) -> Option<&VideoFrame> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.lookup(t) || std::time::Instant::now() >= deadline || self.error().is_some() {
                return self.last.as_ref();
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Last decoder error, if any.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }

    #[must_use]
    pub fn stats(&self) -> PlayerStats {
        self.shared.lock().stats
    }

    /// Frames currently queued (bounded by the capacity).
    #[must_use]
    pub fn queued(&self) -> usize {
        self.shared.lock().queue.len()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn decode_loop<S: MediaSource>(source: S, shared: &Shared) {
    let mut cursor = FrameCursor::new(source);
    let mut generation = 0;
    loop {
        // Wait for work: a reposition, or room in the queue.
        let (target, gen_now) = {
            let mut st = shared.lock();
            loop {
                if st.shutdown {
                    return;
                }
                let reposition = st.generation != generation;
                let room = st.queue.len() < st.capacity && !st.end_of_stream && st.error.is_none();
                if reposition || room {
                    break;
                }
                st = shared
                    .wake
                    .wait(st)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            (st.target, st.generation)
        };
        let result = if gen_now != generation {
            generation = gen_now;
            cursor.frame_at(target).map(|f| f.cloned())
        } else {
            cursor.advance().map(|f| f.cloned())
        };
        let mut st = shared.lock();
        if st.generation != generation {
            continue; // a newer jump arrived while decoding; discard
        }
        match result {
            Ok(Some(frame)) => {
                // frame_at may return the same frame that is already queued.
                if st.queue.back().is_none_or(|b| b.pts != frame.pts) {
                    st.queue.push_back(frame);
                } else {
                    st.end_of_stream = true;
                }
                st.error = None;
            }
            Ok(None) => st.end_of_stream = true,
            Err(e) => st.error = Some(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MediaError, StillImage};
    use om_time::Rate;
    use std::time::Duration;

    /// A synthetic source: frame i is a 1x1 image whose red channel is i.
    struct Counter {
        rate: Rate,
        frames: i64,
        next: i64,
        desc: MediaDescriptor,
        delay: Duration,
    }

    impl Counter {
        fn new(frames: i64, delay: Duration) -> Self {
            let rate = Rate::FPS_25;
            Self {
                rate,
                frames,
                next: 0,
                delay,
                desc: MediaDescriptor {
                    width: 1,
                    height: 1,
                    frame_rate: Some(rate),
                    duration: RationalTime::from_frame(frames, rate).ok(),
                    audio: None,
                    codec: "counter".into(),
                },
            }
        }
    }

    impl MediaSource for Counter {
        fn descriptor(&self) -> &MediaDescriptor {
            &self.desc
        }
        fn seek(&mut self, t: RationalTime) -> Result<(), MediaError> {
            // Keyframes every 10 frames.
            self.next = (t.to_frame_floor(self.rate).unwrap_or(0) / 10 * 10).clamp(0, self.frames);
            Ok(())
        }
        fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
            if self.next >= self.frames {
                return Ok(None);
            }
            std::thread::sleep(self.delay);
            let i = self.next;
            self.next += 1;
            Ok(Some(VideoFrame {
                pts: RationalTime::from_frame(i, self.rate).unwrap(),
                duration: None,
                image: Arc::new(StillImage::from_rgba8(1, 1, vec![i as u8, 0, 0, 255]).unwrap()),
            }))
        }
    }

    fn red(f: Option<&VideoFrame>) -> Option<u8> {
        f.and_then(|f| f.image.pixel(0, 0)).map(|p| p[0])
    }

    #[test]
    fn playback_is_frame_exact_and_bounded() {
        let mut p = VideoPlayer::spawn(Counter::new(100, Duration::ZERO), RationalTime::ZERO, 4);
        let rate = Rate::FPS_25;
        for i in 0..100 {
            let t = RationalTime::from_frame(i, rate).unwrap();
            assert_eq!(
                red(p.frame_at_blocking(t, Duration::from_secs(5))),
                Some(i as u8),
                "frame {i}"
            );
            assert!(p.queued() <= 4);
        }
        // Past the end holds the last frame.
        let end = RationalTime::from_seconds(60);
        assert_eq!(
            red(p.frame_at_blocking(end, Duration::from_secs(5))),
            Some(99)
        );
        // Jump backwards.
        let t = RationalTime::from_frame(37, rate).unwrap();
        assert_eq!(
            red(p.frame_at_blocking(t, Duration::from_secs(5))),
            Some(37)
        );
        assert!(p.stats().jumps >= 1);
    }

    #[test]
    fn slow_decoder_never_blocks_render_side() {
        let mut p = VideoPlayer::spawn(
            Counter::new(50, Duration::from_millis(30)),
            RationalTime::ZERO,
            4,
        );
        let start = std::time::Instant::now();
        // Ask for a far frame repeatedly: each call must return immediately.
        for _ in 0..20 {
            let _ = p.frame_at(RationalTime::from_seconds(1));
        }
        assert!(
            start.elapsed() < Duration::from_millis(30),
            "frame_at blocked"
        );
        assert_eq!(
            red(p.frame_at_blocking(RationalTime::from_seconds(1), Duration::from_secs(5))),
            Some(25)
        );
        assert!(p.stats().misses > 0);
    }

    #[test]
    fn dropping_player_stops_thread() {
        let p = VideoPlayer::spawn(
            Counter::new(1_000_000, Duration::from_millis(1)),
            RationalTime::ZERO,
            8,
        );
        drop(p); // must join promptly, not hang
    }
}

#[cfg(test)]
mod end_of_stream {
    use super::*;
    #[test]
    fn exact_end_time_holds_last_frame() {
        use crate::ImageSequence;
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5u8 {
            image::save_buffer(
                dir.path().join(format!("img_{i:03}.png")),
                &[i * 10, 0, 0, 255],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        }
        let seq = ImageSequence::open(dir.path(), om_time::Rate::FPS_25).unwrap();
        let mut p = VideoPlayer::spawn(seq, RationalTime::ZERO, 6);
        let t3 = RationalTime::from_frame(3, om_time::Rate::FPS_25).unwrap();
        let f = p
            .frame_at_blocking(t3, std::time::Duration::from_secs(2))
            .map(|f| f.pts);
        assert_eq!(f, Some(t3));
        // Exactly the end of the last frame (its duration is known).
        let end = RationalTime::new(1, 5).unwrap();
        let f = p
            .frame_at_blocking(end, std::time::Duration::from_secs(2))
            .map(|f| f.pts);
        assert_eq!(f, Some(RationalTime::new(4, 25).unwrap()));
    }
}
