# OpenMapper user guide

OpenMapper is projection-mapping and LED-mapping software. You place
**surfaces** on a **canvas**, assign **media** to them, then send the canvas
to projectors (**outputs**), LED fixtures (**DMX**) and other applications
(**publishing**). Shows are run with **cues**, **timelines** and external
control (OSC, MIDI, DMX).

This guide covers everyday use. The reference documents (in `docs/` next to
this guide) cover each area in depth:

| Topic | Document |
|---|---|
| Effects and ISF shaders | `effects.md`, `isf.md` |
| Media files and playback | `media/playback.md`, `media/ffmpeg.md` |
| Live inputs, Syphon/Spout/NDI, streams | `live-io.md` |
| Cues, timelines, modulators, OSC/MIDI/DMX control | `control.md` |
| LED fixtures and DMX output | `dmx.md` |
| Output mapping, soft edges, 3-D calibration | `calibration.md` |
| Plugins | `plugins.md` |
| Project file format | `project-format.md` |

## Installing

Unpack the archive for your platform anywhere and run `openmapper` (the
desktop app) or `openmapper-cli` (the command-line tool). Check the archive
against `SHA256SUMS` from the release page first (`shasum -a 256 -c
SHA256SUMS` on macOS/Linux, `CertUtil -hashfile <archive> SHA256` on
Windows). `SHA256SUMS` itself is signed; `docs/release/signing.md` shows
how to verify the signature with `cosign`.

The binaries are not yet code-signed. On **macOS**, download with `curl`
or clear quarantine after unpacking (`xattr -dr com.apple.quarantine
openmapper-*/`), or allow the app under System Settings ▸ Privacy &
Security ▸ *Open Anyway*. On **Windows**, SmartScreen asks once: *More
info* ▸ *Run anyway*.

On **Linux**, to list OpenMapper with your other applications, save this
as `~/.local/share/applications/openmapper.desktop` (with the path where
you unpacked it):

```ini
[Desktop Entry]
Type=Application
Name=OpenMapper
Exec=/home/you/Applications/openmapper/openmapper %f
Icon=video-display
Categories=AudioVideo;Video;
```

Video and audio play through the FFmpeg libraries included in the archive
(`lib/`; LGPL — licence, source reference and build recipe in `ffmpeg/`).
They decode the usual delivery and intermediate formats (H.264, ProRes,
DNxHD, HAP, MPEG-2/4, VP8/VP9, FFV1, MJPEG; AAC, MP3, FLAC, Opus, PCM) but
**not HEVC**; `docs/media/ffmpeg.md` lists every format and explains how
to replace the libraries if you need more. `openmapper-cli ffmpeg` shows
what is loaded.

NDI needs the NDI runtime from ndi.video (see `live-io.md`). NDI® is a
registered trademark of Vizrt NDI AB; OpenMapper is not affiliated with
Vizrt.

## Projects

- **File ▸ New** starts an empty project; **Open path / Save / Save As path**
  use the path typed in the toolbar field. Projects are `.omproj` JSON files;
  media paths are stored relative to the project when possible, so a show
  folder can be moved as a whole.
- Every change is undoable (**Ctrl/Cmd+Z**, **Ctrl/Cmd+Shift+Z**) and saved
  continuously to a recovery journal next to the project. After a crash or
  power cut, reopening the project restores the unsaved work and says so in
  the status bar.
- **Missing media** (moved files): use *Relink* with a folder; files are found
  by name, preferring matching folder structure, in one undo step.
- To start a show unattended: `openmapper show.omproj --play`. The app
  plays and its window becomes the first enabled output, fullscreen on that
  output's display (it waits until the display is connected); other outputs
  open in their own windows. **Escape** returns to the editor; **Run show**
  in the toolbar goes back. To start the show when the computer starts, add
  that command to your desktop's autostart (log in automatically, and turn
  off screen locking and display sleep).

### Projects from other people

A project can ask OpenMapper to use the camera, send video or DMX to network
addresses, or accept control from other computers. When you open a project
that does this and you have not allowed it before, those connections are
**held back** and a bar lists them; choose **Allow for this project** if you
trust it. The choice is remembered for that project. If the project later
asks for something new, you are asked again. On a show computer without a
screen to click on (set up over SSH), `openmapper-cli trust show.omproj`
lists what is held back and `openmapper-cli trust --allow show.omproj`
allows it for that user account.

## Canvas, surfaces and media

1. Add media in the **Media** section: an image, video, image sequence,
   test pattern, ISF shader generator, or a live input.
2. Add a **surface** and choose its media. Drag its corners on the canvas to
   fit the object; switch the surface to a mesh for curved objects, and add a
   **mask** to cut shapes out (mask mode).
3. Set opacity, blend mode and **effects** in the inspector (colour, blur,
   pixelate, invert, or any ISF effect shader). Shader inputs appear as
   controls; audio-reactive shaders can use `audio`/`audioFFT` inputs.
4. Video plays on the show transport (play, pause, restart). Each media item
   has its own loop mode, speed (audio follows speed, like a tape) and
   volume.

## Outputs (projectors)

Add an **output** per projector and choose its display; enabling it opens a
fullscreen window there. In **Mapping**, choose the part of the canvas it
shows, adjust its four corners to the screen, and set **soft edges** where
projectors overlap (blended in light, with gamma). For 3-D objects, **Use 3D
model** loads an OBJ file and a calibration from point pairs (see
`calibration.md`; the CLI can generate and decode structured-light patterns
for camera-assisted calibration).

Master opacity and blackout apply to every output.

## LED fixtures and DMX

In **DMX / LED**, add a node (Art-Net address or sACN) and fixtures (strip,
matrix or single light). Fixtures sample the final canvas where you place
them, so everything you see is what the LEDs show. Rate, colour order,
encoding and brightness are per project/fixture. `openmapper-cli dmx monitor`
shows what arrives on the network; `dmx discover` finds Art-Net nodes.

## Running the show

- **Cues** store parameter values; **GO** runs the next cue with its fade,
  **Release** returns to the document's values.
- **Timelines** animate parameters with keyframes and can be played, paused
  and scrubbed.
- **Modulators** drive parameters from an LFO or the audio level.
- **OSC** (port 8010), **OSCQuery** (8011), **MIDI** (any port, with learn)
  and **DMX input** (Art-Net/sACN, with learn) control parameters and cues.
  OSC and OSCQuery accept connections from this computer only until you tick
  *Accept control from other computers*; only do that on a trusted show
  network, and open UDP 8010 and TCP 8011 in the show computer's firewall.
  Any OSC app can then send `/openmapper/cue/go` and the other addresses in
  `control.md`; browsing `http://<show-computer>:8011/` lists every
  parameter, cue and timeline with its current value.

## Sharing video with other applications

Outputs can publish their frames as a Syphon server (macOS), Spout sender
(Windows), NDI source, or a network stream (`srt://`, `udp://`, `rtp://`,
`tcp://`). Live inputs receive the same kinds of sources plus cameras and
capture devices.

## Plugins

Plugins are sandboxed WebAssembly filters applied to a media item's frames.
Add one with **+ Plugin**; its parameters appear as controls. A plugin that
crashes, hangs or uses too much memory is stopped and the original frame is
shown; it cannot read files or use the network.

## Command line

`openmapper-cli --help` lists every command: create, validate, inspect and
edit projects, render frames or outputs to PNG offscreen, DMX tools, live
input probes, calibration, relinking and a GPU stability soak.

## Getting help

The status bar shows the last message or error. Problems with a specific
area are described in that area's reference document. Report security
issues as described in `SECURITY.md`.
