# Architecture

Voice in the Dispatch app. One part outside the app:

**`dispatch-voice`** (`crates/voice-server`): a headless daemon that owns the
microphone, the speakers, STT, TTS, and VAD, and serves a small versioned API on
`127.0.0.1` ([protocol](protocol.md)). It is a binary of its own because its
models take hundreds of MB, and the core and the app stay lean:
`Contents/Resources/dispatch-voice` in the bundle, started on demand by the app.

```text
┌────────────── Dispatch.app (GPUI) ──────────────┐
│ run page · composer mic · Settings › Voice       │──┐ starts on demand
│ app/src/voice.rs (one WebSocket, one thread)     │  │ WS + HTTP, token
└──────────────────────────────────────────────────┘  ▼
                                      dispatch-voice (Rust)
                                      CPAL · whisper.cpp (Metal)
                                      Silero VAD · Piper/Kokoro (ONNX)
```

## Crates

| Path | Role |
|---|---|
| `crates/voice-core` | Protocol types, discovery file, paths. No heavy deps. |
| `crates/voice-audio` | CPAL capture (16 kHz mono) and streamed playback, level meters. |
| `crates/voice-stt` | whisper-rs (Metal) STT; Silero VAD (ort) and energy VAD. |
| `crates/voice-tts` | Piper, Kokoro, Chatterbox (ort), macOS `say` fallback, speech text prep. |
| `crates/voice-server` | The `dispatch-voice` binary: axum HTTP/WS, sessions, model lifecycle, CLI. |

## Key decisions

| Topic | Decision | Why |
|---|---|---|
| Separate binary | `dispatch-voice`, bundled next to the core | Whisper and TTS models are large; the core and the app stay small. |
| STT | whisper.cpp via `whisper-rs` with Metal | Mature, MIT, good pt-BR, fast on Apple Silicon. Default `whisper-small-q5_1`. |
| TTS | Piper (`pt_BR-jeff-medium` default), Kokoro, Chatterbox (experimental) via `ort` | Local voices with pt-BR; `say` covers a missing model. |
| Phonemes | `espeak-ng` executable, spawned per sentence | GPL-3.0 stays a separate process, never linked. |
| VAD | Silero VAD v5 ONNX via `ort`; energy VAD fallback | Robust end of speech for hands-free. Push-to-talk does not need it. |
| Audio I/O | CPAL, streams on dedicated threads | `!Send` streams on macOS; device opened only while active. |
| Transport | HTTP + WebSocket (axum, tokio), 127.0.0.1, token | Small and debuggable; one process shared by every window. |
| Mic owner | The engine, not the app | Audio never leaves the process; levels are tiny JSON messages. |
| Summaries | None (no extra LLM) | Text prep drops code, diffs, tables, and long output. |

## Lifecycle

- The app starts the engine on demand, detached, with its log in `engine.log`.
  A running engine is reused (discovery file + health check), so every window
  shares one process and one set of loaded models.
- Models load on first use (or with `models.warm`) and unload after 10 minutes
  idle. The engine exits 30 minutes after its last client leaves.
- With no client, the engine has no open device and does no audio work.
- macOS attributes the engine's capture to Dispatch, which declares
  `NSMicrophoneUsageDescription`.

## Interaction flow

1. The mic action in the composer starts `listen.start`; levels arrive as
   `level{mic}` and partials as `transcript.partial`.
2. Stopping sends `listen.stop`; `transcript.final` fills the composer, or is
   sent at once when the setting asks for it.
3. When that run ends `COMPLETED`, its reply goes to `speak` with
   `format: markdown`. Talking again cancels playback.
