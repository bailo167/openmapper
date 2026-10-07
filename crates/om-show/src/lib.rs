// SPDX-License-Identifier: Apache-2.0
//! Show runtime.
//!
//! Each frame, [`ShowState::evaluate`] turns the project's cues, timelines
//! and modulators into parameter *overrides* for the current show time.
//! Overrides are applied to a derived copy of the project for rendering
//! (`om_command::params::apply_overrides`); the document is never touched.
//!
//! Precedence (later wins): timelines → cues → modulators.
//!
//! Cues are *tracking*: a cue's values hold until another cue changes them
//! or the show is released. Running a cue fades every one of its parameters
//! from its current effective value to the cue's value over the cue's fade
//! time; bools switch at the start of the fade. Evaluation is deterministic:
//! the same calls at the same times give the same overrides.

pub mod control;

use std::collections::{BTreeMap, HashMap};

use om_command::params;
use om_project::{
    AudioBand, Ease, LfoShape, ModSource, ParamId, ParamKind, ParamValue, Project, Timeline,
};
use om_time::RationalTime;
use om_types::{CueId, TimelineId};

/// Overrides for one frame.
pub type Overrides = BTreeMap<ParamId, ParamValue>;

/// Audio analysis levels (0..=1) feeding audio modulators.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioLevels {
    pub level: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

impl AudioLevels {
    #[must_use]
    pub fn band(&self, band: AudioBand) -> f32 {
        match band {
            AudioBand::Level => self.level,
            AudioBand::Low => self.low,
            AudioBand::Mid => self.mid,
            AudioBand::High => self.high,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Fade {
    start: RationalTime,
    /// Seconds; 0 = snap.
    seconds: f64,
    from: Overrides,
    /// Target values; `None` means "back to the document value" (release).
    to: BTreeMap<ParamId, Option<ParamValue>>,
}

/// Playback state of one timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelinePlay {
    /// Show time when `position` was set.
    anchor_show: RationalTime,
    /// Timeline position at `anchor_show`.
    anchor_position: RationalTime,
    playing: bool,
}

/// Runtime show state (not saved).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShowState {
    held: Overrides,
    fade: Option<Fade>,
    last_cue: Option<CueId>,
    timelines: HashMap<TimelineId, TimelinePlay>,
    /// Overrides produced by the last `evaluate` (fade starting points).
    last: Overrides,
}

fn lerp(a: ParamValue, b: ParamValue, t: f64) -> ParamValue {
    match (a, b) {
        (ParamValue::Float(x), ParamValue::Float(y)) => ParamValue::Float(x + (y - x) * t),
        // Bools switch as soon as the fade starts.
        (_, b) => b,
    }
}

fn seconds(t: RationalTime) -> f64 {
    t.as_seconds_f64()
}

impl ShowState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a cue fade or release is in progress.
    #[must_use]
    pub fn fading(&self) -> bool {
        self.fade.is_some()
    }

    /// The cue most recently run.
    #[must_use]
    pub fn current_cue(&self) -> Option<CueId> {
        self.last_cue
    }

    /// Value a parameter has right now (last overrides, else document).
    fn current(&self, project: &Project, id: &ParamId) -> Option<ParamValue> {
        self.last
            .get(id)
            .copied()
            .or_else(|| params::get(project, id))
    }

    /// Runs `cue` at show time `now`.
    pub fn go(&mut self, project: &Project, cue: CueId, now: RationalTime) {
        let Some(c) = project.show.cues.iter().find(|c| c.id == cue) else {
            return;
        };
        self.settle(project, now);
        let mut from = Overrides::new();
        let mut to = BTreeMap::new();
        for v in &c.values {
            if let Some(cur) = self.current(project, &v.param) {
                from.insert(v.param.clone(), cur);
                to.insert(v.param.clone(), Some(v.value));
            }
        }
        self.fade = Some(Fade {
            start: now,
            seconds: c.fade.get().max(0.0),
            from,
            to,
        });
        self.last_cue = Some(cue);
    }

    /// Runs the cue after the current one (the first if none ran yet).
    /// Returns the cue run, if any.
    pub fn go_next(&mut self, project: &Project, now: RationalTime) -> Option<CueId> {
        let cues = &project.show.cues;
        let next = match self
            .last_cue
            .and_then(|c| cues.iter().position(|x| x.id == c))
        {
            Some(i) => cues.get(i + 1),
            None => cues.first(),
        }?
        .id;
        self.go(project, next, now);
        Some(next)
    }

