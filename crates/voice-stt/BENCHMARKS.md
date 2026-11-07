# Voice STT benchmarks

Date: 2026-11-07  
Machine: Apple M1, 16 GB  
Status: measurements could not be collected in this sandbox.

## Reproduction commands

From the voice workspace root, after `cargo build --release -p voice-stt --example transcribe --offline` succeeds:

```sh
WHISPER="$HOME/Library/Application Support/voice-engine/models/whisper"
VAD="$HOME/Library/Application Support/voice-engine/models/vad/silero_vad.onnx"
for model in ggml-base.bin ggml-small-q5_1.bin ggml-large-v3-turbo-q5_0.bin; do
  for clip in ptbr-command.wav ptbr-long.wav en-command.wav; do
    for language in auto pt en; do
      cargo run --release --offline -p voice-stt --example transcribe -- \
        "$WHISPER/$model" "benchmarks/clips/$clip" "$language"
    done
  done
done
for clip in ptbr-command.wav ptbr-long.wav en-command.wav; do
  cargo run --release --offline -p voice-stt --example transcribe -- \
    vad "$VAD" "benchmarks/clips/$clip"
done
```

The transcribe example prints model load time and two consecutive inference times, plus text and detected language. For automatic Metal verification, retain whisper.cpp stderr and confirm the startup log reports the Metal backend.

## Results

The optimized example did not link: cached ONNX Runtime rc.15 contains CoreML objects referencing the unresolved Apple symbol `___isPlatformVersionAtLeast`. Retrying with `MACOSX_DEPLOYMENT_TARGET=13.4` and `27.2` did not resolve it. The ignored whisper model test also segfaulted while loading the cached model. Therefore no transcription, inference timing, Metal confirmation, or VAD inference timing is reported. The local Silero load test passes; its runtime tensor names are checked by `VadModel::load` against inputs `input`, `state`, `sr` and outputs `output`, `stateN`.

Audio lengths were read from the supplied WAV headers:

| Model | Clip | Language | Text | Detected language | Load ms | Warm inference ms (1 / 2) | Audio length |
|---|---|---|---|---|---:|---:|---:|
| all three | ptbr-command.wav | auto, pt, en | not measured | not measured | — | — | 4.467 s |
| all three | ptbr-long.wav | auto, pt, en | not measured | not measured | — | — | 8.662 s |
| all three | en-command.wav | auto, pt, en | not measured | not measured | — | — | 3.063 s |

VAD speech start/end times were not measured because the release example could not link. No default-model recommendation can be based on these runs; the architecture's existing `ggml-small-q5_1` default remains a reasonable candidate but still needs measurement and transcription-quality comparison on these clips.
