// SPDX-License-Identifier: Apache-2.0
//! Release packaging: third-party notices, a portable archive per platform,
//! SHA-256 checksums, and a smoke test of the unpacked archive.
//!
//! - `cargo xtask notices [OUT]` — licences of every Rust dependency linked
//!   into the release binaries, with their licence texts.
//! - `cargo xtask dist` — release build, then
//!   `target/dist/openmapper-<version>-<os>-<arch>.tar.gz` (binaries,
//!   LICENSE, NOTICE, THIRD_PARTY.yml, THIRD_PARTY_LICENSES.txt, user guide)
//!   and `target/dist/SHA256SUMS`.
//! - `cargo xtask checksums FILE…` — SHA-256 lines for any files.
//! - `cargo xtask smoke ARCHIVE` — unpacks into a fresh directory and runs
//!   the CLI from there (version, new, validate, apply, inspect).
//!
//! Signing the checksum file needs the release key and is done by a person
//! (docs/release/checklist.md).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Binaries shipped in the archive.
const BINARIES: &[(&str, &str)] = &[
    ("openmapper", "openmapper"),
    ("openmapper-cli", "openmapper-cli"),
];

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Reference documents shipped under `docs/` (the user guide links them).
const REFERENCE_DOCS: &[&str] = &[
    "calibration.md",
    "control.md",
    "dmx.md",
    "effects.md",
    "isf.md",
    "live-io.md",
    "plugins.md",
    "project-format.md",
    "media/ffmpeg.md",
    "media/playback.md",
];

/// Vendored non-crate sources compiled into the binaries, with their
/// licence files (macOS builds include Syphon).
const VENDORED: &[(&str, &str)] = &[(
    "Syphon Framework (macOS builds)",
    "crates/om-syphon/vendor/syphon/License.txt",
)];

/// Texts for crates that publish no licence file (Apache-2.0 is LICENSE).
const STANDARD_TEXTS: &[(&str, &str)] = &[
    ("MIT", include_str!("../licences/MIT.txt")),
    ("Zlib", include_str!("../licences/Zlib.txt")),
    ("BSL-1.0", include_str!("../licences/BSL-1.0.txt")),
    ("WTFPL", include_str!("../licences/WTFPL.txt")),
    (
        "Apache-2.0 WITH LLVM-exception (the exception; the licence is LICENSE)",
        include_str!("../licences/LLVM-exception.txt"),
    ),
];

/// Licence and notice files in a crate's folder and its direct
/// subfolders (fonts and vendored code keep theirs next to the files).
fn licence_files(dir: &Path) -> Vec<PathBuf> {
    let is_licence = |f: &Path| {
        let n = f
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_default();
        let in_fonts = f
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|d| d.eq_ignore_ascii_case("fonts"));
        f.is_file()
            && ([
                "LICENSE",
                "LICENCE",
                "COPYING",
                "NOTICE",
                "COPYRIGHT",
                "UNLICENSE",
            ]
            .iter()
            .any(|k| n.contains(k))
                || matches!(n.as_str(), "OFL.TXT" | "UFL.TXT")
                || (in_fonts && n.ends_with(".TXT")))
    };
    let list = |d: &Path| -> Vec<PathBuf> {
        fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect()
    };
    let mut out = Vec::new();
    for entry in list(dir) {
        if entry.is_dir() {
            let skip = entry.file_name().is_some_and(|n| {
                matches!(n.to_str(), Some("src" | "target" | "tests" | "benches"))
            });
            if !skip {
                out.extend(list(&entry).into_iter().filter(|f| is_licence(f)));
            }
        } else if is_licence(&entry) {
            out.push(entry);
        }
    }
    out.sort();
    out
}

