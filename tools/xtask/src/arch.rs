// SPDX-License-Identifier: Apache-2.0
//! Architecture check: crate layering and forbidden dependencies.
//!
//! A first-party crate may depend only on first-party crates in a strictly
//! lower layer (or explicitly allowed same-layer edges). Every first-party
//! crate must be registered in [`LAYERS`]. Runtime AI/LLM/MCP crates are
//! forbidden anywhere in the dependency graph (PRODUCT.md).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};
use cargo_metadata::{DependencyKind, MetadataCommand};

/// (crate, layer). See docs/PLAN.md "Crate layering".
pub const LAYERS: &[(&str, u8)] = &[
    ("om-types", 0),
    ("om-time", 0),
    ("om-geom", 0),
    ("om-colour", 0),
    ("om-project", 1),
    ("om-command", 1),
    ("om-gpu", 2),
    ("om-render", 2),
    ("om-surfaces", 2),
    ("om-effects", 2),
    ("om-isf", 2),
    ("om-media-core", 2),
    ("om-audio", 2),
    ("om-output", 2),
    ("om-osc", 2),
    ("om-oscquery", 2),
    ("om-midi", 2),
    ("om-dmx", 2),
    ("om-show", 2),
    ("om-timeline", 2),
    ("om-modulation", 2),
    ("om-calibration", 2),
    ("om-plugin-api", 2),
    ("om-media-ffmpeg", 3),
    ("om-live-input", 3),
    ("om-syphon", 3),
    ("om-spout", 3),
    ("om-ndi", 3),
    ("om-decklink", 3),
    ("om-plugin-host", 3),
    ("om-engine", 4),
    ("om-testkit", 4),
    ("om-ui-egui", 5),
    ("openmapper", 6),
    ("openmapper-cli", 6),
    ("xtask", 6),
    ("fixturegen", 6),
];

/// Same-layer edges that are allowed (from, to).
pub const SAME_LAYER_ALLOWED: &[(&str, &str)] = &[
    ("om-time", "om-types"),
    ("om-geom", "om-types"),
    ("om-colour", "om-types"),
    ("om-command", "om-project"),
    ("om-render", "om-gpu"),
    // The renderer consumes decoded media frames (DECISIONS.md D-010).
    ("om-render", "om-media-core"),
    ("om-surfaces", "om-render"),
    ("om-effects", "om-render"),
    ("om-isf", "om-render"),
    ("om-oscquery", "om-osc"),
    ("om-timeline", "om-show"),
];

/// Substrings of crate names that indicate runtime AI/LLM/MCP functionality.
pub const FORBIDDEN_DEP_PATTERNS: &[&str] = &[
    "openai",
    "anthropic",
    "ollama",
    "langchain",
    "llama",
    "rmcp",
    "mcp-sdk",
    "mcp-server",
    "mcp-client",
    "rust-mcp",
];

fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

/// Returns violations for first-party edges `(from, to)` (normal and build
/// dependencies; dev-dependencies may reach sideways for test fixtures but
/// still never upward).
pub fn check_edges(edges: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (from, to) in edges {
        let (Some(lf), Some(lt)) = (layer_of(from), layer_of(to)) else {
            for n in [from, to] {
                if layer_of(n).is_none() {
                    out.push(format!(
                        "crate `{n}` is not registered in tools/xtask/src/arch.rs LAYERS"
                    ));
                }
            }
            continue;
        };
        let ok =
            lt < lf || (lt == lf && SAME_LAYER_ALLOWED.iter().any(|(a, b)| a == from && b == to));
        if !ok {
            out.push(format!(
                "`{from}` (layer {lf}) must not depend on `{to}` (layer {lt})"
            ));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Returns forbidden third-party package names.
pub fn check_forbidden(packages: &[String]) -> Vec<String> {
    packages
        .iter()
        .filter(|p| {
            let p = p.to_ascii_lowercase();
            FORBIDDEN_DEP_PATTERNS.iter().any(|f| p.contains(f))
        })
        .map(|p| format!("forbidden runtime AI/MCP dependency `{p}`"))
        .collect()
}

pub fn run(root: &Path) -> Result<()> {
    let meta = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()?;
    let members: BTreeSet<_> = meta.workspace_members.iter().collect();
    let mut edges = Vec::new();
    for pkg in meta.packages.iter().filter(|p| members.contains(&p.id)) {
        if layer_of(&pkg.name).is_none() {
            edges.push((pkg.name.to_string(), pkg.name.to_string()));
        }
        for dep in &pkg.dependencies {
            if dep.path.is_some() && dep.kind != DependencyKind::Development {
                edges.push((pkg.name.to_string(), dep.name.clone()));
            }
        }
    }
    let mut problems = check_edges(&edges);
    let all: Vec<String> = meta.packages.iter().map(|p| p.name.to_string()).collect();
    problems.extend(check_forbidden(&all));
    if !problems.is_empty() {
        for p in &problems {
            eprintln!("  arch: {p}");
        }
        bail!("{} architecture violation(s)", problems.len());
    }
    eprintln!("  arch: {} first-party edges ok", edges.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(a: &str, b: &str) -> (String, String) {
        (a.into(), b.into())
    }

    #[test]
    fn downward_edges_pass() {
        assert!(
            check_edges(&[e("om-engine", "om-command"), e("om-command", "om-project")]).is_empty()
        );
    }

    #[test]
    fn upward_edge_fails() {
        let v = check_edges(&[e("om-project", "om-engine")]);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("must not depend"));
    }

    #[test]
    fn unlisted_same_layer_edge_fails() {
        assert!(!check_edges(&[e("om-midi", "om-dmx")]).is_empty());
        assert!(check_edges(&[e("om-oscquery", "om-osc")]).is_empty());
    }

    #[test]
    fn unregistered_crate_fails() {
        assert!(check_edges(&[e("om-mystery", "om-types")])[0].contains("not registered"));
    }

    #[test]
    fn ai_dependencies_are_forbidden() {
        let v = check_forbidden(&["serde".into(), "async-openai".into(), "rmcp".into()]);
        assert_eq!(v.len(), 2);
    }
}
