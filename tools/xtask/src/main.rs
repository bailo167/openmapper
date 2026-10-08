// SPDX-License-Identifier: Apache-2.0
//! Repository automation. `cargo xtask ci` is the single quality gate used
//! locally and in GitHub Actions.

mod arch;
mod dist;
mod provenance;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};

const USAGE: &str = "\
usage: cargo xtask <task>

tasks:
  ci           fmt + clippy + test + deny + provenance + arch (the full gate)
  fmt          check formatting
  clippy       lint with warnings as errors
  test         run all workspace tests
  deny         cargo-deny licence/advisory/source checks
  provenance   clean-room and licence-header scan of repository files
               (--history: every blob in every ref)
  arch         crate layering and forbidden-dependency check
  notices [OUT] third-party licences of the release binaries
  dist         release build with the bundled FFmpeg (FFMPEG_DIR, default
               target/ffmpeg from tools/ffmpeg/build.sh) + archive + SHA256SUMS
  checksums F… SHA-256 lines for files
  smoke ARCHIVE unpack an archive and exercise the CLI and bundled FFmpeg from it";

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "fmt" => fmt(),
        "clippy" => clippy(),
        "test" => test(),
        "deny" => deny(),
        "provenance" if std::env::args().nth(2).as_deref() == Some("--history") => {
            provenance::run_history(&root())
        }
        "provenance" => provenance::run(&root()),
        "arch" => arch::run(&root()),
        "notices" => {
            let out = std::env::args().nth(2).map_or_else(
                || root().join("target/dist/THIRD_PARTY_LICENSES.txt"),
                PathBuf::from,
            );
            dist::notices(&root(), &out, None)
        }
        "dist" => dist::dist(&root()).map(|_| ()),
        "checksums" => {
            let files: Vec<PathBuf> = std::env::args().skip(2).map(PathBuf::from).collect();
            dist::checksum_lines(&files).map(|s| print!("{s}"))
        }
        "smoke" => match std::env::args().nth(2) {
            Some(a) => dist::smoke(&root(), Path::new(&a)),
            None => Err(anyhow::anyhow!("usage: cargo xtask smoke ARCHIVE")),
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask {task}: FAILED: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

type Step = fn() -> Result<()>;

fn ci() -> Result<()> {
    let steps: [(&str, Step); 6] = [
        ("fmt", fmt),
        ("arch", || arch::run(&root())),
        ("provenance", || provenance::run(&root())),
        ("clippy", clippy),
        ("test", test),
        ("deny", deny),
    ];
    for (name, step) in steps {
        eprintln!("==> {name}");
        step().with_context(|| format!("step `{name}`"))?;
    }
    eprintln!("==> ci: all checks passed");
    Ok(())
}

fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .args(args)
        .current_dir(root())
        .status()
        .with_context(|| format!("running {cargo} {}", args.join(" ")))?;
    if !status.success() {
        bail!("`cargo {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

fn fmt() -> Result<()> {
    cargo(&["fmt", "--all", "--check"])
}

fn clippy() -> Result<()> {
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])
}

fn test() -> Result<()> {
    cargo(&["test", "--workspace", "--locked"])
}

fn deny() -> Result<()> {
    let has_deny = Command::new("cargo-deny")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_deny {
        bail!("cargo-deny is not installed (install: `cargo install --locked cargo-deny`)");
    }
    cargo(&["deny", "--locked", "check"])
}
