# FFmpeg integration and licensing policy

OpenMapper uses FFmpeg (libavformat, libavcodec, libswscale, libswresample)
only through `crates/om-media-ffmpeg`. No FFmpeg type crosses that crate's
boundary; the rest of OpenMapper sees `om_media_core` types.

## Linking

- Always **dynamic** linking against shared FFmpeg libraries (`pkg-config`
  on Linux/macOS, `FFMPEG_DIR` on Windows). Never static.
- Bindings: `ffmpeg-next` / `ffmpeg-sys-next` 9.x (WTFPL; D-013). They work
  with FFmpeg 6.1 (Ubuntu 24.04 CI) through 9.0.

## Licence profile

FFmpeg is LGPL-2.1-or-later unless built with `--enable-gpl` (GPL) or
`--enable-nonfree` (not redistributable). `openmapper-cli ffmpeg` and
`om_media_ffmpeg::info()` report the profile of the libraries actually loaded.

| Context | Allowed profile |
|---|---|
| Development (e.g. Homebrew FFmpeg, which is GPL) | any |
| CI | any (Windows uses BtbN's LGPL shared build) |
| **Release builds that bundle FFmpeg** | **LGPL only** |
| Release builds using a user-provided FFmpeg | user's choice; OpenMapper ships no FFmpeg binaries |

## Release checklist (bundled FFmpeg)

1. Build FFmpeg without `--enable-gpl` and `--enable-nonfree`; record the full
   `configure` line (must classify as `LGPL` via `openmapper-cli ffmpeg`).
2. Ship shared libraries next to the app; users must be able to replace them.
3. Publish the exact matching FFmpeg source archive and build recipe with the
   release, plus FFmpeg's licence text and attribution in `THIRD_PARTY.yml`.
4. Do not bundle x264, x265 or other GPL/non-free codec libraries.
5. Codec **patents** are separate from copyright licensing (H.264, HEVC, AAC
   …): review with counsel before distributing encoders/decoders in regions
   where patents apply.

## Codecs used by the test corpus

Generated at test time with FFmpeg's built-in LGPL encoders: FFV1 (lossless),
MPEG-4 Part 2, PCM and the native AAC encoder. No media files are committed.
