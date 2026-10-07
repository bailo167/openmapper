# OpenMapper Agent Contract

OpenMapper is production software. The objective is correctness,
recoverability and cross-platform behaviour, not demo quality.

## Authority order

1. CLEANROOM.md
2. PRODUCT.md
3. architecture ADRs (docs/architecture/)
4. current milestone specification (MILESTONES.md)
5. this file
6. implementation convenience

## Working rules

- Work on one active feature milestone at a time (see MILESTONES.md).
- Do not mutate project state outside the typed command system (`om-command`).
- Do not add runtime AI, LLM, MCP or cloud-AI dependencies.
- Core crates must remain platform-independent.
- Vendor/platform APIs belong in dedicated adapter crates.
- No panic on user input, media failure, device loss or malformed project data.
- No `unsafe` outside approved boundary modules without a written SAFETY comment.
- Every bug fix adds a regression test.
- Every externally observable feature gets an acceptance test.
- Do not lower test thresholds to make a failing implementation pass.
- Record material architectural decisions in DECISIONS.md.
- Run `cargo xtask ci` before completion.

## Clean-room

Implementation agents must not access `openmapper-reference`.
Only neutral specifications under `docs/behaviour/` may inform implementation.
See CLEANROOM.md.

## Autonomy

When a routine decision is unspecified, choose the smallest design consistent
with architecture and tests, document it in DECISIONS.md and continue.

Stop only for:
- an irreversible purchase;
- unavailable credentials or physical hardware;
- a legal decision explicitly requiring professional review;
- a contradiction between higher-authority project documents.
