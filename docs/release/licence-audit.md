# Licence, NOTICE and FFmpeg distribution audit

Date: 2026-10-07, updated 2026-10-08. This is the engineering record for the
release gate in docs/PLAN.md ("FFmpeg LGPL/codec patent audit; NDI/vendor
SDK redistribution audit"). It records facts and the engineering decisions
taken on them (D-031…D-033). The solicitor review that was to draw legal
conclusions from it was waived by the owner (docs/release/legal-posture.md);
nothing here is a legal clearance.

## OpenMapper itself

- Licence: Apache-2.0 (`LICENSE`), `NOTICE` at the root, SPDX header on
  every Rust source (enforced by `cargo xtask provenance`).
- Clean-room: `cargo xtask provenance --history` scans all 685 blobs in every
  ref (all branches) for evidence paths, analysis databases, native
  binaries, decompiler-style symbols and the reference product named
  outside documentation. Result: clean, with one reviewed false positive (an
  invented example symbol in an early version of the clean-room policy
  itself, listed with its reason in `tools/xtask/src/provenance.rs`).

## Rust dependencies

- `cargo deny check` (part of `cargo xtask ci`) enforces the licence
  allow-list in `deny.toml`, advisories (one reviewed exception, D-008),
  banned sources and wildcards. Result: clean.
- `cargo xtask notices` generates `THIRD_PARTY_LICENSES.txt` for the
  binaries: every crate reachable through normal dependencies of
  `openmapper` and `openmapper-cli` (all platforms; 521 crates), with its
  SPDX expression and every licence/notice file it publishes (including
  font licences in sub-folders). 85 crates publish no licence file; for
  those the file names the authors from the crate metadata and the
  standard licence text is appended (MIT, Zlib, BSL-1.0, WTFPL, the LLVM
  exception; Apache-2.0 is `LICENSE`). The archive built by
  `cargo xtask dist` always contains it.
- Licences present (SPDX, as declared): MIT and/or Apache-2.0 (most),
  Apache-2.0 WITH LLVM-exception (Wasmtime/Cranelift), Zlib, ISC, BSL-1.0,
  BSD-2/3-Clause, Unicode-3.0, CC0-1.0, MIT-0, WTFPL (`ffmpeg-next`,
  `ffmpeg-sys-next`), OFL-1.1 and Ubuntu-font-1.0 (egui's default fonts).
- Dual-licensed crates whose alternatives include copyleft are used under
  their permissive option: `self_cell` (Apache-2.0 OR GPL-2.0-only),
  `r-efi` (MIT OR Apache-2.0 OR LGPL-2.1-or-later).

## Vendored sources

`THIRD_PARTY.yml` lists every non-crate file. Compiled into binaries: the
Syphon Framework (BSD-3-Clause AND BSD-2-Clause, macOS builds only; one
marked patch). Its `License.txt` is included in `THIRD_PARTY_LICENSES.txt`.

## FFmpeg

- OpenMapper links FFmpeg **dynamically**. Releases **bundle** the shared
  libraries built by `tools/ffmpeg/build.sh` (D-031): FFmpeg n9.0.2 at a
  pinned git commit, LGPL-2.1-or-later, no `--enable-gpl`, no
  `--enable-nonfree`, no external libraries, components limited to
  `tools/ffmpeg/components.txt` (docs/media/ffmpeg.md lists them).
- LGPL compliance mechanics: libraries are separate, user-replaceable
  shared files (§6b); every archive carries `ffmpeg/COPYING.LGPLv2.1`,
  FFmpeg's `LICENSE.md`, `SOURCE.txt` (commit, configure line) and the
  recipe; the complete corresponding source archive is attached to the
  release next to the binaries (§6d); FFmpeg is named in NOTICE and
  THIRD_PARTY_LICENSES.txt. `cargo xtask dist` verifies the staged binaries
  load the bundled build and that it passes `openmapper-cli ffmpeg
  --require-release` before archiving.
- **Codec patents** are separate from copyright. Without a legal review
  (D-033) the shipped set is deliberately narrow: decoders for production
  formats, **no HEVC/VVC/VC-1**, encoders only for MPEG-2 (expired), FFV1,
  raw and PCM. Residual exposure is recorded in
  docs/release/legal-posture.md.
- CI gates A/B still develop against distribution FFmpeg (Linux), Homebrew
  (macOS; GPL, development only) and BtbN's LGPL shared build (Windows).

## NDI

The NDI runtime is never bundled or redistributed; users install it from
ndi.video (D-022). OpenMapper declares the C ABI it calls in its own source.
The name is used descriptively for the protocol the user's runtime provides,
with the attribution "NDI® is a registered trademark of Vizrt NDI AB" and a
non-affiliation statement in NOTICE, THIRD_PARTY_LICENSES.txt and the user
documentation (D-033). No legal opinion on the naming was obtained.

## Platform frameworks

D3D11/DXGI (Windows), Metal/IOSurface/AVFoundation (macOS) and system
libraries are used through the OS; nothing is redistributed.

## Signing and checksums

`cargo xtask dist` writes `SHA256SUMS` for the archive and the FFmpeg
source archive; `cargo xtask smoke` verifies it before testing the unpacked
archive. `release.yml` merges the per-OS checksum files and signs the result
keylessly with Sigstore (D-032, docs/release/signing.md). Code-signing and
notarising the binaries is wired behind repository secrets and inactive
until the owner has the accounts.
