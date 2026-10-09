# Legal posture of the 1.0 release

**This document is not legal advice and records no legal clearance.** It
records an owner's decision and the engineering done in response.

## The decision

On 2026-10-08 the project owner decided that the Australian IP solicitor
review planned in CLEANROOM.md, docs/PLAN.md and RELEASE_GAPS.md G-10
(clean-room record, FFmpeg LGPL and codec patents, NDI naming and
attribution, third-party notices) **will not be done** before 1.0. The gap
is therefore closed as *waived by the owner*, not as *passed*. The review
can be commissioned at any later time; everything it would examine is
public with each release (see below).

## What software can and did reduce

| Area | Reduction |
|---|---|
| FFmpeg copyright (LGPL-2.1) | The bundled build has no GPL or non-free parts and no external libraries (D-031). Every archive carries `ffmpeg/COPYING.LGPLv2.1`, FFmpeg's `LICENSE.md`, `SOURCE.txt` (git commit, configure line) and the recipe; the complete corresponding source archive is attached to the release (§6d); the libraries are ordinary shared libraries the user can replace (§6b). `cargo xtask dist` enforces this mechanically and refuses to archive otherwise. |
| Codec patents | Decoders only for the formats shows need; **no HEVC/VVC/VC-1**; encoders only for MPEG-2 (expired), FFV1, raw and PCM. The policy is code (`om_media_ffmpeg::policy`) and tests keep the shipped component list within it. OpenMapper is distributed free of charge, which keeps it within the royalty-free tier of the H.264 pool as the project understands it — an understanding, not an opinion of counsel. |
| NDI name and runtime | The runtime is never bundled, downloaded or redistributed; only public C entry points are declared (D-022). The name is used descriptively for the protocol the user's own runtime provides, with the attribution "NDI® is a registered trademark of Vizrt NDI AB" and a non-affiliation statement in NOTICE, THIRD_PARTY_LICENSES.txt and the user documentation. |
| Clean-room record | `cargo xtask provenance --history` scans every blob in every ref for reference evidence, binary-analysis artefacts and the reference product named outside documentation; the report (`PROVENANCE.txt`) is attached to every release so the record is open to inspection. CLEANROOM.md's forbidden-transfer rules are unchanged. |
| Third-party notices | Generated from the dependency graph of the shipped binaries on every build (`cargo xtask notices`), never hand-maintained; licence allow-list enforced by `cargo deny`. |
| Trademarks of the reference product | PRODUCT.md excludes its artwork, shaders, presets and project import; the provenance scan rejects its name outside Markdown documentation. |

## Residual risks the owner accepts

1. **Codec patents.** Shipping H.264, AAC, MPEG-4 Part 2, ProRes, DNxHD and
   VP8/VP9 decoders without a pool licence is common practice for
   open-source media software but has never been tested in court for a
   project of this kind. Exposure differs by jurisdiction.
2. **NDI.** Vizrt's SDK licence and brand guidelines govern how the mark is
   used. OpenMapper does not accept the SDK licence (it never ships the SDK)
   and uses the name descriptively; an objection would be resolved by
   renaming the feature, which the project format allows (the type name
   `ndi` is internal).
3. **Clean-room.** The record shows process, not a legal opinion that the
   implementation is free of the reference product's protectable
   expression. The reference-product licence agreement was archived on
   2026-10-09 (D-035): it forbids reverse engineering, decompiling and
   disassembling, so clean-room research is limited to black-box use
   (CLEANROOM.md L0–L2), which is all the project has recorded. Whether
   black-box observation itself counts as "reverse engineering" under that
   Swiss-law agreement is untested; it is the question to put to counsel
   if a rights holder ever objects.
4. **LGPL mechanics.** A mistake in how source or notices are offered would
   be a compliance defect, curable by publishing what is missing.

## What would change this posture

Commercial distribution, paid support, embedding in hardware, or a request
from a rights holder — any of these should trigger the review that was
waived here, before continuing.
