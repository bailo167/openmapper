# DMX and LED pixel mapping

Milestone 7. OpenMapper drives LED strips, LED matrices and single lights
by sampling the rendered canvas and sending DMX over the network with
Art-Net 4 or sACN (ANSI E1.31). Decision: DECISIONS.md D-024.

## Model

```
canvas frame (sRGB RGBA8) ──► fixtures sample their pixels ──► universes ──► nodes (Art-Net / sACN)
```

- A **node** is a network destination:
  - Art-Net to an IPv4 address. A broadcast address such as
    `2.255.255.255` reaches every node on that network.
  - sACN to the standard multicast group of each universe, or unicast to
    an address. Priority is 0–200, default 100.
- A **fixture** is an LED strip, matrix or light placed on the canvas:
  - **Shape**: `point` (one pixel); `line` (`count` pixels evenly spaced
    from `from` to `to`, both ends included); or `grid` (`columns × rows`
    pixels filling a quad, wired `rows`, `rows_snake`, `columns` or
    `columns_snake` from the top-left).
  - **Channels**: colour order `rgb`, `rbg`, `grb`, `gbr`, `brg`, `bgr`,
    `rgbw`, `grbw` or `mono`, starting at `universe`/`address`. Pixels never
    straddle universes: a pixel that does not fit starts at channel 1 of
    the next universe (170 RGB or 128 RGBW pixels per universe from
    channel 1).
  - **Brightness** scales linear light. **Encoding** is `srgb` (default:
    the values a monitor would show) or `linear`.

Fixture brightness and enabled state are live parameters
(`fixture/<id>/brightness`, `fixture/<id>/enabled`). Cues, timelines,
modulators, MIDI and OSC (`/openmapper/fixture/<id>/brightness`) can
drive them like any other parameter.

## Sampling

Canvas coordinates are normalised (`[0, 1]²`, origin top-left, as for
surfaces). Each pixel averages the canvas over its footprint **in linear
light**:
- a line pixel covers a square as wide as the pixel spacing;
- a grid pixel covers its cell;
- a point is sampled bilinearly.

Alpha counts as coverage over black. Footprints larger than 32×32 canvas
pixels are averaged on a regular sub-grid. Master opacity and blackout
apply automatically, because fixtures sample the final canvas.

## Sending

A dedicated thread sends **every universe of every enabled node** at the
project's refresh rate (1–44 Hz, default 40). DMX receivers expect a
steady refresh, and sACN receivers treat 2.5 s of silence as data loss.
Universes always carry 512 slots.

| | Art-Net | sACN |
|---|---|---|
| Port | 6454 | 5568 |
| Universe range | 0–32767 (15-bit Port-Address) | 1–63999 |
| Sequence | 1–255 per universe (0 is never sent) | 0–255 per universe, wrapping |
| Source identity | — | CID = the project id; name `OpenMapper – <project>` |
| On stop | — | 3 stream-terminated packets per universe (E1.31 6.2.6) |

Submitting a frame never blocks rendering. Network errors are counted and
shown, not fatal: a pulled cable recovers by itself. After a stall, such as
a suspended laptop, the sender resumes its rate instead of bursting.

## Command line

```
openmapper-cli dmx discover [--broadcast 2.255.255.255]   # ArtPoll → nodes and their outputs
openmapper-cli dmx monitor [--sacn 1 --sacn 2]            # print universes arriving here
openmapper-cli dmx frame show.omproj --at 12.5 [--json]   # channel values a frame produces
```

`discover` and `monitor` need UDP port 6454 free (and 5568 for sACN), so
they cannot run alongside another Art-Net program on the same machine.

## Verification

| Item | Evidence |
|---|---|
| Packet formats | Byte-for-byte goldens from the Art-Net 4 and E1.31 field tables; malformed-packet rejection |
| Pixel mapping | CPU golden (RGBW snake matrix over a known image); GPU end-to-end golden through the CLI (checkerboard → alternating channels) |
| Stream stability | A 30-minute stream at 44 Hz, compressed in time, over real sockets: every packet arrives in order with continuous sequences and bounded state |
| Pacing and shutdown | Refresh rate within tolerance; prompt stop; sACN termination |
| Node discovery | ArtPoll/ArtPollReply against a simulated node |
| Physical node | **Pending hardware**: run `dmx discover`, then light a strip and check colour order and wiring |

To run the full 30-minute real-time soak on a machine with a node, send
from the app and watch `openmapper-cli dmx monitor` on another machine,
or watch the node's own status page.
