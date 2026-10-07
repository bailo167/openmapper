# Soak testing

The release gate needs a 12–24 hour run on the reference system
(RELEASE_GAPS.md G-09). This describes the tooling and the procedure.

## Tooling

`openmapper-cli soak PROJECT --seconds N [--max-rss-growth-mib M]` renders
the project continuously offscreen (media, effects, shaders, plugins) and
fails if:

- GPU resources (textures, buffers, pipelines, models) change after the
  first 120 frames, or
- resident memory grows by more than `M` MiB (default 256) after a minute
  of warm-up (Linux reports resident memory; other platforms report GPU
  resources only).

Every 10 s it prints elapsed time, frames, frame rate, GPU resource counts
and resident memory. The last line reports warm-up and peak memory.

Shorter checks that run in CI: the 30-minute compressed DMX stream test
(M7), the 10-minute no-drift video test in release builds on every OS (M3),
render-resource stability tests (M2) and the crash-injection test (M9).

## Reference-system procedure (human)

1. Use the show machine, its GPU and its projectors; the real show project
   with its media (video loops, live inputs if used), effects, DMX fixtures
   and control mapping. Disable sleep and screen savers.
2. Render path: `openmapper-cli soak show.omproj --seconds 43200` (12 h) and
   keep the log.
3. Full system: start `openmapper show.omproj --play` with outputs enabled,
   DMX sending and OSC/MIDI connected; run 12–24 h. Every few hours check
   the output for drift or stalls, the DMX fixtures, and the memory and CPU
   use in the OS monitor (record readings).
4. Pass: no crash, no visible drift or stall, memory flat after warm-up
   (within the soak limit), DMX continuous, control responsive. Record the
   machine, OS, GPU driver, FFmpeg version and logs in the release notes.
