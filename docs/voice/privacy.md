# Privacy and security

## What stays local

- Microphone audio is captured, processed, and discarded inside the
  `voice-engine` process. It is never written to disk, never sent to a client,
  and never sent to OpenCode or any model provider.
- Only the final transcript text is submitted, as a normal prompt of the
  session the user is looking at. From there it follows the user's OpenCode
  configuration like any typed prompt.
- Spoken replies are synthesized locally from the assistant text the plugin
  already received from OpenCode.
- No telemetry, analytics, or crash reporting exists in the engine or the
  plugin.

## External network traffic

The runtime makes no network requests. Traffic happens only when you ask for
it:

| When | Destination | What |
|---|---|---|
| `voice-engine models download` (explicit confirmation) | `huggingface.co` (ggerganov/whisper.cpp, onnx-community/Kokoro-82M-v1.0-ONNX), `raw.githubusercontent.com` (snakers4/silero-vad) | Model files |
| Building from source | crates.io, npm, and the ONNX Runtime prebuilt binaries the `ort` crate fetches at build time | Dependencies |
| Installing tools | Homebrew (`cmake`, `espeak-ng`) | Build tool and phonemizer |

## Local API protection

- The engine binds `127.0.0.1` only.
- A random 256-bit token, stored in an owner-only (`0600`) discovery file, is
  required on every endpoint except the health check.
- Browser origins other than localhost are refused, so a web page cannot drive
  the microphone even if it finds the port.
- Message sizes are capped and every message is validated against the typed
  protocol; malformed input gets an error, never a crash.
- Voice never answers OpenCode permission prompts: approvals stay in
  OpenCode's own UI.

## Licenses of runtime components

| Component | License |
|---|---|
| whisper.cpp / whisper-rs | MIT / Unlicense |
| Whisper model weights | MIT |
| Kokoro-82M weights | Apache-2.0 |
| Silero VAD | MIT |
| ONNX Runtime | MIT |
| espeak-ng (separate executable, not linked) | GPL-3.0 |
| CPAL, axum, tokio | Apache-2.0 / MIT |
