# OpenCode Local Voice

A local OpenCode Desktop voice companion backed by `voice-engine`.

## Install

Build this workspace, then add the package directory to the global OpenCode config at `~/.config/opencode/opencode.json`:

```json
{ "plugins": ["/absolute/path/to/packages/opencode-plugin"] }
```

This is documentation only; the config file is not edited by the build.

## Build

From the repository root run `bun install`, then `cd packages/opencode-plugin && bun run typecheck && bun run build`.

## Current limitations

The TUI plugin can start and stop push-to-talk capture, but the documented TUI plugin API does not expose a composer insertion or prompt submission API; it reports captured text in a toast. The GUI host integration is still experimental. Install `voice-engine` and make it available on PATH or set `VOICE_ENGINE_BIN`.
