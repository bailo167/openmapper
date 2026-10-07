// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;
use std::time::{Duration, Instant};

use om_media_core::{AudioBlock, AudioFormat, AudioPlayer, AudioSource, MediaError};

use super::*;

/// Sample n (both channels) = (n + 1) as f32, length `len`.
struct Ramp {
    pos: i64,
    len: i64,
}

impl AudioSource for Ramp {
    fn format(&self) -> AudioFormat {
        AudioFormat {
            sample_rate: 1000,
            channels: 2,
        }
    }
    fn length(&self) -> Option<i64> {
        Some(self.len)
    }
    fn seek(&mut self, frame: i64) -> Result<(), MediaError> {
        self.pos = frame.clamp(0, self.len);
        Ok(())
    }
    fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
        if self.pos >= self.len {
            return Ok(None);
        }
        let n = 64.min(self.len - self.pos);
        let samples = (self.pos..self.pos + n)
            .flat_map(|i| [(i + 1) as f32; 2])
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

fn voice(len: i64, origin: i64, looping: bool, gain: f32) -> Voice {
    Voice {
        player: Arc::new(AudioPlayer::spawn_looping(
            Ramp { pos: 0, len },
            4096,
            looping.then_some(len),
        )),
        origin,
        gain,
    }
}

/// Renders, retrying until decode-ahead has caught up (no underrun).
fn render(m: &Mixer, start: i64, frames: usize) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut out = vec![0.0; frames * 2];
        m.render_at(start, &mut out);
        let mut again = vec![0.0; frames * 2];
        std::thread::sleep(Duration::from_millis(5));
        m.render_at(start, &mut again);
        if out == again || Instant::now() > deadline {
            return again;
        }
    }
}

fn left(out: &[f32]) -> Vec<f32> {
    out.chunks(2).map(|f| f[0]).collect()
}

#[test]
fn loops_split_exactly_at_the_boundary() {
    let m = Mixer::new(1000);
    m.set_voices(vec![voice(100, 0, true, 1.0)]);
    // Show frames 95..105 cross the loop point at 100.
    let out = left(&render(&m, 95, 10));
    assert_eq!(
        out,
        vec![96.0, 97.0, 98.0, 99.0, 100.0, 1.0, 2.0, 3.0, 4.0, 5.0]
    );
    // Many loops later, still exact.
    let out = left(&render(&m, 100 * 1_000 + 3, 2));
    assert_eq!(out, vec![4.0, 5.0]);
}

#[test]
fn one_shot_voices_start_at_origin_and_stop_at_end() {
    let m = Mixer::new(1000);
    m.set_voices(vec![voice(10, 5, false, 0.5)]);
    let out = left(&render(&m, 0, 20));
    let mut expected = vec![0.0; 5];
    expected.extend((1..=10).map(|i| i as f32 * 0.5));
    expected.extend(vec![0.0; 5]);
    assert_eq!(out, expected);
}

#[test]
fn voices_mix_additively_with_master_gain() {
    let m = Mixer::new(1000);
    m.set_voices(vec![voice(50, 0, true, 1.0), voice(50, 0, true, 1.0)]);
    m.set_master_gain(0.25);
    let out = left(&render(&m, 10, 1));
    assert_eq!(out, vec![2.0 * 11.0 * 0.25]);
}

#[test]
fn stopped_clock_outputs_silence_and_drift_resyncs() {
    let m = Mixer::new(1000);
    m.set_voices(vec![voice(1000, 0, true, 1.0)]);
    let mut out = vec![1.0; 64];
    m.fill(&mut out, Instant::now());
    assert!(out.iter().all(|s| *s == 0.0), "stopped clock is silent");

    let t0 = Instant::now();
    m.set_clock(Clock::Playing { since: t0, base: 0 });
    let mut out = vec![0.0; 64]; // 32 frames
    m.fill(&mut out, t0);
    // Next callback arrives exactly when expected: contiguous, no resync.
    m.fill(&mut out, t0 + Duration::from_millis(32));
    assert_eq!(m.resyncs(), 0);
    // Device clock running slow by 100 ms: beyond threshold, resyncs.
    m.fill(&mut out, t0 + Duration::from_millis(200));
    assert_eq!(m.resyncs(), 1);
}

#[test]
fn clock_frames_are_exact() {
    let t0 = Instant::now();
    let c = Clock::Playing {
        since: t0,
        base: 480,
    };
    assert_eq!(
        c.frame_at(t0 + Duration::from_secs(10), 48_000),
        Some(480 + 480_000)
    );
    assert_eq!(Clock::Stopped.frame_at(t0, 48_000), None);
}
