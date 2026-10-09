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
DMX **input** (Art-Net/sACN as a control source) was deferred here to the
control work in a later milestone (done in D-028); `dmx monitor` covers diagnostics. The
physical node test stays pending hardware, so the DMX parity rows are
verified in software and the node row is implemented-unverified.

## D-025 — Output mapping, soft edges and 3-D calibration (2026-10-07)

Outputs show a canvas **region** through a **corner pin**, two exact
homographies evaluated per pixel on the GPU directly from the linear
canvas. A corner pin is the common case and exact; per-output mesh warps
can follow. Soft edges are symmetric S-curves in **light**, so
overlapping ramps sum to one, applied to the **signal** with display
gamma compensation (`w^(1/γ)`, γ default 2.2). 3-D mapping uses
**OBJ models with UVs**: the canvas is the model's texture, and the
output renders the model from a **pinhole projector** (no skew, no
distortion). The projector is calibrated from ≥ 6 non-coplanar 3-D ↔ 2-D
point pairs: normalised DLT, then RQ, then Levenberg–Marquardt. Planar
point sets are refused with a clear message rather than calibrated
wrongly. The maths lives in a new `om-calibration` crate (layer 2), which
`om-render` may use (same-layer edge allowed). Measured points and the
fitted projector are both stored in the project for repeatability.
**Camera-assisted calibration** is implemented as Gray-code structured
light (pattern + inverse per bit, so no global threshold is needed),
decoding to camera ↔ projector correspondences, plus a robust homography
fit for flat screens. It is verified with a synthetic camera; physical
verification waits for a camera. Driving the camera from the app,
camera-based 3-D calibration, click-to-pick in output windows and lens
distortion come later.

## D-026 — Plugin ABI, sandbox and resilience (2026-10-07)

**ABI v1 is a core WebAssembly module**, not a WIT component. The
interface is the WIT-equivalent `openmapper:plugin@1.0.0` contract
(metadata, configure-by-parameters, process on CPU buffers, host
log/time) expressed as five exports and two imports. Reasons: fixtures
can be plain WAT text checked into the repo, so no guest toolchain is
needed in CI; it is simple to support from any language; and Wasmtime
needs only its core runtime. A WIT/component front end can wrap the same
semantics later as ABI v2. Wasmtime (49.0.2) runs with fuel *and* epoch
interruption, a per-instance memory cap, and no WASI. Imports outside the
declared capabilities refuse to load. Plugins are **media filters**,
applied to every frame of a media item on the CPU before upload, so
every surface showing that media sees the result, and run on per-plugin
threads (newest frame wins). The original frame shows until a chain
produces output and whenever it cannot run. Parameters are the persisted
state; instance memory is not saved in v1.

Resilience: crash injection (random process kills while editing and
saving) found that reopening a project **rewrote the recovery journal in
place**, so a crash during that rewrite could lose recovered work. The
journal is now replaced atomically (temp file, fsync, rename). Journal
entries are also forced to disk at most 1 s after being written (timed
fsync batching, revisiting D-004), bounding the power-loss window.
N-2 migration is verified with a synthetic three-version chain until real
versions exist; a v1 compatibility fixture using every feature must keep
loading. Missing media is relinked by file name from a folder the user
chooses, preferring candidates whose parent folders match, in one undo
step.

## D-027 — Varispeed audio and ISF audio inputs (2026-10-07)

Audio at speeds other than 1× now plays **varispeed** (tape/turntable
behaviour: pitch follows speed), resampled with linear interpolation on
the same `(show − origin) × speed` timeline as the video, so picture and
sound stay together at any speed up to 16×. Reverse and zero speeds are
silent. This supersedes D-015's "muted at non-1×". Pitch-preserving time
stretching is a known difference from tools that offer it; it can be
added later as an option behind the same timeline.

