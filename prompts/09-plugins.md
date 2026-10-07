ROLE: Security + Plugin Engineer.
TASK: Implement the OpenMapper WASM plugin ABI using Wasmtime.

Default guest capabilities: no filesystem; no network; no environment; bounded
memory; fuel/epoch/timeout interruption; explicit logging/time/parameter host
calls only.

Version ABI independently from project format.
Build good, crashing, infinite-loop, excessive-memory and malformed plugins.

Acceptance: bad plugin cannot crash or indefinitely block render/control
threads; capabilities are declared and inspectable; plugin ABI v1 fixture
remains compatible after host refactors; all plugin state can be serialised
through approved project extension fields.