    /// Fades every cue-held value back to the document over `fade_seconds`.
    pub fn release(&mut self, project: &Project, now: RationalTime, fade_seconds: f64) {
        self.settle(project, now);
        let mut from = Overrides::new();
        let mut to = BTreeMap::new();
        for id in self.held.keys() {
            if let Some(cur) = self.current(project, id) {
                from.insert(id.clone(), cur);
                to.insert(id.clone(), None);
            }
        }
        self.fade = Some(Fade {
            start: now,
            seconds: fade_seconds.max(0.0),
            from,
            to,
        });
        self.last_cue = None;
    }

    /// Completes a fade that has finished (or is being interrupted), moving
    /// its effective values into `held`.
    fn settle(&mut self, project: &Project, now: RationalTime) {
        let Some(f) = self.fade.take() else { return };
        let done = seconds(now) - seconds(f.start) >= f.seconds;
        for (id, target) in &f.to {
            let value = if done {
                *target
            } else {
                // Interrupted: freeze where it is now.
                self.fade_value(project, &f, id, now)
            };
            match value {
                Some(v) => {
                    self.held.insert(id.clone(), v);
                }
                None => {
                    self.held.remove(id);
                }
            }
        }
    }

    fn fade_value(
        &self,
        project: &Project,
        f: &Fade,
        id: &ParamId,
        now: RationalTime,
    ) -> Option<ParamValue> {
        let progress = if f.seconds <= 0.0 {
            1.0
        } else {
            ((seconds(now) - seconds(f.start)) / f.seconds).clamp(0.0, 1.0)
        };
        let target =
            f.to.get(id)
                .copied()
                .flatten()
                .or_else(|| params::get(project, id))?;
        let from = f.from.get(id).copied().unwrap_or(target);
        if progress >= 1.0 {
            return f.to.get(id).copied().flatten();
        }
        Some(lerp(from, target, progress))
    }

    /// Starts (or resumes) a timeline at show time `now`.
    pub fn play_timeline(&mut self, id: TimelineId, now: RationalTime) {
        let position = self
            .timeline_position_raw(id, now)
            .unwrap_or(RationalTime::ZERO);
        self.timelines.insert(
            id,
            TimelinePlay {
                anchor_show: now,
                anchor_position: position,
                playing: true,
            },
        );
    }

    pub fn pause_timeline(&mut self, id: TimelineId, now: RationalTime) {
        if let Some(position) = self.timeline_position_raw(id, now) {
            self.timelines.insert(
                id,
                TimelinePlay {
                    anchor_show: now,
                    anchor_position: position,
                    playing: false,
                },
            );
        }
    }

    /// Jumps a timeline to `position` (keeps play/pause).
    pub fn seek_timeline(&mut self, id: TimelineId, position: RationalTime, now: RationalTime) {
        let playing = self.timelines.get(&id).is_some_and(|t| t.playing);
        self.timelines.insert(
            id,
            TimelinePlay {
                anchor_show: now,
                anchor_position: position,
                playing,
            },
        );
    }

    /// Stops a timeline; it no longer drives its parameters.
    pub fn stop_timeline(&mut self, id: TimelineId) {
        self.timelines.remove(&id);
    }

    #[must_use]
    pub fn timeline_playing(&self, id: TimelineId) -> bool {
        self.timelines.get(&id).is_some_and(|t| t.playing)
    }

    fn timeline_position_raw(&self, id: TimelineId, now: RationalTime) -> Option<RationalTime> {
        let t = self.timelines.get(&id)?;
        if !t.playing {
            return Some(t.anchor_position);
        }
        now.checked_sub(t.anchor_show)
            .and_then(|d| t.anchor_position.checked_add(d))
            .ok()
    }

    /// Current position of a timeline (wrapped or clamped), if active.
    #[must_use]
    pub fn timeline_position(
        &self,
        timeline: &Timeline,
        now: RationalTime,
    ) -> Option<RationalTime> {
        let raw = self.timeline_position_raw(timeline.id, now)?;
        Some(wrap_or_clamp(raw, timeline))
    }

