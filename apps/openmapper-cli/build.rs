// SPDX-License-Identifier: Apache-2.0
//! Release builds (`cargo xtask dist`) set `OM_BUNDLED_FFMPEG_RPATH=1` so the
//! executable finds the bundled FFmpeg libraries in `lib/` next to itself,
//! wherever the archive is unpacked (docs/media/ffmpeg.md). Development
//! builds are unaffected.

fn main() {
    println!("cargo:rerun-if-env-changed=OM_BUNDLED_FFMPEG_RPATH");
    if std::env::var_os("OM_BUNDLED_FFMPEG_RPATH").is_none() {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        // DT_RPATH (not RUNPATH) also applies to the libraries' own
        // dependencies (libavcodec → libavutil), so no patching is needed.
        Ok("linux") => {
            println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/lib");
            println!("cargo:rustc-link-arg-bins=-Wl,--disable-new-dtags");
        }
        // The libraries are built with @rpath install names.
        Ok("macos") => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/lib"),
        // Windows searches the executable's own directory first.
        _ => {}
    }
}
