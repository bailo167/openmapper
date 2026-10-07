// SPDX-License-Identifier: Apache-2.0
//! Audio decoding to interleaved stereo `f32` at a fixed output rate, with
//! sample-exact positions.

use std::path::Path;

use ff::util::error::EAGAIN;
use ffmpeg_next as ff;
use om_media_core::{AudioBlock, AudioFormat, AudioSource, MediaError};

/// The audio track of a media file.
pub struct FfmpegAudio {
    path: String,
    input: ff::format::context::Input,
    decoder: ff::decoder::Audio,
    stream_index: usize,
    time_base: ff::Rational,
    start_pts: i64,
    out: AudioFormat,
    length: Option<i64>,
    resampler: Option<Resampler>,
    decoded: ff::frame::Audio,
    sent_eof: bool,
    flushed: bool,
    /// Output frame index of the next sample to emit; `None` until the first
    /// decoded frame after open/seek establishes it from its timestamp.
    position: Option<i64>,
    /// Discard output before this frame (exact seeking).
    discard_before: i64,
}

impl std::fmt::Debug for FfmpegAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfmpegAudio")
            .field("path", &self.path)
            .field("out", &self.out)
            .finish_non_exhaustive()
    }
}

/// Owns an `SwrContext`.
/// libswresample context converting any input to interleaved stereo f32.
struct Resampler {
    key: (ff::format::Sample, i32, u32),
    ctx: *mut ff::ffi::SwrContext,
}

// SAFETY: an SwrContext has no thread affinity; it is owned exclusively by
// one `FfmpegAudio` and never shared, so moving it between threads is sound.
#[allow(unsafe_code)]
unsafe impl Send for Resampler {}

#[allow(unsafe_code)]
impl Resampler {
    fn new(
        in_format: ff::format::Sample,
        in_layout: &ff::ChannelLayout,
        in_rate: u32,
        out_rate: u32,
    ) -> Result<Self, String> {
        let mut ctx: *mut ff::ffi::SwrContext = std::ptr::null_mut();
        let out_layout = ff::ChannelLayout::STEREO;
        let in_fmt: ff::ffi::AVSampleFormat = in_format.into();
        // SAFETY: all pointers are valid for the call: `ctx` is a local out
        // parameter, the layouts are live AVChannelLayout values copied by
        // FFmpeg, and the log context may be null. On success `ctx` owns a
        // new SwrContext that `Drop` frees.
        let ret = unsafe {
            ff::ffi::swr_alloc_set_opts2(
                &raw mut ctx,
                &raw const out_layout.0,
                ff::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                i32::try_from(out_rate).unwrap_or(48_000),
                &raw const in_layout.0,
                in_fmt,
                i32::try_from(in_rate).unwrap_or(48_000),
                0,
                std::ptr::null_mut(),
            )
        };
        if ret < 0 || ctx.is_null() {
            return Err(format!("swr_alloc_set_opts2 failed ({ret})"));
        }
        let me = Self {
            key: (in_format, in_layout.channels(), in_rate),
            ctx,
        };
        // SAFETY: `me.ctx` is a freshly allocated, configured SwrContext.
        let ret = unsafe { ff::ffi::swr_init(me.ctx) };
        if ret < 0 {
            return Err(format!("swr_init failed ({ret})"));
        }
        Ok(me)
    }

    /// Converts `frame` (or flushes buffered samples when `None`) to
    /// interleaved stereo f32.
    fn convert(&mut self, frame: Option<&ff::frame::Audio>) -> Result<Vec<f32>, String> {
        let in_samples = frame.map_or(0, |f| i32::try_from(f.samples()).unwrap_or(0));
        // SAFETY: `self.ctx` is a valid initialised SwrContext.
        let capacity = unsafe { ff::ffi::swr_get_out_samples(self.ctx, in_samples) }.max(0);
        let mut out = vec![0.0f32; usize::try_from(capacity).unwrap_or(0) * 2 + 2];
        let mut out_ptr = out.as_mut_ptr().cast::<u8>();
        let in_ptr = frame.map_or(std::ptr::null(), |f| {
            // SAFETY: the frame's AVFrame is valid; extended_data holds one
            // pointer per plane for the frame's sample count.
            unsafe { (*f.as_ptr()).extended_data.cast_const().cast::<*const u8>() }
        });
        // SAFETY: `out_ptr` points to a buffer of `capacity + 1` interleaved
        // stereo f32 frames (one plane for packed output); `in_ptr` is either
        // null (flush) or the frame's plane pointers with `in_samples` frames.
        let n = unsafe {
            ff::ffi::swr_convert(self.ctx, &raw mut out_ptr, capacity + 1, in_ptr, in_samples)
        };
        if n < 0 {
            return Err(format!("swr_convert failed ({n})"));
        }
        out.truncate(usize::try_from(n).unwrap_or(0) * 2);
        Ok(out)
    }
}

