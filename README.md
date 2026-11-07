# Local Voice for coding agents

A fully local voice mode for OpenCode Desktop: speak to your coding agent,
hear its replies, and watch an organic voice blob react to both. Speech
recognition (whisper.cpp) and synthesis (Kokoro) run on your machine; no
speech API, no telemetry, no recordings stored.

- `crates/`: the `voice-engine` daemon (Rust).
- `packages/`: the reusable TS client, the blob, and the OpenCode plugin.
- `docs/`: [architecture](docs/architecture.md), [protocol](docs/protocol.md).

Installation and usage: see `docs/install.md` (work in progress).
