# Show control

## Parameters

Every live-controllable value is a parameter with a stable name:

| Parameter | Type |
|---|---|
| `master/opacity` | float 0..1 |
| `master/blackout` | bool |
| `surface/<id>/opacity` | float 0..1 |
| `surface/<id>/enabled` | bool |
| `media/<id>/speed` | float −16..16 |
| `media/<id>/volume` | float 0..1 |
| `media/<id>/input/<name>` | shader input (float or bool) |

Direct control (UI, OSC, MIDI) changes the document through normal commands:
undoable, saved, one undo step per continuous gesture. Cues, timelines and
modulators produce *live overrides* each frame instead — they never modify
the document or flood undo history.

## Precedence and clocks

Timelines → cues → modulators (later wins). Cues, timelines and modulators
run on the live show clock, which always runs; the play/pause transport
controls media playback only.

## Cues

A cue list in GO order. Running a cue fades each of its parameters from the
current value to the cue's value over the fade time (bools switch at the
start). Values *track*: they hold until another cue changes them or the show
is released (fade back to the document). Interrupting a fade continues from
where it was.

## Timelines

Keyframe tracks on exact time (rational seconds). Linear, step and smooth
(smoothstep) easing; bool tracks step. Before the first key the first value
holds; after the last, the last. Looping timelines wrap at their duration.
Play, pause, stop (stops driving parameters) and seek.

## Modulators

`value = offset + depth × signal` with signal in 0..1: LFO (sine, triangle,
square, saw; rate in Hz, phase) or audio (overall level or low/mid/high band
of the audio being played, × gain).

## Listening and trust

OSC and OSCQuery listen on **this computer only** (127.0.0.1) unless the
project enables *Accept control from other computers*
(`controls.network`); only then do they bind every interface and advertise
OSCQuery over mDNS. Anyone who can reach those ports can control the show,
so enable it on trusted show networks only. DMX input is off unless
enabled.

A project opened from a file may come from someone else. Its camera, NDI
and network-stream inputs, NDI and stream outputs, DMX output, network
control and DMX input stay **held back** until you allow them for that
project (a bar lists them). The permission is remembered per user for that
project and that exact list; adding a new destination later asks again
(edits you make yourself keep the project allowed). Without a screen,
`openmapper-cli trust <project>` shows the list and `--allow` allows it for
the current user (D-034). See DECISIONS.md D-029.

## OSC (UDP, default port 8010)

| Address | Arguments |
|---|---|
| `/openmapper/<parameter>` | float / int / bool |
| `/openmapper/transport/play` · `pause` · `restart` | — |
| `/openmapper/cue/go` | — (next cue) |
| `/openmapper/cue/<id>/go` | — |
| `/openmapper/cue/release` | fade seconds (optional) |
| `/openmapper/timeline/<id>/play` · `pause` · `stop` | — |
| `/openmapper/timeline/<id>/seek` | seconds |

Bundles are accepted. Invalid messages are ignored and listed in the
OSC / MIDI tab. Non-finite numbers are rejected; fades are capped at an
hour and seeks at a year. At most 4096 messages queue between frames (the
rest are dropped), and within a frame only the last value per parameter is
applied (at most 1024 messages).

## OSCQuery (HTTP, default port 8011)

`GET /` returns the full tree (`FULL_PATH`, `CONTENTS`, `TYPE`, `VALUE`,
`RANGE`, `ACCESS`, `DESCRIPTION`); `GET /<path>?VALUE` one attribute;
`GET /?HOST_INFO` server info. Advertised over mDNS as `_oscjson._tcp`.
Values refresh 10× per second. Not yet: `LISTEN` (WebSocket streaming).
The HTTP server answers `GET` only, reads at most 8 KiB of request within
2 s, serves at most 16 connections at once and closes every connection
after its response.

## MIDI

All input ports are connected and re-scanned every 2 s (hot-plug). Bindings
map a CC or note on a channel (optionally a specific port) to a parameter
(0–127 scaled to its range; bools on at ≥ 64), to GO, or to a specific cue
(triggers on the rising edge). MIDI learn: choose a target, move a control.
Bindings are saved in the project.

## DMX input (Art-Net / sACN)

Off by default (`controls.dmx_input.enabled`). When on, OpenMapper listens on
UDP 6454 (Art-Net) and 5568 (sACN, joining the multicast group of every
bound universe; unicast also works). A binding maps a channel of a universe
(the Art-Net port address or sACN universe number) to a parameter (full
range scaled to its range; bools on at ≥ 50 %), to GO, or to a specific cue
(fires when the value rises through 50 %, never when a universe is first
seen). A binding can be 16-bit: the channel is the coarse byte and the next
channel the fine byte. Messages are produced only when a value changes, so a
console's 40 Hz refresh does not flood the undo history. DMX learn: choose a
target, move a fader by at least 8 steps. The newest packet for a universe
wins: sources are not merged and sACN priority is not arbitrated. Do not
bind a universe that this machine also sends, or output feeds back into
control.
