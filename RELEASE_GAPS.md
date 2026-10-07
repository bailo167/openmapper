# Release gaps — OpenMapper 1.0

**Verdict (2026-10-07): NOT READY.** Every software requirement of the
release gate that can be met without hardware, accounts or legal advice is
met; the items below cannot be closed by automation and each blocks 1.0.
Per prompts/10-release.md, a release is never made because a date arrived.

Status of the gate (prompts/10-release.md):

| Requirement | State |
|---|---|
| All P0/P1 parity rows verified or waived | **52 of 58 verified**; 6 need hardware (G-01…G-05). 1 P2 row deferred (DeckLink, D-023). |
| Clean macOS/Windows/Linux package install | Archive + checksum + unpacked-CLI smoke automated (`cargo xtask dist`, `smoke`, CI `package` job). **Blocked** on FFmpeg distribution (G-07) and signing/notarisation (G-08). |
| 12–24 h reference-system soak | Tooling ready (`openmapper-cli soak`, GPU resources + resident memory); software soak run in CI-class environment (docs/release/soak.md). **Reference-system run pending** (G-09). |
| Crash/recovery injection tests | Verified: 40 random process kills per run, 600 locally (M9). |
| Project migration tests | Verified: N-2 chain, v1 compatibility fixture (M9). |
| Physical projector validation | **Pending** (G-01, G-04). |
| MIDI / DMX / live-I/O sign-off where hardware exists | Software verified on 3-OS CI (virtual MIDI, loopback DMX, Syphon, Spout); **physical sign-off pending** (G-02, G-03, G-05, G-06). |
| Dependency / licence / NOTICE audit | Done: docs/release/licence-audit.md; `cargo deny` clean; `THIRD_PARTY_LICENSES.txt` generated into every archive. Legal confirmation in G-10. |
| FFmpeg distribution audit | Facts recorded (docs/release/licence-audit.md, docs/media/ffmpeg.md); **decision pending** (G-07). |
| Clean-room evidence audit | `cargo xtask provenance` (tree) and `--history` (all 685 blobs in all refs): clean. Solicitor review pending (G-10). |
| No proprietary artefacts in history | Verified by `provenance --history` (one reviewed false positive). |
| Security review of WASM and FFI boundaries | Done, findings fixed with tests: docs/release/security-review.md. External review recommended (G-11). |
| User documentation | docs/user-guide.md plus reference docs, shipped in the archive. |
| Signed checksum generation | `SHA256SUMS` generated and verified; **signing needs the release key** (G-08). |

## Blocking items

Each needs a person, hardware, an account or professional advice.

### G-01 Projector output (P0 `output.fullscreen-display`)
With a physical projector on macOS, Windows and Linux: display
enumeration, fullscreen on the chosen display, hot-plug (unplug/replug
while running), sleep/wake, and a 30-minute run without drift or resource
growth. Procedure: docs/release/checklist.md §Hardware.

### G-02 Camera input (P0 `live.camera`)
A real camera on each OS, including the OS permission prompt (cannot be
answered unattended), disconnect/reconnect, and a project-trust prompt for
a shared project using the camera.

### G-03 DMX node and console (P0 `dmx.physical-node`)
An Art-Net node and an sACN receiver driving an LED fixture from a pixel
mapping (colour order, universe wrap, 30-minute stream), and a lighting
console driving DMX input bindings (8- and 16-bit, learn).

### G-04 Physical calibration and blending (P0 `calibration.physical-projector`, P1 `calibration.camera-assisted`)
Calibrate a real projector against a real object from point pairs (error
within the documented bound), blend two overlapping projectors, and run the
structured-light workflow with a real camera.

### G-05 NDI runtime (P1 `live.ndi`)
Install the NDI runtime on each OS and run the NDI tests with
`OM_REQUIRE_NDI=1` (send/receive loopback, discovery, reconnect).

### G-06 MIDI controller
A physical MIDI controller: hot-plug, learn, CC and notes driving
parameters and cues (virtual-port tests already pass on 3-OS CI).

### G-07 FFmpeg distribution decision
The binaries link FFmpeg at load time and do not start without its shared
libraries; macOS users' usual source (Homebrew) is a GPL build. Decide:
(a) bundle an LGPL FFmpeg build per platform following
docs/media/ffmpeg.md (publish matching source and build recipe), or
(b) load FFmpeg at run time and run without video when it is absent. Then
the codec-patent question below (G-10). Engineering for (a) or (b) follows
the decision.

### G-08 Signing and notarisation
Sign `SHA256SUMS` with the release key; code-sign and notarise the macOS
app (Apple Developer ID), Authenticode-sign the Windows binaries. Needs the
publisher's keys and accounts. Unsigned macOS apps are blocked by
Gatekeeper, so clean-install testing on macOS also waits for this.

### G-09 12–24 hour soak on the reference system
Run the show configuration on the reference machine with real projectors,
media, DMX and control for 12–24 h: `openmapper-cli soak` for the render
path, and the desktop app with outputs and DMX for the full system.
Procedure: docs/release/soak.md.

### G-10 Legal review
Australian IP solicitor review of the clean-room record (CLEANROOM.md,
provenance results); FFmpeg LGPL compliance and codec patents for any
distributed FFmpeg; NDI name/brand use and attribution; confirmation of the
third-party notices.

## Recommended before a public release (not blocking)

### G-11 External security review
docs/release/security-review.md is an internal engineering review. An
external review of the plugin sandbox, FFI adapters and network surfaces is
recommended before wide distribution.

## Waived

| Row | Priority | Reason |
|---|---|---|
| `live.decklink` | P2 | Deferred (D-023): needs the vendor SDK and hardware; not part of 1.0. |
