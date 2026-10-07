// SPDX-License-Identifier: Apache-2.0
//! Sample-exact audio sources and a threaded read-ahead buffer.
//!
//! Positions are integer sample (frame) indices at the source's output rate,
//! relative to the start of the media: sample `n` plays at `n / rate`
//! seconds exactly.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::MediaError;
use crate::source::{AudioBlock, AudioFormat};

/// A decodable audio stream, already converted to interleaved `f32` at
/// [`AudioSource::format`].
pub trait AudioSource: Send {
    fn format(&self) -> AudioFormat;

    /// Total length in sample frames, if known.
    fn length(&self) -> Option<i64>;

    /// Positions so the next block starts exactly at `frame`.
    fn seek(&mut self, frame: i64) -> Result<(), MediaError>;

    /// Next block in order, or `None` at end of stream.
    fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError>;
}

impl AudioSource for Box<dyn AudioSource> {
    fn format(&self) -> AudioFormat {
        (**self).format()
    }
    fn length(&self) -> Option<i64> {
        (**self).length()
    }
    fn seek(&mut self, frame: i64) -> Result<(), MediaError> {
        (**self).seek(frame)
    }
    fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
        (**self).next_block()
    }
}

/// Opens audio streams; implemented by media adapters.
pub trait AudioOpener: Send + Sync {
    /// Opens the audio track of `path`, converted to `rate` Hz stereo.
    /// `Ok(None)` if the file has no audio.
    fn open_audio(
        &self,
        path: &std::path::Path,
        rate: u32,
    ) -> Result<Option<Box<dyn AudioSource>>, MediaError>;
}

struct State {
    /// Sample frame index of `samples[0]`.
    start: i64,
    samples: VecDeque<f32>,
    /// Where the decoder should be producing from (bumped on jumps).
    want: i64,
    generation: u64,
    capacity_frames: usize,
    eos: bool,
    error: Option<String>,
    shutdown: bool,
    underruns: u64,
    loop_len: Option<i64>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Reads ahead of the playhead on a thread so the audio callback never
/// waits for decoding. Missing samples are rendered as silence (counted as
/// underruns), never blocked on.
///
/// With a loop length, positions are on an *unwrapped* timeline: frame
/// `k·len + n` is source frame `n` of pass `k`, and the decoder runs on into
/// the next pass before the current one ends, so loop points are seamless.
pub struct AudioPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    format: AudioFormat,
    length: Option<i64>,
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer")
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl AudioPlayer {
    /// `ahead_frames`: how many sample frames to keep decoded ahead.
    pub fn spawn<S: AudioSource + 'static>(source: S, ahead_frames: usize) -> Self {
        Self::spawn_looping(source, ahead_frames, None)
    }

