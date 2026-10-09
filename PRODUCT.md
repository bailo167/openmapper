# OpenMapper Product Definition

**OpenMapper is an open-source, production-grade, cross-platform system for
projection mapping, live visuals, show control, LED/DMX mapping and real-time
media output. It uses an independently implemented Rust engine, supports
standard creative-media protocols and formats, and targets the capability
and reliability of established commercial mapping tools as the minimum 1.0
product class.**

"Parity" means workflow and observable-function parity where useful — not a
byte-compatible, UI-identical or project-file-compatible clone.

## 1.0 target

| Domain | OpenMapper 1.0 target |
|---|---|
| Media | Images, folders/sequences, video, audio, cameras/capture, live textures/network sources |
| Mapping | Quads, triangles, circles, lines, masks, meshes, UV manipulation, perspective/projective mapping |
| Rendering | Opacity, colour controls, compositing/blend modes, effects, feedback, generators |
| Materials | Standard ISF/GLSL compatibility plus original OpenMapper extensions |
| Output | Multiple screens/projectors, arbitrary output regions, virtual buffers, fullscreen, snapshots, soft-edge blending |
| Show control | Cues/scenes, timelines, markers, fades/crossfades, looping, scheduling hooks |
| Control | OSC, OSCQuery, MIDI, Art-Net, sACN/E1.31, configurable control bindings |
| Lighting | DMX fixture definitions, LED/pixel mapping, media-to-fixture sampling |
| Live video | Syphon (macOS), Spout (Windows), NDI, cameras, optional DeckLink |
| 3D | OBJ/world geometry, projector calibration, camera-assisted calibration, 3D mapping |
| Reliability | Autosave, journal recovery, atomic project writes, missing-media relinking, device reconnect |
| Extension | Sandboxed WASM plugin system |
| Automation | CLI/API for deterministic external control; **no AI runtime** |

ISF support implements the **public ISF specification** independently, with an
explicit OpenMapper extension namespace.

## Permanent exclusions

- No AI/LLM/MCP functionality in the shipped product. AI tooling is development
  machinery only.
- No reference-product icons, imagery, shader libraries, example projects, presets or UI
  artwork.
- No proprietary project-file import in 1.0.

## Deferred to 1.x

- Laser/ILDA output (physical-safety obligations need a separate design review).
- Standalone playback node and distributed render nodes — the project schema and
  engine interfaces must anticipate them so they do not require a core rewrite.

## Licence

First-party code: `Apache-2.0` only (no MIT dual licence). See LICENSE, NOTICE
and THIRD_PARTY.yml. FFmpeg is used only through a dynamically linked,
LGPL-only adapter (see docs/PLAN.md).
