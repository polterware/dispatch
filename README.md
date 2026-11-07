# Local Voice for OpenCode

**MVP:** a local voice companion for OpenCode Desktop. Speak to your coding
agent, see live transcription, and hear its replies through locally run speech
recognition and synthesis.

- Rust `voice-engine` handles microphone input, transcription, speech, and VAD.
- An OpenCode plugin provides voice controls and an animated voice blob.
- Push-to-talk and hands-free modes, with pt-BR and English auto-detection.
- Microphone audio and models stay on your machine; there is no telemetry.
- Permission prompts remain in OpenCode and are never answered by voice.

## Quick start

From a checkout of this repository:

```sh
bash scripts/install.sh
voice-engine models download
voice-engine doctor
```

The installer prints the plugin entry to add to
`~/.config/opencode/opencode.json` or `.jsonc`; add it and restart OpenCode
Desktop. The model downloader asks before contacting model hosts. The
installer builds the engine and plugin but does not edit OpenCode
configuration. See the [installation guide](docs/install.md),
[usage guide](docs/usage.md),
[architecture](docs/architecture.md), [protocol](docs/protocol.md), and
[privacy notes](docs/privacy.md).
