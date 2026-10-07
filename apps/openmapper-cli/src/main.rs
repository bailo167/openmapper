// SPDX-License-Identifier: Apache-2.0
//! Deterministic command-line control of OpenMapper projects.
//!
//! Exit codes: 0 success, 1 operation failed, 2 usage error (from clap).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use om_command::Command;
use std::sync::Arc;

use om_engine::{MediaRuntime, Session};
use om_gpu::GpuContext;
use om_media_core::VideoOpener;
use om_project::{Project, store};
use om_render::Compositor;
use om_time::RationalTime;

#[derive(Debug, Parser)]
#[command(name = "openmapper-cli", version, about = "OpenMapper project tool")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Create a new empty project file.
    New {
        path: PathBuf,
        #[arg(long, default_value = "Untitled")]
        name: String,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Check that a project parses, migrates and validates.
    Validate { path: PathBuf },
    /// Print a project summary, or the canonical JSON with --json.
    Inspect {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Apply commands from a JSON file (an array, or one command per line)
    /// and save. Stops at the first rejected command without saving.
    Apply { path: PathBuf, commands: PathBuf },
    /// Render the project's canvas offscreen to a PNG (8-bit sRGB).
    Render {
        path: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        /// Show time in seconds (selects video frames exactly).
        #[arg(long, default_value_t = 0.0)]
        at: f64,
    },
    /// Show FFmpeg version and licence profile of the loaded libraries.
    Ffmpeg,
    /// List connected displays (index order used by outputs).
    Displays,
    /// Show the GPU adapter the renderer would use.
    Gpu,
    /// Render continuously and fail if GPU resources grow (stability test).
    Soak {
        path: PathBuf,
        #[arg(long, default_value_t = 60)]
        seconds: u64,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("openmapper-cli: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.action {
        Action::New { path, name, force } => {
            if path.exists() && !force {
                return Err(format!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                ));
            }
            if name.trim().is_empty() {
                return Err("project name must not be empty".into());
            }
            let mut session = Session::new(name);
            session.save_as(&path).map_err(|e| e.to_string())?;
            session.close().map_err(|e| e.to_string())?;
            println!("created {}", path.display());
            Ok(())
        }
        Action::Validate { path } => {
            let loaded = store::load(&path).map_err(|e| e.to_string())?;
            match loaded.migrated_from {
                Some(v) => println!("ok (version {v}, migrates to current)"),
                None => println!("ok"),
            }
            Ok(())
        }
        Action::Inspect { path, json } => {
            let loaded = store::load(&path).map_err(|e| e.to_string())?;
            let p = loaded.project;
            if json {
                print!("{}", p.to_canonical_json().map_err(|e| e.to_string())?);
            } else {
                println!("name:       {}", p.name);
                println!("project_id: {}", p.project_id);
                println!("version:    {}", p.version);
                println!("revision:   {}", p.revision);
                println!("surfaces:   {}", p.surfaces.len());
                for s in &p.surfaces {
                    println!(
                        "  {}  {:<24} enabled={} opacity={}",
                        s.id,
                        s.name,
                        s.enabled,
                        s.opacity.get()
                    );
                }
                println!("media:      {}", p.media.len());
                println!("outputs:    {}", p.outputs.len());
                if !p.extensions.is_empty() {
                    let keys: Vec<_> = p.extensions.keys().map(String::as_str).collect();
                    println!("extensions: {}", keys.join(", "));
                }
            }
            Ok(())
        }
        Action::Apply { path, commands } => {
            let cmds = read_commands(&commands)?;
            let (mut session, report) = Session::open(&path).map_err(|e| e.to_string())?;
            for w in &report.warnings {
                eprintln!("warning: {w}");
            }
            for (i, c) in cmds.into_iter().enumerate() {
                if let Err(e) = session.execute(c) {
                    // Discard the partial batch, including its journal entries,
                    // so a later open does not "recover" it.
                    let msg = format!("command {} rejected: {e}; nothing saved", i + 1);
                    return match session.close() {
                        Ok(()) => Err(msg),
                        Err(close) => Err(format!("{msg} (and removing journal failed: {close})")),
                    };
                }
            }
            session.save().map_err(|e| e.to_string())?;
            println!("revision {}", session.project().revision);
            session.close().map_err(|e| e.to_string())
        }
        Action::Render { path, out, at } => render(&path, &out, at),
        Action::Ffmpeg => {
            om_media_ffmpeg::init()?;
            let info = om_media_ffmpeg::info();
            println!("libavcodec {}", info.avcodec_version);
            println!("licence:   {}", info.licence);
            println!("configure: {}", info.configuration);
            Ok(())
        }
        Action::Displays => {
            let displays = om_output::list_displays().map_err(|e| e.to_string())?;
            for d in &displays {
                println!("{d}");
            }
            if displays.is_empty() {
                println!("no displays found");
            }
            Ok(())
        }
        Action::Gpu => {
            let gpu = GpuContext::headless().map_err(|e| e.to_string())?;
            println!("{}", gpu.capabilities());
            Ok(())
        }
        Action::Soak { path, seconds } => soak(&path, seconds),
    }
}

/// Opens a project read-only and prepares a compositor with its media.
/// Opens a project read-only and prepares a compositor plus media runtime.
fn prepare(path: &Path) -> Result<(Project, Compositor, MediaRuntime), String> {
    let project = store::load(path).map_err(|e| e.to_string())?.project;
    let gpu = GpuContext::headless().map_err(|e| e.to_string())?;
    let opener: Arc<dyn VideoOpener> = Arc::new(om_media_ffmpeg::FfmpegOpener);
    Ok((
        project,
        Compositor::new(gpu),
        MediaRuntime::new(Some(opener)),
    ))
}

/// Loads the media frames for show time `t` (waiting for exact video
/// frames) into the compositor, warning about anything that failed.
fn load_media(
    project: &Project,
    dir: Option<&Path>,
    compositor: &mut Compositor,
    media: &mut MediaRuntime,
    t: RationalTime,
    warn: bool,
) {
    let changes = media.update_blocking(project, dir, t, Duration::from_secs(10));
    for id in &changes.unload {
        compositor.remove_image(*id);
    }
    for (id, img) in &changes.upload {
        if let Err(e) = compositor.set_image(*id, img) {
            eprintln!("warning: media {id}: {e}");
        }
    }
    if warn {
        for m in &project.media {
            if let Some(err) = media.status(project, m.id, t).and_then(|s| s.error) {
                eprintln!("warning: media {:?}: {err}", m.name);
            }
        }
    }
}

fn render(path: &Path, out: &Path, at: f64) -> Result<(), String> {
    let (project, mut compositor, mut media) = prepare(path)?;
    let t = seconds(at)?;
    load_media(
        &project,
        path.parent(),
        &mut compositor,
        &mut media,
        t,
        true,
    );
    let report = compositor.render(&project).map_err(|e| e.to_string())?;
    for (id, reason) in &report.plan.skipped {
        eprintln!("note: surface {id} not drawn: {reason}");
    }
    let pixels = compositor.read_rgba8().map_err(|e| e.to_string())?;
    let (w, h) = report.canvas;
    image::save_buffer(out, &pixels, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("writing {}: {e}", out.display()))?;
    println!("rendered {}x{} -> {}", w, h, out.display());
    Ok(())
}

/// Show time from decimal seconds (millisecond precision).
fn seconds(s: f64) -> Result<RationalTime, String> {
    if !s.is_finite() || s < 0.0 {
        return Err(format!("time {s} must be a non-negative number of seconds"));
    }
    #[allow(clippy::cast_possible_truncation)]
    let ms = (s * 1000.0).round() as i128;
    RationalTime::new(ms, 1000).map_err(|e| e.to_string())
}

fn soak(path: &Path, seconds: u64) -> Result<(), String> {
    let (project, mut compositor, mut media) = prepare(path)?;
    println!("gpu: {}", compositor.gpu().capabilities());
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut frames: u64 = 0;
    let mut baseline = None;
    let mut next_report = Instant::now() + Duration::from_secs(10);
    let start = Instant::now();
    while Instant::now() < deadline {
        let t = RationalTime::from_nanos(start.elapsed().as_nanos());
        load_media(
            &project,
            path.parent(),
            &mut compositor,
            &mut media,
            t,
            frames == 0,
        );
        compositor.render(&project).map_err(|e| e.to_string())?;
        // Read back periodically to force GPU completion and exercise that path.
        if frames.is_multiple_of(60) {
            compositor.read_rgba8().map_err(|e| e.to_string())?;
        }
        frames += 1;
        if frames == 120 {
            baseline = Some(compositor.resource_counts());
        }
        if let Some(b) = baseline {
            let now = compositor.resource_counts();
            if now != b {
                return Err(format!(
                    "resources changed after {frames} frames: {b:?} -> {now:?}"
                ));
            }
        }
        if Instant::now() >= next_report {
            println!(
                "{:>6.0}s  {frames} frames  {:.1} fps  {:?}",
                start.elapsed().as_secs_f64(),
                frames as f64 / start.elapsed().as_secs_f64(),
                compositor.resource_counts()
            );
            next_report += Duration::from_secs(10);
        }
    }
    println!("ok: {frames} frames in {seconds}s with stable GPU resources");
    Ok(())
}

fn read_commands(path: &Path) -> Result<Vec<Command>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if text.trim_start().starts_with('[') {
        return serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()));
    }
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(n, l)| {
            serde_json::from_str(l).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))
        })
        .collect()
}
