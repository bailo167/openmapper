// SPDX-License-Identifier: Apache-2.0
//! Video decoding.

use std::path::Path;
use std::sync::Arc;

use ff::util::error::EAGAIN;
use ffmpeg_next as ff;
use om_media_core::{MediaDescriptor, MediaError, MediaSource, StillImage, VideoFrame};
use om_time::{Rate, RationalTime};

/// A video file decoded with FFmpeg.
pub struct FfmpegVideo {
    path: String,
    input: ff::format::context::Input,
    decoder: ff::decoder::Video,
    stream_index: usize,
    time_base: ff::Rational,
    start_pts: i64,
    rate: Option<Rate>,
    descriptor: MediaDescriptor,
    scaler: Option<(ScalerKey, Scaler)>,
    decoded: ff::frame::Video,
    sent_eof: bool,
    last_pts: Option<RationalTime>,
    /// Corrupt packets skipped so far (diagnostics).
    pub skipped_packets: u64,
}

impl std::fmt::Debug for FfmpegVideo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfmpegVideo")
            .field("path", &self.path)
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

/// Owns an `SwsContext`.
struct Scaler(ff::software::scaling::Context);

// SAFETY: an SwsContext has no thread affinity; it only must not be used from
// two threads at once. `Scaler` is owned exclusively by one `FfmpegVideo`
// (never shared or aliased), so moving it to another thread is sound.
#[allow(unsafe_code)]
unsafe impl Send for Scaler {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScalerKey {
    format: ff::format::Pixel,
    width: u32,
    height: u32,
    space: ff::color::Space,
    range: ff::color::Range,
}

fn open_err(path: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Open {
        path: path.to_owned(),
        message: e.to_string(),
    }
}

fn stream_err(path: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Stream(format!("{path}: {e}"))
}

impl FfmpegVideo {
    /// Opens the best video stream in `path`.
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        let shown = path.display().to_string();
        crate::init().map_err(|e| open_err(&shown, e))?;
        if !path.is_file() {
            return Err(MediaError::NotFound { path: shown });
        }
        let input = ff::format::input(path).map_err(|e| open_err(&shown, e))?;
        Self::from_input(input, shown, false)
    }

