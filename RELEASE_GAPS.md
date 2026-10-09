# Release gaps — OpenMapper 1.0

**Verdict (2026-10-09): NOT READY — close.** Every software
requirement is met. The owner waived the remaining hardware sign-offs for
v1 as **"software-verified, not hardware-verified"** (DMX, MIDI,
calibration, hot-plug, Windows/Linux cameras, NDI on Windows/Linux, the
12–24 h soak), kept binaries unsigned for 1.0 (G-08) and waived the legal
review (G-10). The release archives were installed and run on real
machines for the first time (macOS, and the Linux show machine, which now
boots into the show and is controlled from the Mac over OSC); that found
and fixed a P0 output bug (D-034). CI runs again (the repository is now
public) and built, smoke-tested and signed the 1.0.0 archives on all
three OSes (G-13, run 37894912943); the Linux one runs the show on
octv01. The macOS projector run and projector sleep/wake passed on
2026-10-09 (G-01). Still open: the Windows clean install and Windows
projector run (G-12, G-01), and the owner's go-ahead to tag.
Per prompts/10-release.md, a release is never made because a date arrived.

Status of the gate (prompts/10-release.md):

| Requirement | State |
|---|---|
| All P0/P1 parity rows verified or waived | **52 of 58 verified; 5 waived** for v1 as software-verified, not hardware-verified (owner, 2026-10-08: camera, NDI, DMX node, both calibration rows); `output.fullscreen-display` passes on Linux and macOS, Windows projector run open (G-01). 1 P2 row deferred (DeckLink, D-023). |
| Clean macOS/Windows/Linux package install | Archive bundles an LGPL FFmpeg (D-031); `cargo xtask dist` verifies the bundle, `smoke` decodes video from the unpacked archive; CI `package` job on all three OSes. **macOS and Linux installs from the CI archive done 2026-10-08** (G-12); Windows pending. Binaries unsigned for 1.0, owner's decision (G-08). |
| 12–24 h reference-system soak | Tooling ready (`openmapper-cli soak`, GPU resources + resident memory); 1-hour software soak passed (stable GPU resources; memory 343→361 MiB peak, to be confirmed flat over 12–24 h; docs/release/soak.md). **Waived for v1** by the owner (G-09). |
| Crash/recovery injection tests | Verified: 40 random process kills per run, 600 locally (M9). |
| Project migration tests | Verified: N-2 chain, v1 compatibility fixture (M9). |
| Physical projector validation | **Linux passed**, including boot-into-show and projector sleep/wake; **macOS passed** (30 minutes over AirPlay) (G-01). Windows projector run open; physical calibration waived (G-04). |
| MIDI / DMX / live-I/O sign-off where hardware exists | Software verified on 3-OS CI (virtual MIDI, loopback DMX, Syphon, Spout); physical sign-off **waived for v1** by the owner (G-02, G-03, G-05, G-06). OSC/OSCQuery over a real LAN verified (Mac → octv01). |
| Dependency / licence / NOTICE audit | Done: docs/release/licence-audit.md; `cargo deny` clean; `THIRD_PARTY_LICENSES.txt` generated into every archive, now including FFmpeg's notice and the NDI attribution. No legal confirmation (G-10 waived). |
| FFmpeg distribution audit | **Decided and implemented** (G-07, D-031): bundled minimal LGPL build with source, recipe and policy check. |
| Clean-room evidence audit | `cargo xtask provenance` (tree) and `--history` (all blobs in all refs): clean. Report published with every release (`PROVENANCE.txt`). Solicitor review waived (G-10). |
| No proprietary artefacts in history | Verified by `provenance --history` (one reviewed false positive). |
| Security review of WASM and FFI boundaries | Done, findings fixed with tests: docs/release/security-review.md. External review recommended (G-11). |
| User documentation | docs/user-guide.md plus reference docs, shipped in the archive; now covers verification, unsigned-binary steps and the bundled codecs. |
| Signed checksum generation | `SHA256SUMS` generated and verified; **signed keylessly (Sigstore) by `release.yml`** (G-08, D-032). |

