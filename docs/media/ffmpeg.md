# FFmpeg integration and licensing policy

OpenMapper uses FFmpeg (libavformat, libavcodec, libavdevice, libswscale,
libswresample) only through `crates/om-media-ffmpeg`. No FFmpeg type crosses
that crate's boundary; the rest of OpenMapper sees `om_media_core` types.

## Linking

- Always **dynamic** linking against shared FFmpeg libraries (`pkg-config`
  on Linux/macOS, `FFMPEG_DIR` anywhere). Never static.
- Bindings: `ffmpeg-next` / `ffmpeg-sys-next` 9.x (WTFPL; D-013), built
  without `avfilter` (unused). They work with FFmpeg 6.1 (Ubuntu 24.04 CI)
  through 9.0.

## What releases ship (D-031)

Every release archive **bundles** FFmpeg shared libraries built by
`tools/ffmpeg/build.sh` from the component list in
`tools/ffmpeg/components.txt`: FFmpeg **n9.0.2** (git commit
`946fcce07b6dcd0331c8cc609192aeff5e1924f8`), LGPL-2.1-or-later, with

- no `--enable-gpl`, no `--enable-nonfree`, **no external libraries**;
- decoders for show delivery and intermediate formats — H.264, MPEG-4
  Part 2, MPEG-1/2, MJPEG, ProRes, DNxHD, CineForm, HAP, FFV1, HuffYUV,
  Ut Video, raw/v210, VP8, VP9, Theora, QuickTime RLE, DV, H.263, FLV; AAC,
  ALAC, FLAC, MP3, Opus, Vorbis, PCM;
- **no HEVC, VVC, VC-1 or AV1 decoders** (patent-pool and external-library
  reasons; see D-031 and docs/release/legal-posture.md);
- encoders only for publishing and lossless/raw formats: MPEG-2, FFV1, raw
  video, PCM — no H.264, HEVC, AAC or MPEG-4 Part 2 encoders;
- containers MOV/MP4, Matroska/WebM, AVI, MXF, DV, MPEG-TS/PS, FLV, raw
  streams, WAV/AIFF/CAF/FLAC/Ogg/MP3/ADTS, RTSP/RTP/SDP/HLS; protocols file,
  TCP, UDP, RTP, HTTP(S), RTMP. **No SRT** (needs libsrt): use TCP for
  lossless streams.
- cameras: AVFoundation (macOS), DirectShow (Windows), V4L2 (Linux).

The libraries live in `lib/` next to the executables (beside them on
Windows). The archive's `ffmpeg/` folder holds `COPYING.LGPLv2.1`, FFmpeg's
`LICENSE.md`, `SOURCE.txt` (commit and full `configure` line) and the recipe
(`build.sh`, `components.txt`). The complete corresponding source archive
`ffmpeg-n9.0.2-src.tar.gz` is attached to every release.

`openmapper-cli ffmpeg` reports the version (`n9.0.2-openmapper` for the
bundled build), licence profile, configuration and codecs of the libraries
actually loaded; `--require-release` fails unless they satisfy the release
policy (`om_media_ffmpeg::policy`, tested against `components.txt`).
`cargo xtask dist` runs that check from the staging directory and refuses
to produce an archive otherwise; `cargo xtask smoke` decodes a generated
FFV1 clip through the unpacked archive.

### Replacing the bundled libraries

The LGPL requires, and OpenMapper intends, that you can swap the libraries.
Any FFmpeg **9.0.x** shared build is ABI-compatible: replace the files in
`lib/` (or next to the executables on Windows) with the same-named files
from, for example, a BtbN "LGPL shared" build or your own build with HEVC
or SRT enabled (`tools/ffmpeg/build.sh` with an edited `components.txt`).
`openmapper-cli ffmpeg` shows what loaded. OpenMapper does not distribute
those alternatives, and a GPL build makes the combination GPL for *your*
redistribution, not for your own use.

## Licence profile

FFmpeg is LGPL-2.1-or-later unless built with `--enable-gpl` (GPL) or
`--enable-nonfree` (not redistributable). `openmapper-cli ffmpeg` and
`om_media_ffmpeg::info()` report the profile of the libraries actually loaded.

| Context | Allowed profile |
|---|---|
| Development (e.g. Homebrew FFmpeg, which is GPL) | any |
| CI gates A/B | any (Windows uses BtbN's LGPL shared build) |
| **Release archives** | **the bundled build above, verified by `cargo xtask dist`** |
| User-replaced libraries | user's choice; OpenMapper ships nothing else |

## Building the bundle

```
tools/ffmpeg/build.sh [PREFIX]      # default target/ffmpeg; ~5 min
cargo xtask dist                    # uses FFMPEG_DIR or target/ffmpeg
```

Needs `git`, `make`, a C compiler and `nasm` (x86). On Windows run it from
an MSYS2 UCRT64 shell with `mingw-w64-ucrt-x86_64-toolchain` and `nasm`;
it produces MSVC-compatible import libraries. `package.yml` does exactly
this on all three OSes with the prefix cached by recipe hash.

Changing the component list is a policy change: edit `components.txt`,
keep `om_media_ffmpeg::policy` (allow/deny lists) in agreement — the unit
tests fail otherwise — and record the reason in DECISIONS.md.

## Codecs used by the test corpus

Generated at test time with FFmpeg's built-in LGPL encoders: FFV1 (lossless),
MPEG-4 Part 2, PCM and the native AAC encoder. No media files are committed.
The MPEG-4/AAC fixtures need a development FFmpeg; the bundled build has
only the FFV1/PCM encoders, which the packaging smoke test uses.
