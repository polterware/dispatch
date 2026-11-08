# Voice TTS benchmarks

Date: 2026-11-07  
Machine: Apple M1, 16 GB RAM  
Runtime: release build, ONNX Runtime CPU execution, 4 intra-op threads  
Kokoro sample rate: 24 kHz mono  

Each cold/warm measurement synthesizes the first sentence twice in one process. `audio` is the duration of the second WAV; RTF is synthesis time divided by that duration. Peak is the absolute float sample peak before WAV quantization.

## Results

| Model/backend | Language / voice | Load ms | Cold synth ms | Warm synth ms | Audio ms | Cold / warm RTF | Peak |
|---|---|---:|---:|---:|---:|---:|---:|
| Kokoro quantized | pt-BR / `pf_dora` | 2314.5 | 633.4 | 482.5 | 2830.7 | 0.224 / 0.170 | 0.5822 |
| Kokoro quantized | pt-BR / `pm_alex` | 2414.9 | 684.7 | 595.1 | 2937.6 | 0.233 / 0.203 | 0.6467 |
| Kokoro quantized | English / `af_heart` | 2224.0 | 576.3 | 472.9 | 2202.2 | 0.262 / 0.215 | 0.5187 |
| Kokoro fp16 | pt-BR / `pf_dora` | 1127.5 | 894.6 | 901.4 | 2921.9 | 0.306 / 0.308 | 0.6021 |
| Kokoro fp16 | pt-BR / `pm_alex` | 1128.5 | 892.8 | 929.2 | 2912.9 | 0.306 / 0.319 | 0.6528 |
| Kokoro fp16 | English / `af_heart` | 1016.0 | 743.9 | 742.3 | 2202.0 | 0.338 / 0.337 | 0.5234 |
| macOS `say` | pt-BR / default | 0.0 | 999.2* | 850.6* | 0.0* | n/a | 0.0000* |

`*` The sandboxed `say` command exited successfully but wrote a 44-byte WAV header with zero frames. These timings and peak describe the failed empty-audio attempt, not valid speech; the backend now returns `say returned an empty audio file` for this condition.

## Phonemes and WAV checks

pt-BR text: `Pronto, criei o arquivo e todos os testes passaram.`  
Phonemes (both Kokoro models and both pt-BR voices): `prˈoŋtʃ , kriˈeɪ u ˌaɾəkˈivw i tˈodʊz ʊs tˈɛstʃys pˌasˈaɾæ .`

English text: `The file is ready and all tests passed.`  
Phonemes (both Kokoro models): `ðə fˈaɪl ɪz ɹˈɛdi ænd ˈɔːl tˈɛsts pˈæs .`

All six Kokoro WAVs were nonempty, mono 16-bit PCM at 24 kHz, with durations 2.202–2.938 s. A sample scan found 98%+ nonzero samples and maximum peaks from 0.5187 to 0.6467; no clipping or silence was found. The English clips are about 2.20 s; pt-BR clips are about 2.83–2.94 s.

The tokenizer JSON contains a `model.vocab` character-to-ID map, including the `$` special token with ID 0. The graph reported inputs `input_ids` (int64), `style` (float32), `speed` (float32), and output `waveform` (float32). The style vector row follows kokoro-onnx's `_style_for`: for `n` phoneme tokens use row `n - 1` (clamped to available rows); input IDs separately wrap those tokens with BOS/EOS. See [kokoro-onnx reference](https://github.com/thewh1teagle/kokoro-onnx/blob/main/src/kokoro_onnx/__init__.py#L1234-L1242).

## Commands

The cached ONNX Runtime static library needs macOS 13.4 symbols when linking this release binary, so the build sets the deployment target and links Clang's OS runtime archive:

```sh
TMPDIR="$PWD/crates/voice-tts/.tmp" \
MACOSX_DEPLOYMENT_TARGET=13.4 \
RUSTFLAGS="-C link-arg=$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a" \
cargo build -p voice-tts --example say --release --offline
```

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
