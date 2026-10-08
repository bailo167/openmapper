// SPDX-License-Identifier: Apache-2.0
//! Publishing output frames as a network stream.
//!
//! - [`StreamCodec::Compatible`]: MPEG-2 video (BT.709, limited range,
//!   intra refresh every half second, no B-frames) in MPEG-TS — playable by
//!   VLC, OBS, FFmpeg and most media servers. Over `rtp://` it uses RTP
//!   MPEG-TS.
//! - [`StreamCodec::Lossless`]: FFV1 BGRA (alpha kept) in Matroska — every
//!   pixel arrives bit-exact; needs a reliable transport (TCP or SRT).
//!
//! The connection is made on the first frame (the encoder needs its size);
//! a size change ends the stream and the publisher reopens it at the new
//! size.

use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ffmpeg_next as ff;
use om_media_core::{FrameSink, MediaError, SinkOpener, StillImage};
use om_project::{Publish, StreamCodec};

use crate::OPEN_TIMEOUT;
use crate::interrupt::Interrupt;

/// Longest a single frame write may block (a stalled receiver).
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns an `SwsContext` (see the note on `video::Scaler`).
struct Scaler(ff::software::scaling::Context);

// SAFETY: an SwsContext has no thread affinity and is owned exclusively by
// one `Connection`, so moving it between threads is sound.
#[allow(unsafe_code)]
unsafe impl Send for Scaler {}

struct Connection {
    // Encoder and muxer drop before the interrupt they point at.
    enc: ff::encoder::Video,
    octx: ff::format::context::Output,
    scaler: Scaler,
    frame: ff::frame::Video,
    size: (u32, u32),
    stream_tb: ff::Rational,
    enc_tb: ff::Rational,
    pts: i64,
}

// SAFETY: the FFmpeg contexts inside are used by one thread at a time (the
// publisher thread that owns the sink); none have thread affinity.
#[allow(unsafe_code)]
unsafe impl Send for Connection {}

/// A network stream publisher.
pub struct StreamSink {
    url: String,
    codec: StreamCodec,
    fps: u32,
    conn: Option<Connection>,
    interrupt: Box<Interrupt>,
}

impl std::fmt::Debug for StreamSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamSink({})", self.url)
    }
}

fn err(url: &str, e: impl std::fmt::Display) -> MediaError {
    MediaError::Stream(format!("{url}: {e}"))
}

impl StreamSink {
    /// Prepares a publisher; connects on the first frame.
    pub fn new(
        url: &str,
        codec: StreamCodec,
        fps: u32,
        stop: Arc<AtomicBool>,
    ) -> Result<Self, MediaError> {
        Publish::Stream {
            url: url.to_owned(),
            codec,
            fps,
        }
        .validate()
        .map_err(|e| err(url, e))?;
        crate::init().map_err(|e| err(url, e))?;
        let id = match codec {
            StreamCodec::Compatible => ff::codec::Id::MPEG2VIDEO,
            StreamCodec::Lossless => ff::codec::Id::FFV1,
        };
        if ff::encoder::find(id).is_none() {
            return Err(err(url, "FFmpeg was built without the needed encoder"));
        }
        Ok(Self {
            url: url.to_owned(),
            codec,
            fps,
            conn: None,
            interrupt: Interrupt::new(stop),
        })
    }