ISF `audio` and `audioFFT` inputs read the mixer output: 512 waveform
samples (stored `0.5 + 0.5 × s`) and 512 spectrum bins (1024-point Hann
FFT, full-scale sine = 1) per channel, one texture row per channel with
channel 0 at the bottom (ISF's bottom-left origin). Sizes are fixed;
`MAX` is ignored. The FFT is computed on the UI thread from a copy of the
analyser's ring, never inside the audio callback lock.

## D-028 — DMX input as a control source (2026-10-07)

Art-Net and sACN input drive parameters and cues like MIDI: bindings name a
universe and channel (optionally a 16-bit pair) and reuse the MIDI target
type. Input is **off by default** and, once enabled, listens on all
interfaces (consoles are on the lighting network). Because DMX repeats
continuously, a binding emits only on change; cue triggers need a rising
edge through 50 % and never fire on first sight of a universe, so
connecting a console mid-show cannot fire cues. Sources are not merged
(newest packet wins); HTP/LTP merging and sACN priority arbitration are
later options. Sockets are non-blocking and drained once a frame with a
per-frame packet budget, and per-universe state is kept only for bound
universes (bounded), so a packet flood cannot stall a frame or grow memory.

## D-029 — Hardening from the pre-release security review (2026-10-07)

A review of every untrusted-input boundary (docs/release/security-review.md)
changed these defaults and limits:

- **Remote control is local by default.** OSC and OSCQuery bind
  127.0.0.1 unless the project sets `controls.network`; mDNS advertisement
  only then. Previously every project listened on all interfaces, so anyone
  on the LAN could drive a show. Existing projects become local-only until
  the option is ticked — a deliberate behaviour change.
- **OSCQuery uses a small bounded HTTP server** (GET only, 8 KiB request,
  2 s deadline, 16 connections, connection closed per response) instead of
  `tiny_http`, which buffered unbounded headers and spawned a thread per
  connection.
- **Project trust.** Opening a project holds back camera, NDI and network
  stream inputs, NDI and stream outputs, DMX output, network control and
  DMX input until the user allows them; the permission is remembered per
  user as a fingerprint of the project id and that list of connections, so
  a changed list asks again. Inter-app sharing (Syphon/Spout) and local
  files are not gated: they do not leave the computer on their own.
- **Bounded reads.** Files named by a project (project, journal, shaders,
  plugins, OBJ models) must be regular files and are read with a size cap,
  so `/dev/zero` or a FIFO cannot exhaust memory or hang the UI.
- **Bounded parsing and work.** ISF size expressions are limited in length
  and nesting (no stack overflow), ISF passes to 16, OBJ faces are checked
  before fan-out, DMX projects to 1024 universes, OSC queues to 4096
  messages with per-frame coalescing (bounding journal growth), and OSC
  fades/seeks to finite, capped values. The naga GLSL frontend runs under
  `catch_unwind`.

## D-030 — Release packaging and the 1.0 verdict (2026-10-07)

Releases are portable `.tar.gz` archives per platform (binaries, LICENSE,
NOTICE, THIRD_PARTY.yml, a generated THIRD_PARTY_LICENSES.txt, the user
guide and reference docs) with `SHA256SUMS`, built by `cargo xtask dist`
and smoke-tested from a fresh directory by `cargo xtask smoke` on all three
OSes in CI (on demand). Installers (`.app`/`.dmg`, MSI) wait for signing
accounts. Third-party notices are generated from the dependency graph of
the shipped binaries rather than maintained by hand, with standard licence
texts and author lines for crates that publish no licence file. The
clean-room scan also runs over every blob in every ref
(`provenance --history`), with reviewed false positives listed in code.
FFmpeg is not bundled; whether to bundle an LGPL build or load FFmpeg at
run time is a release decision (RELEASE_GAPS.md G-07). The 1.0 verdict is
NOT READY until the hardware, soak, distribution, signing and legal items
in RELEASE_GAPS.md are closed.

## D-031 — FFmpeg is bundled: a minimal, self-built LGPL build (2026-10-08)

Delegated by the owner (RELEASE_GAPS.md G-07, "you decide what's best").
Releases **bundle** FFmpeg shared libraries built by `tools/ffmpeg/build.sh`
from the component list in `tools/ffmpeg/components.txt`, rather than
loading FFmpeg at run time or asking users to install it.

Why not run-time loading: `ffmpeg-sys-next` binds FFmpeg's structs and
functions at build time for one specific FFmpeg version; a dlopen layer would
mean rewriting the adapter against a hand-maintained ABI, and the binaries
would still need *some* FFmpeg to play video — a projection mapper that
cannot play video out of the box is not a 1.0 product. Why not user-installed
FFmpeg: macOS users' usual source (Homebrew) is a GPL build, Linux
distributions ship incompatible SONAMEs (6.1 on Ubuntu 24.04, 7.x on Fedora),
and Windows has no system FFmpeg, so no clean install would work anywhere.

Why a *self-built, minimal* build instead of a third-party LGPL build
(BtbN): one source archive and one licence to publish instead of a dozen
external libraries; a reviewable list of exactly which codecs are shipped;
and a build pinned by git commit (`n9.0.2`,
`946fcce07b6dcd0331c8cc609192aeff5e1924f8`) rather than a moving
"latest" download.

Codec policy, chosen because the legal review will not happen (D-033):

- **No GPL or non-free parts, no external libraries** (`--enable-lib*`), so
  the LGPL compliance story is FFmpeg alone.
- **Decoders** for the delivery and intermediate codecs shows actually use:
  H.264, MPEG-4 Part 2, MPEG-1/2, MJPEG, ProRes, DNxHD, CineForm, HAP, FFV1,
  HuffYUV, Ut Video, raw/v210, VP8/VP9, Theora, QuickTime RLE, DV, H.263,
  FLV; audio AAC, ALAC, FLAC, MP3, Opus, Vorbis, PCM.
- **No HEVC, VVC or VC-1 decoders.** HEVC's patent pools have no royalty-free
  tier and license software decoders; H.264's pool (Via LA) has a
  royalty-free tier below 100 000 units a year and its core patents are
  expiring. That is the same line the Chromium project draws for the FFmpeg
  decoders it ships. Users who need HEVC drop in any ABI-compatible FFmpeg 9
  build (docs/media/ffmpeg.md) — the libraries are deliberately replaceable.
- **Encoders only where no patent licensing programme is active**: MPEG-2
  (patents expired 2018) and FFV1 for publishing (sink.rs), raw video and
  PCM. No H.264, HEVC, AAC or MPEG-4 Part 2 encoders; the test corpus's
  MPEG-4/AAC fixtures therefore need a development FFmpeg, not the bundle.
- AV1 is not shipped (FFmpeg's native decoder needs hardware; software AV1
  needs libdav1d, an external library). SRT is not shipped (needs libsrt);
  TCP remains the reliable transport for lossless streams.

Mechanics: `cargo xtask dist` builds against the prefix (`FFMPEG_DIR`), gives
the executables an rpath to `lib/` (DT_RPATH on Linux so it also covers
libavcodec → libavutil; `@executable_path/lib` with `@rpath` install names
on macOS; same directory on Windows), copies the six libraries plus
`ffmpeg/` (LGPL text, FFmpeg's LICENSE.md, SOURCE.txt with the commit and
configure line, and the recipe itself), and refuses to archive unless the
staged `openmapper-cli ffmpeg --require-release` loads the bundled build
(`--extra-version=openmapper` marker) and passes the policy, which is also
encoded and tested in `om_media_ffmpeg::policy` against `components.txt`.
The smoke test then decodes a generated FFV1 clip through the unpacked
archive into a rendered frame. The complete corresponding source
(`ffmpeg-n9.0.2-src.tar.gz`) is attached to every release next to the
archives (LGPL-2.1 §6d). `libavfilter` is no longer linked (unused).
Development and gate-B CI still use distribution, Homebrew (GPL,
development only) or BtbN builds.

## D-032 — Signing posture for 1.0 without publisher accounts (2026-10-08)

Delegated by the owner (G-08). No Apple Developer, Windows code-signing or
long-term signing key exists.

- **Checksums are signed keylessly with Sigstore** by `publish.yml`, called
  from `release.yml`: `cosign sign-blob` with the job's GitHub Actions OIDC
  identity produces `SHA256SUMS.sigstore.json`. There is no private key to
  generate, store or lose; the signature binds the files to this repository,
  the `publish.yml` workflow and the tag, recorded in the public Rekor
  transparency log (which exposes the repository name — acceptable for a
  project that is going public). The job verifies its own signature the way
  a user would, and `ci.yml`'s manual runs call the same job (without a
  release) so the signing path is proven before the first tag. A minisign
  or GPG release key can be added later without changing anything else.
