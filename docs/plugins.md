# Plugins

Milestone 9. OpenMapper runs **WebAssembly plugins** as media filters. Each
plugin is sandboxed: it gets no filesystem, network, environment or
clock access beyond what it declares, has bounded memory and time, and
runs off the render thread. Decision: DECISIONS.md D-026.

## ABI v1 (`openmapper:plugin@1.0.0`)

A plugin is a **core WebAssembly module** (`.wasm`, or `.wat` text). It is
versioned independently of the project format. Full contract:
`crates/om-plugin-api/src/lib.rs`.

| Guest export | Signature | Meaning |
|---|---|---|
| `memory` | memory | shared linear memory |
| `om_abi_version` | `() -> i32` | must return 1 |
| `om_manifest` | `() -> i64` | `ptr << 32 \| len` of UTF-8 JSON manifest |
| `om_alloc` | `(size) -> i32` | guest buffer (0 = failure) |
| `om_process` | `(in, w, h, params, n, out) -> i32` | sRGB RGBA8 (straight alpha) `w·h·4` bytes at `in` → same at `out`; `n` × `f32` params; 0 = success |

| Host import (`openmapper`) | Signature | Capability |
|---|---|---|
| `log` | `(ptr, len)` | `log` |
| `time` | `() -> f64` (show seconds) | `time` |

Manifest:

```json
{ "name": "Invert", "version": "1.0.0", "abi": 1,
  "capabilities": ["log", "time"],
  "params": [ { "name": "amount", "min": 0, "max": 1, "default": 1 } ] }
```

A plugin that imports anything else (WASI, unknown functions), uses a
capability it does not declare, lacks an export, or targets another ABI
**fails to load** with a clear message. Capabilities are inspectable: the
manifest lists them, and the host checks them against the module's
actual imports.

The ABI v1 compatibility fixture is
`crates/om-plugin-host/tests/plugins/invert.wat`, and it must keep
loading and producing the same output after any host change.

## Limits and failure handling

| Limit | Default |
|---|---|
| Memory per instance | 256 MB (growth beyond fails) |
| Instructions per call | 4·10⁹ fuel units |
| Wall-clock per call | 500 ms (epoch interruption, 10 ms resolution) |
| Frame size | 8192 × 8192 |

A crash (trap), timeout or memory exhaustion discards the instance. The
runner retries with a fresh instance after a back-off, and after three
consecutive faults it **disables** the plugin and reports why. Plugins run
on their own threads, newest frame wins, so the render thread never waits
on plugin code. While a chain has no output yet, or cannot run, the
**original frame is shown**.

## Using plugins

Each media item has a list of plugin filters (`Media.plugins`), applied
in order to every frame:

```json
"plugins": [ { "path": "plugins/invert.wasm", "enabled": true, "params": { "amount": 1.0 } } ]
```

Parameters are the plugin's saved state and are stored in the project.
Instance memory is not persisted in ABI v1. In the UI, open a media item's
*Plugins* section to add files, toggle them, set parameters (sliders from
the manifest) and see live status. Use *Reload plugins* after editing a
plugin file. Offline CLI renders apply plugins too, waiting (bounded) for
each frame.

## Writing a plugin

Any language that targets `wasm32-unknown-unknown` without WASI works:
Rust with `#![no_std]` or plain `std` without I/O, C/Clang, Zig,
AssemblyScript, or WAT by hand. Export the five symbols above, keep a
bump or real allocator behind `om_alloc`, and embed the manifest JSON as a
static string.
