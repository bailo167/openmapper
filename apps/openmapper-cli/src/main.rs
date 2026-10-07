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

use om_engine::{MediaRuntime, Session};
use om_gpu::GpuContext;
use om_media_core::{LiveFeed, LiveState};
use om_project::{LiveInput, Project, store};
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
    /// Live inputs: list what is available, or test one.
    Live {
        #[command(subcommand)]
        action: LiveAction,
    },
    /// DMX: find Art-Net nodes, watch incoming DMX, or show the channel
    /// values a project's fixtures send.
    Dmx {
        #[command(subcommand)]
        action: DmxAction,
    },
}

#[derive(Debug, Subcommand)]
enum DmxAction {
    /// Broadcast ArtPoll and list the Art-Net nodes that answer (needs UDP
    /// port 6454 free).
    Discover {
        /// Broadcast address of the lighting network.
        #[arg(long, default_value = "255.255.255.255")]
        broadcast: String,
        #[arg(long, default_value_t = 2)]
        seconds: u64,
    },
    /// Print the Art-Net and sACN universes arriving at this machine.
    Monitor {
        /// sACN universes to join (multicast); Art-Net needs no list.
        #[arg(long)]
        sacn: Vec<u16>,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// Render a project frame and print the DMX its fixtures would send.
    Frame {
        path: PathBuf,
        /// Show time in seconds.
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        /// Print `{"<node>/<universe>": [512 values]}` instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum LiveAction {
    /// List connected cameras and announced senders.
    List,
    /// Receive from an input for a while and report its format and rate.
    Probe {
        /// Camera name, stream URL, or `ndi:NAME`, `syphon:NAME`, `spout:NAME`.
        input: String,
        #[arg(long, default_value_t = 5)]
        seconds: u64,
        /// Save the last frame as a PNG.
        #[arg(long)]
        png: Option<PathBuf>,
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
        Action::Live { action } => live(action),
        Action::Dmx { action } => dmx(action),
    }
}

/// Parses a probe argument into a live input.
fn parse_live(s: &str) -> LiveInput {
    if let Some(n) = s.strip_prefix("ndi:") {
        LiveInput::Ndi { source: n.into() }
    } else if let Some(n) = s.strip_prefix("syphon:") {
        LiveInput::Syphon {
            server: n.into(),
            app: String::new(),
        }
    } else if let Some(n) = s.strip_prefix("spout:") {
        LiveInput::Spout { sender: n.into() }
    } else if s.contains("://") {
        LiveInput::Stream { url: s.into() }
    } else {
        LiveInput::Camera { device: s.into() }
    }
}

fn live(action: LiveAction) -> Result<(), String> {
    let adapters = om_platform::adapters();
    let opener = adapters
        .live
        .ok_or("live inputs are not available in this build")?;
    match action {
        LiveAction::List => {
            let inputs = opener.discover();
            if inputs.is_empty() {
                println!("no cameras or senders found");
            }
            for i in inputs {
                println!("{}", i.label());
            }
            Ok(())
        }
        LiveAction::Probe {
            input,
            seconds,
            png,
        } => {
            let input = parse_live(&input);
            input.validate()?;
            println!("probing {}", input.label());
            let feed = LiveFeed::spawn(opener, input);
            let deadline = Instant::now() + Duration::from_secs(seconds);
            let mut last = None;
            let mut seen = 0;
            while Instant::now() < deadline {
                om_platform::pump_events(Duration::from_millis(50));
                if let Some(f) = feed.newer_than(seen) {
                    seen = f.seq;
                    last = Some(f.image);
                }
            }
            let stats = feed.stats();
            let state = feed.state();
            drop(feed);
            match (&state, &last) {
                (_, Some(img)) => {
                    println!(
                        "{}: {} frames, {:.2} fps, {} reconnects ({}×{})",
                        stats.description,
                        stats.frames,
                        stats.fps,
                        stats.reconnects,
                        img.width(),
                        img.height()
                    );
                    if let Some(path) = png {
                        image::save_buffer(
                            &path,
                            img.rgba8(),
                            img.width(),
                            img.height(),
                            image::ExtendedColorType::Rgba8,
                        )
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                        println!("saved {}", path.display());
                    }
                    Ok(())
                }
                (LiveState::Retrying { error }, None) => Err(format!("no frames: {error}")),
                (_, None) => Err("no frames received".into()),
            }
        }
    }
}

/// Opens a project read-only and prepares a compositor with its media.
/// Opens a project read-only and prepares a compositor plus media runtime.
fn prepare(path: &Path) -> Result<(Project, Compositor, MediaRuntime), String> {
    let project = store::load(path).map_err(|e| e.to_string())?.project;
    let gpu = GpuContext::headless().map_err(|e| e.to_string())?;
    let adapters = om_platform::adapters();
    Ok((
        project,
        Compositor::new(gpu),
        MediaRuntime::new(adapters.video).with_live(adapters.live),
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
) -> om_render::FrameInputs {
    let changes = media.update_blocking(project, dir, t, Duration::from_secs(10));
    for id in &changes.unload {
        compositor.remove_image(*id);
    }
    for path in &changes.shaders_removed {
        compositor.remove_shader(path);
    }
    for (path, compiled) in &changes.shaders {
        compositor.set_shader(path, compiled);
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
    if warn {
        for m in &project.media {
            if let om_project::MediaSource::Shader { path, .. } = &m.source
                && let Some(e) = media.shader_error(path)
            {
                eprintln!("warning: shader {path}: {e}");
            }
        }
    }
    om_render::FrameInputs {
        show_seconds: t.as_seconds_f64(),
        media_seconds: changes.shader_times.iter().copied().collect(),
    }
}

fn render(path: &Path, out: &Path, at: f64) -> Result<(), String> {
    let (project, mut compositor, mut media) = prepare(path)?;
    let t = seconds(at)?;
    let inputs = load_media(
        &project,
        path.parent(),
        &mut compositor,
        &mut media,
        t,
        true,
    );
    let report = compositor
        .render_with(&project, &inputs)
        .map_err(|e| e.to_string())?;
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

fn dmx(action: DmxAction) -> Result<(), String> {
    match action {
        DmxAction::Discover { broadcast, seconds } => {
            let ip: std::net::Ipv4Addr = broadcast
                .parse()
                .map_err(|_| format!("{broadcast:?} is not an IPv4 address"))?;
            let target = std::net::SocketAddr::from((ip, om_dmx::artnet::PORT));
            let nodes = om_dmx::net::discover_artnet(
                om_dmx::artnet::PORT,
                target,
                Duration::from_secs(seconds),
            )
            .map_err(|e| format!("Art-Net discovery: {e} (is another Art-Net program running?)"))?;
            if nodes.is_empty() {
                println!("no Art-Net nodes answered");
            }
            for n in nodes {
                let outs: Vec<String> = n.outputs.iter().map(u16::to_string).collect();
                println!(
                    "{}  {} ({})  outputs: {}",
                    n.ip,
                    n.short_name,
                    n.long_name,
                    outs.join(", ")
                );
            }
            Ok(())
        }
        DmxAction::Monitor { sacn, seconds } => dmx_monitor(&sacn, seconds),
        DmxAction::Frame { path, at, json } => dmx_frame(&path, at, json),
    }
}

fn dmx_monitor(sacn: &[u16], seconds: u64) -> Result<(), String> {
    use std::net::{Ipv4Addr, UdpSocket};
    let mut sockets = Vec::new();
    match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, om_dmx::artnet::PORT)) {
        Ok(s) => sockets.push(s),
        Err(e) => eprintln!("Art-Net: cannot listen on port 6454: {e}"),
    }
    if !sacn.is_empty() {
        let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, om_dmx::sacn::PORT))
            .map_err(|e| format!("sACN: cannot listen on port 5568: {e}"))?;
        for u in sacn {
            s.join_multicast_v4(&om_dmx::sacn::multicast_group(*u), &Ipv4Addr::UNSPECIFIED)
                .map_err(|e| format!("sACN universe {u}: {e}"))?;
        }
        sockets.push(s);
    }
    if sockets.is_empty() {
        return Err("nothing to listen on".into());
    }
    for s in &sockets {
        s.set_nonblocking(true).map_err(|e| e.to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut seen: std::collections::BTreeMap<(&'static str, u16), (u64, Vec<u8>)> =
        std::collections::BTreeMap::new();
    let mut next_print = Instant::now() + Duration::from_secs(1);
    let mut buf = [0u8; 1500];
    while Instant::now() < deadline {
        let mut idle = true;
        for s in &sockets {
            while let Ok((n, _)) = s.recv_from(&mut buf) {
                idle = false;
                if let Some(r) = om_dmx::net::parse(&buf[..n]) {
                    let e = seen.entry((r.protocol, r.universe)).or_default();
                    e.0 += 1;
                    e.1 = r.data;
                }
            }
        }
        if Instant::now() >= next_print {
            for ((protocol, universe), (count, data)) in &seen {
                let head: Vec<String> = data.iter().take(12).map(u8::to_string).collect();
                println!(
                    "{protocol} {universe}: {count} pkt/s  [{} …]",
                    head.join(" ")
                );
            }
            if !seen.is_empty() {
                println!();
            }
            seen.values_mut().for_each(|v| v.0 = 0);
            next_print += Duration::from_secs(1);
        }
        if idle {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

fn dmx_frame(path: &Path, at: f64, json: bool) -> Result<(), String> {
    let (project, mut compositor, mut media) = prepare(path)?;
    let t = seconds(at)?;
    let inputs = load_media(
        &project,
        path.parent(),
        &mut compositor,
        &mut media,
        t,
        true,
    );
    let report = compositor
        .render_with(&project, &inputs)
        .map_err(|e| e.to_string())?;
    let pixels = compositor.read_rgba8().map_err(|e| e.to_string())?;
    let (w, h) = report.canvas;
    let frame = om_dmx::mapping::FrameView {
        width: w,
        height: h,
        rgba8: &pixels,
    };
    let plans: Vec<_> = project
        .dmx
        .fixtures
        .iter()
        .filter(|f| f.enabled)
        .map(om_dmx::mapping::FixturePlan::new)
        .collect();
    let universes =
        om_dmx::mapping::render_fixtures(&plans, &om_dmx::mapping::Sampler::default(), &frame);
    let node_name = |id| {
        project
            .dmx
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map_or_else(|| id.to_string(), |n| n.name.clone())
    };
    if json {
        let map: serde_json::Map<String, serde_json::Value> = universes
            .iter()
            .map(|((node, u), data)| {
                (
                    format!("{}/{u}", node_name(*node)),
                    serde_json::Value::from(data.to_vec()),
                )
            })
            .collect();
        println!("{}", serde_json::Value::Object(map));
    } else {
        if universes.is_empty() {
            println!("no enabled fixtures");
        }
        for ((node, u), data) in &universes {
            let used = data.iter().rposition(|&v| v != 0).map_or(0, |i| i + 1);
            let values: Vec<String> = data[..used].iter().map(u8::to_string).collect();
            println!("{} universe {u}: {}", node_name(*node), values.join(" "));
        }
    }
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
        let inputs = load_media(
            &project,
            path.parent(),
            &mut compositor,
            &mut media,
            t,
            frames == 0,
        );
        compositor
            .render_with(&project, &inputs)
            .map_err(|e| e.to_string())?;
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
