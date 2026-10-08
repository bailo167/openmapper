// SPDX-License-Identifier: Apache-2.0
//! Release packaging: third-party notices, a portable archive per platform,
//! SHA-256 checksums, and a smoke test of the unpacked archive.
//!
//! - `cargo xtask notices [OUT]` — licences of every Rust dependency linked
//!   into the release binaries, with their licence texts.
//! - `cargo xtask dist` — release build against the bundled FFmpeg
//!   (`FFMPEG_DIR`, default `target/ffmpeg`, built by
//!   `tools/ffmpeg/build.sh`), then
//!   `target/dist/openmapper-<version>-<os>-<arch>.tar.gz` (binaries, the
//!   FFmpeg shared libraries with their licence, source notice and build
//!   recipe, LICENSE, NOTICE, THIRD_PARTY.yml, THIRD_PARTY_LICENSES.txt,
//!   user guide) and `target/dist/SHA256SUMS`. The staged binaries must
//!   load the bundled libraries and pass the FFmpeg release policy
//!   (`openmapper-cli ffmpeg --require-release`, D-031) before the archive
//!   is written. `OM_SIGN_COMMAND="prog args…"` runs a code-signing command
//!   on the staging directory (its path is appended) before archiving.
//! - `cargo xtask checksums FILE…` — SHA-256 lines for any files.
//! - `cargo xtask smoke ARCHIVE` — unpacks into a fresh directory and runs
//!   the CLI from there (version, new, validate, apply, inspect), checks
//!   that the bundled FFmpeg is what loads, and decodes and renders a
//!   generated video when a GPU is available.
//!
//! Signing `SHA256SUMS` happens in the release workflow (keyless Sigstore;
//! docs/release/signing.md).

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

/// FFmpeg shared libraries bundled with every release (D-031), in `lib/`
/// next to the binaries (Linux, macOS) or beside them (Windows).
const FFMPEG_LIBS: &[&str] = &[
    "avutil",
    "swresample",
    "swscale",
    "avcodec",
    "avformat",
    "avdevice",
];

/// Compliance record written by `tools/ffmpeg/build.sh` into the prefix;
/// shipped as `ffmpeg/` in the archive.
const FFMPEG_RECORD: &str = "share/openmapper-ffmpeg";

/// Where `tools/ffmpeg/build.sh` installed FFmpeg: `FFMPEG_DIR`, else
/// `target/ffmpeg`.
fn ffmpeg_prefix(root: &Path) -> Result<PathBuf> {
    let dir =
        std::env::var_os("FFMPEG_DIR").map_or_else(|| root.join("target/ffmpeg"), PathBuf::from);
    if !dir.join(FFMPEG_RECORD).join("SOURCE.txt").is_file() {
        bail!(
            "{} does not contain an FFmpeg built by tools/ffmpeg/build.sh \
             (run `tools/ffmpeg/build.sh`, or set FFMPEG_DIR to its prefix)",
            dir.display()
        );
    }
    Ok(dir)
}

/// The shared-library files (`libavcodec.so.63`, `libavcodec.63.dylib`,
/// `avcodec-63.dll`) of each bundled library in `prefix`.
fn ffmpeg_library_files(prefix: &Path) -> Result<Vec<PathBuf>> {
    let (dir, is_match): (&str, fn(&str, &str) -> bool) = match std::env::consts::OS {
        "linux" => ("lib", |f, lib| {
            f.strip_prefix(&format!("lib{lib}.so."))
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        }),
        "macos" => ("lib", |f, lib| {
            f.strip_prefix(&format!("lib{lib}."))
                .and_then(|v| v.strip_suffix(".dylib"))
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        }),
        "windows" => ("bin", |f, lib| {
            f.strip_prefix(&format!("{lib}-"))
                .and_then(|v| v.strip_suffix(".dll"))
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        }),
        other => bail!("no FFmpeg bundling rule for {other}"),
    };
    let entries: Vec<PathBuf> = fs::read_dir(prefix.join(dir))
        .with_context(|| format!("reading {}", prefix.join(dir).display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    let mut out = Vec::new();
    for lib in FFMPEG_LIBS {
        let found: Vec<&PathBuf> = entries
            .iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| is_match(n, lib))
            })
            .collect();
        match found.as_slice() {
            [one] => out.push((*one).clone()),
            [] => bail!("{lib}: no shared library in {}", prefix.join(dir).display()),
            many => bail!("{lib}: several candidates: {many:?}"),
        }
    }
    Ok(out)
}

