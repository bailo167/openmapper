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

## D-012 — Frame timing from containers, snapped to the frame grid (2026-10-07)

Frame times come from container timestamps (exact rationals), never from a
counter. Containers with coarse time bases (Matroska: milliseconds) shift
fractional-rate frames by up to 0.5 ms, enough to select the previous frame at
an exact boundary; timestamps within a quarter frame of the stream's nominal
grid are snapped to it. `FrameCursor` seeks to at-or-before the target and
decodes forward; if a demuxer lands late (MP4 indexes by decode time, which
B-frames put ahead of presentation time) it backs off 1 s, 2 s, 4 s and
retries. Playback decodes ahead on a thread into a bounded queue (default 6
frames); the render side never blocks and holds the last frame on a miss.

## D-013 — FFmpeg bindings: ffmpeg-next 9 (2026-10-07)

`ffmpeg-next`/`ffmpeg-sys-next` 9.0 support FFmpeg 9 (the MIT `rsmpeg`
targets 8.0). They are WTFPL, a permissive licence; added to the cargo-deny
allow-list. Linking is dynamic via pkg-config (`FFMPEG_DIR` on Windows). CI
uses distro FFmpeg on Linux, Homebrew on macOS and BtbN's LGPL shared build on
Windows. Homebrew's FFmpeg is a GPL build — acceptable for development only;
`om_media_ffmpeg::info().licence` reports it and the release audit rejects it.

## D-014 — Audio positions from containers, snapped to the packet grid (2026-10-07)

Audio is decoded to interleaved stereo `f32` at the output device rate via
libswresample (called directly through `swr_convert`; the frame-based wrapper
rejected Matroska PCM with unspecified channel order). Sample positions come
from the first packet timestamp after open/seek, then count contiguously.
Where the container time base is coarser than one sample (Matroska: 1 ms),
the first position is snapped to the codec's fixed packet grid. Measured:
PCM onsets exact; 44.1→48 kHz and AAC within 2 samples; seeks sample-exact.

## D-015 — Audio sync and loop model (2026-10-07)

The show clock is master. Each device callback renders the block for the
predicted show position and stays sample-contiguous; drift beyond 20 ms
re-syncs (a small discontinuity) rather than resampling — adequate for
minutes-long shows; an adaptive resampler is a later improvement. Audio plays
only at 1× speed (muted otherwise) until time-stretching exists. Video and
audio players loop on an *unwrapped* timeline (pass k, time t ↦ k·len + t),
so the decoder runs on into the next pass and loop points neither stall video
nor drop audio. Verified end to end (decode → runtime → mixer) across loop
passes.

## D-016 — Smaller dev builds (2026-10-07)

Dev profile keeps line tables only, and no debug info for dependencies
(target dir 13 GB → 1.9 GB on the dev machine, which was nearly out of disk).

## D-017 — Mesh cells are bilinear; quads stay perspective (2026-10-07)

Quads, triangles, lines and ellipses use exact perspective (homography)
mapping. Mesh cells use **bilinear** mapping (inverse-bilinear per pixel):
per-cell perspective maps disagree along shared edges and show texture seams,
while bilinear maps are linear on every edge and therefore seamless.
"Convert to mesh" samples the quad's perspective at the grid points, so a
fine mesh closely follows the original perspective. The inverse uses the
numerically stable quadratic form; the naive form dropped pixels on
near-parallelogram cells. Blend modes are fixed-function on premultiplied
linear colour; Multiply is exact over opaque backgrounds.

## D-018 — ISF implementation strategy (2026-10-07)

ISF GLSL is translated to GLSL 4.50 by explicit rewriting (not preprocessor
tricks): ISF image macros become generated per-image functions, `gl_FragColor`
/ `gl_FragCoord` map to variables with GL's bottom-left origin preserved,
inputs become a std140 uniform block (bools as ints). naga parses and
validates before anything reaches the GPU, so errors are reported with the
user's line numbers. naga 30.0.1 needs `wgsl-in` enabled alongside `glsl-in`
(upstream cfg bug). ISF shaders operate on sRGB straight-alpha colour,
converted around them. Each use (media item or effect slot) owns its program
state (persistent buffers, frame index). Shader files are watched and
recompiled live. The CPU reference treats ISF as identity; ISF is verified
against analytic expectations instead. `om-effects` remains unused: effects
live in `om-render` alongside the compositor (no separate crate needed yet).

## D-019 — Live control model (2026-10-07)