- **Binary code signing is wired but inactive.** `cargo xtask dist` runs
  `OM_SIGN_COMMAND` on the staging directory before archiving;
  `package.yml` enables `.github/scripts/sign-macos.sh` (Developer ID,
  hardened runtime with the JIT entitlements Wasmtime needs, notarytool) and
  `sign-windows.ps1` (Authenticode via signtool) only when the corresponding
  secrets exist. Those scripts are untested until the owner creates the
  accounts (docs/release/signing.md lists the exact secrets and steps).
- **Until then the binaries are unsigned**, and the user guide documents the
  Gatekeeper (`xattr -dr com.apple.quarantine`, or System Settings ▸ Privacy
  & Security ▸ Open Anyway) and SmartScreen ("More info ▸ Run anyway") steps
  and the checksum/Sigstore verification that replaces trust in a publisher
  certificate. Apple Silicon binaries carry the linker's ad-hoc signature,
  which is required for them to run at all. The macOS executable embeds an
  Info.plist (bundle identifier, camera/microphone/local-network usage
  descriptions) so it behaves like a bundled app for privacy prompts and is
  ready for notarisation. Installers (`.app`/`.dmg`, MSI) remain follow-up
  work once signing is live (D-030).
- Releases are created as **drafts** only on a `v*` tag; the owner reviews
  and publishes. GitHub artifact attestations were not used because they
  are unavailable on private repositories outside Enterprise; they can be
  added once the repository is public.

