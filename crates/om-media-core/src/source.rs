// SPDX-License-Identifier: Apache-2.0
//! Time-based media sources.
//!
//! A [`MediaSource`] decodes in presentation order after a seek. Timestamps
//! are exact ([`RationalTime`]) and come from the container, never from a
//! wall clock or an accumulated `frame += 1/fps`. [`FrameCursor`] answers
//! "which frame is visible at time t" efficiently for both playback (mostly
//! sequential) and scrubbing (random access).

use std::sync::Arc;

use om_time::{Rate, RationalTime};

use crate::{MediaError, StillImage};

/// Audio stream format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Static facts about a source.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaDescriptor {
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate, if the stream declares one.
    pub frame_rate: Option<Rate>,
    /// Total duration, if known.
    pub duration: Option<RationalTime>,
    pub audio: Option<AudioFormat>,
    /// Human-readable codec/container summary for diagnostics.
    pub codec: String,
}

/// One decoded picture. `pts` is relative to the start of the media (the
/// first frame is at 0).
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub pts: RationalTime,
    /// Display duration (until the next frame), if known.
    pub duration: Option<RationalTime>,
    pub image: Arc<StillImage>,
}

impl VideoFrame {
    /// True if this frame is the one displayed at `t`.
    #[must_use]
    pub fn covers(&self, t: RationalTime) -> bool {
        let starts_before = self
            .pts
            .checked_cmp(t)
            .is_ok_and(|o| o != std::cmp::Ordering::Greater);
        let ends_after = self
            .duration
            .and_then(|d| self.pts.checked_add(d).ok())
            .is_none_or(|end| {
                end.checked_cmp(t)
                    .is_ok_and(|o| o == std::cmp::Ordering::Greater)
            });
        starts_before && ends_after
    }
}

/// Interleaved `f32` samples starting at sample index `start` (relative to
/// the start of the media).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBlock {
    pub start: i64,
    pub format: AudioFormat,
    pub samples: Vec<f32>,
}

impl AudioBlock {
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels.max(1))
    }
}

/// A decodable media stream.
pub trait MediaSource: Send {
    fn descriptor(&self) -> &MediaDescriptor;

    /// Positions the decoder at or before `t` (typically the preceding
    /// keyframe). Frames then come back in presentation order; the caller
    /// ([`FrameCursor`]) decodes forward to the exact frame.
    fn seek(&mut self, t: RationalTime) -> Result<(), MediaError>;

    /// Next video frame in presentation order, or `None` at end of stream.
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError>;
}

impl MediaSource for Box<dyn MediaSource> {
    fn descriptor(&self) -> &MediaDescriptor {
        (**self).descriptor()
    }
    fn seek(&mut self, t: RationalTime) -> Result<(), MediaError> {
        (**self).seek(t)
    }
    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        (**self).next_frame()
    }
}

/// Opens video files. Implemented by media adapters (e.g. FFmpeg) and
/// injected by applications, so the engine does not depend on any adapter.
pub trait VideoOpener: Send + Sync {
    fn open_video(&self, path: &std::path::Path) -> Result<Box<dyn MediaSource>, MediaError>;
}

/// How far ahead (seconds) sequential decoding is preferred over seeking.
const MAX_DECODE_AHEAD_SECONDS: i64 = 2;

/// Random-access frame lookup over a sequential [`MediaSource`].
pub struct FrameCursor<S> {
    source: S,
    current: Option<VideoFrame>,
    lookahead: Option<VideoFrame>,
    at_end: bool,
    pub stats: CursorStats,
}

/// Counters for diagnostics and tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CursorStats {
    pub seeks: u64,
    pub decoded: u64,
}

