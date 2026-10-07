// SPDX-License-Identifier: Apache-2.0
//! OpenMapper desktop application. Usage: `openmapper [project.omproj]`

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OpenMapper")
            .with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let result = eframe::run_native(
        "OpenMapper",
        options,
        Box::new(|_cc| Ok(Box::new(om_ui_egui::OpenMapperApp::new(path)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("openmapper: could not start the user interface: {e}");
            ExitCode::FAILURE
        }
    }
}
