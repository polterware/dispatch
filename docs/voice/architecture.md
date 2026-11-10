# Architecture

A local voice companion for coding agents. Two independent parts:

1. **Voice Engine** (`voice-engine`, Rust): a headless daemon that owns the
   microphone, the speakers, STT, TTS, and VAD, and serves a small versioned
   API on `127.0.0.1` ([protocol](protocol.md)). It knows nothing about
   OpenCode.
2. **OpenCode plugin** (`opencode-local-voice`, TypeScript + Solid): a thin
   client that turns transcripts into prompts of the active session, speaks
   the agent's replies, and draws the voice blob.

```text
┌──────────── OpenCode Desktop (Electron) ────────────┐
│ window (Solid)                 main process (Node)   │
│  Voice panel + blob   ◀─Ipc─▶  EngineBridge          │──┐ spawn on demand
│  commands, header slot         (WS per window)       │  │ WS + HTTP, token
│  session.prompt / events                             │  ▼
└──────────────────────────────────────────────────────┘  voice-engine (Rust)
                                                          CPAL · whisper.cpp(Metal)
                                                          Silero VAD · Kokoro(ONNX)
```

## Crates and packages

| Path | Role |
|---|---|
| `crates/voice-core` | Protocol types, discovery file, paths. No heavy deps. |
| `crates/voice-audio` | CPAL capture (16 kHz mono) and streamed playback, level meters. |
| `crates/voice-stt` | whisper-rs (Metal) STT; Silero VAD (ort) and energy VAD. |
| `crates/voice-tts` | Kokoro (ort + `espeak-ng` process), macOS `say` fallback, speech text prep. |
| `crates/voice-server` | `voice-engine` binary: axum HTTP/WS, sessions, model lifecycle, CLI. |
| `packages/voice-client` | TS protocol mirror, discovery/spawn (Node), WS client. Reusable by any client. |
| `packages/voice-blob` | Solid + Canvas 2D blob component, no OpenCode dependency. |
| `packages/opencode-plugin` | The OpenCode plugin: server, GUI (window + main), and TUI parts. |

## Key decisions

| Topic | Decision | Why |
|---|---|---|
| STT | whisper.cpp via `whisper-rs` with Metal | Mature, MIT, good pt-BR, fast on M1 GPU. Default model `ggml-small-q5_1` (190 MB); `base` and `large-v3-turbo-q5_0` benchmarked as alternatives. |
| TTS | Kokoro-82M v1.0 ONNX (`model_quantized.onnx`, 92 MB) via `ort` | Apache-2.0 weights, pt-BR voices (`pf_dora`, `pm_alex`), real-time-plus on CPU. |
| Phonemes | `espeak-ng` executable, spawned per sentence | Kokoro needs IPA; espeak-ng is GPL-3.0, so it stays a separate process, never linked. |
| TTS fallback | macOS `say` | Zero download, works when Kokoro or espeak-ng is missing. |
| VAD | Silero VAD v5 ONNX (2 MB) via `ort`; energy VAD fallback | Robust end-of-speech for hands-free mode. Push-to-talk does not need it. |
| Audio I/O | CPAL, streams on dedicated threads | `!Send` streams on macOS; device opened only while active. |
| Transport | HTTP + WebSocket (axum, tokio), 127.0.0.1, token | Small, debuggable, reusable by any client. |
| Mic owner | The engine (not the browser) | Keeps audio out of JS; one implementation for every client; levels are tiny JSON. |
| GUI ↔ engine | Through the plugin's `main` process (Ipc) | `main` can spawn processes and is not bound by the window's CSP; OpenCode.app is the TCC-responsible process, and its Info.plist already declares microphone use. |
| Blob | Canvas 2D, one rAF loop only while visible and animating | A smooth radial path with layered gradients is enough at 60 FPS; WebGL adds cost without a visible gain at panel sizes. Throttled when idle, static with reduced motion. |
| Summaries | None (no extra LLM) | Text prep filters code, diffs, tables, and long output instead. |

## Lifecycle

- The plugin starts the engine on demand (when voice mode turns on), detached,
  with its log in `engine.log`. A running engine is reused (discovery file +
  health check), so every window and client shares one process and one set of
  loaded models.
- Models load on first use (or `models.warm` when voice mode turns on) and
  unload after 10 minutes idle. The engine exits 30 minutes after its last
  client leaves.
- With voice mode off, the engine has no open device and does no audio work.
- If the engine is missing or dies, the plugin shows a disconnected state and
  OpenCode keeps working normally.

## Interaction flow (push-to-talk)

1. The user turns on voice mode (command, `/voice`, or the panel). The plugin
   ensures the engine and warms the models.
2. Holding the talk button (or toggling the talk command) sends
   `listen.start`; the blob follows `level{mic}`; partials show live.
3. Release sends `listen.stop`; `transcript.final` arrives; non-empty text is
   sent with the session's normal prompt API, so the session, agent, model,
   context, and permissions are the ones the user already has.
4. The plugin watches the session's events; when the agent's turn ends, it
   reads the last assistant text and sends `speak` with `format: markdown`.
5. The blob follows `level{output}`; on `speak.ended` it returns to idle (or
   to listening in hands-free mode).

Permission prompts are never answered by voice: they stay in OpenCode's UI,
and the plugin announces only that the agent is waiting for approval.