    /// Like [`AudioPlayer::spawn`], looping every `loop_len` frames.
    pub fn spawn_looping<S: AudioSource + 'static>(
        source: S,
        ahead_frames: usize,
        loop_len: Option<i64>,
    ) -> Self {
        let loop_len = loop_len.filter(|l| *l > 0);
        let format = source.format();
        let length = source.length();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                start: 0,
                samples: VecDeque::new(),
                want: 0,
                generation: 1,
                capacity_frames: ahead_frames.max(1024),
                eos: false,
                error: None,
                shutdown: false,
                underruns: 0,
                loop_len,
            }),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("om-audio-decode".into())
            .spawn(move || audio_loop(source, &worker))
            .ok();
        Self {
            shared,
            thread,
            format,
            length,
        }
    }

    #[must_use]
    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// Source length in frames; `None` if unknown. Looping players never end.
    #[must_use]
    pub fn length(&self) -> Option<i64> {
        self.length
    }

    #[must_use]
    pub fn loop_length(&self) -> Option<i64> {
        self.shared.lock().loop_len
    }

    /// Fills `out` (interleaved) with the samples starting at frame `at`,
    /// adding them scaled by `gain`. Never blocks on decoding.
    pub fn mix_into(&self, at: i64, out: &mut [f32], gain: f32) {
        let ch = usize::from(self.format.channels.max(1));
        let frames = out.len() / ch;
        let mut st = self.shared.lock();
        // Discard samples before `at`.
        let behind = at - st.start;
        if behind > 0 {
            let drop = usize::try_from(behind)
                .unwrap_or(usize::MAX)
                .saturating_mul(ch);
            if drop <= st.samples.len() {
                st.samples.drain(..drop);
                st.start = at;
            }
        }
        let available_from = st.start;
        let have = st.samples.len() / ch;
        let in_buffer =
            at >= available_from && at - available_from <= i64::try_from(have).unwrap_or(i64::MAX);
        if !in_buffer {
            // Jump: restart decoding at `at`.
            if st.want != at || st.samples.is_empty() {
                st.want = at;
                st.start = at;
                st.samples.clear();
                st.eos = false;
                st.generation += 1;
            }
            st.underruns += 1;
            drop(st);
            self.shared.wake.notify_all();
            return;
        }
        let offset = usize::try_from(at - available_from).unwrap_or(0) * ch;
        let n = frames.min(have.saturating_sub(offset / ch)) * ch;
        for (o, s) in out[..n]
            .iter_mut()
            .zip(st.samples.range(offset..offset + n))
        {
            *o += *s * gain;
        }
        if n < frames * ch && !st.eos {
            st.underruns += 1;
        }
        drop(st);
        self.shared.wake.notify_all();
    }

    /// Count of reads that had to output silence.
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.shared.lock().underruns
    }

    #[must_use]
    pub fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }

    /// Frames buffered from `at` onwards (for tests/diagnostics).
    #[must_use]
    pub fn buffered_from(&self, at: i64) -> usize {
        let st = self.shared.lock();
        let ch = usize::from(self.format.channels.max(1));
        let have = i64::try_from(st.samples.len() / ch).unwrap_or(0);
        usize::try_from((st.start + have - at).max(0)).unwrap_or(0)
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn audio_loop<S: AudioSource>(mut source: S, shared: &Shared) {
    let ch = usize::from(source.format().channels.max(1));
    let loop_len = shared.lock().loop_len;
    let mut generation = 0;
    // Unwrapped frame index of source frame 0 in the current pass.
    let mut pass_offset = 0i64;
    loop {
        let (want, gen_now, end) = {
            let mut st = shared.lock();
            loop {
                if st.shutdown {
                    return;
                }
                let room =
                    st.samples.len() / ch < st.capacity_frames && !st.eos && st.error.is_none();
                if st.generation != generation || room {
                    break;
                }
                st = shared
                    .wake
                    .wait(st)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            let end = st.start + i64::try_from(st.samples.len() / ch).unwrap_or(0);
            (st.want, st.generation, end)
        };
        let jumped = gen_now != generation;
        if jumped {
            generation = gen_now;
            pass_offset = loop_len.map_or(0, |len| want.div_euclid(len) * len);
            if let Err(e) = source.seek(want - pass_offset) {
                shared.lock().error = Some(e.to_string());
                continue;
            }
        }
        let block = source.next_block();
        let mut st = shared.lock();
        if st.generation != generation {
            continue;
        }
        let expected = if jumped { want } else { end };
        match block {
            Ok(Some(mut b)) => {
                // Trim anything past the loop end; the next pass follows.
                let mut wrap = false;
                if let Some(len) = loop_len {
                    let keep = (len - b.start).max(0);
                    if b.frames() as i64 >= keep {
                        b.samples.truncate(usize::try_from(keep).unwrap_or(0) * ch);
                        wrap = true;
                    }
                }
                let start = b.start + pass_offset;
                let skip = usize::try_from((expected - start).max(0)).unwrap_or(0) * ch;
                if start > expected {
                    // Gap in the stream: pad with silence to stay sample-exact.
                    let gap = usize::try_from(start - expected).unwrap_or(0) * ch;
                    st.samples.extend(std::iter::repeat_n(0.0, gap));
                }
                if skip < b.samples.len() {
                    st.samples.extend(&b.samples[skip..]);
                }
                if wrap {
                    drop(st);
                    next_pass(&mut source, &mut pass_offset, loop_len, shared);
                }
            }
            Ok(None) => match loop_len {
                Some(len) => {
                    // Source shorter than the loop: pad to the loop end.
                    let pass_end = pass_offset + len;
                    if pass_end > expected {
                        let gap = usize::try_from(pass_end - expected).unwrap_or(0) * ch;
                        st.samples.extend(std::iter::repeat_n(0.0, gap));
                    }
                    drop(st);
                    next_pass(&mut source, &mut pass_offset, loop_len, shared);
                }
                None => st.eos = true,
            },
            Err(e) => st.error = Some(e.to_string()),
        }
    }
}

fn next_pass<S: AudioSource>(
    source: &mut S,
    pass_offset: &mut i64,
    loop_len: Option<i64>,
    shared: &Shared,
) {
    if let Some(len) = loop_len {
        *pass_offset += len;
        if let Err(e) = source.seek(0) {
            shared.lock().error = Some(e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source whose sample at frame n (both channels) is n as f32 / 1e6.
    struct Ramp {
        pos: i64,
        len: i64,
    }

    impl AudioSource for Ramp {
        fn format(&self) -> AudioFormat {
            AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            }
        }
        fn length(&self) -> Option<i64> {
            Some(self.len)
        }
        fn seek(&mut self, frame: i64) -> Result<(), MediaError> {
            // Seek lands a little early, like real decoders.
            self.pos = (frame - 100).max(0);
            Ok(())
        }
        fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
            if self.pos >= self.len {
                return Ok(None);
            }
            let n = 1024.min(self.len - self.pos);
            let samples = (self.pos..self.pos + n)
                .flat_map(|i| [i as f32 / 1e6; 2])
                .collect();
            let b = AudioBlock {
                start: self.pos,
                format: self.format(),
                samples,
            };
            self.pos += n;
            Ok(Some(b))
        }
    }

    fn read(p: &AudioPlayer, at: i64, frames: usize) -> Vec<f32> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let mut out = vec![0.0; frames * 2];
            p.mix_into(at, &mut out, 1.0);
            if p.buffered_from(at) >= frames || std::time::Instant::now() > deadline {
                let mut out = vec![0.0; frames * 2];
                p.mix_into(at, &mut out, 1.0);
                return out;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn reads_are_sample_exact_across_jumps() {
        let p = AudioPlayer::spawn(
            Ramp {
                pos: 0,
                len: 480_000,
            },
            8192,
        );
        for at in [0i64, 512, 1500, 100_000, 99_000, 479_000] {
            let out = read(&p, at, 256);
            for (k, frame) in out.chunks(2).enumerate() {
                let expected = (at + k as i64) as f32 / 1e6;
                assert_eq!(frame, [expected, expected], "at {at} + {k}");
            }
        }
    }

    #[test]
    fn loops_continue_seamlessly_into_the_next_pass() {
        let p = AudioPlayer::spawn_looping(Ramp { pos: 0, len: 5_000 }, 8192, Some(5_000));
        // Read across the 2nd loop boundary in one contiguous read.
        let out = read(&p, 2 * 5_000 - 3, 6);
        let got: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        let exp: Vec<f32> = [4_997i64, 4_998, 4_999, 0, 1, 2]
            .iter()
            .map(|n| *n as f32 / 1e6)
            .collect();
        assert_eq!(got, exp);
        // Contiguous playback through a wrap must not underrun.
        let before = p.underruns();
        let mut out = vec![0.0; 512];
        for k in 0..40 {
            p.mix_into(10_000 + k * 256, &mut out, 1.0);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            p.underruns() - before <= 1,
            "underruns across wrap: {}",
            p.underruns() - before
        );
    }

    #[test]
    fn missing_samples_are_silence_not_blocking() {
        let p = AudioPlayer::spawn(
            Ramp {
                pos: 0,
                len: 480_000,
            },
            4096,
        );
        let start = std::time::Instant::now();
        let mut out = vec![0.5; 512];
        p.mix_into(300_000, &mut out, 1.0); // not buffered yet
        assert!(start.elapsed() < std::time::Duration::from_millis(5));
        assert!(out.iter().all(|s| *s == 0.5), "nothing mixed in");
        assert!(p.underruns() >= 1);
    }
}
