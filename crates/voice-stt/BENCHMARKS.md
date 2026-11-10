# Voice STT benchmarks

Date: 2026-11-07

Machine: Apple M1 Pro, 16 GB RAM

Execution: release build, CPU backend (`VOICE_STT_CPU=1`), 4 Whisper threads. Metal is unavailable in this sandbox: a default GPU load failed allocating a 4.41 MiB Metal buffer.

RSS: Mach `task_info(MACH_TASK_BASIC_INFO)` for the benchmark process; values are sampled after load and after transcriptions.

## Reproduction

```sh
TMPDIR="$PWD/crates/voice-tts/.tmp" \
MACOSX_DEPLOYMENT_TARGET=13.4 \
RUSTFLAGS="-C link-arg=$(clang --print-resource-dir)/lib/darwin/libclang_rt.osx.a" \
cargo build --release --offline -p voice-stt --example transcribe

for model in ggml-small-q5_1.bin ggml-base.bin; do
  for clip in ptbr-command.wav ptbr-long.wav en-command.wav; do
    language=pt
    [ "$clip" = en-command.wav ] && language=en
    TMPDIR="$PWD/crates/voice-tts/.tmp" VOICE_STT_CPU=1 \
      target/release/examples/transcribe \
      "$HOME/Library/Application Support/voice-engine/models/whisper/$model" \
      "benchmarks/clips/$clip" "$language"
  done
done
```

The example prints model load time, RSS after load and each transcription, both inference times, detected language, and text. The first inference is cold for that state; the second is warm.

## Short audio context experiment

Whisper-rs 0.16 exposes `FullParams::set_audio_ctx`. An experimental context of `ceil(clip_samples / 160) * 1.25` steps reduced warm CPU inference on most short clips, but changed `ggml-base` transcripts, so it is not retained or enabled by default. Exact transcripts from the baseline and experiment:

| Model | Clip | Default context transcript | 1.25x clip context transcript | Result |
|---|---|---|---|---|
| small-q5_1 | ptbr-command.wav | Cri um arquivo chamado teste.txt e rode os testes. | Cri um arquivo chamado teste.txt e rode os testes. | same |
| small-q5_1 | ptbr-long.wav | Refatore o componente de botão ou para aceitar uma variante secundária, atualize os testes unitários e me diga se algum teste falhar. | Refatore o componente de botão ou para aceitar uma variante secundária, atualize os testes unitários e me diga se algum teste falhar. | same |
| small-q5_1 | en-command.wav | Create a file named notes.md and run the tests. | Create a file named notes.md and run the tests. | same |
| base | ptbr-command.wav | Crie um arquivo chamado test.txt e rode os testes. | Crim o arquivo chamado test.txt e rode os testes. | changed |
| base | ptbr-long.wav | refator e o componente de botão ou para aceitar uma variante secundária, atualize os testes unitários e me diga se algum teste falhar. | refator e o componente de botão ou para aceitar uma variante secundária, atualize os testes unitários e mediga-se algum teste falhar. | changed |
| base | en-command.wav | Create a file named notes.md and run the tests. | Create a file named notes.md and run the tests. | same |

Warm CPU inference, baseline vs. clip context, milliseconds (second run):

| Model | ptbr-command | ptbr-long | en-command |
|---|---:|---:|---:|
| small-q5_1 baseline | 3587 | 3438 | 2671 |
| small-q5_1 clip context | 602 | 980 | 269 |
| base baseline | 544 | 544 | 478 |
| base clip context | 187 | 711 | 171 |

A second trial with a 2x duration margin retained the base long and English texts, but changed the base command to `Crie no arquivo chamado test.txt e rode os testes.` (baseline: `Crie um arquivo chamado test.txt e rode os testes.`). Since neither tested margin preserved every transcript on both models, no reduced-context path is shipped.

## Flash attention experiment

`SttOptions::flash_attn` exposes whisper.cpp's context parameter and defaults to `false`. GPU/Metal could not be exercised. With CPU execution, enabling it preserved all three small-model transcripts and the base command/English transcripts, but changed the base long-command ending to `mediga-se algum teste falhar.` (baseline: `me diga se algum teste falhar.`); the base long clip also ran slower (1.78 s vs. 0.54 s warm). Keep the safe default disabled. The Metal result still needs an architect-side measurement.

## RSS and inference observations

The CPU-only model-load RSS samples were 104.4 MiB for small-q5_1 and 161.0 MiB for base. A small-q5_1 command run rose to 270.9 MiB after the first inference and 362.7 MiB after the second; base rose to 273.0 MiB and 293.9 MiB. These values include Whisper state and compute allocations. whisper-rs 0.16 exposes context flags and per-call audio context, but no public compute-buffer sizing control.

| Model / clip | Before: load ms | Before: warm ms | After: default warm ms | After: transcript |
|---|---:|---:|---:|---|
| small-q5_1 / ptbr-command.wav | 296 | 2798 | 3587 | Cri um arquivo chamado teste.txt e rode os testes. |
| base / ptbr-command.wav | 114 | 609 | 544 | Crie um arquivo chamado test.txt e rode os testes. |

The “before” column is the initial CPU run; repeated runs show substantial CPU contention and timing variance. No default-path transcription optimization was kept, so the after transcripts match the benchmark baseline.
