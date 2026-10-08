# OpenMapper Build Plan (condensed)

Source: "OpenMapper: Execution-Ready Research and Build Plan" (2026-10).
This file keeps the parts future sessions need. Governance lives in
AGENTS.md / CLEANROOM.md / PRODUCT.md; per-milestone prompts live in `prompts/`.

**Principle: build the product first; use MadMapper as a reference oracle, not
as source material.**

## Stack

- Rust, `wgpu` (Metal / D3D12 / Vulkan / GL), `egui`/`eframe`, `glam`.
- FFmpeg only behind `om-media-ffmpeg`, dynamically linked, LGPL-only for release
  (no `--enable-gpl` / `--enable-nonfree`, no bundled x264/x265 without legal review).
- Wasmtime for capability-restricted WASM plugins (no FS/net/env by default).
- Thin FFI adapter crates for Syphon, Spout, NDI, DeckLink.
- Pin `egui`/`wgpu` versions exactly; upgrade only in dedicated PRs.

## Repositories

```
openmapper/             product (eventual open source) — this repo
openmapper-reference/   permanently private research/evidence
```

Only neutral behavioural specs cross from reference → product (see CLEANROOM.md).

## Architecture

```
UI (egui) ─┐
CLI ───────┤
OSC/OSCQ ──┼──► Typed Command Bus ──► Engine/Session ◄──► Project Document ◄──► Atomic Store + Journal
MIDI ──────┤                              │  ▲
DMX ───────┘                              │  └── Cues / Timeline / Modulation
                                          ├──► Media Graph ◄── FFmpeg / Camera / NDI / Syphon / Spout / DeckLink
                                          ├──► wgpu Render Graph ──► Output Manager ──► Displays / Virtual / Network
                                          └──► WASM Plugin Host
```

Rules:
- **Command bus is mandatory.** UI, OSC, MIDI and CLI never mutate state
  directly; every mutation is a serialisable typed `Command` producing a new
  revision + events. This gives undo/redo, replayable tests and an oracle surface.
- **Exact time.** `RationalTime { ticks: i128, ticks_per_second: i128 }`; never
  `f32` seconds. Default timebase 254_016_000_000 ticks/s (divisible by common
  video and audio rates).
- **Rendering**: linear-light float working surface (`Rgba16Float` where
  supported), premultiplied alpha, render-graph based:
  decode → source conversion → effect/material → surface UV + projective
  transform → mask → blend → output correction → soft-edge/crop → sink.
- **Media**: `MediaSource` trait (descriptor/seek/video_frame/audio_block),
  bounded producer/consumer queues, timestamp scheduling.
- **Hardware**: `VideoInput` / `OutputSink` traits; vendor code only in adapters.
- **Plugins**: tiny WIT ABI first (`openmapper:plugin@1.0.0`, host log/time,
  effect metadata/configure/process on CPU buffers); GPU plugin API later.
- **Project format**: see docs/project-format.md.

## Crate layering (enforced by `cargo xtask arch`)

