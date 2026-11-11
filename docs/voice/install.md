# Installation

Voice ships inside `Dispatch.app`: `Contents/Resources/dispatch-voice` is the
engine, started by the app when you first talk to a run. Nothing else to install.

## Requirements

- Apple Silicon Mac running macOS 15 or later (the bundle's minimum; ONNX
  Runtime is built for 13.4 and up).
- About 1 GB of free disk space for the models.

## Models

Settings › Voice lists the models with their size and license; **Download**
fetches the ones you pick. The default is Whisper Small Q5_1 (181 MiB) for
speech recognition, Piper `pt_BR-jeff-medium` (60 MiB) for speech, and Silero
VAD v5 (2 MiB) for end-of-speech detection. Kokoro (92 MiB) and Whisper Base
and Large V3 Turbo are alternatives; Chatterbox is experimental and large.

Files land in `~/Library/Application Support/Polterware Dispatch/voice/models`
(`whisper/`, `piper/`, `kokoro/`, `chatterbox/`, `vad/`). Network traffic
happens only when you ask for a download; see [privacy](privacy.md).

The engine reads models from the same folder when it runs alone from a
development build, and it moves the ones an earlier standalone engine left in
`~/Library/Application Support/voice-engine/models` once, on first start.

## Microphone permission

The engine opens the microphone only when capture starts. macOS asks for
microphone permission for **Dispatch** the first time; the app declares
`NSMicrophoneUsageDescription`, and the system attributes the engine's capture
to it. If you denied it, enable it under **System Settings > Privacy &
Security > Microphone**, then restart Dispatch.

## Troubleshooting

- `dispatch-voice doctor` checks the engine, the models, and the audio devices;
  `dispatch-voice status` inspects a running engine and its loaded models.
- The engine logs to
  `~/Library/Application Support/Polterware Dispatch/voice/engine.log`.
- If speech output fails, the engine falls back to the macOS `say` voice, which
  needs no downloaded files. Piper also needs `espeak-ng` on `PATH`
  (`brew install espeak-ng`) for phonemes.
- The engine writes its port and token to
  `~/Library/Application Support/Polterware Dispatch/voice/engine.json`; the
  app finds it there. Do not expose the port outside localhost.