/// Writes the third-party licence file for the release binaries.
pub fn notices(root: &Path, out: &Path) -> Result<()> {
    let meta = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()
        .context("cargo metadata")?;
    let resolve = meta.resolve.as_ref().context("no dependency graph")?;
    let packages: BTreeMap<_, _> = meta.packages.iter().map(|p| (&p.id, p)).collect();
    let nodes: BTreeMap<_, _> = resolve.nodes.iter().map(|n| (&n.id, n)).collect();
    // Everything reachable from the shipped binaries through normal
    // (non-dev, non-build) dependencies.
    let mut stack: Vec<&cargo_metadata::PackageId> = meta
        .workspace_packages()
        .into_iter()
        .filter(|p| BINARIES.iter().any(|(pkg, _)| p.name.as_str() == *pkg))
        .map(|p| &p.id)
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(node) = nodes.get(id) {
            for dep in &node.deps {
                let normal = dep
                    .dep_kinds
                    .iter()
                    .any(|k| k.kind == cargo_metadata::DependencyKind::Normal);
                if normal {
                    stack.push(&dep.pkg);
                }
            }
        }
    }
    let members: std::collections::BTreeSet<_> = meta.workspace_members.iter().collect();
    let mut text = String::from(
        "Third-party software in OpenMapper binaries\n\
         ==========================================\n\n\
         OpenMapper is licensed under Apache-2.0 (see LICENSE and NOTICE). The\n\
         binaries also contain the Rust crates listed below, under the licences\n\
         shown, followed by each crate's licence and notice files. Material\n\
         that is not a Rust crate (vendored sources, FFmpeg and other runtime\n\
         libraries) is listed in THIRD_PARTY.yml and docs/release/.\n\n",
    );
    let mut deps: Vec<&cargo_metadata::Package> = seen
        .iter()
        .filter(|id| !members.contains(id))
        .filter_map(|id| packages.get(id).copied())
        .collect();
    deps.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    let mut missing = Vec::new();
    for p in &deps {
        let licence = p
            .license
            .clone()
            .unwrap_or_else(|| "(see licence file)".into());
        let _ = writeln!(text, "{} {} — {licence}", p.name, p.version);
    }
    for p in &deps {
        let _ = writeln!(text, "\n\n==== {} {} ====", p.name, p.version);
        if let Some(repo) = &p.repository {
            let _ = writeln!(text, "{repo}");
        }
        let dir = p.manifest_path.parent().map(|d| d.as_std_path().to_owned());
        let files = dir.as_deref().map(licence_files).unwrap_or_default();
        if files.is_empty() {
            missing.push(format!("{} {}", p.name, p.version));
            let _ = writeln!(
                text,
                "The published crate has no licence file.\nCopyright (c) {}\n\
                 Licence: {} — standard texts at the end of this file \
                 (Apache-2.0: see LICENSE).",
                if p.authors.is_empty() {
                    "the authors".to_owned()
                } else {
                    p.authors.join(", ")
                },
                p.license.as_deref().unwrap_or("unknown")
            );
        }
        for f in files {
            let body = fs::read_to_string(&f).unwrap_or_default();
            let name = dir
                .as_deref()
                .and_then(|d| f.strip_prefix(d).ok())
                .map(|r| r.to_string_lossy().into_owned());
            let _ = writeln!(
                text,
                "\n--- {} ---\n{}",
                name.unwrap_or_default(),
                body.trim_end()
            );
        }
    }
    text.push_str("\n\n==== Vendored sources (THIRD_PARTY.yml) ====\n");
    for (what, file) in VENDORED {
        let body =
            fs::read_to_string(root.join(file)).with_context(|| format!("reading {file}"))?;
        let _ = writeln!(text, "\n--- {what} ({file}) ---\n{}", body.trim_end());
    }
    text.push_str("\n\n==== Standard licence texts ====\n");
    for (id, body) in STANDARD_TEXTS {
        let _ = writeln!(text, "\n--- {id} ---\n{}", body.trim_end());
    }
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(out, text).with_context(|| format!("writing {}", out.display()))?;
    eprintln!(
        "  notices: {} crates -> {} ({} without a licence file: {})",
        deps.len(),
        out.display(),
        missing.len(),
        missing.join(", ")
    );
    Ok(())
}

/// `<hex sha256>  <file name>` for each file.
pub fn checksum_lines(files: &[PathBuf]) -> Result<String> {
    let mut out = String::new();
    for f in files {
        let bytes = fs::read(f).with_context(|| format!("reading {}", f.display()))?;
        let digest = Sha256::digest(&bytes);
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .context("file name")?;
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(out, "{hex}  {name}");
    }
    Ok(out)
}

fn version(root: &Path) -> Result<String> {
    let meta = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()?;
    meta.packages
        .iter()
        .find(|p| p.name.as_str() == "openmapper")
        .map(|p| p.version.to_string())
        .context("openmapper package")
}

