# Live video and pro I/O

Milestone 6. How OpenMapper takes live video in and publishes its outputs
to other applications and the network. Decisions: DECISIONS.md D-020 to
D-023.

## Model

Two supervisors in `om-media-core` do the hard parts once for every
adapter:

- **`LiveFeed`** runs one input on its own thread. It keeps **only the
  newest frame** (live video never queues; a late frame is worthless),
  counts frames that were replaced before anyone took them (`dropped`),
  and treats a source that stops delivering as lost. It then reopens it
  with exponential back-off (250 ms doubling to 4 s) until the feed is
  dropped. **The last good frame stays on screen while it reconnects.**
- **`PublishFeed`** runs one output target on its own thread. The renderer
  hands it frames without blocking. The sink sends each new frame once, or
  for fixed-rate sinks such as encoded streams the newest frame at each
  tick. A failed sink is reopened with the same back-off.

Dropping either supervisor stops it within a bounded time (2 s). A driver
stuck in a call that never returns is detached instead of freezing the
app.

Adapters implement `LiveOpener`/`LiveSource` and `SinkOpener`/`FrameSink`.
`om-platform` assembles the ones this build and OS provide. It is the only
place the app and CLI get adapters, so both always offer the same set.

Frames cross the CPU (sRGB RGBA8, straight alpha) for now: the output is
read back from the GPU through a non-blocking ring (`om-render`
`FrameReader`), and received frames are uploaded like any other media. A
zero-copy GPU path is a later optimisation and does not change the project
format.

## Identity

Devices and senders are stored **by name**, not by index, port or
session id. A camera is therefore found again after it is unplugged,
moved to another port or the machine restarts, and a sender after its
application restarts (D-020).

## Inputs (`MediaSource::Live`)

| `type` | Field(s) | Adapter | Platforms |
|---|---|---|---|
| `camera` | `device` | FFmpeg (AVFoundation / DirectShow / V4L2) | all |
| `stream` | `url` | FFmpeg | all |
| `ndi` | `source` (`MACHINE (Name)`) | om-ndi, needs an installed NDI runtime | all |
| `syphon` | `server`, optional `app` | om-syphon | macOS |
| `spout` | `sender` | om-spout | Windows |

Stream URL schemes: `srt`, `udp`, `rtp`, `rtsp`, `rtmp`, `tcp`, `http`,
`https`. A receiver can wait for a sender with FFmpeg's listen options, for
example `tcp://0.0.0.0:9000?listen=1` or `srt://:9000?mode=listener`.

A network stream that is silent for 5 s counts as lost and is reopened.
Syphon and Spout senders may legitimately stop sending while their picture
is unchanged. Those inputs are not timed out; they are reported lost when
the sender leaves the directory.

## Publishing (`Output.publish`)

Each output can publish to up to 8 targets besides its window:

| `type` | Fields | Notes |
|---|---|---|
| `syphon` | `name` | macOS. BGRA8 Metal texture. |
| `spout` | `name` | Windows. BGRA8 D3D11 shared texture. Names must be unique on the machine. |
| `ndi` | `name` | Needs an installed NDI runtime. RGBA, progressive. |
| `stream` | `url`, `codec`, `fps` | Push to `srt`, `udp`, `rtp` or `tcp`. |

Stream codecs:
- `compatible` (default): MPEG-2 video in MPEG-TS. It plays in VLC, OBS,
  FFmpeg and most media servers. BT.709, limited range, intra refresh every
  half second, no B-frames. Rates 24/25/30/50/60.
- `lossless`: FFV1 (BGRA, alpha kept) in Matroska. Every pixel arrives
  bit-exact, at high bandwidth. It needs a reliable transport (`tcp` or
  `srt`).

A stream connects on its first frame. A change of output size ends the
stream and it is reopened at the new size.

## Adapters

### Cameras and streams (FFmpeg)

Every blocking FFmpeg call runs under an interrupt callback. A vanished
camera or silent stream therefore becomes an error and a reconnect, not a
hang. There is no separate `om-live-input` crate: FFmpeg already covers
capture devices on all three OSes (D-020). On macOS the camera list comes
from AVFoundation, whose names match FFmpeg's. The first use asks for camera
permission (a one-time OS dialog). macOS attributes the request to the app
that launched OpenMapper: run from Terminal, it is Terminal that needs
camera access; a future signed app bundle must declare
`NSCameraUsageDescription` in its `Info.plist`.