| Layer | Crates |
|---|---|
| 0 foundation | om-types, om-time, om-geom, om-colour |
| 1 model | om-project, om-command |
| 2 subsystems | om-gpu, om-render, om-surfaces, om-effects, om-isf, om-media-core, om-audio, om-output, om-osc, om-oscquery, om-midi, om-dmx, om-show, om-timeline, om-modulation, om-calibration, om-plugin-api |
| 3 adapters | om-media-ffmpeg, om-live-input, om-syphon, om-spout, om-ndi, om-decklink, om-plugin-host |
| 4 orchestration | om-engine, om-testkit |
| 5 presentation | om-ui-egui |
| 6 apps/tools | apps/*, tools/* |

A crate may depend only on crates in a strictly lower layer, except within
layer 2 where explicitly allowed in `tools/xtask/src/arch.rs`. Crates are
created when their milestone starts, not before.

## CI gates

- **A — every commit** (`cargo xtask ci`): fmt, clippy `-D warnings`, tests,
  cargo-deny, provenance scan, architecture check.
- **B — every PR**: Linux/Windows/macOS build+test, migrations, render goldens,
  protocol tests, fuzz/property tests.
- **C — nightly**: physical GPU suite, media corpus, cross-platform image
  comparison, 60-min stress, device connect/disconnect simulation.
- **D — release candidate**: physical projector/MIDI/DMX/capture,
  Syphon/Spout/NDI, DeckLink if available, 12–24h soak, crash recovery,
  migration, clean-machine installs.

## Milestones (sequential; see MILESTONES.md for status)

| # | Milestone | Acceptance (summary) | Est. AI hrs |
|---|---|---|---|
| 1 | Foundation | 3-OS builds; project open/save round-trip; CI green; no reference artefacts | 24–36 |
| 2 | Mapping renderer | checkerboard/UV-grid quad goldens; projector hotplug survives; 30-min render without resource growth | 50–75 |
| 3 | Media engine | timestamp-accurate seeks; no A/V drift over 10 min; corrupt media fails gracefully | 45–70 |
| 4 | Surfaces, masks, effects, ISF | geometry property tests; ISF conformance corpus; mask edge tolerance; deterministic offline effect chain | 70–110 |
| 5 | Control & show | OSCQuery self-describes state; deterministic cue fades; MIDI learn persists; exact timeline stepping | 55–85 |
| 6 | Live video & pro I/O | software loopbacks; reconnect stress; cross-app frame identity | 55–90 |
| 7 | DMX & LED mapping | packet goldens; 30-min stream; pixel-mapping golden; physical node test | 50–80 |
| 8 | Advanced mapping & calibration | synthetic calibration error bound; overlap blend test; repeatable saved calibration | 70–120 |
| 9 | Plugins & resilience | hanging plugin terminated; crash-injected recovery; N-2 migration; media relink | 50–80 |
| 10 | Parity audit & hardening | all P0/P1 rows verified; 12–24h soak; clean installs; release checklist | 90–150 |

Feature status ladder: Specified → Implemented → Unit tested → Golden/protocol
tested → Cross-platform tested → Reference tested (where relevant) →
Failure/recovery tested → Documented → **Verified**. Hardware-dependent features
without hardware stay `implemented-unverified`, never `verified`.

Parity rows live in `docs/parity/*.yaml`:

```yaml
feature: surface.quad.perspective
priority: P0
status: verified
source: [public-doc, black-box-oracle]
platforms: { macos: pass, windows: pass, linux: pass }
tests: [render_quad_identity, render_quad_perspective, oracle_quad_017]
known_differences: []
```

## Pixel tolerances (OpenMapper policy)

| Test type | Tolerance |
|---|---|
| Pure geometry/math | exact where integer/rational; else ≤2–4 ULP |
| CPU reference output | exact or ≤1 8-bit LSB |
| GPU golden, flat interior | mean abs ≤0.25/255; max normally ≤1/255 |
| Transformed edge | displacement ≤1 px |
| Licensed MadMapper reference | mean abs ≤0.5/255; p99.9 ≤2/255 |
| Demo-watermark reference | watermark masked; geometry separately; relaxed colour |
| Video timing | exact logical frame/PTS; zero cumulative drift |
| Cue/timeline state | exact state and ordering |

Wider thresholds need a committed justification file. Compare in linear light,
with separate interior/edge masks; never rely on SSIM alone.

## Oracle

Neutral experiment YAML → driven against both MadMapper (OSC/OSCQuery +
capture, in `openmapper-reference`) and OpenMapper (CLI + offscreen render) →
frame/state diff → neutral behaviour database. Vary one parameter at a time
first. Use the free MadMapper demo for discovery; rent the €39 month only once
the OSCQuery dumper, state-diff recorder, capture automation, fixtures, 100+
queued experiments, frame-diff tool and parity DB are ready.

## Hardware

Existing projector is used from milestone 2 (query EDID/modes; do not hard-code
a model). Everything else (DeckLink, MIDI controller, DMX node, camera) is
deferred until its adapter reaches a physical-verification gate; software
loopbacks first. Target 4K60-class on capable hardware; qualify against the
projector's actual native mode.

## Human intervention

Only for: unprovisionable credentials/payment, disconnected physical hardware,
unattended-impossible OS permission dialogs, legal decisions reserved for a
solicitor/user, irreversible purchases. Everything else → DECISIONS.md.

## Release gates (before any public beta)

FFmpeg LGPL/codec patent audit (engineering record: docs/release/licence-audit.md,
decision D-031); NDI/vendor SDK redistribution audit (D-022, D-033); no
proprietary artefacts in history (`cargo xtask provenance --history`). The
Australian IP solicitor review of the clean-room record was waived by the owner
on 2026-10-08 (D-033, docs/release/legal-posture.md).