## D-033 — No solicitor review before 1.0; recorded waiver (2026-10-08)

The owner decided that the Australian IP solicitor review of the clean-room
record, FFmpeg/codec patents, NDI naming and third-party notices (G-10,
CLEANROOM.md, docs/PLAN.md) **will not be done**. This is the owner's risk
decision, recorded here and in docs/release/legal-posture.md; nothing in the
repository claims legal clearance, and the review may still be commissioned
later.

Risk was reduced where software can reduce it:

- FFmpeg: the narrow, decoder-biased, external-library-free LGPL build of
  D-031, with source, recipe and licence text in every release.
- NDI: the name is used descriptively with the required attribution ("NDI®
  is a registered trademark of Vizrt NDI AB") and a non-affiliation note in
  NOTICE, the third-party notices file and the user documentation; the
  runtime is never redistributed (D-022).
- Clean-room evidence: the full-history provenance scan runs in every
  package job and its report (`PROVENANCE.txt`) is attached to releases, so
  the evidence a reviewer would examine is public.
- Third-party notices stay generated from the dependency graph, never
  hand-maintained.

Residual risks the owner carries, with the mitigations above: codec patents
on the shipped decoders; a trademark or SDK-licence objection to the NDI
naming; a clean-room challenge resting on process rather than counsel's
opinion; mistakes in LGPL compliance mechanics. The reference-product
licence agreement is still to be archived by the owner as CLEANROOM.md
requires (that step needs no solicitor).

## D-034 — Show mode replaces minimising the control window; headless trust (2026-10-08)

Found in the first install of a release archive on the Linux show machine
(KDE Plasma, Wayland, one projector):

- **`--play` showed a black, then frozen, projector.** Output windows are
  egui immediate viewports, painted inside the main window's frame. `--play`
  minimised the main window so it would not cover the output (KWin will not
  raise a window without user input). On Wayland a client cannot tell it is
  minimised and the compositor sends a minimised window no frame callbacks,
  so the main frame — and every output — stopped; on macOS, Windows and X11
  eframe repaints a minimised window only every 100 ms, capping outputs at
  10 fps. The G-01 run missed it because its test content was static.
  **Decision:** `--play` (and the new *Run show* button) puts the main
  window itself in *show mode*: borderless fullscreen on the first enabled
  output whose display is connected (waiting until one is), drawing that
  output edge to edge with the pointer hidden. Further outputs still open
  in their own windows. Escape returns to the editor; disabling or removing
  the shown output leaves show mode; a display that disappears keeps the
  window where it is. Deferred viewports were rejected: the canvas is
  rendered in the main frame, so they would repaint a stale texture.
- **Network control could not be allowed without a screen.** A project
  with `controls.network` (or any other D-029 connection) is held back
  until allowed in the app's bar, which a show machine set up over SSH
  cannot click. **Decision:** `openmapper-cli trust <project>` lists the
  project's external connections and whether this user allowed them;
  `--allow` records the same per-user permission as the app's button. Running
  it is the user's own explicit action on their own account, so D-029's
  protection against a project from someone else is unchanged.
- The desktop app now parses its arguments: `--help` and `--version` print
  and exit, unknown options are an error (previously `openmapper --help`
  opened the app). `displays` explains a missing graphical session
  (`WAYLAND_DISPLAY`/`DISPLAY` unset) instead of the library's parse error.
- The workspace version is **1.0.0**, and the release workflow refuses a
  `v*` tag whose archives were built with a different version.

## D-035 — Reference licence archived; L3/L4 ruled out; Windows waived for 1.0 (2026-10-09)

- **Licence agreement archived.** CLEANROOM.md requires the agreement
  presented by the legitimately installed reference product to be archived
  before a public release. The owner's installed demo version came from
  the vendor's installer image, downloaded from the vendor's website on
  2026-09-28;
  the agreement is the DMG's Software License Agreement (RTF SHA-256
  `c320383413f5b9fd4ff45c2afc8bbf9f5f9c6400b55941d069723bc32fb5a692`). It is
  kept privately by the owner, not in this repository, because the text is
  the vendor's copyright.