Cameras are opened at 1920×1080, then 1280×720, then the device's own
choice, each at 30, 25, 60 or 15 fps before FFmpeg's default 29.97, which
many devices reject; without a size, Mac cameras pick a portrait mode. On
macOS the native NV12 format is requested. Capture devices may answer
"try again" when no new picture is ready, and some ignore FFmpeg's
interrupt, so that wait happens in OpenMapper's own loop, which returns
within the supervisor's poll interval and gives up after the no-data
timeout.

### Syphon (macOS)

`om-syphon` builds the Syphon framework from vendored, pinned BSD source
(`crates/om-syphon/vendor/syphon`, THIRD_PARTY.yml) plus a small
Objective-C shim, as a static library: there is no framework to install or
ship. One documented patch: the Metal renderer compiles its shaders from
embedded source when no framework bundle exists.

Syphon announces servers with distributed notifications. macOS delivers
these on the **main thread's run loop**. The desktop app's event loop runs
it. Command-line tools call `om_platform::pump_events` from the main thread,
and the Syphon tests use a custom harness that runs it.

Frames are published unflipped (Metal orientation, first row at the top)
and read back the same way.

### Spout (Windows)

`om-spout` implements the Spout 2 conventions directly. It does not link the
Spout SDK; the layouts follow the BSD-licensed SDK:
- `SpoutSenderNames`: shared memory holding consecutive 256-byte,
  NUL-terminated names, kept sorted. Capacity is `MaxSenders` from
  `HKCU\Software\Leading Edge\Spout`, 64 by default. It is guarded by the
  named mutex `SpoutSenderNames_mutex`.
- One 280-byte block per sender, named after it: shared handle, width,
  height, DXGI format, usage, 256-byte description (the program path),
  partner id.
- `ActiveSenderName`, set when empty or stale.
- `<name>_SpoutAccessMutex` around texture access, and the frame counter
  semaphore `<name>_Count_Semaphore` (take one, give two per frame).
- The texture is a legacy DXGI shared texture (`D3D11_RESOURCE_MISC_SHARED`),
  BGRA8 for sending. Receiving converts BGRA/RGBA 8-bit, 10:10:10:2,
  16-bit unorm/float and 32-bit float.

The D3D11 device is the default adapter, or WARP when there is no GPU (as
on CI). Sender and receiver must share an adapter, so on multi-GPU
machines they need to agree; selecting the adapter is future work. A
sender waits for the GPU to finish each update before releasing the access
mutex, so receivers never see a half-written frame.

### NDI

NDI® is a registered trademark of Vizrt NDI AB; OpenMapper is not affiliated
with or endorsed by Vizrt. OpenMapper **never ships NDI**. `om-ndi` loads an
NDI runtime that the user installed (NDI Tools or a vendor installer) at
run time. Search order:
`OM_NDI_LIBRARY` (a file), then `NDI_RUNTIME_DIR_V6` / `NDI_RUNTIME_DIR_V5`,
then the platform's usual locations and library path. Without a runtime,
NDI inputs and outputs report "the NDI runtime is not installed" and
nothing else is affected. Only the runtime's public C entry points are
declared (D-022). Status: **implemented-unverified** until tested on a
machine with the runtime (`OM_REQUIRE_NDI=1 cargo test -p om-ndi`).

### DeckLink

Deferred until hardware is available (D-023).

## Command line

```
openmapper-cli live list                       # cameras, NDI, Syphon/Spout senders
openmapper-cli live probe "FaceTime HD Camera" --seconds 5 --png frame.png
openmapper-cli live probe srt://:9000?mode=listener
openmapper-cli live probe ndi:"STUDIO (Main)"
openmapper-cli live probe syphon:"OpenMapper Output 1"
openmapper-cli live probe spout:"OpenMapper Output 1"
```

## Verification

| Feature | Evidence |
|---|---|
| Supervisors | Unit tests for back-off, stalls, newest-frame-wins and prompt shutdown |
| Streams | Bit-exact lossless TCP loopback, in-process and cross-process; MPEG-2/UDP colour round trip; reconnect stress; engine publish→receive loopback |
| Syphon | macOS CI: bit-exact loopback with discovery and resize, reconnect rounds, cross-process bit-exact |
| Spout | Windows CI (WARP): the same tests, plus duplicate-name refusal |
| NDI | Layout and conversion unit tests; loopback (lossy-codec tolerance) where a runtime is installed — passed on macOS |
| Cameras | macOS: physical test passed (1080p30, no reconnects). Windows and Linux: implemented-unverified, need a physical camera |

Interoperability with *third-party* Syphon/Spout applications follows
their published conventions, but each is checked only by hand (release
gate D in docs/PLAN.md).
