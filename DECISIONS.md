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

## D-008 — Display enumeration via display-info (2026-10-07)

egui/eframe place fullscreen windows by monitor *index* (winit order) but do
not list monitors. `om-output` lists displays with `display-info`, which uses
the same OS enumeration APIs as winit (CGGetActiveDisplayList, EnumDisplayMonitors,
XRandR), so indices are assumed to agree. Outputs store the display *name*
plus index and resolve by name first, so a re-plugged projector is found
again; an unplugged display resolves to nothing rather than another screen.
**Unverified until the physical projector test.** display-info pulls an
unmaintained hash crate on Windows (RUSTSEC-2025-0057, ignored with reason in
deny.toml); revisit at the Cross-platform milestone.

## D-009 — Media path storage (2026-10-07)

Image paths are stored relative to the project file's directory (with `/`)
when the file lies under it, otherwise as given. Content hashes and the
missing-media relink workflow arrive with the Plugins & resilience milestone.
Failed loads are retried every 2 s in the GUI.

## D-010 — Renderer design (2026-10-07)

- `om-render` may depend on `om-media-core` (same layer) to consume frames.
- Media is uploaded as linear, premultiplied `Rgba16Float` (converted on the
  CPU), so bilinear filtering happens on premultiplied linear values.
- Surfaces are drawn as triangle fans; the canvas→UV homography is evaluated
  per pixel in the fragment shader (exact perspective, no subdivision).
- Concave/degenerate shapes may be stored; they render nothing and the plan
  reports why (the UI shows "Not drawn: …").
- Goldens compare the GPU against an independent CPU reference renderer
  rather than stored PNGs, so they are platform-independent and need no binary
  fixtures. Tolerances: crates/om-render/tests/tolerances.md.
- wgpu's default uncaptured-error handler panics; `om-gpu` records errors
  instead and the compositor reports them, so a live show keeps running.
- The GUI asks eframe for the adapter's full limits; canvas/media larger than
  the GPU supports are rejected with an error.

## D-011 — Output windows show the shared preview texture (2026-10-07)

Output windows display the presented canvas texture scaled to the window.
Pixel-exact per-output rendering (output regions, crops, soft-edge) is part of
the Advanced mapping milestone; set the canvas to the projector's native size
("Match canvas") for 1:1 output now.
