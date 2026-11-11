# Usage

## In the app

The mic action in the composer (on the New page and on a run's page) starts
listening; a second press stops it. The transcript fills the composer, or is
sent at once when **Send when I stop** is on. When a run started that way ends
`COMPLETED`, its reply is read aloud; a play action on any reply in the
transcript reads it on demand, and talking again cancels playback.

Speech drops code blocks, diffs, tables, and long paths. Approvals are never
answered by voice: they stay in the app.

Language detection covers Portuguese (Brazil) and English. The default
Portuguese voice is Piper `pt_BR-jeff-medium`; English uses Kokoro `af_heart`.

## Settings › Voice

Language (pt-BR by default), a voice per language with **Test**, speed, live
transcript, read replies aloud, send when I stop, the STT model, and the model
list with **Download** (size, license, progress) and **Delete** (refused while
in use). The same settings are served by the engine as `GET/PATCH /v1/settings`
([protocol](protocol.md)).

## Running the engine alone

```sh
cargo run -p voice-server --bin dispatch-voice          # serve (default)
dispatch-voice status | stop | doctor
dispatch-voice models list
dispatch-voice models download                    # asks before any request
dispatch-voice models download --stt base --no-tts
dispatch-voice models remove kokoro-v1
dispatch-voice bench --wav speech.wav             # STT/TTS timing
```

`serve --port <n>` fixes the port; `serve --if-not-running` reuses a running
engine instead of failing.

## Configuration

The optional JSON configuration is
`~/Library/Application Support/Polterware Dispatch/voice/config.json`. Omitted
keys use these defaults:

| Key | Default | Meaning |
|---|---|---|
| `language` | `"pt-br"` | Language spoken and heard; `auto` detects per utterance. |
| `stt_model` | `"whisper-small-q5_1"` | Catalog id of the Whisper model. |
| `voice_pt` | `"piper:pt_BR-jeff-medium"` | Voice id for pt-BR. |
| `voice_en` | `"kokoro:af_heart"` | Voice id for English. |
| `speed` | `1.0` | Speech synthesis speed, 0.5 to 2.0. |
| `partials` | `true` | Interim transcript updates while listening. |
| `model_idle_secs` | `600` | Unload idle models after 10 minutes. |
| `exit_after_idle_secs` | `1800` | Exit 30 minutes after the last client leaves; `0` disables it. |
| `input_device` | `null` | Input device name; `null` uses the system default. |
| `output_device` | `null` | Output device name; `null` uses the system default. |
| `max_utterance_secs` | `60` | Maximum duration of one listening session. |

The app writes the first block through `PATCH /v1/settings`; the device and
utterance keys are read at startup.
