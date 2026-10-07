ROLE: Platform Engineer.
TASK: Eliminate accidental platform assumptions after the core feature set.

Audit filesystem paths, case sensitivity, window/display IDs, DPI,
GPU feature selection, timers, audio/device naming and native library loading.

Each platform-specific API must reside behind its adapter crate and `cfg`.

Build a capabilities report shown in OpenMapper diagnostics: OS, GPU backend,
adapter, supported texture formats, displays, available live-I/O adapters and
disabled features with reasons.

Acceptance: same sample project opens/renders on macOS/Windows/Linux;
unsupported vendor APIs report unavailable rather than breaking startup;
project JSON is byte-identical after load/save where no migration occurred.