## Blocking items

Still blocking: G-01 (Windows projector run) and G-12 (Windows install).
G-13 (CI) is resolved. G-02…G-06 and G-09 are waived for v1
(software-verified, not hardware-verified) and kept here for the record.

### G-01 Projector output (P0 `output.fullscreen-display`)
With a physical projector on macOS, Windows and Linux: display
enumeration, fullscreen on the chosen display, hot-plug (unplug/replug
while running), sleep/wake, and a 30-minute run without drift or resource
growth. Procedure: docs/release/checklist.md §Hardware.
**Linux passed** on 2026-10-08 (XGIMI HORIZON 20 via octv01: enumeration
by EDID name, fullscreen, frame matches the offscreen reference, 30-minute
run with flat memory and GPU use). Still open: hot-plug, sleep/wake, and
macOS/Windows projector runs.
**Release archive and boot-into-show, 2026-10-08:** the CI archive found
that `--play` left the projector black, then frozen (output windows were
painted in the minimised control window's frame; the 30-minute run above
used static content, so it could not show this). Fixed by show mode
(D-034) and verified on the projector: moving video from the first
seconds. octv01 now auto-logs in, starts the show from a systemd user
service and is controlled from the Mac over OSC/OSCQuery, including after
a cold reboot with no input (owner's notes outside the repository).
Hot-plug is waived for v1 (owner).
**2026-10-09, CI archives of `2fab89a` (run 37894912943):**
- *Projector sleep/wake, Linux — passed.* With the show running on
  octv01, the owner put the XGIMI into standby and woke it. Each time
  the HDMI hot-plug dropped for about 2 s (KWin removed and restored the
  output once); the same process stayed fullscreen on the projector and
  in front, the output kept moving and OSC kept answering, with nothing
  to do by hand. A power-cycle at the wall was not tested (owner
  declined). The computer itself does not sleep on a show machine
  (suspend is disabled on octv01), so machine sleep was not tested.
- *macOS projector run — passed.* MacBook Air (M4, macOS 26.6.2) drove
  the XGIMI for 30 minutes with `openmapper <project> --play`. The owner
  wanted it wireless, so the path was an AirPlay extended display
  received by UxPlay on octv01 and shown on the projector over HDMI;
  OpenMapper enumerated it as `octv01 (AirPlay)` (1920×1080 @ 60 Hz) and
  show mode took it fullscreen. The burned-in timecode of the looping
  60 s clip, read at 7 points over 27 minutes, matched wall-clock time
  within ±17 ms (one frame at 30 fps is 33 ms; no drift and no gap at the
  27 loop points); 57.3 fps delivered at the start, 57.9 at the end; RSS
  389–455 MiB with no trend (418 MiB at the start, 406 MiB at the end).
  Not covered: a directly cabled projector on macOS, so matching by its
  EDID name was not exercised there.

Still open: the Windows projector run.

### G-02 Camera input (P0 `live.camera`)
A real camera on each OS, including the OS permission prompt (cannot be
answered unattended), disconnect/reconnect, and a project-trust prompt for
a shared project using the camera. **macOS capture passed** on 2026-10-08
(MacBook Air camera, 1920×1080 at 30 fps, no reconnects); still open:
Windows and Linux cameras, unplug/replug, and the trust prompt. The release
build of the macOS executable now embeds `NSCameraUsageDescription` and a
bundle identifier (apps/openmapper/macos/Info.plist); confirm the prompt
appears from the unpacked archive.
**Waived for v1 (owner, 2026-10-08): software-verified, not hardware-verified.**


### G-03 DMX node and console (P0 `dmx.physical-node`)
An Art-Net node and an sACN receiver driving an LED fixture from a pixel
mapping (colour order, universe wrap, 30-minute stream), and a lighting
console driving DMX input bindings (8- and 16-bit, learn).
**Waived for v1 (owner, 2026-10-08): software-verified, not hardware-verified.**


### G-04 Physical calibration and blending (P0 `calibration.physical-projector`, P1 `calibration.camera-assisted`)
Calibrate a real projector against a real object from point pairs (error
within the documented bound), blend two overlapping projectors, and run the
structured-light workflow with a real camera.
**Waived for v1 (owner, 2026-10-08): software-verified, not hardware-verified.**


### G-05 NDI runtime (P1 `live.ndi`)
Install the NDI runtime on each OS and run the NDI tests with
`OM_REQUIRE_NDI=1` (send/receive loopback, discovery, reconnect).
**macOS passed** on 2026-10-08 with an installed runtime; Windows and Linux
remain, plus interop with a third-party NDI application.
**Waived for v1 (owner, 2026-10-08): software-verified, not hardware-verified.**


### G-06 MIDI controller
A physical MIDI controller: hot-plug, learn, CC and notes driving
parameters and cues (virtual-port tests already pass on 3-OS CI).
**Waived for v1 (owner, 2026-10-08): software-verified, not hardware-verified.**


### G-09 12–24 hour soak on the reference system
Run the show configuration on the reference machine with real projectors,
media, DMX and control for 12–24 h: `openmapper-cli soak` for the render
path, and the desktop app with outputs and DMX for the full system.
Procedure: docs/release/soak.md.
**Waived for v1 (owner, 2026-10-08): software-verified, not hardware-verified.**

### G-12 Clean-machine install check (from the former G-07/G-08 blocker)
On a fresh macOS, Windows and Linux machine or VM without developer tools
or FFmpeg: unpack the archive, verify `SHA256SUMS` and its Sigstore bundle,
follow the unsigned-binary steps (docs/release/signing.md), start the app,
open a project with an H.264 and a ProRes clip, render, quit. Automated
equivalents (`cargo xtask smoke` on runners without FFmpeg packages) pass.
**macOS and Linux done 2026-10-08** with the CI archives of `d5b3cda` (run
37742769233): checksums verified, `openmapper-cli ffmpeg --require-release`
passes, the bundled n9.0.2 libraries are the ones loaded (`ldd`/`LD_DEBUG`
on Fedora 44, which has its own FFmpeg 8.1; `@executable_path/lib` rpath on
macOS, which has Homebrew FFmpeg), and an H.264 and a ProRes clip render
and play in the desktop app. Neither machine was free of developer tools,
which the bundle check makes irrelevant for FFmpeg. Found: `openmapper
--help` opened the app; no `.desktop` file or Linux menu instructions;
network control could not be allowed without a screen; `displays` over SSH
gave an unhelpful error; the build still said 0.1.0 — all fixed (D-034).
Not exercised: the browser-download quarantine path on macOS (the archive
came via `gh`). **Still open: Windows.**

### G-13 CI was blocked by GitHub billing — resolved 2026-10-09
From 2026-10-08 ~08:40 UTC GitHub refused to start Actions jobs on the
private repository ("recent account payments have failed or your spending
limit needs to be increased"). The owner chose to make the project public,
where Actions are free: the repository was re-created as public
`bailo167/openmapper` with the same tree and commits, re-authored to the
owner's GitHub no-reply address (the private original is kept as
`openmapperold`; commit IDs quoted in older entries refer to it). The first
manual 3-OS run on the public repository (37894912943, on the 1.0.0 commit
`2fab89a`) passed gate A, gate B on Windows, the `package` job on all
three OSes (archives built, bundled FFmpeg checked, smoke-tested) and the
keyless checksum signing. Its one failure, a 5 ms wall-clock bound in an
audio unit test on the macOS runner, was a test that measured runner
scheduling rather than blocking, and now uses a deliberately stalling
decoder instead. On 2026-10-09 the CI Linux archive (`SHA256SUMS`
verified) replaced the locally built one on octv01, which is kept beside
it as `…-linux-x86_64.local-build`, and runs the show; the CI macOS
archive ran the macOS projector test (G-01).


## Decided and implemented (owner-delegated)

### G-07 FFmpeg distribution — bundled minimal LGPL build (D-031)
Releases bundle FFmpeg n9.0.2 shared libraries built by
`tools/ffmpeg/build.sh` from `tools/ffmpeg/components.txt`: LGPL only, no
external libraries, decoders for production formats (no HEVC/VVC/VC-1),
encoders only for MPEG-2, FFV1, raw and PCM. `cargo xtask dist` refuses to
archive unless the staged binaries load the bundled build and pass
`openmapper-cli ffmpeg --require-release`; `smoke` decodes a clip through
the unpacked archive; the complete corresponding source archive is attached
to each release. Users can replace the libraries (docs/media/ffmpeg.md).
**Remaining for the owner:** nothing. The CI `package` job built, bundled
and smoke-tested the archive on all three OSes (run 77, 2026-10-08).

### G-08 Signing and notarisation — unsigned for 1.0, owner's decision (D-032)
`release.yml` signs `SHA256SUMS` with Sigstore (via `publish.yml`) using the
job's GitHub OIDC identity and verifies it; no key to manage. The same
signing job runs in manual `ci` runs, so it is exercised on this branch;
`release.yml` itself can only be dispatched once it exists on the default
branch. macOS Developer ID
signing/notarisation and Windows Authenticode are implemented in
`.github/scripts/` and run automatically once the secrets listed in
docs/release/signing.md exist; until then binaries are unsigned and the
user guide documents the Gatekeeper/SmartScreen steps.
**Owner's decision (2026-10-08): binaries ship unsigned for 1.0.** Later,
optionally: an Apple Developer Program
membership (US$99/yr) and a Windows code-signing certificate or Azure
Trusted Signing account, then the secrets. The signing scripts are untested
until then.

### G-10 Legal review — waived by the owner (D-033)
The Australian IP solicitor review will not be done before 1.0, by the
owner's decision of 2026-10-08. docs/release/legal-posture.md records the
waiver, what software reduced (narrow decoder-only codec set, no external
libraries, NDI attribution and non-affiliation, published provenance
report, generated notices) and the residual risks the owner carries. No
document in the repository claims legal clearance.
**Remaining for the owner:** archive the reference product's licence
agreement (CLEANROOM.md; no solicitor needed), and re-read the posture
document before any commercial distribution.

## Recommended before a public release (not blocking)

### G-11 External security review
docs/release/security-review.md is an internal engineering review. An
external review of the plugin sandbox, FFI adapters and network surfaces is
recommended before wide distribution.

## Waived

| Row | Priority | Reason |
|---|---|---|
| `live.decklink` | P2 | Deferred (D-023): needs the vendor SDK and hardware; not part of 1.0. |
| `live.camera` (Windows, Linux) | P0 | Owner, 2026-10-08: software-verified, not hardware-verified (G-02); macOS passed. |
| `live.ndi` (Windows, Linux) | P1 | Owner, 2026-10-08: software-verified, not hardware-verified (G-05); macOS passed. |
| `dmx.physical-node` | P0 | Owner, 2026-10-08: software-verified, not hardware-verified (G-03). |
| `calibration.physical-projector` | P0 | Owner, 2026-10-08: software-verified, not hardware-verified (G-04). |
| `calibration.camera-assisted` | P1 | Owner, 2026-10-08: software-verified, not hardware-verified (G-04). |
| MIDI controller, output hot-plug, 12–24 h soak | gate | Owner, 2026-10-08: software-verified, not hardware-verified (G-06, G-01, G-09). |
| G-08 code signing | gate | Owner, 2026-10-08: unsigned for 1.0 (D-032). |
| G-10 legal review | gate | Owner's decision (D-033, docs/release/legal-posture.md); risk reduced in software, residual risk accepted by the owner. |
