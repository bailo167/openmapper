// SPDX-License-Identifier: Apache-2.0
//! Audio output.
//!
//! The **show clock is master**. Each device callback asks the [`Mixer`]
//! for the block that starts at the current show position (in output sample
//! frames). The mixer maps that position into every voice's own timeline
//! (restart offset) and mixes the decoded samples. Looping is handled by the
//! voice's [`AudioPlayer`] on an unwrapped timeline, so loop points are
//! seamless. Playback stays
//! sample-contiguous between callbacks; if the device clock drifts more than
//! [`RESYNC_THRESHOLD_MS`] from the show clock, the next block jumps back
//! into sync (DECISIONS.md D-015).
//!
//! Voices play only at normal speed; other speeds are muted until a
//! time-stretching resampler exists.

pub mod analysis;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use om_media_core::AudioPlayer;

/// Drift beyond this re-syncs output to the show clock.
pub const RESYNC_THRESHOLD_MS: i64 = 20;

/// Show clock snapshot, in output frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    Stopped,
    /// Playing: show frame `base` at `since`.
    Playing {
        since: Instant,
        base: i64,
    },
}

impl Clock {
    /// Show position in frames at `now` for a device running at `rate`.
    #[must_use]
    pub fn frame_at(&self, now: Instant, rate: u32) -> Option<i64> {
        match *self {
            Self::Stopped => None,
            Self::Playing { since, base } => {
                let ns = now.saturating_duration_since(since).as_nanos();
                let frames = ns * u128::from(rate) / 1_000_000_000;
                Some(base + i64::try_from(frames).unwrap_or(i64::MAX / 2))
            }
        }
    }
}

/// One sound being played.
#[derive(Clone)]
pub struct Voice {
    pub player: Arc<AudioPlayer>,
    /// Show frame at which the voice's media frame 0 plays.
    pub origin: i64,
    pub gain: f32,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("origin", &self.origin)
            .field("gain", &self.gain)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct State {
    clock: Clock,
    voices: Vec<Voice>,
    /// Next show frame to render if playback is contiguous.
    next: Option<i64>,
    resyncs: u64,
    master_gain: f32,
}

/// Mixes voices against the show clock. Shared between the UI thread
/// (which updates clock and voices) and the audio callback.
#[derive(Debug, Clone)]
pub struct Mixer {
    state: Arc<Mutex<State>>,
    rate: u32,
    /// Analysis of what is actually played (for audio-reactive control).
    analyzer: analysis::SharedAnalyzer,
}

impl Mixer {
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                clock: Clock::Stopped,
                voices: Vec::new(),
                next: None,
                resyncs: 0,
                master_gain: 1.0,
            })),
            rate,
            analyzer: analysis::SharedAnalyzer::new(rate),
        }
    }

    /// Levels of the audio being played.
    #[must_use]
    pub fn levels(&self) -> analysis::Levels {
        self.analyzer.levels()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn set_clock(&self, clock: Clock) {
        let mut st = self.lock();
        if st.clock != clock {
            st.clock = clock;
            st.next = None;
        }
    }

    pub fn set_voices(&self, voices: Vec<Voice>) {
        self.lock().voices = voices;
    }

    pub fn set_master_gain(&self, gain: f32) {
        self.lock().master_gain = gain.clamp(0.0, 1.0);
    }

    /// Times output re-synced to the show clock (diagnostics).
    #[must_use]
    pub fn resyncs(&self) -> u64 {
        self.lock().resyncs
    }

    /// Fills `out` (interleaved stereo) for a callback whose first sample
    /// plays at `plays_at`.
    pub fn fill(&self, out: &mut [f32], plays_at: Instant) {
        out.fill(0.0);
        let frames = i64::try_from(out.len() / 2).unwrap_or(0);
        let mut st = self.lock();
        let Some(predicted) = st.clock.frame_at(plays_at, self.rate) else {
            st.next = None;
            return;
        };
        let start = match st.next {
            Some(n)
                if (n - predicted).abs() <= RESYNC_THRESHOLD_MS * i64::from(self.rate) / 1000 =>
            {
                n
            }
            Some(_) => {
                st.resyncs += 1;
                predicted
            }
            None => predicted,
        };
        st.next = Some(start + frames);
        let gain = st.master_gain;
        let voices = st.voices.clone();
        drop(st);
        for v in &voices {
            render_voice(v, start, out, gain);
        }
        self.analyzer.process(out);
    }

    /// Renders the block starting at show frame `start` (no clock); for
    /// tests and offline rendering.
    pub fn render_at(&self, start: i64, out: &mut [f32]) {
        out.fill(0.0);
        let st = self.lock();
        let gain = st.master_gain;
        let voices = st.voices.clone();
        drop(st);
        for v in &voices {
            render_voice(v, start, out, gain);
        }
    }
}

/// Mixes one voice into `out`.
fn render_voice(v: &Voice, show_start: i64, out: &mut [f32], master: f32) {
    let gain = v.gain * master;
    if gain <= 0.0 {
        return;
    }
    let total = i64::try_from(out.len() / 2).unwrap_or(0);
    let mut media = show_start - v.origin;
    let mut skip = 0i64;
    if media < 0 {
        // Not started yet: begin at the origin.
        skip = (-media).min(total);
        media = 0;
    }
    let mut run = total - skip;
    if v.player.loop_length().is_none()
        && let Some(len) = v.player.length()
    {
        run = run.min((len - media).max(0));
    }
    if run <= 0 {
        return;
    }
    let (a, b) = (
        usize::try_from(skip).unwrap_or(0) * 2,
        usize::try_from(skip + run).unwrap_or(0) * 2,
    );
    v.player.mix_into(media, &mut out[a..b], gain);
}

/// Device output failure.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no audio output device")]
    NoDevice,
    #[error("audio device error: {0}")]
    Device(String),
}

/// A running device stream feeding from a [`Mixer`].
pub struct Output {
    _stream: cpal::Stream,
    pub device_name: String,
    pub rate: u32,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Output({} @ {} Hz)", self.device_name, self.rate)
    }
}

/// Default output device's sample rate (to size the mixer before opening).
pub fn default_rate() -> Result<u32, OutputError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(OutputError::NoDevice)?;
    let cfg = device
        .default_output_config()
        .map_err(|e| OutputError::Device(e.to_string()))?;
    Ok(cfg.sample_rate())
}

impl Output {
    /// Opens the default output device at the mixer's rate, stereo f32.
    pub fn open(mixer: Mixer) -> Result<Self, OutputError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(OutputError::NoDevice)?;
        let device_name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "default".into());
        let config = cpal::StreamConfig {
            channels: 2,
            sample_rate: mixer.rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        let rate = mixer.rate();
        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let ts = info.timestamp();
                    let latency = ts.playback.duration_since(ts.callback);
                    mixer.fill(data, Instant::now() + latency);
                },
                |e| eprintln!("openmapper: audio stream error: {e}"),
                None,
            )
            .map_err(|e| OutputError::Device(e.to_string()))?;
        stream
            .play()
            .map_err(|e| OutputError::Device(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            device_name,
            rate,
        })
    }
}

#[cfg(test)]
mod tests;