Parameters have stable string ids that double as OSC addresses
(`/openmapper/<param>`). Direct control (UI/OSC/MIDI) edits the document via
commands coalesced per parameter; cues, timelines and modulators produce
per-frame overrides applied to a derived copy for rendering, so shows never
pollute undo history or the saved file. Precedence: timelines < cues <
modulators. The show runtime runs on its own always-running live clock; the
transport governs media only. Cues track (values persist until changed or
released). OSCQuery is served with tiny_http and advertised via mdns-sd;
LISTEN/WebSocket streaming is deferred. Audio-reactive levels come from the
playback mix (no microphone permission needed); a live input source can be
added later.

## D-020 — Live I/O model (2026-10-07)

Live devices and senders are identified **by name** (camera name, stream
URL, NDI source name, Syphon server/app, Spout sender), never by index,
port or session id, so they are found again after unplugging, re-ordering
or restarting. Live feeds keep **only the newest frame**; a source that
stops is reopened with exponential back-off (250 ms → 4 s) while **the
last good frame stays visible**. Publishing is the mirror image:
submission never blocks the renderer, and a failed sink reopens itself.
Frames cross the **CPU** (RGBA8 readback/upload) for now; a zero-copy GPU
path can replace it without format changes. Cameras use FFmpeg's capture
devices on all three OSes, so there is **no `om-live-input` crate**.
`om-platform` (layer 4) is the single place the app and CLI obtain
adapters, and the `Adapters` bundle lives in `om-media-core` so lower
layers can carry it without depending on adapters.

## D-021 — Syphon and Spout implementation (2026-10-07)

**Syphon**: the BSD Syphon framework is vendored as source at a pinned
commit and compiled with `cc` into a static library, with a small
Objective-C shim exposing a C API. This avoids installing or shipping
`Syphon.framework`. The only patch makes the Metal renderer compile its
shaders from embedded source when no framework bundle exists. Discovery
needs the main thread's run loop (distributed notifications); the GUI's
event loop provides it, and the CLI and tests pump it explicitly.
**Spout**: implemented natively in Rust (`windows` crate) following the
Spout 2 SDK's shared-memory directory, description block, access mutex,
frame-count semaphore and legacy DXGI shared textures, rather than
building the C++ SDK. It is smaller, has no C++ toolchain dependency, and
keeps the protocol testable on all OSes (format unit tests). The sender
blocks on a GPU event query before releasing the access mutex, so
receivers never copy an incomplete texture.

## D-022 — NDI via the user's installed runtime (2026-10-07)

OpenMapper does not bundle, download or redistribute NDI, and does not
accept the NDI SDK licence on anyone's behalf. `om-ndi` loads an
already-installed NDI runtime with `libloading` and declares only the few
public C entry points and plain structures it calls. A missing runtime is
a normal, reported condition. The NDI adapter stays
**implemented-unverified** until tested on a machine with the runtime
(`OM_REQUIRE_NDI=1`). Redistribution and trademark use remain a release
gate (docs/PLAN.md: NDI/vendor SDK redistribution audit).

## D-023 — DeckLink deferred (2026-10-07)

No DeckLink hardware is available, and the DeckLink SDK has its own
licence terms. The `om-decklink` adapter is deferred until a card is
available for physical verification (docs/PLAN.md hardware policy). SDI
needs can be met meanwhile through NDI or network streams. The project
format needs no change for it: a later `decklink` live input/publish type
is an additive schema change.

## D-024 — DMX output and pixel-mapping model (2026-10-07)

LED fixtures **sample the final canvas**, the same CPU readback used for
publishing, so master, cues and effects apply without special cases.
Fixture placement is in normalised canvas space like surfaces. Sampling
averages each pixel's footprint in **linear light**; the default channel
encoding is **sRGB**, matching what a monitor shows, with `linear` as an
option. RGBW white is the common part of R, G and B. Pixels never straddle
universes, which is the common LED-controller convention. The sender
refreshes every universe at a fixed project rate (default 40 Hz, at most
44, the DMX512 frame-rate ceiling) rather than only on change, because
receivers time out on silence. Art-Net sequences skip 0. sACN uses the
project id as its CID, so a project keeps its identity across restarts,
and it terminates streams on stop. ArtSync and sACN synchronisation are
not sent yet: every universe of a frame is sent back-to-back, and fixtures
that need tearing-free latching across universes are a later option.
DMX **input** (Art-Net/sACN as a control source) is deferred to the
control work in a later milestone; `dmx monitor` covers diagnostics. The
physical node test stays pending hardware, so the DMX parity rows are
verified in software and the node row is implemented-unverified.
