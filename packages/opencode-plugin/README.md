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

## Behavior and limitations

The GUI exposes Voice mode (`/voice`, `mod+shift+v`), push-to-talk (`mod+shift+space`), mute, stop and hands-free controls. It submits transcripts to the currently routed session and speaks successful assistant text. The OpenCode 2.2.6 `SessionPromptInput` has no model field; before sending a transcript, the plugin reads `screen.composer.model()` and applies that selection through `session.switchModel`, including its variant, then calls `session.prompt`. This matches the composer's selected choice and makes it the session's current model choice.

The TUI command starts/stops capture and submits transcripts directly when the current route is a session. It preserves `ctx.ui.model.current()` by applying that model with `session.switchModel` before `session.prompt`. Outside a session it shows the recognized text in a toast. The GUI host integration is experimental. Install `voice-engine` and make it available on PATH or set `VOICE_ENGINE_BIN`.
