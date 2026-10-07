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
OSC / MIDI tab.

## OSCQuery (HTTP, default port 8011)

`GET /` returns the full tree (`FULL_PATH`, `CONTENTS`, `TYPE`, `VALUE`,
`RANGE`, `ACCESS`, `DESCRIPTION`); `GET /<path>?VALUE` one attribute;
`GET /?HOST_INFO` server info. Advertised over mDNS as `_oscjson._tcp`.
Values refresh 10× per second. Not yet: `LISTEN` (WebSocket streaming).

## MIDI

All input ports are connected and re-scanned every 2 s (hot-plug). Bindings
map a CC or note on a channel (optionally a specific port) to a parameter
(0–127 scaled to its range; bools on at ≥ 64), to GO, or to a specific cue
(triggers on the rising edge). MIDI learn: choose a target, move a control.
Bindings are saved in the project.
