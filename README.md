# OpenMapper

Open-source projection mapping, LED/DMX mapping and show control for
macOS, Windows and Linux. You place **surfaces** on a **canvas**, assign
**media** to them, and send the result to projectors, LED fixtures and other
applications; shows run from **cues**, **timelines** and external control
(OSC, OSCQuery, MIDI, DMX).

OpenMapper is an independent implementation in Rust, licensed under
Apache-2.0.

## Features

- **Mapping:** quads, triangles, circles, lines, masks and meshes, with UV
  editing, perspective mapping, output regions, corner pin and soft-edge
  blending.
- **Media:** video, images, image sequences and audio. Bundled LGPL FFmpeg
  decodes H.264, ProRes, DNxHD, HAP, MPEG-2/4, VP8/VP9, FFV1 and MJPEG
  (not HEVC).
- **Effects:** colour controls, blend modes and ISF/GLSL shaders
  (the public ISF specification), plus sandboxed WebAssembly plugins.
- **Show control:** cues with fades, timelines with keyframes, and LFO or
  audio modulators. *Run show* (`--play`) puts the output fullscreen for
  unattended shows.
- **Control:** OSC, OSCQuery, MIDI (with learn), Art-Net and sACN input.
- **Lighting:** DMX fixtures and LED pixel mapping over Art-Net and sACN.
- **Live video:** cameras and capture devices, Syphon (macOS), Spout
  (Windows), NDI (with the user's own NDI runtime) and SRT/UDP/RTP streams.
- **3-D:** OBJ geometry, projector calibration from point pairs, and
  camera-assisted structured-light calibration.
- **Reliability:** atomic saves, a recovery journal, missing-media relinking
  and device reconnect.
- **Command line:** `openmapper-cli` creates, validates, edits and renders
  projects offscreen, with DMX tools and a GPU soak test.

There is no AI or cloud component in the product.

## Download

Get the archive for your platform from the
[releases page](https://github.com/bailo167/openmapper/releases), together
with `SHA256SUMS` and `SHA256SUMS.sigstore.json`.

```bash
shasum -a 256 -c --ignore-missing SHA256SUMS
```

`SHA256SUMS` is signed keylessly with Sigstore by the release workflow;
[docs/release/signing.md](docs/release/signing.md) shows how to verify it
with `cosign`.

Unpack the archive anywhere and run `openmapper` (the desktop app) or
`openmapper-cli`. The binaries are **not code-signed**:

- **macOS:** clear quarantine (`xattr -dr com.apple.quarantine openmapper-*/`)
  or use System Settings ▸ Privacy & Security ▸ *Open Anyway*.
- **Windows:** SmartScreen asks once: *More info* ▸ *Run anyway*.

Then open a project, or start one with `openmapper-cli new`:

```bash
./openmapper path/to/show.omproj          # editor
./openmapper path/to/show.omproj --play   # fullscreen show on the project's output
```

## Documentation

- [User guide](docs/user-guide.md), the place to start; it is also inside
  every archive.
- Reference: [effects](docs/effects.md), [ISF](docs/isf.md),
  [media](docs/media/), [live I/O](docs/live-io.md),
  [control](docs/control.md), [DMX](docs/dmx.md),
  [calibration](docs/calibration.md), [plugins](docs/plugins.md),
  [project format](docs/project-format.md).

## Status of 1.0

Every software requirement for 1.0 is met and verified on CI on all three
operating systems. Physical tests were done on macOS and Linux, including
30-minute projector runs. Some hardware sign-offs are **software-verified,
not hardware-verified** in 1.0:
- DMX nodes, MIDI controllers and projector calibration;
- Windows and Linux cameras, and NDI on Windows and Linux;
- output hot-plug and the 12–24 h soak;
- the Windows clean install and projector run.

[RELEASE_GAPS.md](RELEASE_GAPS.md) records each one.

## Building from source

Requires stable Rust (see `rust-toolchain.toml`) and FFmpeg shared libraries
for development ([docs/media/ffmpeg.md](docs/media/ffmpeg.md)).

```bash
cargo run --release -p openmapper -- path/to/show.omproj
cargo xtask ci        # the full quality gate
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CLEANROOM.md](CLEANROOM.md).
OpenMapper is a clean-room project: never submit code, assets or
implementation details taken from proprietary software. Report security
issues as described in [SECURITY.md](SECURITY.md).

## Licence

Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). Release archives bundle
LGPL-2.1 FFmpeg libraries, which you can replace; their source is attached to
each release. Third-party notices are in `THIRD_PARTY_LICENSES.txt` inside
each archive.

NDI® is a registered trademark of Vizrt NDI AB. OpenMapper is not affiliated
with Vizrt.
