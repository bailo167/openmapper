ROLE: Lead Architect.
TASK: Bootstrap the OpenMapper private implementation repository.

Create the Cargo workspace and directory layout specified in docs/PLAN.md.
Create typed IDs, error types, exact-time foundation, version-1 project schema,
typed command bus, undo/redo skeleton, minimal engine, egui desktop shell and CLI.

Create `cargo xtask ci` implementing format, Clippy, test, dependency,
licence/provenance and architecture-layer checks.

The GUI must start on macOS/Windows/Linux and show an empty project.
The CLI must create, validate and inspect a project.

Acceptance:
- workspace builds on three target OSes;
- project JSON round-trips deterministically;
- every project mutation used in the demo passes through Command;
- architecture test rejects upward dependency violations;
- no runtime AI/MCP dependency exists;
- CI green.

Do not implement mapping/media yet.
