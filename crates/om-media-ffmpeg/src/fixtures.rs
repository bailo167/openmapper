// SPDX-License-Identifier: Apache-2.0
//! Generated test corpus: videos whose frames carry their own index.
//!
//! Each frame stamps its index as 16 black/white blocks across the top
//! quarter (MSB first), large enough to survive lossy coding, plus a moving
//! gradient so inter-frame codecs have real motion to encode. Only FFmpeg's
//! built-in LGPL encoders are used (FFV1, MPEG-4 Part 2), so no binary test
//! media is committed and no GPL encoder is needed.

use std::path::Path;

use ffmpeg_next as ff;
use om_media_core::StillImage;
use om_time::Rate;

/// Codec/container combination for a fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureCodec {
    /// Lossless intra-only FFV1 in Matroska (millisecond timestamps).
    Ffv1Mkv,
    /// MPEG-4 Part 2 in MP4 with B-frames and 12-frame GOPs (exact time base).
    Mpeg4Mp4,
}

impl FixtureCodec {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Ffv1Mkv => "mkv",
            Self::Mpeg4Mp4 => "mp4",
        }
    }
}

/// Fixture parameters.
#[derive(Debug, Clone, Copy)]
pub struct FixtureSpec {
    pub width: u32,
    pub height: u32,
    pub rate: Rate,
    pub frames: u32,
    pub codec: FixtureCodec,
}

const BITS: u32 = 16;

fn luma_for(index: u32, x: u32, y: u32, w: u32, h: u32) -> u8 {
    if y < h / 4 {
        let block = x * BITS / w;
        let bit = (index >> (BITS - 1 - block)) & 1;
        return if bit == 1 { 235 } else { 16 };
    }
    #[allow(clippy::cast_possible_truncation)]
    let v = ((x + y + index * 7) % 200 + 28) as u8;
    v
}

/// Reads the frame index stamped by [`write_index_video`].
#[must_use]
pub fn read_index(img: &StillImage) -> u32 {
    let (w, h) = (img.width(), img.height());
    let y = h / 8;
    (0..BITS).fold(0, |acc, block| {
        let x = block * w / BITS + w / (BITS * 2);
        let bright = img.pixel(x, y).is_some_and(|p| p[1] > 128);
        (acc << 1) | u32::from(bright)
    })
}

/// Encodes a frame-index video at `path`.
pub fn write_index_video(path: &Path, spec: FixtureSpec) -> Result<(), String> {
    crate::init()?;
    let e = |e: ff::Error| e.to_string();
    let codec_id = match spec.codec {
        FixtureCodec::Ffv1Mkv => ff::codec::Id::FFV1,
        FixtureCodec::Mpeg4Mp4 => ff::codec::Id::MPEG4,
    };
    let codec = ff::encoder::find(codec_id).ok_or("encoder not available in this FFmpeg build")?;
    let mut octx = ff::format::output(path).map_err(e)?;
    let global_header = octx
        .format()
        .flags()
        .contains(ff::format::Flags::GLOBAL_HEADER);
    let mut ost = octx.add_stream(codec).map_err(e)?;
    let mut enc = ff::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(e)?;
    #[allow(clippy::cast_possible_truncation)]
    let tb = ff::Rational::new(spec.rate.den() as i32, spec.rate.num() as i32);
    enc.set_width(spec.width);
    enc.set_height(spec.height);
    enc.set_format(ff::format::Pixel::YUV420P);
    enc.set_time_base(tb);
    enc.set_frame_rate(Some(ff::Rational::new(tb.denominator(), tb.numerator())));
    if spec.codec == FixtureCodec::Mpeg4Mp4 {
        enc.set_gop(12);
        enc.set_max_b_frames(2);
        enc.set_bit_rate(8_000_000);
    }
    if global_header {
        enc.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let mut enc = enc.open_as(codec).map_err(e)?;
    ost.set_parameters(&enc);
    ost.set_time_base(tb);
    ost.set_avg_frame_rate(ff::Rational::new(tb.denominator(), tb.numerator()));
    octx.write_header().map_err(e)?;
    let ost_tb = octx.stream(0).ok_or("no output stream")?.time_base();

    let drain = |enc: &mut ff::encoder::Video,
                 octx: &mut ff::format::context::Output|
     -> Result<(), String> {
        let mut packet = ff::Packet::empty();
        while enc.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.rescale_ts(tb, ost_tb);
            packet.write_interleaved(octx).map_err(|e| e.to_string())?;
        }
        Ok(())
    };

    let (w, h) = (spec.width, spec.height);
    let mut frame = ff::frame::Video::new(ff::format::Pixel::YUV420P, w, h);
    for i in 0..spec.frames {
        let stride = frame.stride(0);
        let luma = frame.data_mut(0);
        for y in 0..h {
            for x in 0..w {
                luma[y as usize * stride + x as usize] = luma_for(i, x, y, w, h);
            }
        }
        for plane in 1..3 {
            let stride = frame.stride(plane);
            let data = frame.data_mut(plane);
            for y in 0..h.div_ceil(2) as usize {
                data[y * stride..y * stride + w.div_ceil(2) as usize].fill(128);
            }
        }
        frame.set_pts(Some(i64::from(i)));
        enc.send_frame(&frame).map_err(e)?;
        drain(&mut enc, &mut octx)?;
    }
    enc.send_eof().map_err(e)?;
    drain(&mut enc, &mut octx)?;
    octx.write_trailer().map_err(e)?;
    Ok(())
}