/// Where the bundled libraries live relative to the binaries.
fn bundled_lib_dir(top: &Path) -> PathBuf {
    if cfg!(windows) {
        top.to_owned()
    } else {
        top.join("lib")
    }
}

/// Environment for running fixturegen (not shipped, so no rpath) against the
/// prefix's libraries.
fn with_prefix_libraries(cmd: &mut Command, prefix: &Path) {
    let (var, dir) = match std::env::consts::OS {
        "macos" => ("DYLD_LIBRARY_PATH", prefix.join("lib")),
        "windows" => ("PATH", prefix.join("bin")),
        _ => ("LD_LIBRARY_PATH", prefix.join("lib")),
    };
    let mut paths = vec![dir];
    paths.extend(
        std::env::var_os(var)
            .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env(var, joined);
    }
}

/// Runs `openmapper-cli ffmpeg --require-release` from `top` with the
/// library search variables cleared, so only the bundled libraries can
/// satisfy the binary, and checks that they are the ones built by the
/// recipe (version marker).
fn verify_bundled_ffmpeg(top: &Path) -> Result<()> {
    let cli = top.join(exe("openmapper-cli"));
    let out = Command::new(&cli)
        .args(["ffmpeg", "--require-release"])
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .current_dir(top)
        .output()
        .with_context(|| format!("running {}", cli.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!(
            "bundled FFmpeg failed the release policy ({}):\n{stdout}{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let version = stdout
        .lines()
        .find_map(|l| l.strip_prefix("version:"))
        .map(str::trim)
        .unwrap_or_default();
    if !version.contains("openmapper") {
        bail!(
            "the binaries loaded an FFmpeg that is not the bundled build (version {version:?}). \
             Another FFmpeg's development libraries were on the linker's search path and won \
             the link; build the distribution on a machine without FFmpeg development packages \
             (as package.yml does) or remove them first"
        );
    }
    eprintln!("  ffmpeg: {version}, release policy ok");
    Ok(())
}

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
    "release/signing.md",
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
/// `ffmpeg_record` is the bundled FFmpeg's compliance folder (SOURCE.txt,
/// LICENSE.md), when FFmpeg is bundled.
pub fn notices(root: &Path, out: &Path, ffmpeg_record: Option<&Path>) -> Result<()> {
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
         that is not a Rust crate (vendored sources, the bundled FFmpeg shared\n\
         libraries) follows at the end; see also THIRD_PARTY.yml and ffmpeg/.\n\n\
         NDI® is a registered trademark of Vizrt NDI AB. OpenMapper is not\n\
         affiliated with or endorsed by Vizrt; it uses an NDI runtime the user\n\
         installs separately.\n\n",
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
    if let Some(record) = ffmpeg_record {
        text.push_str("\n\n==== FFmpeg (bundled shared libraries) ====\n");
        text.push_str(
            "FFmpeg is licensed under the GNU Lesser General Public License v2.1 or\n\
             later; the full text is ffmpeg/COPYING.LGPLv2.1 next to this file. The\n\
             libraries are dynamically linked and may be replaced; the exact source\n\
             and build recipe are recorded below (ffmpeg/SOURCE.txt).\n",
        );
        for f in ["SOURCE.txt", "LICENSE.md"] {
            let body = fs::read_to_string(record.join(f))
                .with_context(|| format!("reading {}", record.join(f).display()))?;
            let _ = writeln!(text, "\n--- ffmpeg/{f} ---\n{}", body.trim_end());
        }
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
    let ffmpeg = ffmpeg_prefix(root)?;
    let libs = ffmpeg_library_files(&ffmpeg)?;
    let record = ffmpeg.join(FFMPEG_RECORD);
    eprintln!("  dist: bundling FFmpeg from {}", ffmpeg.display());
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build
        .args(["build", "--profile", "distribution", "--locked"])
        // Link against the bundled libraries and find them in lib/ at run time.
        .env("FFMPEG_DIR", &ffmpeg)
        .env("OM_BUNDLED_FFMPEG_RPATH", "1")
        .current_dir(root);
    for (pkg, _) in BINARIES {
        build.args(["-p", pkg]);
    }
    // Not shipped: generates the video the smoke test decodes.
    build.args(["-p", "fixturegen"]);
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
    // FFmpeg: the libraries (symlinks resolved), then the compliance record
    // as ffmpeg/ (licence text, source notice, build recipe).
    let lib_dir = bundled_lib_dir(&stage);
    fs::create_dir_all(&lib_dir)?;
    for lib in &libs {
        let name = lib.file_name().context("library file name")?;
        fs::copy(lib, lib_dir.join(name)).with_context(|| format!("copying {}", lib.display()))?;
    }
    fs::create_dir_all(stage.join("ffmpeg"))?;
    for entry in fs::read_dir(&record)?.flatten() {
        let p = entry.path();
        let is_source_archive = p.extension().is_some_and(|e| e == "gz");
        if p.is_file() && !is_source_archive {
            fs::copy(&p, stage.join("ffmpeg").join(entry.file_name()))?;
        }
    }
    for f in ["LICENSE", "NOTICE", "THIRD_PARTY.yml"] {
        fs::copy(root.join(f), stage.join(f))?;
    }
    fs::copy(root.join("docs/user-guide.md"), stage.join("USER-GUIDE.md"))?;
    fs::copy(root.join("SECURITY.md"), stage.join("SECURITY.md"))?;
    for doc in REFERENCE_DOCS {
        let to = stage.join("docs").join(doc);
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::copy(root.join("docs").join(doc), &to)
            .with_context(|| format!("copying docs/{doc}"))?;
    }
    notices(root, &stage.join("THIRD_PARTY_LICENSES.txt"), Some(&record))?;
    // Prove the staged binaries run on the bundled libraries alone and
    // that those satisfy the release policy, before anything is archived.
    verify_bundled_ffmpeg(&stage)?;
    if let Some(cmd) = std::env::var_os("OM_SIGN_COMMAND") {
        let cmd = cmd.to_string_lossy().into_owned();
        let mut parts = cmd.split_whitespace();
        let prog = parts.next().context("OM_SIGN_COMMAND is empty")?;
        eprintln!("  dist: signing with `{cmd}`");
        run(Command::new(prog).args(parts).arg(&stage).current_dir(root))?;
    }
    let archive = dist.join(format!("{name}.tar.gz"));
    run(tar()
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&dist)
        .arg(&name))?;
    // The complete corresponding FFmpeg source travels with the release
    // (LGPL-2.1 §6d), outside the binary archive.
    let mut summed = vec![archive.clone()];
    for entry in fs::read_dir(&record)?.flatten() {
        let p = entry.path();
        if p.is_file() && p.extension().is_some_and(|e| e == "gz") {
            let to = dist.join(entry.file_name());
            fs::copy(&p, &to)?;
            summed.push(to);
        }
    }
    let sums = checksum_lines(&summed)?;
    fs::write(dist.join("SHA256SUMS"), &sums)?;
    eprintln!("  dist: {}\n  {}", archive.display(), sums.trim_end());
    Ok(archive)
}

/// Unpacks `archive` into a fresh directory and exercises the CLI there:
/// project commands, the bundled FFmpeg, and (with a GPU, or always when
/// `OM_REQUIRE_GPU=1`) decoding a generated video into a rendered frame.
pub fn smoke(root: &Path, archive: &Path) -> Result<()> {
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
        "ffmpeg/SOURCE.txt",
        "ffmpeg/COPYING.LGPLv2.1",
        "ffmpeg/LICENSE.md",
        "ffmpeg/build.sh",
        "ffmpeg/components.txt",
    ] {
        if !top.join(f).is_file() {
            bail!("archive lacks {f}");
        }
    }
    let cli = top.join(exe("openmapper-cli"));
    if !top.join(exe("openmapper")).is_file() {
        bail!("archive lacks the desktop app");
    }
    let shipped = fs::read_dir(bundled_lib_dir(&top))?
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "dll" || x == "dylib")
                || e.file_name().to_string_lossy().contains(".so.")
        })
        .count();
    if shipped != FFMPEG_LIBS.len() {
        bail!(
            "archive has {shipped} FFmpeg libraries, expected {}",
            FFMPEG_LIBS.len()
        );
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
    verify_bundled_ffmpeg(&top)?;
    smoke_video(root, &top, &dir)?;
    let _ = fs::remove_dir_all(&dir);
    eprintln!("  smoke: ok");
    Ok(())
}

