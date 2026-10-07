# Licence, NOTICE and FFmpeg distribution audit

Date: 2026-10-07. This is the engineering record for the release gate in
docs/PLAN.md ("FFmpeg LGPL/codec patent audit; NDI/vendor SDK
redistribution audit"). Legal conclusions belong to the solicitor review
(RELEASE_GAPS.md); this records the facts that review needs.

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

- OpenMapper links FFmpeg **dynamically** and ships **no FFmpeg binaries**
  today (`cargo xtask dist` does not bundle them). Users install FFmpeg;
  `openmapper-cli ffmpeg` reports the licence profile of what was loaded.
- Because linking is at load time, **the binaries do not start on a machine
  without FFmpeg shared libraries** (Linux: system packages; Windows: DLLs
  on `PATH` or next to the executable; macOS: Homebrew, whose FFmpeg is a
  GPL build). This is a release blocker for "clean install": either bundle
  an LGPL build per platform (docs/media/ffmpeg.md, release checklist:
  publish the matching source and build recipe, no GPL/non-free parts,
  user-replaceable libraries) or load FFmpeg at run time and run without
  video when it is absent. Decision needed (RELEASE_GAPS.md G-07).
- CI on Windows uses BtbN's LGPL shared build; macOS CI uses Homebrew (GPL;
  development only).
- **Codec patents** (H.264, HEVC, AAC …) are separate from copyright and
  need counsel before any FFmpeg is distributed (RELEASE_GAPS.md).

## NDI

The NDI runtime is never bundled or redistributed; users install it from
ndi.video (D-022). OpenMapper declares the C ABI it calls in its own source.
Whether the product may use the "NDI" name and how it must attribute NDI
under the NDI SDK licence/brand guidelines is for the legal review.

## Platform frameworks

D3D11/DXGI (Windows), Metal/IOSurface/AVFoundation (macOS) and system
libraries are used through the OS; nothing is redistributed.

## Signing and checksums

`cargo xtask dist` writes `SHA256SUMS` for the archive; `cargo xtask smoke`
verifies it before testing the unpacked archive. Signing `SHA256SUMS` (and
code-signing/notarising the binaries) needs the publisher's keys and
accounts and is a human step (RELEASE_GAPS.md).
