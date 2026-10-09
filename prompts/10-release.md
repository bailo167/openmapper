ROLE: Lead + QA + Provenance Auditor.
TASK: Decide whether OpenMapper 1.0 is releasable. Do not fix by lowering
acceptance thresholds.

Require: all P0/P1 parity rows verified or explicitly waived in RELEASE_GAPS.md;
clean macOS/Windows/Linux package install; 12-24 hour reference-system soak;
project crash/recovery injection tests; project migration tests; physical
projector validation; MIDI/DMX/live-I/O sign-off where hardware exists;
dependency/licence/NOTICE audit; FFmpeg distribution audit; clean-room evidence
audit; no reference-product proprietary artefacts in repository history; security
review of WASM and FFI boundaries; user documentation; signed checksum generation.

If a requirement fails, create a release-blocking issue and return NOT READY.
Never release simply because the scheduled date has arrived.
