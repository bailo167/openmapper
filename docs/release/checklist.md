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

1. FFmpeg is bundled (D-031): the `package` job builds it and `cargo xtask
   dist` refuses to archive unless the staged binaries load the bundled
   build and `openmapper-cli ffmpeg --require-release` passes. Nothing to
   do by hand; read the `package` logs' `ffmpeg:` line.
2. Clean machines (fresh OS install or VM without developer tools or
   FFmpeg): unpack the archive, verify `SHA256SUMS`, start the app, open a
   sample project with a video, render, quit. Repeat on all three OSes.
   Expect the Gatekeeper/SmartScreen steps in docs/release/signing.md
   while the binaries are unsigned.
3. Signing (D-032): `SHA256SUMS` is signed keylessly by `release.yml`;
   nothing to do by hand. Binary code signing activates by itself once the
   secrets in docs/release/signing.md exist.

## Legal (G-10 — waived)

The solicitor review was waived by the owner (D-033,
docs/release/legal-posture.md). Before publishing, re-read that document
and confirm nothing has changed the posture (commercial distribution, a
rights-holder request). Archive the reference product's licence agreement
(CLEANROOM.md) if not yet done.

## Publish

Push a `v*` tag: `release.yml` packages all three OSes, merges and signs
`SHA256SUMS`, and opens a **draft** release with the archives, the FFmpeg
source archive, `SHA256SUMS`, `SHA256SUMS.sigstore.json` and
`PROVENANCE.txt`. Review it, add release notes listing known gaps
(RELEASE_GAPS.md), and publish.
