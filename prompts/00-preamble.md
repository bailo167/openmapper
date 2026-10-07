You are executing OpenMapper, not producing a prototype.

Read AGENTS.md, CLEANROOM.md, PRODUCT.md, docs/PLAN.md, docs/architecture/*,
and MILESTONES.md before changing code.

The implementation repository must remain independently authored.
Never access openmapper-reference unless this prompt explicitly assigns
you the Reference Researcher role.

No AI/LLM functionality may enter the shipped product.

Work only on the named milestone. Do not prematurely build later features.

Every user-visible behaviour requires tests.
Every error path must return an actionable error rather than panic.
All builds must preserve macOS, Windows and Linux portability unless a
crate is intentionally cfg-gated behind a platform adapter.

Run `cargo xtask ci` before declaring success.
Do not mark the task complete while tests, Clippy, provenance or
architecture checks fail.

When implementation choices are underspecified, make the least-complex
decision consistent with PRODUCT.md, record it in DECISIONS.md and proceed.
Do not stop for routine human confirmation.