/// `tar` (on Windows the system's bsdtar: Git's GNU tar, often first on
/// `PATH`, reads `C:\…` as a remote host).
fn tar() -> Command {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let sys = PathBuf::from(root).join("System32").join("tar.exe");
        if sys.is_file() {
            return Command::new(sys);
        }
    }
    Command::new("tar")
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed ({status})");
    }
    Ok(())
}

/// Builds the release archive and checksums; returns the archive path.
pub fn dist(root: &Path) -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build
        .args(["build", "--profile", "distribution", "--locked"])
        .current_dir(root);
    for (pkg, _) in BINARIES {
        build.args(["-p", pkg]);
    }
    run(&mut build)?;
    let name = format!(
        "openmapper-{}-{}-{}",
        version(root)?,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let dist = root.join("target/dist");
    let stage = dist.join(&name);
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir_all(&stage)?;
    for (_, bin) in BINARIES {
        let from = root.join("target/distribution").join(exe(bin));
        fs::copy(&from, stage.join(exe(bin)))
            .with_context(|| format!("copying {}", from.display()))?;
    }
    for f in ["LICENSE", "NOTICE", "THIRD_PARTY.yml"] {
        fs::copy(root.join(f), stage.join(f))?;
    }
    fs::copy(root.join("docs/user-guide.md"), stage.join("USER-GUIDE.md"))?;
    fs::copy(root.join("SECURITY.md"), stage.join("SECURITY.md"))?;
    fs::create_dir_all(stage.join("docs/media"))?;
    for doc in REFERENCE_DOCS {
        fs::copy(root.join("docs").join(doc), stage.join("docs").join(doc))
            .with_context(|| format!("copying docs/{doc}"))?;
    }
    notices(root, &stage.join("THIRD_PARTY_LICENSES.txt"))?;
    let archive = dist.join(format!("{name}.tar.gz"));
    run(tar()
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&dist)
        .arg(&name))?;
    let sums = checksum_lines(std::slice::from_ref(&archive))?;
    fs::write(dist.join("SHA256SUMS"), &sums)?;
    eprintln!("  dist: {}\n  {}", archive.display(), sums.trim_end());
    Ok(archive)
}

/// Unpacks `archive` into a fresh directory and exercises the CLI there.
pub fn smoke(archive: &Path) -> Result<()> {
    let sums = archive.with_file_name("SHA256SUMS");
    if sums.is_file() {
        let want = fs::read_to_string(&sums)?;
        let got = checksum_lines(&[archive.to_owned()])?;
        if !want.lines().any(|l| l == got.trim_end()) {
            bail!("{} does not match SHA256SUMS", archive.display());
        }
    }
    let dir = std::env::temp_dir().join(format!("openmapper-smoke-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    run(tar().arg("-xzf").arg(archive).arg("-C").arg(&dir))?;
    let top = fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .context("archive has no top-level folder")?;
    for f in [
        "LICENSE",
        "NOTICE",
        "THIRD_PARTY.yml",
        "THIRD_PARTY_LICENSES.txt",
        "USER-GUIDE.md",
    ] {
        if !top.join(f).is_file() {
            bail!("archive lacks {f}");
        }
    }
    let cli = top.join(exe("openmapper-cli"));
    if !top.join(exe("openmapper")).is_file() {
        bail!("archive lacks the desktop app");
    }
    let project = dir.join("smoke.omproj");
    let commands = dir.join("commands.json");
    fs::write(
        &commands,
        r#"[{"type":"set_canvas","canvas":{"width":640,"height":360}}]"#,
    )?;
    let p = project.to_string_lossy().into_owned();
    let c = commands.to_string_lossy().into_owned();
    for args in [
        vec!["--version"],
        vec!["new", &p, "--name", "Smoke"],
        vec!["validate", &p],
        vec!["apply", &p, &c],
        vec!["inspect", &p],
    ] {
        eprintln!("  smoke: openmapper-cli {}", args.join(" "));
        run(Command::new(&cli).args(&args).current_dir(&dir))?;
    }
    let _ = fs::remove_dir_all(&dir);
    eprintln!("  smoke: ok");
    Ok(())
}
