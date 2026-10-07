// SPDX-License-Identifier: Apache-2.0
//! Repository automation. `cargo xtask ci` is the single quality gate used
//! locally and in GitHub Actions.

mod arch;
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
  arch         crate layering and forbidden-dependency check";

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "fmt" => fmt(),
        "clippy" => clippy(),
        "test" => test(),
        "deny" => deny(),
        "provenance" => provenance::run(&root()),
        "arch" => arch::run(&root()),
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
