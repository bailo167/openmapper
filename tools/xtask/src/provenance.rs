// SPDX-License-Identifier: Apache-2.0
//! Clean-room and licence provenance scan (CLEANROOM.md).
//!
//! Checks every tracked or unignored file for:
//! - paths that look like reference-repository evidence;
//! - binary-analysis databases, native binaries and proprietary project files;
//! - decompiler artefacts (auto-generated symbol names) in text;
//! - mentions of the reference product outside Markdown documentation;
//! - missing SPDX headers in Rust sources;
//! - binary assets (images, media, fonts, models) without a THIRD_PARTY.yml entry.
//!
//! This file is exempt from the content checks because it defines the patterns.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;

const SELF_PATH: &str = "tools/xtask/src/provenance.rs";
const SPDX: &str = "// SPDX-License-Identifier: Apache-2.0";

/// Path segments that only belong in openmapper-reference.
const FORBIDDEN_SEGMENTS: &[&str] = &[
    "openmapper-reference",
    "rea-evidence",
    "binary-notes",
    "oscquery-dumps",
    "output-captures",
    "decompiled",
    "disassembly",
];

/// Extensions of analysis databases, native binaries and proprietary projects.
const FORBIDDEN_EXTENSIONS: &[&str] = &[
    // Ghidra / IDA / Hopper / Binary Ninja databases
    "gzf",
    "gpr",
    "rep",
    "i64",
    "idb",
    "hop",
    "hopperdb",
    "bndb",
    // native binaries
    "dylib",
    "dll",
    "exe",
    "so",
    "a",
    "lib",
    "o",
    "obj-native",
    // reference-product project files
    "mad",
];

/// Extensions that need a THIRD_PARTY.yml entry (or `generated` entry).
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "exr", "hdr", "webp", "mp4", "mov", "mkv",
    "webm", "avi", "wav", "mp3", "flac", "aif", "aiff", "ogg", "ttf", "otf", "woff", "woff2",
    "obj", "fbx", "gltf", "glb", "svg", "ico", "icns", "fs", "vs", "glsl", "frag", "vert",
];

#[derive(Debug, Default)]
pub struct Findings(Vec<String>);

impl Findings {
    fn push(&mut self, path: &str, msg: impl AsRef<str>) {
        self.0.push(format!("{path}: {}", msg.as_ref()));
    }
}

struct Rules {
    artefact: Regex,
    product: Regex,
    third_party: GlobSet,
}

impl Rules {
    fn new(third_party_yml: &str) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for line in third_party_yml.lines() {
            let line = line.trim().trim_start_matches('-').trim();
            if let Some(p) = line.strip_prefix("path:") {
                let p = p.trim().trim_matches(['"', '\'']);
                b.add(Glob::new(p).with_context(|| format!("THIRD_PARTY.yml glob {p:?}"))?);
            }
        }
        Ok(Self {
            // Ghidra/IDA/Hopper auto-generated names.
            artefact: Regex::new(
                r"\b(FUN|DAT|LAB|PTR|thunk_FUN)_[0-9a-fA-F]{6,}\b|\bsub_[0-9a-fA-F]{5,}\b|\bloc_[0-9a-fA-F]{5,}\b",
            )?,
            product: Regex::new(r"(?i)mad\s*mapper|minimad|garagecube")?,
            third_party: b.build()?,
        })
    }
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Checks one file. `content` is `None` for non-UTF-8 files.
fn check_file(rules: &Rules, path: &str, content: Option<&str>, f: &mut Findings) {
    let lower = path.to_ascii_lowercase();
    for seg in FORBIDDEN_SEGMENTS {
        if lower.split('/').any(|s| s == *seg) {
            f.push(
                path,
                format!("path segment `{seg}` belongs in openmapper-reference"),
            );
        }
    }
    let ext = extension(path);
    if FORBIDDEN_EXTENSIONS.contains(&ext.as_str()) {
        f.push(path, format!("`.{ext}` files must not be committed"));
    }
    if ASSET_EXTENSIONS.contains(&ext.as_str()) && !rules.third_party.is_match(path) {
        f.push(path, "asset has no THIRD_PARTY.yml provenance entry");
    }
    if path == SELF_PATH {
        return;
    }
    let Some(text) = content else { return };
    if let Some(m) = rules.artefact.find(text) {
        f.push(path, format!("decompiler-style symbol `{}`", m.as_str()));
    }
    let is_markdown = matches!(ext.as_str(), "md" | "markdown");
    if !is_markdown && rules.product.is_match(text) {
        f.push(
            path,
            "reference product named outside Markdown documentation",
        );
    }
    if ext == "rs" && !text.starts_with(SPDX) && !rules.third_party.is_match(path) {
        f.push(path, format!("missing `{SPDX}` first line"));
    }
}

