# Milestone Status

One milestone is active at a time. Detail and acceptance criteria: docs/PLAN.md
and `prompts/`.

| # | Milestone | Status |
|---|---|---|
| 1 | Foundation | **verified** (3-OS CI green) |
| 2 | Mapping renderer | done — renderer verified on 3 OSes; **projector output awaits your physical test** |
| 3 | Media engine | **verified** (3-OS CI; audio device output checked manually on macOS) |
| 4 | Surfaces, masks, effects, ISF | **verified** (3-OS CI incl. WARP and lavapipe) |
| 5 | Control & show engine | **verified** (3-OS CI; physical MIDI controller sign-off pending hardware) |
| 6 | Live video & pro I/O | **verified** (3-OS CI incl. Syphon on macOS, Spout on Windows/WARP; camera, NDI and DeckLink await hardware/runtime — see docs/parity/m6-live-io.yaml) |
| 7 | DMX & LED mapping | done — software **verified** on 3-OS CI (packet goldens, pixel-mapping goldens, compressed 30-min stream); **physical Art-Net/sACN node test awaits hardware** |
| 8 | Advanced mapping & calibration | done — software verified (synthetic calibration bound, blend, structured light; 3-OS CI re-run pending); **physical projector/camera calibration awaits hardware** |
| 9 | Plugins & resilience | done — hanging plugin terminated, crash injection, N-2 migration, relink (3-OS CI re-run pending) |
| 10 | Parity audit & production hardening | **active — verdict NOT READY** (RELEASE_GAPS.md): software gaps closed, security review fixed, packaging/notices/provenance tooling in place; blocked on hardware, soak, FFmpeg decision, signing and legal review |
