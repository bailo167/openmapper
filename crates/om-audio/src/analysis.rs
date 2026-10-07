// SPDX-License-Identifier: Apache-2.0
//! Audio analysis for audio-reactive modulation: overall level and three
//! bands (low < 250 Hz, mid 250 Hz–4 kHz, high > 4 kHz), as smoothed RMS
//! in 0..=1.

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

/// Streaming analyzer. Feed it interleaved stereo blocks.
#[derive(Debug, Clone)]
pub struct Analyzer {
    rate: f32,
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
