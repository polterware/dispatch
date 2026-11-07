# Usage

## Voice controls

Turn voice mode on or off from the OpenCode command palette with **Voice mode**,
type `/voice`, or press **⌘⇧V**. Voice mode starts the engine when needed and
warms the speech models. When voice mode is off, the engine does not capture
audio.

- **Push-to-talk:** hold the panel's **Hold to talk** button, or focus the
  Voice panel and press **Space** to start or stop. Release the button to
  finish the utterance. Press
  **⌘⇧Space** to start or stop push-to-talk from the keyboard.
- **Hands-free:** turn on **Hands-free** in the panel or command palette. The
  voice activity detector ends each utterance after speech stops. You can still
  stop capture manually.
- **Mute:** use **Mute** in the panel or the **Mute voice microphone** command.
  Select **Unmute** to capture again.
- **Stop:** stops current listening and cancels speech playback.
- **Back to text:** leave voice mode and return focus to OpenCode's composer.

Recognized non-empty transcripts are sent as prompts to the currently selected
OpenCode session. The plugin speaks assistant text replies. It omits code
blocks, diffs, and tables, and shortens long paths; ordinary prose and headings
are spoken. Permission prompts are never answered by voice: approvals remain in
OpenCode's own UI, and voice only announces that the agent is waiting for
approval.

Language detection is automatic for Portuguese (Brazil) and English. The
configured Portuguese voice is `pf_dora`; English uses `af_heart` by default.

## Configuration

The optional JSON configuration file is
`~/Library/Application Support/voice-engine/config.json` on macOS and
`${XDG_DATA_HOME:-~/.local/share}/voice-engine/config.json` on Linux. Omitted
keys use these defaults:

| Key | Default | Meaning |
|---|---|---|
| `stt_model` | `"ggml-small-q5_1.bin"` | Whisper model filename under `models/whisper`. |
| `tts_model_file` | `"model_quantized.onnx"` | Kokoro model filename under `models/kokoro`. |
| `voice_pt` | `"pf_dora"` | Kokoro voice for pt-BR. |
| `voice_en` | `"af_heart"` | Kokoro voice for English. |
| `speed` | `1.0` | Speech synthesis speed. |
| `model_idle_secs` | `600` | Unload idle speech models after 10 minutes. |
| `exit_after_idle_secs` | `1800` | Exit 30 minutes after the last client disconnects; `0` disables this. |
| `input_device` | `null` | Optional input device name; `null` uses the system default. |
| `output_device` | `null` | Optional output device name; `null` uses the system default. |
| `max_utterance_secs` | `60` | Maximum duration of one listening session. |
| `partials` | `true` | Enable interim transcript updates while listening. |

## TUI fallback

The plugin also provides a terminal UI command for starting and stopping voice
capture. The engine must be on `PATH` or selected with `VOICE_ENGINE_BIN`. In a
session, recognized text is sent as a prompt; outside a session, the plugin
shows the transcript in a toast. The graphical panel and its keyboard
shortcuts are available in OpenCode Desktop.
