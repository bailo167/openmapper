// SPDX-License-Identifier: Apache-2.0
//! `fixturegen <out> [--rate 30000/1001] [--frames N] [--size WxH] [--codec mpeg4|ffv1]`
//!
//! Writes a video whose frames carry their own index (see
//! `om_media_ffmpeg::fixtures`). Output is never committed; regenerate it.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use om_media_ffmpeg::fixtures::{FixtureCodec, FixtureSpec, write_index_video};
use om_time::Rate;

#[derive(Debug, Parser)]
#[command(about = "Generate frame-index test videos")]
struct Args {
    out: PathBuf,
    /// Frame rate as num/den or an integer.
    #[arg(long, default_value = "30000/1001")]
    rate: String,
    #[arg(long, default_value_t = 300)]
    frames: u32,
    #[arg(long, default_value = "640x360")]
    size: String,
    /// mpeg4 (MP4, B-frames) or ffv1 (lossless MKV).
    #[arg(long, default_value = "mpeg4")]
    codec: String,
    /// Add an audio track at this sample rate (1 kHz burst every second).
    #[arg(long)]
    audio: Option<u32>,
}

fn parse_pair(s: &str, sep: char) -> Option<(i64, i64)> {
    let (a, b) = s.split_once(sep).unwrap_or((s, "1"));
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn run(a: Args) -> Result<(), String> {
    let (num, den) = parse_pair(&a.rate, '/').ok_or("bad --rate")?;
    let rate = Rate::new(num, den).map_err(|e| e.to_string())?;
    let (w, h) = parse_pair(&a.size, 'x').ok_or("bad --size")?;
    let codec = match a.codec.as_str() {
        "mpeg4" => FixtureCodec::Mpeg4Mp4,
        "ffv1" => FixtureCodec::Ffv1Mkv,
        other => return Err(format!("unknown codec {other}")),
    };
    let spec = FixtureSpec {
        width: u32::try_from(w).map_err(|e| e.to_string())?,
        height: u32::try_from(h).map_err(|e| e.to_string())?,
        rate,
        frames: a.frames,
        codec,
        audio_rate: a.audio,
    };
    write_index_video(&a.out, spec)?;
    println!(
        "wrote {} ({:?}, {} frames)",
        a.out.display(),
        rate,
        a.frames
    );
    Ok(())
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fixturegen: {e}");
            ExitCode::FAILURE
        }
    }
}