    /// Computes this frame's overrides.
    pub fn evaluate(
        &mut self,
        project: &Project,
        now: RationalTime,
        audio: AudioLevels,
    ) -> Overrides {
        let mut out = Overrides::new();
        // Timelines.
        for tl in &project.show.timelines {
            let Some(pos) = self.timeline_position(tl, now) else {
                continue;
            };
            for track in &tl.tracks {
                if let Some(v) = track_value(track, pos) {
                    out.insert(track.param.clone(), v);
                }
            }
        }
        // Cues: finish a completed fade, then held values and any fade.
        if self
            .fade
            .as_ref()
            .is_some_and(|f| seconds(now) - seconds(f.start) >= f.seconds)
        {
            self.settle(project, now);
        }
        for (id, v) in &self.held {
            out.insert(id.clone(), *v);
        }
        if let Some(f) = &self.fade {
            for id in f.to.keys() {
                match self.fade_value(project, f, id, now) {
                    Some(v) => {
                        out.insert(id.clone(), v);
                    }
                    None => {
                        out.remove(id);
                    }
                }
            }
        }
        // Modulators.
        for m in project.show.modulators.iter().filter(|m| m.enabled) {
            let signal = match &m.source {
                ModSource::Lfo { shape, rate, phase } => {
                    lfo(*shape, seconds(now) * rate.get() + phase.get())
                }
                ModSource::Audio { band, gain } => {
                    (f64::from(audio.band(*band)) * gain.get()).clamp(0.0, 1.0)
                }
            };
            let value = m.offset.get() + m.depth.get() * signal;
            if let Some(kind) = params::kind(project, &m.param) {
                let v = match kind {
                    ParamKind::Bool => ParamValue::Bool(value >= 0.5),
                    ParamKind::Float { .. } => ParamValue::Float(value),
                };
                out.insert(m.param.clone(), v.coerce(kind));
            }
        }
        // Keep only parameters that still exist, coerced to their range.
        out.retain(|id, _| params::kind(project, id).is_some());
        for (id, v) in &mut out {
            if let Some(k) = params::kind(project, id) {
                *v = v.coerce(k);
            }
        }
        self.last = out.clone();
        out
    }
}

/// LFO signal in 0..=1 at `phase` cycles.
#[must_use]
pub fn lfo(shape: LfoShape, phase: f64) -> f64 {
    let f = phase.rem_euclid(1.0);
    match shape {
        LfoShape::Sine => 0.5 + 0.5 * (std::f64::consts::TAU * f).sin(),
        LfoShape::Triangle => 1.0 - (2.0 * f - 1.0).abs(),
        LfoShape::Square => {
            if f < 0.5 {
                1.0
            } else {
                0.0
            }
        }
        LfoShape::Saw => f,
    }
}

fn wrap_or_clamp(raw: RationalTime, tl: &Timeline) -> RationalTime {
    if tl.looping {
        raw.rem_euclid(tl.duration).unwrap_or(RationalTime::ZERO)
    } else if raw.ticks() < 0 {
        RationalTime::ZERO
    } else if raw
        .checked_cmp(tl.duration)
        .is_ok_and(|o| o == std::cmp::Ordering::Greater)
    {
        tl.duration
    } else {
        raw
    }
}

/// Value of a track at exact position `t`.
#[must_use]
pub fn track_value(track: &om_project::Track, t: RationalTime) -> Option<ParamValue> {
    use std::cmp::Ordering::{Greater, Less};
    let mut keys: Vec<&om_project::Keyframe> = track.keys.iter().collect();
    keys.sort_by(|a, b| a.at.checked_cmp(b.at).unwrap_or(Less));
    let first = keys.first()?;
    if t.checked_cmp(first.at).is_ok_and(|o| o != Greater) {
        return Some(first.value);
    }
    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t.checked_cmp(b.at).is_ok_and(|o| o == Less) {
            let span = b.at.checked_sub(a.at).ok()?;
            let into = t.checked_sub(a.at).ok()?;
            let alpha = if span.ticks() == 0 {
                1.0
            } else {
                // Exact ratio, then to f64.
                into.as_seconds_f64() / span.as_seconds_f64()
            };
            let eased = match a.ease {
                Ease::Linear => alpha,
                Ease::Step => 0.0,
                Ease::Smooth => alpha * alpha * (3.0 - 2.0 * alpha),
            };
            return Some(match (a.value, b.value) {
                (ParamValue::Float(x), ParamValue::Float(y)) => {
                    ParamValue::Float(x + (y - x) * eased)
                }
                // Bool tracks always step.
                (v, _) => v,
            });
        }
    }
    keys.last().map(|k| k.value)
}

#[cfg(test)]
mod tests;