    fn muxer(&self) -> &'static str {
        let rtp = self.url.to_ascii_lowercase().starts_with("rtp://");
        match (self.codec, rtp) {
            (StreamCodec::Compatible, true) => "rtp_mpegts",
            (StreamCodec::Compatible, false) => "mpegts",
            (StreamCodec::Lossless, _) => "matroska",
        }
    }

    #[allow(unsafe_code)]
    fn connect(&self, width: u32, height: u32) -> Result<Connection, MediaError> {
        use ff::ffi;
        let url = &self.url;
        let e = |x: ff::Error| err(url, x);
        let listen = url.contains("listen") || url.contains("mode=listener");
        // A listening publisher waits for its receiver for as long as needed.
        self.interrupt.arm(if listen {
            Duration::from_secs(u64::from(u32::MAX))
        } else {
            OPEN_TIMEOUT
        });
        let c_url = CString::new(url.as_str()).map_err(|_| err(url, "URL contains a NUL byte"))?;
        let c_fmt = CString::new(self.muxer()).map_err(|_| err(url, "bad muxer name"))?;
        // SAFETY: standard FFmpeg output setup. The context gets the boxed
        // interrupt (owned by `self`, outliving the context); avio_open2
        // stores the opened I/O context in `pb`, which `Output`'s destructor
        // closes; on failure everything allocated here is freed once.
        let mut octx = unsafe {
            let mut ps: *mut ffi::AVFormatContext = std::ptr::null_mut();
            let r = ffi::avformat_alloc_output_context2(
                &raw mut ps,
                std::ptr::null(),
                c_fmt.as_ptr(),
                c_url.as_ptr(),
            );
            if r < 0 || ps.is_null() {
                return Err(err(url, ff::Error::from(r)));
            }
            (*ps).interrupt_callback = self.interrupt.callback();
            let r = ffi::avio_open2(
                &raw mut (*ps).pb,
                c_url.as_ptr(),
                ffi::AVIO_FLAG_WRITE,
                &raw const (*ps).interrupt_callback,
                std::ptr::null_mut(),
            );
            if r < 0 {
                let text = self.interrupt.error_text(r);
                ffi::avformat_free_context(ps);
                return Err(err(url, text));
            }
            ff::format::context::Output::wrap(ps)
        };
        let global_header = octx
            .format()
            .flags()
            .contains(ff::format::Flags::GLOBAL_HEADER);
        let (id, pix) = match self.codec {
            StreamCodec::Compatible => (ff::codec::Id::MPEG2VIDEO, ff::format::Pixel::YUV420P),
            StreamCodec::Lossless => (ff::codec::Id::FFV1, ff::format::Pixel::BGRA),
        };
        let codec = ff::encoder::find(id).ok_or_else(|| err(url, "encoder not available"))?;
        let mut ost = octx.add_stream(codec).map_err(e)?;
        let mut enc = ff::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(e)?;
        let fps = i32::try_from(self.fps).map_err(|x| err(url, x))?;
        let enc_tb = ff::Rational::new(1, fps);
        enc.set_width(width);
        enc.set_height(height);
        enc.set_format(pix);
        enc.set_time_base(enc_tb);
        enc.set_frame_rate(Some(ff::Rational::new(fps, 1)));
        let mut opts = ff::Dictionary::new();
        match self.codec {
            StreamCodec::Compatible => {
                enc.set_gop((self.fps / 2).max(1));
                enc.set_max_b_frames(0);
                let bits = u64::from(width) * u64::from(height) * u64::from(self.fps) / 4;
                enc.set_bit_rate(usize::try_from(bits.clamp(2_000_000, 60_000_000)).unwrap_or(0));
                enc.set_colorspace(ff::color::Space::BT709);
                enc.set_color_range(ff::color::Range::MPEG);
            }
            StreamCodec::Lossless => {
                // Every frame a keyframe: receivers can join at any frame.
                enc.set_gop(1);
                opts.set("level", "3");
                opts.set("slices", "4");
                opts.set("slicecrc", "1");
            }
        }
        enc.set_threading(ff::threading::Config {
            kind: ff::threading::Type::Slice,
            count: 0,
        });
        if global_header {
            enc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
        }
        let enc = enc.open_as_with(codec, opts).map_err(e)?;
        ost.set_parameters(&enc);
        ost.set_time_base(enc_tb);
        ost.set_avg_frame_rate(ff::Rational::new(fps, 1));
        self.interrupt.arm(WRITE_TIMEOUT);
        let mut mux_opts = ff::Dictionary::new();
        if self.codec == StreamCodec::Lossless {
            mux_opts.set("live", "1");
        }
        octx.write_header_with(mux_opts).map_err(e)?;
        let stream_tb = octx
            .stream(0)
            .ok_or_else(|| err(url, "no output stream"))?
            .time_base();
        let mut scaler = ff::software::scaling::Context::get(
            ff::format::Pixel::RGBA,
            width,
            height,
            pix,
            width,
            height,
            ff::software::scaling::Flags::BILINEAR | ff::software::scaling::Flags::ACCURATE_RND,
        )
        .map_err(e)?;
        if self.codec == StreamCodec::Compatible {
            set_bt709(&mut scaler);
        }
        Ok(Connection {
            enc,
            octx,
            scaler: Scaler(scaler),
            frame: ff::frame::Video::new(pix, width, height),
            size: (width, height),
            stream_tb,
            enc_tb,
            pts: 0,
        })
    }
}