#[allow(unsafe_code)]
impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: `self.ctx` was allocated by swr_alloc_set_opts2 and is
        // freed exactly once here; swr_free nulls the pointer.
        unsafe { ff::ffi::swr_free(&raw mut self.ctx) };
    }
}

fn err(path: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Stream(format!("{path}: {e}"))
}

impl FfmpegAudio {
    /// Opens the best audio stream, converting to `rate` Hz stereo.
    /// `Ok(None)` if the file has no audio stream.
    pub fn open(path: &Path, rate: u32) -> Result<Option<Self>, MediaError> {
        let shown = path.display().to_string();
        crate::init().map_err(|e| err(&shown, e))?;
        if !path.is_file() {
            return Err(MediaError::NotFound { path: shown });
        }
        let input = ff::format::input(path).map_err(|e| MediaError::Open {
            path: shown.clone(),
            message: e.to_string(),
        })?;
        let Some(stream) = input.streams().best(ff::media::Type::Audio) else {
            return Ok(None);
        };
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let start_pts = match stream.start_time() {
            ff::ffi::AV_NOPTS_VALUE => 0,
            s => s,
        };
        let length = if stream.duration() > 0 && time_base.denominator() > 0 {
            Some(
                i64::try_from(
                    i128::from(stream.duration())
                        * i128::from(time_base.numerator())
                        * i128::from(rate)
                        / i128::from(time_base.denominator()),
                )
                .unwrap_or(i64::MAX),
            )
        } else {
            None
        };
        let ctx = ff::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| err(&shown, e))?;
        let decoder = ctx.decoder().audio().map_err(|e| err(&shown, e))?;
        Ok(Some(Self {
            path: shown,
            input,
            decoder,
            stream_index,
            time_base,
            start_pts,
            out: AudioFormat {
                sample_rate: rate,
                channels: 2,
            },
            length,
            resampler: None,
            decoded: ff::frame::Audio::empty(),
            sent_eof: false,
            flushed: false,
            position: None,
            discard_before: 0,
        }))
    }

    /// Output-rate frame index of the first decoded frame after open/seek.
    ///
    /// Containers with a time base coarser than one sample (Matroska stores
    /// milliseconds: 48 samples at 48 kHz) misplace packets by up to half a
    /// tick. Codecs with fixed-size packets (PCM as muxed, AAC, MP3) start
    /// every packet on a multiple of the packet size from stream start, so
    /// the source position is snapped to that grid (DECISIONS.md D-014).
    fn first_position(&self, pts: i64) -> i64 {
        let tb = self.time_base;
        let src_rate = i128::from(self.decoded.rate().max(1));
        let round_div = |num: i128, den: i128| (num * 2 + den).div_euclid(den * 2);
        let mut src = round_div(
            i128::from(pts - self.start_pts) * i128::from(tb.numerator()) * src_rate,
            i128::from(tb.denominator()),
        );
        let ticks_per_second = i128::from(tb.denominator()) / i128::from(tb.numerator().max(1));
        let packet = i128::try_from(self.decoded.samples()).unwrap_or(0);
        if ticks_per_second < src_rate && packet > 0 {
            src = round_div(src, packet) * packet;
        }
        i64::try_from(round_div(src * i128::from(self.out.sample_rate), src_rate)).unwrap_or(0)
    }

    fn decode_frame(&mut self) -> Result<bool, MediaError> {
        loop {
            match self.decoder.receive_frame(&mut self.decoded) {
                Ok(()) => return Ok(true),
                Err(ff::Error::Eof) => return Ok(false),
                Err(ff::Error::Other { errno }) if errno == EAGAIN => {}
                Err(ff::Error::InvalidData) => continue,
                Err(e) => return Err(err(&self.path, e)),
            }
            if self.sent_eof {
                return Ok(false);
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream_index => {
                    match self.decoder.send_packet(&packet) {
                        Ok(()) | Err(ff::Error::InvalidData) => {}
                        Err(e) => return Err(err(&self.path, e)),
                    }
                }
                Ok(()) | Err(ff::Error::InvalidData) => {}
                Err(ff::Error::Eof) => {
                    self.sent_eof = true;
                    self.decoder.send_eof().map_err(|e| err(&self.path, e))?;
                }
                Err(e) => return Err(err(&self.path, e)),
            }
        }
    }

    fn resample(&mut self, flush: bool) -> Result<Vec<f32>, MediaError> {
        if flush {
            return match self.resampler.as_mut() {
                Some(r) => r.convert(None).map_err(|e| err(&self.path, e)),
                None => Ok(Vec::new()),
            };
        }
        normalize_layout(&mut self.decoded, self.decoder.channels());
        let f = &self.decoded;
        let key = (f.format(), f.channel_layout().channels(), f.rate());
        if self.resampler.as_ref().is_none_or(|r| r.key != key) {
            let r = Resampler::new(
                f.format(),
                &f.channel_layout(),
                f.rate(),
                self.out.sample_rate,
            )
            .map_err(|e| err(&self.path, e))?;
            self.resampler = Some(r);
        }
        match self.resampler.as_mut() {
            Some(r) => r
                .convert(Some(&self.decoded))
                .map_err(|e| err(&self.path, e)),
            None => Ok(Vec::new()),
        }
    }
}

