# Release checklist

Run top to bottom for every release candidate. RELEASE_GAPS.md records
what is still open; the release is NOT READY while any blocking gap is.

## Automated (any contributor)

1. `cargo xtask ci` — the full gate (fmt, clippy, tests, deny, provenance,
   architecture).
2. Run the `ci` workflow manually (workflow_dispatch) on the candidate
   commit: gate A, gate B (macOS, Windows) and `package` on all three OSes
   must pass. `package` runs `cargo xtask provenance --history`, builds the
   archive with `cargo xtask dist`, and smoke-tests the unpacked archive.
3. Parity: every P0/P1 row in docs/parity is `verified`, or waived in
   RELEASE_GAPS.md with a reason.
4. Download the `package` artefacts (archive + `SHA256SUMS` per OS).

## Human: hardware (RELEASE_GAPS.md G-01…G-06)

For each OS (macOS, Windows, Linux) on real hardware:

- **Projector**: list displays; enable an output on the projector
  (fullscreen on the right display); unplug and replug it while running;
  sleep and wake the machine; run 30 minutes and check for drift and stable
  memory.
- **Camera**: add a camera input; answer the OS permission prompt; unplug
  and replug the camera. Open a project from another machine that uses the
  camera and confirm it is held back until allowed.
- **DMX**: drive an LED fixture through an Art-Net node and an sACN
  receiver from a pixel mapping (strip and matrix, colour order, universe
  wrap); 30 minutes continuous. Map a console fader and button to a
  parameter and a cue (8-bit, 16-bit, learn).
- **Calibration**: calibrate a projector on a real object; blend two
  projectors; run the structured-light workflow with a camera.
- **NDI**: install the NDI runtime; `OM_REQUIRE_NDI=1 cargo test -p om-ndi`;
  send to and receive from NDI Studio Monitor.
- **MIDI**: hot-plug a controller; learn a fader and a pad; drive a
  parameter and GO.

## Human: soak (G-09)

docs/release/soak.md, 12–24 h on the reference system.

## Human: distribution (G-07, G-08)

1. FFmpeg: as decided in G-07 (bundled LGPL build with source and recipe,
   or run-time loading); verify `openmapper-cli ffmpeg` reports `LGPL` for
   anything shipped.
2. Clean machines (fresh OS install or VM without developer tools or
   FFmpeg): unpack the archive, verify `SHA256SUMS`, start the app, open a
   sample project, render, quit. Repeat on all three OSes.
3. Code-sign and notarise the macOS app; Authenticode-sign the Windows
   binaries; sign `SHA256SUMS` with the release key and publish the
   signature next to it.

## Human: legal (G-10)

Solicitor sign-off on the clean-room record, third-party notices,
FFmpeg/codec patents and NDI naming before any public release.

## Publish

Tag the commit, attach the archives, `SHA256SUMS` and its signature,
`THIRD_PARTY_LICENSES.txt`, and release notes listing known gaps.
