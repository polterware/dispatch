# OpenCode Local Voice

An OpenCode Desktop plugin for the local `voice-engine`. The GUI provides
voice mode, push-to-talk, hands-free listening, mute and stop controls, plus an
animated voice blob. It submits transcripts to the active session and speaks
assistant text locally.

## Install

From the repository root, run `bash scripts/install.sh`. It builds the engine
and GUI bundle, installs `voice-engine` to `~/.local/bin`, and prints the
absolute plugin path to add to the global OpenCode config at
`~/.config/opencode/opencode.json` or `opencode.jsonc`. The installer does not
edit that config. Follow [the installation guide](../../docs/install.md) and
restart OpenCode Desktop after adding the plugin.

The engine must be on `PATH` or set `VOICE_ENGINE_BIN` to its absolute path.
Use `voice-engine models download` to download speech models; it asks before
making network requests. See [usage](../../docs/usage.md) and
[privacy](../../docs/privacy.md).

## Development

From the repository root, run `bun install`, then `bun run --cwd
packages/opencode-plugin typecheck` and `bun run --cwd
packages/opencode-plugin build --configLoader runner`.

The GUI host integration is experimental. Voice permission prompts are never
answered by speech; they stay in OpenCode's UI. The TUI command starts/stops
capture and submits transcripts in an active session; outside a session it
shows the transcript in a toast.
