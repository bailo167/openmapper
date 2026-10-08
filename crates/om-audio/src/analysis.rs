// SPDX-License-Identifier: Apache-2.0
//! Audio analysis for audio-reactive modulation: overall level and three
//! bands (low < 250 Hz, mid 250 Hz–4 kHz, high > 4 kHz), as smoothed RMS
//! in 0..=1; plus the recent waveform and its spectrum per channel for ISF
//! `audio` / `audioFFT` inputs ([`Scope`]).

use std::sync::{Arc, Mutex};

/// Smoothed levels (0..=1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Levels {
    pub level: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

/// One-pole low-pass filter.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    a: f32,
    z: f32,
}

impl OnePole {
    fn new(cutoff: f32, rate: f32) -> Self {
        Self {
            a: 1.0 - (-std::f32::consts::TAU * cutoff / rate).exp(),
            z: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        self.z += self.a * (x - self.z);
        self.z
    }
}

/// Frames kept for the waveform and spectrum (one FFT window).
pub const FFT_SIZE: usize = 1024;
/// Waveform samples per channel in a [`Scope`].
pub const WAVE_SAMPLES: usize = 512;
/// Spectrum bins per channel in a [`Scope`] (0 Hz up to just below Nyquist).
pub const FFT_BINS: usize = FFT_SIZE / 2;

/// Recent audio for display and shaders, per channel (left, right).
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    /// The newest [`WAVE_SAMPLES`] samples, oldest first, in −1..=1.
    pub wave: [Vec<f32>; 2],
    /// Magnitude spectrum of the newest [`FFT_SIZE`] samples (Hann window),
    /// [`FFT_BINS`] bins; a full-scale sine on a bin centre reads 1.
    pub fft: [Vec<f32>; 2],
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            wave: [vec![0.0; WAVE_SAMPLES], vec![0.0; WAVE_SAMPLES]],
            fft: [vec![0.0; FFT_BINS], vec![0.0; FFT_BINS]],
        }
    }
}

/// In-place iterative radix-2 FFT; `re.len()` must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        #[allow(clippy::cast_precision_loss)]
        let ang = -std::f64::consts::TAU / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                #[allow(clippy::cast_precision_loss)]
                let (s, c) = (ang * k as f64).sin_cos();
                #[allow(clippy::cast_possible_truncation)]
                let (c, s) = (c as f32, s as f32);
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Hann-windowed magnitude spectrum of `samples` (power-of-two length),
/// scaled so a full-scale sine on a bin centre reads 1.
#[must_use]
pub fn spectrum(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    if n < 2 || !n.is_power_of_two() {
        return Vec::new();
    }
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f32;
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, x)| {
            #[allow(clippy::cast_precision_loss)]
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / nf).cos();
            x * w
        })
        .collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    // Hann coherent gain is 1/2; a real sine splits over ±f: 4/N.
    re.iter()
        .zip(&im)
        .take(n / 2)
        .map(|(r, i)| (r.hypot(*i) * 4.0 / nf).min(1.0))
        .collect()
}

/// Streaming analyzer. Feed it interleaved stereo blocks.
#[derive(Debug, Clone)]
pub struct Analyzer {
    rate: f32,
    /// Last [`FFT_SIZE`] stereo frames (ring, `pos` is the oldest).
    ring: Vec<[f32; 2]>,
    pos: usize,
    low_lp: OnePole,
    mid_lp: OnePole,
    levels: Levels,
    attack: f32,
    release: f32,
}

impl Analyzer {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate = sample_rate.max(1) as f32;
        Self {
            rate,
            ring: vec![[0.0; 2]; FFT_SIZE],
            pos: 0,
            low_lp: OnePole::new(250.0, rate),
            mid_lp: OnePole::new(4000.0, rate),
            levels: Levels::default(),
            // Per-block smoothing coefficients are derived from block length.
            attack: 0.010,
            release: 0.250,
        }
    }

    /// Processes one block; returns the updated levels.
    pub fn process(&mut self, interleaved_stereo: &[f32]) -> Levels {
        let frames = interleaved_stereo.len() / 2;
        if frames == 0 {
            return self.levels;
        }
        let (mut all, mut low, mut mid, mut high) = (0f32, 0f32, 0f32, 0f32);
        for f in interleaved_stereo.as_chunks::<2>().0 {
            self.ring[self.pos] = *f;
            self.pos = (self.pos + 1) % FFT_SIZE;
            let x = 0.5 * (f[0] + f[1]);
            let l = self.low_lp.process(x);
            let lm = self.mid_lp.process(x);
            let (m, h) = (lm - l, x - lm);
            all += x * x;
            low += l * l;
            mid += m * m;
            high += h * h;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = frames as f32;
        // √2 scales a full-scale sine's RMS (0.707) to 1.
        let rms = |e: f32| ((e / n).sqrt() * std::f32::consts::SQRT_2).clamp(0.0, 1.0);
        let dt = n / self.rate;
        let smooth = |old: f32, new: f32, attack: f32, release: f32| {
            let tau = if new > old { attack } else { release };
            old + (new - old) * (1.0 - (-dt / tau).exp())
        };
        let (a, r) = (self.attack, self.release);
        self.levels = Levels {
            level: smooth(self.levels.level, rms(all), a, r),
            low: smooth(self.levels.low, rms(low), a, r),
            mid: smooth(self.levels.mid, rms(mid), a, r),
            high: smooth(self.levels.high, rms(high), a, r),
        };
        self.levels
    }

    #[must_use]
    pub fn levels(&self) -> Levels {
        self.levels
    }

    /// The last [`FFT_SIZE`] frames per channel, oldest first.
    fn history(&self) -> [Vec<f32>; 2] {
        let ordered = self.ring[self.pos..].iter().chain(&self.ring[..self.pos]);
        let (l, r) = ordered.map(|f| (f[0], f[1])).unzip();
        [l, r]
    }

    /// Waveform and spectrum of the most recent audio.
    #[must_use]
    pub fn scope(&self) -> Scope {
        scope_of(self.history())
    }
}

