# Decisions

Material architectural and process decisions, newest last.

## D-001 — Toolchain: stable Rust, MSRV 1.98 (2026-10-07)

The primary dev machine uses Homebrew Rust 1.98 (no rustup). `rust-toolchain.toml`
pins the `stable` channel with rustfmt/clippy for rustup users and CI;
`rust-version = "1.98"` in the workspace sets the MSRV.

## D-002 — Repository layout (2026-10-07)

`openmapper` (this repo) and `openmapper-reference` are separate private GitHub
repositories. The reference repo is cloned as a sibling directory and is never
opened by implementation sessions.

## D-003 — Exact float round-trip in JSON (2026-10-07)

`serde_json` is built with `float_roundtrip`. Without it, a property test found
opacities that changed in the last bit across save/load, breaking byte-identical
round-trips.

## D-004 — Journal durability policy (2026-10-07)

Journal entries are written and flushed to the OS per command, not fsynced, so
slider drags stay cheap. This survives application crashes but not power loss.
Revisit (timed fsync batching) in the Plugins & resilience milestone.

## D-005 — Project root rejects unknown fields (2026-10-07)

Unknown fields are only preserved inside `extensions` maps. Unknown root fields
are rejected so typos fail loudly instead of silently dropping data. Future
versions are rejected by the version check first.

## D-006 — GPU stack follows eframe's wgpu (2026-10-07)

The renderer uses the `wgpu` version re-exported by `eframe`/`egui-wgpu`
(`eframe::wgpu`) so the UI and mapping renderer share one device. The workspace
does not declare a separate `wgpu` dependency.

## D-007 — CI cost control (2026-10-07)

Gate A (full `cargo xtask ci`) runs on Linux for every push. macOS and Windows
build/test only on pull requests, pushes to `main` and manual runs.