fn list_files(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect())
}

pub fn run(root: &Path) -> Result<()> {
    let third_party = fs::read_to_string(root.join("THIRD_PARTY.yml")).unwrap_or_default();
    let rules = Rules::new(&third_party)?;
    let mut findings = Findings::default();
    let files = list_files(root)?;
    for path in &files {
        let full = root.join(path);
        if !full.is_file() {
            continue; // deleted in working tree
        }
        let bytes = fs::read(&full).with_context(|| format!("reading {path}"))?;
        let text = String::from_utf8(bytes).ok();
        check_file(&rules, path, text.as_deref(), &mut findings);
    }
    if !findings.0.is_empty() {
        for f in &findings.0 {
            eprintln!("  provenance: {f}");
        }
        bail!(
            "{} provenance problem(s); see CLEANROOM.md",
            findings.0.len()
        );
    }
    eprintln!("  provenance: {} files ok", files.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(path: &str, content: Option<&str>) -> Vec<String> {
        let rules = Rules::new("entries:\n  - path: tests/fixtures/ok/*.png\n").unwrap();
        let mut f = Findings::default();
        check_file(&rules, path, content, &mut f);
        f.0
    }

    #[test]
    fn clean_rust_file_passes() {
        assert!(
            findings(
                "crates/a/src/lib.rs",
                Some("// SPDX-License-Identifier: Apache-2.0\nfn x() {}")
            )
            .is_empty()
        );
    }

    #[test]
    fn missing_spdx_fails() {
        assert_eq!(findings("crates/a/src/lib.rs", Some("fn x() {}")).len(), 1);
    }

    #[test]
    fn decompiler_artefacts_fail() {
        let hdr = "// SPDX-License-Identifier: Apache-2.0\n";
        for sym in ["FUN_00401a2c", "sub_100A32", "DAT_0040c0de"] {
            let text = format!("{hdr}// see {sym}\n");
            assert!(
                !findings("crates/a/src/lib.rs", Some(&text)).is_empty(),
                "{sym}"
            );
        }
        assert!(
            findings(
                "crates/a/src/lib.rs",
                Some(&format!("{hdr}let sub_total = 1;"))
            )
            .is_empty()
        );
    }

    #[test]
    fn product_name_only_in_markdown() {
        let text = "// SPDX-License-Identifier: Apache-2.0\n// like MadMapper does\n";
        assert!(!findings("crates/a/src/lib.rs", Some(text)).is_empty());
        assert!(findings("docs/behaviour/x.md", Some("MadMapper parity")).is_empty());
    }

    #[test]
    fn evidence_paths_and_binaries_fail() {
        assert!(!findings("docs/rea-evidence/a.json", Some("{}")).is_empty());
        assert!(!findings("vendor/foo.dylib", None).is_empty());
        assert!(!findings("analysis/app.gzf", None).is_empty());
    }

    #[test]
    fn assets_need_third_party_entry() {
        assert!(!findings("tests/fixtures/new/a.png", None).is_empty());
        assert!(findings("tests/fixtures/ok/a.png", None).is_empty());
    }
}
