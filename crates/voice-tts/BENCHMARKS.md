# Voice TTS benchmarks

Date: 2026-11-07

Machine: Apple M1 Pro, 16 GB RAM

Model: Kokoro quantized ONNX, pt-BR `pf_dora`, ORT CPU, 4 intra-op threads

RSS: Mach `task_info(MACH_TASK_BASIC_INFO)` in the benchmark process. Synthesis timings vary with host load.

## First-piece latency

Input: `Pronto, criei o arquivo e todos os testes passaram.` Before the split, `text::sentences` returned the entire sentence as piece one. The updated splitter emits `Pronto,` first, then the remainder. The first piece produced 602.7 ms of audio.

| Measure | Before: whole sentence | After: first piece `Pronto,` |
|---|---:|---:|
| Cold synthesis | 4457 ms | 361 ms median (3 runs: 490, 312, 361) |
| Warm synthesis | 4080 ms | 205 ms median (3 runs: 294, 191, 205) |
| Audio duration | 2831 ms | 603 ms |
| First-piece text | `Pronto, criei o arquivo e todos os testes passaram.` | `Pronto,` |

The input is intentionally shorter after the change; the numbers measure first-piece synthesis time, not full-reply completion.

## RSS and ORT session settings

The configured session uses 4 intra-op threads, 1 inter-op thread, memory pattern off, and the CPU EP arena allocator off. The table compares the original session builder with the configured builder, using the same first-piece text and three syntheses. One paired run showed essentially unchanged RSS; a repeat configured run was higher, so this is not a demonstrated resident-memory reduction. Sampling is sensitive to host pressure and allocator behavior.

| Session | RSS after load | RSS after synthesis 1 | RSS after synthesis 2 | RSS after synthesis 3 |
|---|---:|---:|---:|---:|
| Before: original ORT defaults | 112.8 MiB | 105.8 MiB | 193.6 MiB | 209.5 MiB |
| After: arena/pattern off, 4/1 threads (paired run) | 112.9 MiB | 196.2 MiB | 225.2 MiB | 209.0 MiB |
| After: repeat | 170.8 MiB | 251.1 MiB | 251.7 MiB | 252.0 MiB |

## Reproduction

```sh
TMPDIR="$PWD/crates/voice-tts/.tmp" \
MACOSX_DEPLOYMENT_TARGET=13.4 \
RUSTFLAGS="-C link-arg=$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a" \
cargo build --release --offline -p voice-tts --example say

TMPDIR="$PWD/crates/voice-tts/.tmp" \
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" \
target/release/examples/say --model-file model_quantized.onnx --lang pt-br --voice pf_dora \
  "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/tts.wav
```

The example prints load RSS, RSS after each of three syntheses, each synthesis duration, audio duration, peak, and phonemes. The Kokoro tensor names and types are available at `debug` log level; loading no longer writes model diagnostics to stdout or stderr by default.

The six Kokoro commands used the same executable and model directory:

```sh
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" target/release/examples/say --model-file model_quantized.onnx --lang pt-br --voice pf_dora "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/q-dora.wav
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" target/release/examples/say --model-file model_quantized.onnx --lang pt-br --voice pm_alex "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/q-alex.wav
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" target/release/examples/say --model-file model_quantized.onnx --lang en --voice af_heart "The file is ready and all tests passed." crates/voice-tts/.tmp/q-heart.wav
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" target/release/examples/say --model-file model_fp16.onnx --lang pt-br --voice pf_dora "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/f16-dora.wav
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" target/release/examples/say --model-file model_fp16.onnx --lang pt-br --voice pm_alex "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/f16-alex.wav
VOICE_KOKORO_DIR="$HOME/Library/Application Support/voice-engine/models/kokoro" target/release/examples/say --model-file model_fp16.onnx --lang en --voice af_heart "The file is ready and all tests passed." crates/voice-tts/.tmp/f16-heart.wav
target/release/examples/say --backend system --lang pt-br "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/system-pt.wav
```

## Piper pt-BR medium (2026-11-07)

ONNX Runtime CPU execution, 4 intra-op threads, 22.05 kHz mono. Text: `Pronto, criei o arquivo e todos os testes passaram.`

```sh
TMPDIR="$PWD/crates/voice-tts/.tmp" MACOSX_DEPLOYMENT_TARGET=13.4 RUSTFLAGS="-C link-arg=$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a" cargo build -p voice-tts --example piper --release --offline
TMPDIR="$PWD/crates/voice-tts/.tmp" target/release/examples/piper "$HOME/Library/Application Support/voice-engine/models/piper/pt_BR-cadu-medium.onnx" "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/piper-cadu.wav
TMPDIR="$PWD/crates/voice-tts/.tmp" target/release/examples/piper "$HOME/Library/Application Support/voice-engine/models/piper/pt_BR-jeff-medium.onnx" "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/piper-jeff.wav
TMPDIR="$PWD/crates/voice-tts/.tmp" target/release/examples/piper "$HOME/Library/Application Support/voice-engine/models/piper/pt_BR-faber-medium.onnx" "Pronto, criei o arquivo e todos os testes passaram." crates/voice-tts/.tmp/piper-faber.wav
```

| Model | Load ms | Synthesis ms | Audio ms | RTF | Peak |
|---|---:|---:|---:|---:|---:|
| `pt_BR-cadu-medium` | 5816.5 | 755.5 | 2844.1 | 0.266 | 0.5546 |
| `pt_BR-jeff-medium` | 2876.4 | 287.7 | 3177.0 | 0.091 | 0.5521 |
| `pt_BR-faber-medium` | failed | failed | — | — | — |

The Faber model loads, and shorter inputs synthesize, but `Session::run` segfaulted for the requested sentence and for its longer second clause. A short Faber excerpt (`Pronto,`) synthesized in 186.6 ms to 427.6 ms audio at peak 0.4827. The Cadu and Jeff WAVs were 2.844 s and 3.177 s, respectively; all three successful WAVs were mono and unclipped. The Faber crash is reproducible in this sandbox and needs investigation on the architect's machine.

Whisper `ggml-small-q5_1` on CPU heard Cadu as `Conte, criei o arquivo e todos os testes passaram.` and Jeff as `Pronto, vejo o arquivo e todos os testes passaram.` (each result repeated on a second pass). The short Faber excerpt transcribed as `PONCH`, so it was not a useful intelligibility check. The default Metal attempt failed to allocate a small buffer in this sandbox; `VOICE_STT_CPU=1` enabled the CPU check. Faber has no WAV for the requested sentence because inference crashed.
