# Security Policy

OpenMapper is pre-release and developed privately. Until a public release,
report issues directly to the maintainers.

Security-sensitive surfaces that receive explicit review before every release:

- WASM plugin host (capability policy, memory/fuel/time limits);
- FFI adapter crates (FFmpeg, NDI, Syphon, Spout, DeckLink);
- network control surfaces (OSC, OSCQuery HTTP/WebSocket, Art-Net, sACN);
- project-file parsing and migration (untrusted input).
