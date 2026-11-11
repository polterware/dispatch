# Privacy and security

## What stays local

- Microphone audio is captured, processed, and discarded inside
  `dispatch-voice`. It is never written to disk, never sent to a client, and
  never sent to a model provider.
- Only the final transcript text reaches the app, which puts it in the composer
  like typed text. From there it follows the run's normal routing.
- Spoken replies are synthesized locally from the reply text the app already
  holds.
- No telemetry, analytics, or crash reporting exists in the engine or the app.

## External network traffic

The runtime makes no network requests of its own. Traffic happens only when you
ask for it:

| When | Destination | What |
|---|---|---|
| **Download** in Settings › Voice (explicit confirmation) | `huggingface.co`, `raw.githubusercontent.com` (Silero VAD) | Model files |
| Building from source | crates.io and the ONNX Runtime prebuilt binaries the `ort` crate fetches at build time | Dependencies |
| Installing tools | Homebrew (`espeak-ng`) | Phonemizer |

## Local API protection

- The engine binds `127.0.0.1` only.
- A random 256-bit token, stored in an owner-only (`0600`) discovery file, is
  required on every endpoint except the health check.
- Browser origins other than localhost are refused, so a web page cannot drive
  the microphone even if it finds the port.
- Message sizes are capped and every message is validated against the typed
  protocol; malformed input gets an error, never a crash.

## Licenses of runtime components

| Component | License |
|---|---|
| whisper.cpp / whisper-rs | MIT / Unlicense |
| Whisper model weights | MIT |
| Piper voices | CC0 dataset; Piper MIT |
| Kokoro-82M weights | Apache-2.0 |
| Chatterbox weights | MIT |
| Silero VAD | MIT |
| ONNX Runtime | MIT |
| espeak-ng (separate executable, not linked) | GPL-3.0 |
| CPAL, axum, tokio | Apache-2.0 / MIT |
