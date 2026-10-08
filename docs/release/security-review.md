# Pre-release security review

Date: 2026-10-07. Scope: every boundary where OpenMapper handles input it
does not control (SECURITY.md): the WebAssembly plugin host, the FFI
adapters (FFmpeg, NDI, Syphon, Spout), network control surfaces (OSC,
OSCQuery, Art-Net, sACN) and files named by project files (project,
journal, shaders, plugins, models, media).

Method: three independent code reviews, one per area, each tracing input
from its source to every use and reporting only findings verified in the
code; then fixes with regression tests and a re-run of the full gate on
Linux, macOS and Windows. Decisions: DECISIONS.md D-029.

This is an engineering review, not an external audit or a penetration test.
A third-party review before a public release is recommended
(RELEASE_GAPS.md).

## Findings and resolution

Severity is as reported: H high, M medium, L low, I informational.

### Network control and project files

| # | Sev | Finding | Resolution |
|---|---|---|---|
| N1 | H | OSCQuery HTTP (`tiny_http`) on all interfaces by default: unbounded header buffering and a thread per connection let anyone on the LAN exhaust memory or threads. | Replaced by a bounded GET-only server (8 KiB request, 2 s deadline, 16 connections, close per response); listens on 127.0.0.1 unless network control is enabled. Tested with oversized, slow, flooding and non-GET clients. |
| N2 | H | OSC on all interfaces by default: anyone on the LAN could change parameters and fire cues. | Local-only by default; `controls.network` opts in (UI checkbox with warning). mDNS only when enabled. |
| N3 | H | Opening a project started the camera, network-stream output to a URL the project names, NDI, and DMX to any address, without asking. | Project trust gate (`om_engine::trust`): those connections are held back until the user allows the project; permission is remembered per project and connection list. Tested. |
| N4 | M | Files named by a project were read without limits on the UI thread (`/dev/zero`, FIFOs, huge files). | `store::read_limited`: regular files only, size caps (project 64 MiB, shader 256 KiB, plugin 16 MiB, OBJ 512 MiB). Tested with `/dev/zero`. |
| N5 | M | ISF size-expression parser recursion without a depth limit (stack overflow from a downloaded shader). | Length (256 B) and nesting (32) limits; unary-minus chains iterative. Tested. |
| N6 | M | An OSC flood grows the recovery journal (one line per message). | Per-frame coalescing (last value per parameter), 1024 messages per frame. |
| N7 | M | DMX: unbounded universes per project (~2 M streams at 44 Hz). | At most 1024 universes per project; DMX output is also gated by N3. |
| N8 | L | One huge OBJ face allocated past the vertex cap. | Checked before fan-out. Tested. |
| N9 | L | OSC queue unbounded while the UI is not draining. | Bounded (4096), excess dropped. Tested. |
| N10 | L | Non-finite or huge OSC fade/seek values froze fades. | Finite only; fades ≤ 1 h, seeks ≤ 1 year. Tested. |
| N11 | L | ISF pass count uncapped (GPU memory). | 16 passes. Tested. |
| N12 | I | naga GLSL frontend could panic on hostile shaders. | Runs under `catch_unwind`. |

### WebAssembly plugin host

| # | Sev | Finding | Resolution |
|---|---|---|---|
| W1 | M | Tables were not limited: table memory is host memory outside the guest cap; `table.grow`/`table.fill` could commit gigabytes. | `table_elements` capped at 10 000. Regression test fails without the cap. |
| W2 | M | Plugin files read without limits on the UI thread. | As N4, plus WebAssembly magic required unless the file is `.wat` (a WAT parse error echoed a line of whatever file a project named). |
| W3 | M/L | Compilation on the UI thread; cache keyed by path spelling. | Canonical cache key; module size cap. Residual: compile of a large (≤ 16 MiB) module still runs on the UI thread once per file (see "Accepted residual risks"). |
| W4 | L | Dropping a runner waited up to 3 s on the UI thread; parameter drags rebuilt chains. | Drop never waits (busy threads detach and exit after their bounded call); parameter-only changes update in place. Tested. |
| W5 | L | One thread per plugin use, uncapped. | At most 64 plugin instances. |
| W6 | L | Runner log unbounded on the error-return path. | Ring of the newest 256 lines. Tested. |
| W7 | L | WAT parse errors could display a line of an arbitrary local file. | See W2. |
| W8 | I | Timer-thread spawn failure silently disabled deadlines. | Now an error; runner spawn failure reports Disabled. |

Checked and sound: import allow-list and capability declaration, export
types, manifest pointer/length bounds and size cap, log import bounds, all
guest buffer accesses bounds-checked, frame size arithmetic in 64-bit with a
pixel cap, fuel and epoch deadline re-armed before every call including
instantiation, no WASI, ticker shutdown.

### FFI adapters

| # | Sev | Finding | Resolution |
|---|---|---|---|
| F1 | M | Syphon: an Objective-C exception (non-string values in another process's server announcement) could unwind into Rust; `UTF8String` NULL passed to `CStr::from_ptr`. | Every exported shim function catches exceptions; announcement values accepted only as strings; NULL-safe conversion on both sides. |
| F2 | M | NDI on Windows loaded by bare DLL name (current directory and `PATH` searched: DLL planting). | Absolute known paths only, loaded with `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR \| DEFAULT_DIRS`. |
| F3 | L/M | Spout: shared memory used without checking it is committed (a reserved block planted by another process would fault). | `MEM_COMMIT` required. |
| F4 | L | Spout: foreign-writable shared memory borrowed as a Rust slice (formally a data race). | Copied with volatile accesses; only changed bytes written back. |
| F5 | L | Frame sizes chosen by other processes forced huge allocations (Spout, Syphon, NDI). | Received frames capped at 8192 × 8192. |
| F6 | L | Spout sender names could be the reserved registry block names or address other kernel namespaces. | Reserved names and backslashes refused. Tested. |
| F7 | L | NDI sender stride `w * 4` could overflow `i32`. | Checked. |
| F8 | L | FFmpeg timestamp arithmetic unchecked (debug panic, release wrap); audio time base not validated. | Checked arithmetic; invalid time bases refused. |
| F9 | I | FFmpeg could follow URLs from playlist-style local files. | Local files open with `protocol_whitelist=file`. |

Checked and sound: every other `unsafe` block in om-spout, om-ndi,
om-syphon and om-media-ffmpeg (handle lifetimes, single frees on all error
paths, buffer lengths against pitches and strides, `Send` impls, interrupt
callback lifetime). `unsafe` is denied in every other crate.

## Accepted residual risks

- **Network control, when enabled, is unauthenticated.** OSC and OSCQuery
  have no authentication in their protocols; enabling network control
  trusts the network. Documented in the UI and `control.md`.
- **DMX input** listens on all interfaces when enabled (consoles are on the
  lighting network) and accepts any source; it is off by default.
- **Plugin compilation** of a module up to 16 MiB runs once per file on the
  UI thread (a few hundred milliseconds at worst on current hardware).
  Moving it off-thread is a planned improvement.
- **Media decoding** relies on FFmpeg's own robustness for hostile files;
  OpenMapper ships no FFmpeg of its own (users install an LGPL build), so
  FFmpeg security updates come from that installation.
- **Windows UNC paths** in project files (`\\host\share\…`) are opened like
  any path and can trigger SMB authentication to that host; the trust gate
  does not cover file paths. A future option could refuse UNC paths in
  projects from untrusted sources.