/// Gives frames with an unspecified channel order (e.g. PCM in Matroska) the
/// default layout for their channel count. libswresample normalises such
/// layouts internally and would otherwise reject every frame as changed.
#[allow(unsafe_code)]
fn normalize_layout(frame: &mut ff::frame::Audio, decoder_channels: u16) {
    // SAFETY: `as_ptr` points to the frame's valid AVFrame for the duration
    // of the read; `ch_layout.order` is a plain enum field.
    let unspecified = unsafe {
        (*frame.as_ptr()).ch_layout.order == ff::ffi::AVChannelOrder::AV_CHANNEL_ORDER_UNSPEC
    };
    if unspecified {
        let n = match frame.channel_layout().channels() {
            0 => i32::from(decoder_channels),
            n => n,
        };
        frame.set_channel_layout(ff::ChannelLayout::default(n));
    }
}

impl AudioSource for FfmpegAudio {
    fn format(&self) -> AudioFormat {
        self.out
    }

    fn length(&self) -> Option<i64> {
        self.length
    }

    fn seek(&mut self, frame: i64) -> Result<(), MediaError> {
        let frame = frame.max(0);
        let us = i128::from(frame) * i128::from(ff::ffi::AV_TIME_BASE)
            / i128::from(self.out.sample_rate);
        let start_us = i128::from(self.start_pts)
            * i128::from(ff::ffi::AV_TIME_BASE)
            * i128::from(self.time_base.numerator())
            / i128::from(self.time_base.denominator());
        // Land a little early so decoder warm-up (priming) is discarded.
        let target = i64::try_from((us + start_us - 100_000).max(0)).unwrap_or(0);
        self.input
            .seek(target, ..target)
            .or_else(|_| self.input.seek(0, ..0))
            .map_err(|e| MediaError::Seek(format!("sample {frame}"), e.to_string()))?;
        self.decoder.flush();
        self.resampler = None;
        self.sent_eof = false;
        self.flushed = false;
        self.position = None;
        self.discard_before = frame;
        Ok(())
    }

    fn next_block(&mut self) -> Result<Option<AudioBlock>, MediaError> {
        loop {
            let samples = if self.decode_frame()? {
                if self.position.is_none() {
                    let pts = self
                        .decoded
                        .timestamp()
                        .or_else(|| self.decoded.pts())
                        .unwrap_or(self.start_pts);
                    self.position = Some(self.first_position(pts));
                }
                self.resample(false)?
            } else if !self.flushed {
                self.flushed = true;
                self.resample(true)?
            } else {
                return Ok(None);
            };
            if samples.is_empty() {
                continue;
            }
            let start = self.position.unwrap_or(0);
            let frames = i64::try_from(samples.len() / 2).unwrap_or(0);
            self.position = Some(start + frames);
            // Drop anything before the seek target.
            let skip =
                usize::try_from((self.discard_before - start).clamp(0, frames)).unwrap_or(0) * 2;
            if skip >= samples.len() {
                continue;
            }
            return Ok(Some(AudioBlock {
                start: start.max(self.discard_before),
                format: self.out,
                samples: samples[skip..].to_vec(),
            }));
        }
    }
}