/// RGB → BT.709 limited-range YUV.
#[allow(unsafe_code)]
fn set_bt709(ctx: &mut ff::software::scaling::Context) {
    use ff::ffi;
    // SAFETY: valid owned SwsContext; the coefficient table is static and
    // only read; arguments are in documented ranges.
    unsafe {
        let table = ffi::sws_getCoefficients(ffi::SWS_CS_ITU709);
        ffi::sws_setColorspaceDetails(ctx.as_mut_ptr(), table, 1, table, 0, 0, 1 << 16, 1 << 16);
    }
}

impl Connection {
    fn write(&mut self, image: &StillImage, url: &str) -> Result<(), MediaError> {
        let e = |x: ff::Error| err(url, x);
        let (w, h) = (image.width(), image.height());
        let mut src = ff::frame::Video::new(ff::format::Pixel::RGBA, w, h);
        let stride = src.stride(0);
        let row = w as usize * 4;
        let data = src.data_mut(0);
        for (y, line) in image.rgba8().chunks_exact(row).enumerate() {
            data[y * stride..y * stride + row].copy_from_slice(line);
        }
        self.scaler.0.run(&src, &mut self.frame).map_err(e)?;
        self.frame.set_pts(Some(self.pts));
        self.pts += 1;
        self.enc.send_frame(&self.frame).map_err(e)?;
        self.drain(url)
    }

    fn drain(&mut self, url: &str) -> Result<(), MediaError> {
        let mut packet = ff::Packet::empty();
        while self.enc.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.rescale_ts(self.enc_tb, self.stream_tb);
            packet
                .write_interleaved(&mut self.octx)
                .map_err(|x| err(url, x))?;
        }
        Ok(())
    }
}

impl FrameSink for StreamSink {
    fn send(&mut self, image: &Arc<StillImage>) -> Result<(), MediaError> {
        let size = (image.width(), image.height());
        if self.conn.as_ref().is_some_and(|c| c.size != size) {
            self.conn = None;
            return Err(err(&self.url, "output size changed; restarting the stream"));
        }
        if self.conn.is_none() {
            self.conn = Some(self.connect(size.0, size.1)?);
        }
        self.interrupt.arm(WRITE_TIMEOUT);
        let url = self.url.clone();
        let result = match self.conn.as_mut() {
            Some(c) => c.write(image, &url),
            None => Ok(()),
        };
        if result.is_err() {
            self.conn = None;
        }
        result
    }

    fn describe(&self) -> String {
        let codec = match self.codec {
            StreamCodec::Compatible => "MPEG-2/TS",
            StreamCodec::Lossless => "FFV1/MKV lossless",
        };
        format!("{codec} @ {} fps → {}", self.fps, self.url)
    }

    fn rate(&self) -> Option<u32> {
        Some(self.fps)
    }
}

impl Drop for StreamSink {
    fn drop(&mut self) {
        // Finish the stream cleanly if the receiver is still there, but never
        // block shutdown on it.
        if let Some(mut c) = self.conn.take() {
            self.interrupt.arm(Duration::from_millis(500));
            let _ = c.enc.send_eof();
            let _ = c.drain(&self.url);
            let _ = c.octx.write_trailer();
        }
    }
}

/// [`SinkOpener`] for network streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct FfmpegSinkOpener;

impl SinkOpener for FfmpegSinkOpener {
    fn supports(&self, target: &Publish) -> bool {
        matches!(target, Publish::Stream { .. })
    }

    fn open_sink(
        &self,
        target: &Publish,
        stop: &Arc<AtomicBool>,
    ) -> Result<Box<dyn FrameSink>, MediaError> {
        match target {
            Publish::Stream { url, codec, fps } => Ok(Box::new(StreamSink::new(
                url,
                *codec,
                *fps,
                Arc::clone(stop),
            )?)),
            other => Err(err(&other.label(), "not an FFmpeg target")),
        }
    }
}
