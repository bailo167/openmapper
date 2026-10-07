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
