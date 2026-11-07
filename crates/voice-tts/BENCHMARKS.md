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