    /// Decodes the best video stream of an opened input. `live` selects
    /// low-latency decoding (slice threading; frame threading would hold
    /// back several frames).
    pub(crate) fn from_input(
        input: ff::format::context::Input,
        shown: String,
        live: bool,
    ) -> Result<Self, MediaError> {
        let stream = input
            .streams()
            .best(ff::media::Type::Video)
            .ok_or_else(|| MediaError::NoVideo(shown.clone()))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
            return Err(open_err(&shown, "stream has an invalid time base"));
        }
        let start_pts = match stream.start_time() {
            ff::ffi::AV_NOPTS_VALUE => 0,
            s => s,
        };
        let avg = stream.avg_frame_rate();
        let rate = Rate::new(i64::from(avg.numerator()), i64::from(avg.denominator())).ok();
        let mut ctx = ff::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| open_err(&shown, e))?;
        ctx.set_threading(ff::threading::Config {
            kind: if live {
                ff::threading::Type::Slice
            } else {
                ff::threading::Type::Frame
            },
            count: 0,
        });
        if live {
            ctx.set_flags(ff::codec::Flags::LOW_DELAY);
        }
        let decoder = ctx.decoder().video().map_err(|e| open_err(&shown, e))?;
        let duration = if stream.duration() > 0 {
            RationalTime::new(
                i128::from(stream.duration()) * i128::from(time_base.numerator()),
                i128::from(time_base.denominator()),
            )
            .ok()
        } else if input.duration() > 0 {
            RationalTime::new(
                i128::from(input.duration()),
                i128::from(ff::ffi::AV_TIME_BASE),
            )
            .ok()
        } else {
            None
        };
        let codec = format!(
            "{} in {}",
            decoder
                .codec()
                .map_or_else(|| "unknown codec".to_owned(), |c| c.name().to_owned()),
            input.format().name()
        );
        let descriptor = MediaDescriptor {
            width: decoder.width(),
            height: decoder.height(),
            frame_rate: rate,
            duration,
            audio: None,
            codec,
        };
        if descriptor.width == 0 || descriptor.height == 0 {
            return Err(open_err(&shown, "video stream has zero size"));
        }
        Ok(Self {
            path: shown,
            input,
            decoder,
            stream_index,
            time_base,
            start_pts,
            rate,
            descriptor,
            scaler: None,
            decoded: ff::frame::Video::empty(),
            sent_eof: false,
            last_pts: None,
            skipped_packets: 0,
        })
    }

    /// Converts a stream timestamp to media time, snapped to the nominal
    /// frame grid when within a quarter frame of it. Containers with coarse
    /// time bases (Matroska stores milliseconds) otherwise shift fractional-
    /// rate frames by up to half a millisecond, which can select the wrong
    /// frame at exact frame boundaries (DECISIONS.md D-012).
    fn media_time(&self, pts: i64) -> Option<RationalTime> {
        let tb = self.time_base;
        let raw = RationalTime::new(
            i128::from(pts - self.start_pts) * i128::from(tb.numerator()),
            i128::from(tb.denominator()),
        )
        .ok()?;
        let Some(rate) = self.rate else {
            return Some(raw);
        };
        // Nearest frame index: floor(raw * rate + 1/2).
        let half = RationalTime::new(i128::from(rate.den()), 2 * i128::from(rate.num())).ok()?;
        let n = raw.checked_add(half).ok()?.to_frame_floor(rate).ok()?;
        let grid = RationalTime::from_frame(n, rate).ok()?;
        let quarter = RationalTime::new(i128::from(rate.den()), 4 * i128::from(rate.num())).ok()?;
        let err = raw.checked_sub(grid).ok()?;
        let abs_err = if err.ticks() < 0 {
            RationalTime::new(-err.ticks(), err.ticks_per_second()).ok()?
        } else {
            err
        };
        if abs_err.checked_cmp(quarter).ok()? == std::cmp::Ordering::Less {
            Some(grid)
        } else {
            Some(raw)
        }
    }

    fn convert(&mut self) -> Result<StillImage, MediaError> {
        let f = &self.decoded;
        let key = ScalerKey {
            format: f.format(),
            width: f.width(),
            height: f.height(),
            space: f.color_space(),
            range: f.color_range(),
        };
        if self.scaler.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut ctx = ff::software::scaling::Context::get(
                key.format,
                key.width,
                key.height,
                ff::format::Pixel::RGBA,
                key.width,
                key.height,
                ff::software::scaling::Flags::BILINEAR | ff::software::scaling::Flags::ACCURATE_RND,
            )
            .map_err(|e| stream_err(&self.path, e))?;
            set_colorspace(&mut ctx, key);
            self.scaler = Some((key, Scaler(ctx)));
        }
        let mut rgba = ff::frame::Video::empty();
        let Some((_, scaler)) = self.scaler.as_mut() else {
            return Err(stream_err(&self.path, "no scaler"));
        };
        scaler
            .0
            .run(&self.decoded, &mut rgba)
            .map_err(|e| stream_err(&self.path, e))?;
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let stride = rgba.stride(0);
        let data = rgba.data(0);
        let mut px = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            px.extend_from_slice(&data[y * stride..y * stride + w * 4]);
        }
        StillImage::from_rgba8(rgba.width(), rgba.height(), px)
    }

    /// Feeds the decoder until it yields a frame or the stream ends.
    fn decode_next(&mut self) -> Result<bool, MediaError> {
        loop {
            match self.decoder.receive_frame(&mut self.decoded) {
                Ok(()) => return Ok(true),
                Err(ff::Error::Eof) => return Ok(false),
                Err(ff::Error::Other { errno }) if errno == EAGAIN => {}
                Err(ff::Error::InvalidData) => {
                    self.skipped_packets += 1;
                    continue;
                }
                Err(e) => return Err(stream_err(&self.path, e)),
            }
            if self.sent_eof {
                return Ok(false);
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {
                    if packet.stream() != self.stream_index {
                        continue;
                    }
                    match self.decoder.send_packet(&packet) {
                        Ok(()) => {}
                        Err(ff::Error::InvalidData) => self.skipped_packets += 1,
                        Err(e) => return Err(stream_err(&self.path, e)),
                    }
                }
                Err(ff::Error::Eof) => {
                    self.sent_eof = true;
                    self.decoder
                        .send_eof()
                        .map_err(|e| stream_err(&self.path, e))?;
                }
                // A corrupt packet the demuxer can resync past.
                Err(ff::Error::InvalidData) => self.skipped_packets += 1,
                Err(e) => return Err(stream_err(&self.path, e)),
            }
        }
    }
}

