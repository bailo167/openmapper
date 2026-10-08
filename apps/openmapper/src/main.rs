// SPDX-License-Identifier: Apache-2.0
//! OpenMapper desktop application. Usage: `openmapper [project.omproj] [--play]`
//! (`--play` starts the show immediately and shows the first enabled output
//! fullscreen in the main window, for unattended shows).

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, wgpu};

const USAGE: &str = "\
Usage: openmapper [PROJECT.omproj] [--play]

Opens the OpenMapper desktop app, optionally with a project.

Options:
  --play         Start the show: play, and show the first enabled output
                 fullscreen on its display (Escape returns to the editor)
  -h, --help     Print this help
  -V, --version  Print the version

The command-line tool is openmapper-cli (openmapper-cli --help).";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
enum Args {
    Run { path: Option<PathBuf>, play: bool },
    Help,
    Version,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut path = None;
    let mut play = false;
    for a in args {
        match a.to_str() {
            Some("--play") => play = true,
            Some("-h" | "--help") => return Ok(Args::Help),
            Some("-V" | "--version") => return Ok(Args::Version),
            Some(s) if s.starts_with('-') && s.len() > 1 => {
                return Err(format!("unknown option {s}"));
            }
            _ if path.is_some() => return Err("only one project can be opened".into()),
            _ => path = Some(PathBuf::from(a)),
        }
    }
    Ok(Args::Run { path, play })
}

fn main() -> ExitCode {
    let (path, autoplay) = match parse_args(std::env::args_os().skip(1)) {
        Ok(Args::Run { path, play }) => (path, play),
        Ok(Args::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Args::Version) => {
            println!("openmapper {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("openmapper: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Ask for the adapter's full limits (eframe defaults to conservative
    // ones), so large canvases and media fit on capable GPUs.
    let mut wgpu_options = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.device_descriptor = Arc::new(|adapter| wgpu::DeviceDescriptor {
            label: Some("openmapper"),
            required_limits: adapter.limits(),
            ..Default::default()
        });
    }
    let options = eframe::NativeOptions {
        wgpu_options,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OpenMapper")
            .with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let result = eframe::run_native(
        "OpenMapper",
        options,
        Box::new(|cc| {
            let adapters = om_platform::adapters();
            let mut app = om_ui_egui::OpenMapperApp::new(cc, path, &adapters);
            if autoplay {
                app.run_show();
            }
            Ok(Box::new(app))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("openmapper: could not start the user interface: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(OsString::from))
    }

    /// Regression: `openmapper --help` opened the app (and tried to open a
    /// project called "--help") instead of printing usage.
    #[test]
    fn help_and_version_do_not_start_the_app() {
        assert_eq!(parse(&["--help"]), Ok(Args::Help));
        assert_eq!(parse(&["show.omproj", "-h"]), Ok(Args::Help));
        assert_eq!(parse(&["--version"]), Ok(Args::Version));
        assert_eq!(parse(&["-V"]), Ok(Args::Version));
    }

    #[test]
    fn project_and_play_in_any_order() {
        let want = Ok(Args::Run {
            path: Some(PathBuf::from("show.omproj")),
            play: true,
        });
        assert_eq!(parse(&["show.omproj", "--play"]), want);
        assert_eq!(parse(&["--play", "show.omproj"]), want);
        assert_eq!(
            parse(&[]),
            Ok(Args::Run {
                path: None,
                play: false
            })
        );
    }

    #[test]
    fn unknown_options_and_extra_paths_are_errors() {
        assert!(parse(&["--fullscreen"]).is_err());
        assert!(parse(&["a.omproj", "b.omproj"]).is_err());
        // A lone "-" is a path, not an option.
        assert!(parse(&["-"]).is_ok());
    }
}
