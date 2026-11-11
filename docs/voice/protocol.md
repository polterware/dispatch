# Voice engine protocol v1

The normative types are `crates/voice-core/src/protocol.rs`. This document
explains the behavior around them.

## Transport and security

- The engine listens on `127.0.0.1` only, on a free port chosen at startup.
  It never binds `0.0.0.0` or `::`.
- At startup it generates a 32-byte random token (hex) and writes the
  discovery file (`voice_core::paths::discovery_file()`, mode `0600`):

  ```json
  { "protocol": 1, "engine_version": "0.1.0", "pid": 123, "port": 49152, "token": "…" }
  ```

  On macOS that is `~/Library/Application Support/Polterware Dispatch/voice/engine.json`.
- Every endpoint except `GET /v1/health` requires the token:
  `Authorization: Bearer <token>`, or `?token=<token>` on `/v1/ws`. Comparison
  is constant-time.
- Requests whose `Origin` header is present and is not `null`, `file://`, or
  an `http(s)://127.0.0.1|localhost` origin are refused (403). This blocks a
  web page from driving the engine even if it guessed the port.
- Messages over 1 MiB and `speak.text` over 64 KiB are refused (`bad_request`).
- Raw audio never leaves the engine. Clients receive levels and text only.

## HTTP

| Method | Path | Auth | Body / answer |
|---|---|---|---|
| GET | `/v1/health` | no | `Health` |
| GET | `/v1/status` | yes | `{ state: EngineState, models: ModelsStatus, memory_rss_bytes }` |
| POST | `/v1/shutdown` | yes | `{}`; the engine exits gracefully |
| GET | `/v1/ws` | yes | WebSocket upgrade |

`ModelsStatus`: `{ stt: { path, loaded, bytes? }, tts: { backend, loaded }, vad: { kind: "silero"|"energy", loaded } }`.

## WebSocket

Text frames, one JSON message each, tagged by `type`.

1. On connect the engine sends `hello` with its state.
2. `state` is broadcast to every client whenever `EngineState` changes.
3. `level` is broadcast to every client, ~30 Hz per source, only while the
   microphone is listening (`source: "mic"`) or speech is playing
   (`source: "output"`). Values are raw linear RMS and peak; clients smooth
   and normalize for display.
4. Replies to a request (`transcript.*`, `speak.*`, `error` with `id`) go to
   the client that sent it; `state` and `level` go to everyone.

### Listening

- `listen.start { id, language, mode }` opens the microphone. If another
  client is listening, the answer is `error { code: "busy", id }`. If the
  same client is listening, the previous utterance is cancelled first.
- Starting to listen cancels any playback (barge-in) and sends
  `speak.ended { reason: "cancelled" }` for it.
- `push-to-talk`: the utterance ends at `listen.stop`. `hands-free`: it ends
  when VAD reports end of speech, or at `listen.stop`. In both modes the
  engine enforces a 60 s maximum.
- While listening, the engine may send `vad` and `transcript.partial`
  (partials at most every ~800 ms, skipped while a previous partial is still
  running; they are best effort).
- After the end: `transcript.final { id, text, language, timings }`. Empty
  text means nothing intelligible was heard. The mic closes at once (it does
  not wait for inference).
- `listen.cancel` closes the mic and discards audio; no `transcript.final`.
- A client that disconnects while listening cancels its utterance.

### Speaking

- `speak { id, text, format, language, voice? }` queues an item. The engine
  prepares the text (`voice_tts::text::speakable`), splits it into sentences,
  synthesizes the first, starts playback, and keeps synthesizing ahead while
  it plays. `language: auto` uses `detect_language`.
- `speak.started { id, first_audio_ms }` when the first sample is audible;
  `speak.ended { id, reason }` when it finishes, is cancelled, or fails.
  An item whose prepared text is empty ends at once with `done`.
- `speak.cancel { id? }` stops the item (or all items) immediately.

### Models

- Models load on first use, or ahead of it with `models.warm`. Loading is
  shared: concurrent requests wait for one load.
- STT and TTS models unload after 10 minutes without use (configurable).
- The engine exits by itself 30 minutes after its last client disconnected
  (configurable; `0` disables), removing the discovery file.

## Errors

`error { code, message, id? }`, with `code` one of `busy`, `model_missing`,
`audio_device`, `mic_permission`, `stt_failed`, `tts_failed`, `bad_request`,
`internal`. An error never closes the connection.

## Catalog, models, and settings

Types: `crates/voice-core/src/catalog.rs`.

| Method | Path | Answer |
|---|---|---|
| GET | `/v1/catalog` | `Catalog { models, voices }` with installed/active flags |
| POST | `/v1/models/{id}/download` | 202; then `model.progress`, `model.installed` or `model.failed` on every WebSocket |
| DELETE | `/v1/models/{id}` | 204; 409 when the current settings use it; broadcasts `model.removed` |
| GET | `/v1/settings` | `EngineSettings` |
| PATCH | `/v1/settings` | `SettingsPatch` in, new `EngineSettings` out; persisted and broadcast as `settings.changed` |

Downloads run only on an explicit request (Settings › Voice shows the size and
license before the user confirms). Files go to `.part` and are renamed when
complete; one download per model at a time.

Voice ids name the backend: `piper:pt_BR-jeff-medium`, `kokoro:af_heart`,
`system:Luciana`. A `speak` request may name a voice id; otherwise the engine
uses `voice_pt` or `voice_en` from the settings for the text's language.
