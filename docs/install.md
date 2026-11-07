# Installation

Local Voice is an MVP for OpenCode Desktop. The installer builds the Rust
engine, builds the plugin GUI bundle, and prints the OpenCode configuration
entry for you to add. It does not edit your OpenCode configuration.

## Requirements

- Apple Silicon Mac running macOS 13.4 or later. Intel Macs and Linux are not
  currently verified.
- About 1 GB of free disk space for downloaded models and build artifacts.
- Rust and Cargo, CMake, `espeak-ng`, and Bun.
- OpenCode Desktop.

On macOS, install the system tools with Homebrew and install Rust with
[rustup](https://rustup.rs/) and Bun from its
[installation guide](https://bun.sh/docs/installation):

```sh
brew install cmake espeak-ng bun
```

The installer checks all four tools and reports any missing prerequisites. It
never installs system packages and never uses `sudo`.

## Build and install

From the repository checkout, run:

```sh
bash scripts/install.sh
```

The script builds `voice-engine` in release mode, copies it to
`~/.local/bin/voice-engine`, runs `bun install`, and builds the plugin's GUI
bundle. It is safe to run again; it replaces the copied binary and rebuilds the
bundle. If `~/.local/bin` is not on `PATH`, add it or set `VOICE_ENGINE_BIN` to
the binary path.

The script prints a `plugins` entry with the absolute path to
`packages/opencode-plugin`. Add that path to the existing global OpenCode
configuration at `~/.config/opencode/opencode.json` or
`~/.config/opencode/opencode.jsonc`, preserving any other plugin entries. Then
restart OpenCode Desktop or reload its plugins if that version provides a
reload action.

## Download models

Run this after installation when you are ready to download model files:

```sh
voice-engine models download
```

The command lists the files and asks `Download these model files? [y/N]` before
making any request. Answer `y` to consent. The default download set is:

| File | Size (bytes) | Approximate size |
|---|---:|---:|
| `whisper/ggml-small-q5_1.bin` | 190,085,487 | 181 MiB |
| `kokoro/model_quantized.onnx` | 92,361,116 | 88 MiB |
| `kokoro/tokenizer.json` | 3,497 | 3.4 KiB |
| `kokoro/config.json` | 44 | 44 B |
| `kokoro/voices/pf_dora.bin` | 522,240 | 510 KiB |
| `kokoro/voices/pm_alex.bin` | 522,240 | 510 KiB |
| `kokoro/voices/af_heart.bin` | 522,240 | 510 KiB |
| `kokoro/voices/am_michael.bin` | 522,240 | 510 KiB |
| `vad/silero_vad.onnx` | 2,327,524 | 2.2 MiB |

The Whisper model can be selected instead with `--stt base` (147,951,465
bytes, about 141 MiB) or `--stt turbo` (574,041,195 bytes, about 547 MiB).
These replace the default Whisper file. The default full set is 286,866,628
bytes (about 274 MiB).
After download, runtime speech recognition, speech synthesis, and voice
activity detection work offline; network traffic is used only when you
explicitly request model downloads. See [privacy](privacy.md) for details.

## Microphone permission

The engine opens the microphone only when voice capture starts. On macOS,
OpenCode Desktop should ask for microphone permission the first time the engine
captures audio. If permission was denied, enable it under **System Settings >
Privacy & Security > Microphone** for OpenCode, then restart the app if needed.

## Troubleshooting

- Run `voice-engine doctor` for engine, model, and audio checks. Run
  `voice-engine status` to inspect a running engine and loaded models.
- A detached engine writes logs to
  `~/Library/Application Support/voice-engine/engine.log` on macOS. On Linux,
  the log is under `${XDG_DATA_HOME:-~/.local/share}/voice-engine/engine.log`.
- If the plugin cannot find the engine, ensure `~/.local/bin` is on OpenCode's
  `PATH`, restart OpenCode, or set `VOICE_ENGINE_BIN` to the absolute path of
  `voice-engine`.
- The engine chooses a free localhost port at startup and writes its port and
  authentication token to `engine.json`. On macOS this is normally under
  `~/Library/Application Support/voice-engine/engine.json`; on Linux it is
  under `$XDG_RUNTIME_DIR/voice-engine/engine.json` when that runtime directory
  exists, or the app data directory otherwise. The plugin discovers this file
  automatically; do not expose the port outside localhost.
- If `espeak-ng` is missing or Kokoro cannot initialize, speech output falls
  back to macOS `say` where available. The system voice fallback does not need
  downloaded Kokoro files.
- On Linux, verify that the ALSA development library and audio device are
  available if compilation or microphone access fails (see Linux notes below).

## Linux notes (untested)

Linux support has not been tested. CPAL uses the system audio stack (typically
ALSA, with PulseAudio devices exposed through ALSA); Metal is unavailable, so
Whisper builds without the macOS Metal feature. Install a C/C++ toolchain,
`pkg-config`, ALSA development headers, CMake, and `espeak-ng`, for example on
Debian or Ubuntu:

```sh
apt install build-essential pkg-config libasound2-dev cmake espeak-ng
```

Install Rust/Cargo and Bun separately. Build with the normal release command;
no macOS deployment-target or Metal flags apply on Linux:

```sh
cargo build --release -p voice-server --bin voice-engine
```

## Uninstall

Run:

```sh
bash scripts/uninstall.sh
```

It removes `~/.local/bin/voice-engine`, prints how to remove the plugin path
from the OpenCode config, and asks before deleting downloaded model files.
Build artifacts and the source checkout are left in place.
