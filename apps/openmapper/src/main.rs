// SPDX-License-Identifier: Apache-2.0
//! OpenMapper desktop application. Usage: `openmapper [project.omproj] [--play]`
//! (`--play` starts the show transport immediately, e.g. for unattended shows).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, wgpu};

fn main() -> ExitCode {
    // Usage: openmapper [project.omproj] [--play]
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let autoplay = args.iter().any(|a| a == "--play");
    let path = args.into_iter().find(|a| a != "--play").map(PathBuf::from);
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
            let opener: Arc<dyn om_media_core::VideoOpener> =
                Arc::new(om_media_ffmpeg::FfmpegOpener);
            let mut app = om_ui_egui::OpenMapperApp::new(cc, path, Some(opener));
            if autoplay {
                app.play();
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