fn scope_of(history: [Vec<f32>; 2]) -> Scope {
    let wave = |h: &[f32]| h[h.len() - WAVE_SAMPLES..].to_vec();
    Scope {
        wave: [wave(&history[0]), wave(&history[1])],
        fft: [spectrum(&history[0]), spectrum(&history[1])],
    }
}

/// Analyzer shared between an audio callback and the UI thread.
#[derive(Debug, Clone)]
pub struct SharedAnalyzer(Arc<Mutex<Analyzer>>);

impl SharedAnalyzer {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self(Arc::new(Mutex::new(Analyzer::new(sample_rate))))
    }

    pub fn process(&self, block: &[f32]) {
        if let Ok(mut a) = self.0.lock() {
            a.process(block);
        }
    }

    #[must_use]
    pub fn levels(&self) -> Levels {
        self.0.lock().map(|a| a.levels()).unwrap_or_default()
    }

    /// Waveform and spectrum; the FFT runs after the lock is released so
    /// the audio callback is never held up by it.
    #[must_use]
    pub fn scope(&self) -> Scope {
        self.0
            .lock()
            .map(|a| a.history())
            .map(scope_of)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, seconds: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amp * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin();
                [v, v]
            })
            .collect()
    }

    fn settle(freq: f32, amp: f32) -> Levels {
        let mut a = Analyzer::new(48_000);
        let signal = sine(freq, 48_000, 1.0, amp);
        for block in signal.chunks(1024) {
            a.process(block);
        }
        a.levels()
    }

    #[test]
    fn bands_follow_frequency() {
        let bass = settle(60.0, 1.0);
        assert!(bass.low > 0.8 && bass.high < 0.1, "{bass:?}");
        let mids = settle(1000.0, 1.0);
        assert!(mids.mid > mids.low && mids.mid > mids.high, "{mids:?}");
        let treble = settle(12_000.0, 1.0);
        assert!(treble.high > 0.7 && treble.low < 0.1, "{treble:?}");
        assert!(
            (bass.level - 1.0).abs() < 0.05,
            "full-scale sine is level 1: {bass:?}"
        );
    }

    #[test]
    fn scope_holds_recent_wave_and_spectrum() {
        let mut a = Analyzer::new(48_000);
        // Bin 64 of 1024 at 48 kHz: 3000 Hz, exactly on a bin centre.
        let bin = 64;
        let freq = 48_000.0 * bin as f32 / FFT_SIZE as f32;
        let mut signal = sine(freq, 48_000, 0.1, 0.5);
        // Silence the right channel.
        for f in signal.chunks_mut(2) {
            f[1] = 0.0;
        }
        for block in signal.chunks(300) {
            a.process(block);
        }
        let s = a.scope();
        assert_eq!(s.wave[0].len(), WAVE_SAMPLES);
        assert_eq!(s.fft[0].len(), FFT_BINS);
        let last = *s.wave[0].last().unwrap();
        assert!(
            (last - signal[signal.len() - 2]).abs() < 1e-6,
            "newest last"
        );
        assert!(s.wave[1].iter().all(|v| *v == 0.0));
        let peak = s.fft[0][bin];
        assert!((peak - 0.5).abs() < 0.01, "amplitude 0.5 reads 0.5: {peak}");
        let leak = s.fft[0]
            .iter()
            .enumerate()
            .filter(|(i, _)| i.abs_diff(bin) > 2)
            .map(|(_, v)| *v)
            .fold(0.0, f32::max);
        assert!(leak < 1e-3, "energy stays at its bin: {leak}");
        assert!(s.fft[1].iter().all(|v| *v == 0.0));
        let shared = SharedAnalyzer::new(48_000);
        shared.process(&signal);
        assert_eq!(shared.scope(), s);
    }

    #[test]
    fn level_tracks_amplitude_and_decays() {
        let half = settle(1000.0, 0.5);
        assert!((half.level - 0.5).abs() < 0.05, "{half:?}");
        let mut a = Analyzer::new(48_000);
        for block in sine(1000.0, 48_000, 0.5, 1.0).chunks(1024) {
            a.process(block);
        }
        let loud = a.levels().level;
        // One second of stereo silence (2 × 48 000 samples).
        for block in vec![0.0; 96_000].chunks(1024) {
            a.process(block);
        }
        assert!(
            a.levels().level < loud * 0.05,
            "250 ms release decays within 1 s"
        );
    }
}
