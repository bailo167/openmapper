# OpenMapper Project Format

## Principles

- Human-readable JSON (`.omproj`).
- Versioned from the first commit.
- Deterministically serialised (stable key order, stable float formatting).
- Unknown extension fields survive round-trip.
- No machine-specific absolute path is authoritative.
- Every incompatible schema change has a migration.
- A project is never modified in place before a successful replacement file
  has been fully written.

## Root

```json
{
  "format": "openmapper-project",
  "version": 1,
  "project_id": "...",
  "name": "...",
  "timebase": { "ticks_per_second": "254016000000" },
  "assets": {},
  "media": [],
  "surfaces": [],
  "outputs": [],
  "show": { "cues": [], "timelines": [] },
  "extensions": {}
}
```

Large integers that may exceed 2^53 (ticks) are serialised as decimal strings.

## Asset references

Prefer:
1. content hash;
2. project-relative path;
3. last-known absolute path only as a recovery hint.

## IDs

Persistent objects use stable opaque IDs (ULID strings). Array position is
never identity.

## Numeric values

Persistent geometry uses finite validated floating-point values.
Time uses exact integer/rational representations.
NaN and Infinity are rejected on load and on command validation.

## Live media and publishing

A media item's `source` may be live instead of file-backed (docs/live-io.md):

```json
{ "kind": "live", "input": { "type": "camera", "device": "FaceTime HD Camera" } }
{ "kind": "live", "input": { "type": "stream", "url": "srt://:9000?mode=listener" } }
{ "kind": "live", "input": { "type": "ndi", "source": "STUDIO (Main)" } }
{ "kind": "live", "input": { "type": "syphon", "server": "Main", "app": "Resolume" } }
{ "kind": "live", "input": { "type": "spout", "sender": "Main" } }
```

Devices and senders are stored by name, not index (`app` may be omitted
to match any application). Each output may also publish its frames, with
at most 8 targets:

```json
"publish": [
  { "type": "syphon", "name": "OpenMapper Output 1" },
  { "type": "stream", "url": "srt://192.168.1.20:9000", "codec": "compatible", "fps": 30 }
]
```

`codec` is `compatible` (MPEG-2/TS, default) or `lossless` (FFV1/MKV,
`tcp`/`srt` only); `fps` is one of 24, 25, 30, 50, 60. Names are 1–255
bytes. Both lists are validated on load and by the `SetOutputPublish`
command.

## DMX and LED fixtures

Optional `dmx` section (omitted when empty; see docs/dmx.md):

```json
"dmx": {
  "rate": 40,
  "nodes": [
    { "id": "…", "name": "Pixel node", "enabled": true,
      "protocol": { "type": "art_net", "address": "2.255.255.255" } },
    { "id": "…", "name": "sACN", "enabled": true,
      "protocol": { "type": "sacn", "priority": 100 } }
  ],
  "fixtures": [
    { "id": "…", "name": "Strip", "enabled": true, "node": "…",
      "universe": 1, "address": 1, "order": "grb", "encoding": "srgb", "brightness": 1.0,
      "shape": { "kind": "line", "from": [0.1, 0.5], "to": [0.9, 0.5], "count": 60 } },
    { "id": "…", "name": "Matrix", "node": "…", "universe": 2, "address": 1,
      "shape": { "kind": "grid", "corners": [[0.3,0.2],[0.7,0.2],[0.7,0.8],[0.3,0.8]],
                 "columns": 16, "rows": 16, "wiring": "rows_snake" } }
  ]
}
```

Validation: rate 1–44; node addresses are IPv4 (sACN may be empty, which
means multicast); fixture `address` 1–512 with its first pixel fitting;
1–65536 pixels; every fixture's universes within its node's protocol
range (Art-Net 0–32767, sACN 1–63999); ids unique; nodes referenced by
fixtures cannot be removed.

## Output mapping and 3-D projection

Outputs may carry a `mapping` (omitted when it is the identity) and a
`projection` (see docs/calibration.md):

```json
{ "id": "…", "name": "Left", "enabled": true,
  "mapping": {
    "region": [[0,0],[0.6,0],[0.6,1],[0,1]],
    "warp":   [[0.02,0.01],[0.99,0],[1,1],[0,0.98]],
    "soft_edge": { "right": 0.333, "curve": 2.0, "gamma": 2.2 }
  } }

{ "id": "…", "name": "Stage",
  "projection": {
    "model": "models/stage.obj",
    "projector": { "width": 1920, "height": 1080, "fx": 2210.5, "fy": 2209.8,
                   "cx": 961.2, "cy": 1012.7,
                   "rotation": [0.31, -0.52, 0.04], "translation": [0.01, -0.3, 3.02] },
    "points": [ { "world": [-0.5, -0.5, -0.5], "pixel": [512.25, 288.5] } ]
  } }
```

Validation: region and warp must be mappable quads (no three corners
collinear, no self-intersection); soft-edge widths 0–0.5, curve 1–8,
gamma 0.5–4; at most 1000 calibration points; projector resolution
1–16384 and positive focal lengths. When `projection` is set, `mapping`
is not used.

## Plugins on media

`Media.plugins` (omitted when empty) lists WebAssembly filters applied in
order (docs/plugins.md): `{ "path": "plugins/invert.wasm", "enabled":
true, "params": { "amount": 1.0 } }`. At most 8 per media item; paths
follow the media path rules.

## External control

`controls` (all optional): `osc_port` (default 8010, 0 = off),
`oscquery_port` (8011, 0 = off), `network` (default false: listen on this
computer only; true: all interfaces and mDNS), `midi` (bindings), and
`dmx_input` (omitted when unused): `{ "enabled": false, "bindings": [ {
"universe": 1, "channel": 10, "fine": true, "target": { "kind": "param",
"param": "master/opacity" } } ] }`. Channels are 1–512 (1–511 for 16-bit
`fine` pairs); at most 4096 MIDI and 4096 DMX bindings (docs/control.md).

Opening a file never starts its camera, network, DMX or remote-control
connections by itself: the application asks first (DECISIONS.md D-029).
Size limits when reading: the project file 64 MiB; files it names are read
only if they are regular files (shaders 256 KiB, plugins 16 MiB, OBJ
models 512 MiB).

## Recovery

Normal save:

```
project.omproj.tmp → write → fsync → validate (re-parse) → atomic rename → project.omproj
```

Accepted commands since the last successful save are appended to the recovery
journal (`project.omproj.journal`, JSON lines) and replayed only when the
journal's base revision matches the saved project's revision.

## Compatibility

Readers:
- reject unknown format families and future versions;
- migrate supported old versions forward;
- preserve unknown `extensions` payloads.

Writers:
- only write the newest supported schema.