impl<S: std::fmt::Debug> std::fmt::Debug for FrameCursor<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCursor")
            .field("source", &self.source)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl<S: MediaSource> FrameCursor<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            current: None,
            lookahead: None,
            at_end: false,
            stats: CursorStats::default(),
        }
    }

    #[must_use]
    pub fn source(&self) -> &S {
        &self.source
    }

    /// The frame displayed at `t`: the last frame whose `pts <= t`. Times
    /// before the first frame give the first frame; times after the last
    /// give the last.
    pub fn frame_at(&mut self, t: RationalTime) -> Result<Option<&VideoFrame>, MediaError> {
        use std::cmp::Ordering::Greater;
        let t = if t.ticks() < 0 { RationalTime::ZERO } else { t };
        let after = |a: RationalTime, b: RationalTime| a.checked_cmp(b).is_ok_and(|o| o == Greater);

        if let Some(cur) = &self.current {
            if cur.duration.is_some() && cur.covers(t) {
                return Ok(self.current.as_ref());
            }
            if self.at_end && !after(cur.pts, t) {
                return Ok(self.current.as_ref()); // past the end: hold last frame
            }
        }
        let need_seek = match &self.current {
            None => true,
            Some(cur) => {
                after(cur.pts, t)
                    || t.checked_sub(cur.pts).is_ok_and(|d| {
                        after(d, RationalTime::from_seconds(MAX_DECODE_AHEAD_SECONDS))
                    })
            }
        };
        if need_seek {
            self.current = None;
            self.at_end = false;
            self.seek_at_or_before(t)?;
        }
        loop {
            let next = match self.lookahead.take() {
                Some(f) => Some(f),
                None if self.at_end => None,
                None => {
                    let f = self.source.next_frame()?;
                    self.stats.decoded += u64::from(f.is_some());
                    f
                }
            };
            let Some(frame) = next else {
                self.at_end = true;
                if self.current.is_none() && need_seek {
                    // Sought past the end: back off and decode to the last frame.
                    return self.last_frame();
                }
                return Ok(self.current.as_ref());
            };
            if let Some(cur) = &mut self.current {
                // Containers do not always carry durations; infer from the gap.
                if cur.duration.is_none() {
                    cur.duration = frame.pts.checked_sub(cur.pts).ok();
                }
                if after(frame.pts, t) {
                    self.lookahead = Some(frame);
                    return Ok(self.current.as_ref());
                }
            } else if after(frame.pts, t) {
                // t is before the first available frame.
                self.current = Some(frame);
                return Ok(self.current.as_ref());
            }
            self.current = Some(frame);
        }
    }

    /// The next frame after the current one, in presentation order (for
    /// decode-ahead). `None` at end of stream.
    pub fn advance(&mut self) -> Result<Option<&VideoFrame>, MediaError> {
        let next = match self.lookahead.take() {
            Some(f) => Some(f),
            None if self.at_end => None,
            None => {
                let f = self.source.next_frame()?;
                self.stats.decoded += u64::from(f.is_some());
                f
            }
        };
        match next {
            Some(frame) => {
                if let Some(cur) = &mut self.current
                    && cur.duration.is_none()
                {
                    cur.duration = frame.pts.checked_sub(cur.pts).ok();
                }
                self.current = Some(frame);
                Ok(self.current.as_ref())
            }
            None => {
                self.at_end = true;
                Ok(None)
            }
        }
    }

    /// Seeks so the first decoded frame starts at or before `t`. Demuxers
    /// index by decode time, which with B-frames can land on a keyframe that
    /// *presents* after `t`; in that case back off (1 s, 2 s, 4 s, …) and
    /// retry. The first frame is kept in `lookahead`.
    fn seek_at_or_before(&mut self, t: RationalTime) -> Result<(), MediaError> {
        use std::cmp::Ordering::Greater;
        let mut back = RationalTime::ZERO;
        loop {
            let target = t
                .checked_sub(back)
                .ok()
                .filter(|x| x.ticks() > 0)
                .unwrap_or(RationalTime::ZERO);
            self.source.seek(target)?;
            self.stats.seeks += 1;
            let first = self.source.next_frame()?;
            self.stats.decoded += u64::from(first.is_some());
            let late = first
                .as_ref()
                .is_some_and(|f| f.pts.checked_cmp(t).is_ok_and(|o| o == Greater));
            if !late || target.ticks() == 0 {
                self.lookahead = first;
                return Ok(());
            }
            back = if back.ticks() == 0 {
                RationalTime::from_seconds(1)
            } else {
                back.checked_mul_int(2)
                    .map_err(|e| MediaError::Seek(t.to_string(), e.to_string()))?
            };
        }
    }

    /// Seeks near the end and decodes forward, keeping the final frame.
    fn last_frame(&mut self) -> Result<Option<&VideoFrame>, MediaError> {
        let back_off = RationalTime::from_seconds(1);
        let target = self
            .source
            .descriptor()
            .duration
            .and_then(|d| d.checked_sub(back_off).ok())
            .filter(|t| t.ticks() > 0)
            .unwrap_or(RationalTime::ZERO);
        self.source.seek(target)?;
        self.stats.seeks += 1;
        self.lookahead = None;
        while let Some(frame) = self.source.next_frame()? {
            self.stats.decoded += 1;
            if let Some(cur) = &mut self.current
                && cur.duration.is_none()
            {
                cur.duration = frame.pts.checked_sub(cur.pts).ok();
            }
            self.current = Some(frame);
        }
        self.at_end = true;
        Ok(self.current.as_ref())
    }
}