/// Selects YUV→RGB coefficients from the frame's tagged colour space
/// (BT.709 for untagged HD, BT.601 for untagged SD) and its range.
#[allow(unsafe_code)]
fn set_colorspace(ctx: &mut ff::software::scaling::Context, key: ScalerKey) {
    use ff::color::{Range, Space};
    use ff::ffi;
    let space = match key.space {
        Space::BT709 => ffi::SWS_CS_ITU709,
        Space::BT2020NCL | Space::BT2020CL => ffi::SWS_CS_BT2020,
        Space::SMPTE240M => ffi::SWS_CS_SMPTE240M,
        Space::FCC => ffi::SWS_CS_FCC,
        Space::BT470BG | Space::SMPTE170M => ffi::SWS_CS_ITU601,
        _ if key.height >= 720 => ffi::SWS_CS_ITU709,
        _ => ffi::SWS_CS_ITU601,
    };
    let src_full = i32::from(key.range == Range::JPEG);
    // SAFETY: `ctx.as_mut_ptr()` is a valid, initialised SwsContext owned by
    // `ctx` for the duration of the call; `sws_getCoefficients` returns a
    // pointer to a static table that FFmpeg only reads. Arguments are within
    // the documented ranges (16.16 fixed point brightness/contrast/saturation).
    unsafe {
        let table = ffi::sws_getCoefficients(space);
        ffi::sws_setColorspaceDetails(
            ctx.as_mut_ptr(),
            table,
            src_full,
            table,
            1,
            0,
            1 << 16,
            1 << 16,
        );
    }
}

impl MediaSource for FfmpegVideo {
    fn descriptor(&self) -> &MediaDescriptor {
        &self.descriptor
    }

    fn seek(&mut self, t: RationalTime) -> Result<(), MediaError> {
        let us = t
            .to_ticks_floor(i128::from(ff::ffi::AV_TIME_BASE))
            .map_err(|e| MediaError::Seek(t.to_string(), e.to_string()))?;
        let start_us = i128::from(self.start_pts)
            * i128::from(ff::ffi::AV_TIME_BASE)
            * i128::from(self.time_base.numerator())
            / i128::from(self.time_base.denominator());
        let target = i64::try_from((us + start_us).max(0))
            .map_err(|e| MediaError::Seek(t.to_string(), e.to_string()))?;
        // Keyframe at or before the target; FrameCursor decodes forward.
        self.input
            // `..x` passes `x` as FFmpeg's inclusive max_ts.
            .seek(target, ..target)
            .or_else(|_| self.input.seek(0, ..0))
            .map_err(|e| MediaError::Seek(t.to_string(), e.to_string()))?;
        self.decoder.flush();
        self.sent_eof = false;
        self.last_pts = None;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<VideoFrame>, MediaError> {
        if !self.decode_next()? {
            return Ok(None);
        }
        let pts = self
            .decoded
            .timestamp()
            .or_else(|| self.decoded.pts())
            .and_then(|p| self.media_time(p));
        // Frames without timestamps follow the previous one by a nominal period.
        let pts = match (pts, self.last_pts, self.rate.and_then(|r| r.period().ok())) {
            (Some(p), ..) => p,
            (None, Some(prev), Some(period)) => prev
                .checked_add(period)
                .map_err(|e| stream_err(&self.path, e))?,
            (None, None, _) => RationalTime::ZERO,
            (None, Some(prev), None) => prev,
        };
        self.last_pts = Some(pts);
        let image = Arc::new(self.convert()?);
        Ok(Some(VideoFrame {
            pts,
            duration: None,
            image,
        }))
    }
}