/// Decodes a generated FFV1/Matroska clip (both in the bundled component
/// list) through the unpacked binaries into a PNG, and checks the frame is
/// not blank. Skips without a GPU unless `OM_REQUIRE_GPU=1`.
fn smoke_video(root: &Path, top: &Path, dir: &Path) -> Result<()> {
    let fixturegen = root.join("target/distribution").join(exe("fixturegen"));
    if !fixturegen.is_file() {
        eprintln!("  smoke: no fixturegen build; skipping the video render");
        return Ok(());
    }
    let prefix = ffmpeg_prefix(root)?;
    let clip = dir.join("smoke.mkv");
    let mut generate = Command::new(&fixturegen);
    generate
        .arg(&clip)
        .args([
            "--codec", "ffv1", "--frames", "30", "--rate", "30", "--size", "160x96",
        ])
        .args(["--audio", "48000"]);
    with_prefix_libraries(&mut generate, &prefix);
    run(&mut generate)?;
    let cli = top.join(exe("openmapper-cli"));
    let project = dir.join("video.omproj");
    run(Command::new(&cli).arg("new").arg(&project).current_dir(dir))?;
    let commands = dir.join("video.jsonl");
    fs::write(
        &commands,
        [
            r#"{"type":"set_canvas","canvas":{"width":160,"height":96}}"#,
            r#"{"type":"add_media","media":{"id":"00000000000000000000000010","name":"clip","source":{"kind":"video","path":"smoke.mkv"},"playback":{"looping":true,"speed":{"num":1,"den":1}}}}"#,
            r#"{"type":"add_surface","surface":{"id":"00000000000000000000000001","name":"full","shape":{"kind":"quad","corners":[[0,0],[1,0],[1,1],[0,1]],"uv":[[0,0],[1,0],[1,1],[0,1]]},"media":"00000000000000000000000010"}}"#,
        ]
        .join("\n"),
    )?;
    run(Command::new(&cli)
        .arg("apply")
        .arg(&project)
        .arg(&commands)
        .current_dir(dir))?;
    let png = dir.join("frame.png");
    let out = Command::new(&cli)
        .arg("render")
        .arg(&project)
        .arg("-o")
        .arg(&png)
        .args(["--at", "0.5"])
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .current_dir(dir)
        .output()
        .context("running openmapper-cli render")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let gpu_required = std::env::var("OM_REQUIRE_GPU").as_deref() == Ok("1");
        if stderr.contains("GPU adapter") && !gpu_required {
            eprintln!("  smoke: no GPU adapter; skipping the video render");
            return Ok(());
        }
        bail!("render failed ({}): {stderr}", out.status);
    }
    let img = image::open(&png)
        .with_context(|| format!("decoding {}", png.display()))?
        .to_rgba8();
    let first = img.pixels().next().context("empty frame")?;
    if img.width() != 160 || img.height() != 96 {
        bail!("rendered {}×{}, expected 160×96", img.width(), img.height());
    }
    if img.pixels().all(|p| p == first) {
        bail!("rendered frame is uniform; the video did not decode");
    }
    eprintln!("  smoke: decoded and rendered a video frame through the bundled FFmpeg");
    Ok(())
}
