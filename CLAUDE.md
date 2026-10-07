# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Read AGENTS.md first — it is the agent contract (authority order, working rules,
autonomy limits). Then CLEANROOM.md, PRODUCT.md, docs/PLAN.md and MILESTONES.md
(which milestone is active). Per-milestone task prompts are in `prompts/`.

- Never open the sibling `openmapper-reference` repository from an implementation session.
- `cargo xtask ci` is the single quality gate (fmt, clippy -D warnings, tests,
  cargo-deny, provenance scan, architecture layering). Run it before declaring work done.
- Single test: `cargo test -p <crate> <test_name>`.
- Record material decisions in DECISIONS.md.