- **What it says that matters here:** the licensee may not "modify, adapt,
  translate, reverse engineer, decompile, or disassemble the Software"
  (Swiss law governs). There is no clause on observing behaviour,
  benchmarking or building a competing product. **Decision:** research
  levels L3 (process capture, binary inventory) and L4 (decompilation,
  disassembly) are no longer available for this product at all, rather
  than "by ticket" and "exceptional"; only L0–L2 black-box work is.
  Questions L0–L2 cannot answer are deferred. Neither repository has ever
  had an L3/L4 ticket (no issues exist on either), so the existing record
  is L0–L2 only; the private reference repository was not opened to check,
  as this policy requires.
- **Windows waived for 1.0 (owner, 2026-10-09):** the Windows clean-machine
  install (G-12) and Windows projector run (G-01) are waived as
  software-verified, not hardware-verified. The Windows archive is built,
  bundled and smoke-tested by the CI `package` job on every release run.

## D-036 — The public tree does not name the reference product (2026-10-09)

The repository is public since 2026-10-09. To keep the project's identity
independent and avoid any appearance of trading on another product's name,
no file in the current tree names the reference product or its vendor any
more; documents say "the reference product". The provenance scan enforces
it for every file, Markdown included (previously Markdown was exempt), and
stores the names it looks for ROT13-encoded so that the scanner itself does
not spell them out. History is not rewritten: old Markdown that names the
product remains, and the history scan still allows it there.
