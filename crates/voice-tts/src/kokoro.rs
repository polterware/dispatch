use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

use ort::{ep::CPU, session::{builder::AutoDevicePolicy, Session}, value::Tensor};
use serde_json::Value;
use voice_core::protocol::Language;

use crate::{KokoroOptions, Result, TtsError, VoiceSpec};

pub struct KokoroTts {
    session: Mutex<Session>,
    vocab: HashMap<char, i64>,
    voices: HashMap<String, Vec<f32>>,
    espeak: PathBuf,
}

impl KokoroTts {
    /// Returns normalized IPA for diagnostics and CLI tooling.
    pub fn phonemize(&self, text: &str, language: Language) -> Result<String> {
        let language = if language == Language::Auto {
            crate::text::detect_language(text)
        } else {
            language
        };
        phonemize(&self.espeak, text, language)
    }
}

pub(super) fn load(options: KokoroOptions) -> Result<KokoroTts> {
    let model_file = if options.model_file.is_empty() {
        "model_quantized.onnx"
    } else {
        &options.model_file
    };
    let model = options.dir.join(model_file);
    let tokenizer = options.dir.join("tokenizer.json");
    for path in [&model, &tokenizer] {
        if !path.is_file() {
            return Err(TtsError::ModelMissing(path.clone()));
        }
    }
    let espeak = find_espeak(options.espeak.as_deref()).ok_or(TtsError::PhonemizerMissing)?;
    let tok: Value = serde_json::from_slice(
        &std::fs::read(&tokenizer).map_err(|e| TtsError::Load(e.to_string()))?,
    )
    .map_err(|e| TtsError::Load(e.to_string()))?;
    let mut vocab = HashMap::new();
    let object = tok
        .get("model")
        .and_then(|v| v.get("vocab"))
        .and_then(Value::as_object)
        .ok_or_else(|| TtsError::Load("tokenizer has no model.vocab".into()))?;
    for (ch, id) in object {
        if let (Some(c), Some(id)) = (
            ch.chars().next().filter(|_| ch.chars().count() == 1),
            id.as_i64(),
        ) {
            vocab.insert(c, id);
        }
    }
    let mut voices = HashMap::new();
    for name in ["pf_dora", "pm_alex", "pm_santa", "af_heart", "am_michael"] {
        let path = options.dir.join("voices").join(format!("{name}.bin"));
        if !path.is_file() {
            return Err(TtsError::ModelMissing(path));
        }
        let bytes = std::fs::read(&path).map_err(|e| TtsError::Load(e.to_string()))?;
        if bytes.len() != 510 * 256 * 4 {
            return Err(TtsError::Load(format!(
                "voice file has wrong size: {}",
                path.display()
            )));
        }
        let values = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        voices.insert(name.to_string(), values);
    }
    let threads = if options.threads == 0 {
        4
    } else {
        options.threads
    };
    let environment = ort::init().build().map_err(load_err)?;
    // ort registers CoreML automatically on macOS; keep inference on CPU.
    let session = Session::builder(&environment)
        .map_err(load_err)?
        .with_auto_device(AutoDevicePolicy::PreferCPU)
        .map_err(load_err)?
        .with_intra_threads(threads)
        .map_err(load_err)?
        .with_inter_threads(1)
        .map_err(load_err)?
        .with_memory_pattern(false)
        .map_err(load_err)?
        .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
        .map_err(load_err)?
        .commit_from_file(&model)
        .map_err(load_err)?;
    let inputs = session
        .inputs()
        .iter()
        .map(|value| format!("{}: {:?}", value.name(), value.dtype()))
        .collect::<Vec<_>>();
    let outputs = session
        .outputs()
        .iter()
        .map(|value| format!("{}: {:?}", value.name(), value.dtype()))
        .collect::<Vec<_>>();
    tracing::debug!(?inputs, ?outputs, "Kokoro ONNX model tensors");
    tracing::info!(model = %model.display(), threads, ?inputs, ?outputs, "loaded Kokoro ONNX model");
    Ok(KokoroTts {
        session: Mutex::new(session),
        vocab,
        voices,
        espeak,
    })
}
fn load_err(e: impl std::fmt::Display) -> TtsError {
    TtsError::Load(e.to_string())
}
pub(super) fn find_espeak(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_owned());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("espeak-ng");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    ["/opt/homebrew/bin/espeak-ng", "/usr/local/bin/espeak-ng"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}
static ESPEAK_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn phonemize(bin: &Path, text: &str, lang: Language) -> Result<String> {
    let voice = if lang == Language::PtBr {
        "pt-br"
    } else {
        "en-us"
    };
    phonemize_with_voice(bin, text, voice)
}
pub(super) fn phonemize_with_voice(bin: &Path, text: &str, voice: &str) -> Result<String> {
    let _guard = ESPEAK_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|e| TtsError::Synthesis(e.to_string()))?;
    // eSpeak omits punctuation from IPA output, so process clauses separately
    // and put the original punctuation back for Kokoro's pause model.
    let mut out = String::new();
    let mut start = 0;
    for (idx, ch) in text.char_indices() {
        if !matches!(ch, '.' | '!' | '?' | ',' | ';' | ':' | '—' | '…') {
            continue;
        }
        let words = &text[start..idx];
        if !words.trim().is_empty() {
            out.push_str(&normalize_phonemes(&espeak_words(bin, words, voice)?));
            out.push(' ');
        }
        out.push(ch);
        out.push(' ');
        start = idx + ch.len_utf8();
    }
    if !text[start..].trim().is_empty() {
        out.push_str(&normalize_phonemes(&espeak_words(
            bin,
            &text[start..],
            voice,
        )?));
    }
    Ok(out)
}
fn espeak_words(bin: &Path, text: &str, voice: &str) -> Result<String> {
    let mut child = Command::new(bin)
        .args(["-q", "--ipa=3", "-v", voice, "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| TtsError::Synthesis(e.to_string()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| TtsError::Synthesis("espeak stdin unavailable".into()))?
        .write_all(text.as_bytes())
        .map_err(|e| TtsError::Synthesis(e.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|e| TtsError::Synthesis(e.to_string()))?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(TtsError::Synthesis(format!(
            "espeak-ng failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub(super) fn normalize_phonemes(raw: &str) -> String {
    raw.chars()
        .filter(|&c| {
            !matches!(
                c,
                '\u{0361}' | '\u{200c}' | '\u{200d}' | '\u{feff}' | '\n' | '\r'
            )
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn tokenize(phonemes: &str, vocab: &HashMap<char, i64>) -> Vec<(char, i64)> {
    phonemes
        .chars()
        .filter_map(|c| vocab.get(&c).copied().map(|id| (c, id)))
        .collect()
}
fn split_tokens(tokens: &[(char, i64)]) -> Vec<Vec<(char, i64)>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < tokens.len() {
        let mut end = (start + 510).min(tokens.len());
        if end < tokens.len() {
            if let Some(i) = (start..end)
                .rev()
                .find(|&i| matches!(tokens[i].0, '.' | '!' | '?' | ',' | ';' | ':' | ' '))
            {
                if i > start {
                    end = i + 1;
                }
            }
        }
        chunks.push(tokens[start..end].to_vec());
        start = end;
    }
    chunks
}
pub(super) fn synthesize(tts: &KokoroTts, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
    let lang = if voice.language == Language::Auto {
        crate::text::detect_language(text)
    } else {
        voice.language
    };
    let raw = phonemize(&tts.espeak, text, lang)?;
    let normalized = raw
        .chars()
        .map(|c| {
            if tts.vocab.contains_key(&c) {
                c
            } else if c == '\u{2019}' {
                '\''
            } else {
                ' '
            }
        })
        .collect::<String>();
    let tokens = tokenize(&normalized, &tts.vocab);
    if tokens.is_empty() {
        return Err(TtsError::Synthesis(format!(
            "no supported phonemes from {raw:?}"
        )));
    }
    let voice_name = voice.name.as_deref().unwrap_or(if lang == Language::PtBr {
        "pf_dora"
    } else {
        "af_heart"
    });
    let style_data = tts
        .voices
        .get(voice_name)
        .ok_or_else(|| TtsError::Synthesis(format!("unknown voice: {voice_name}")))?;
    let speed = if voice.speed == 0.0 { 1.0 } else { voice.speed };
    let mut session = tts
        .session
        .lock()
        .map_err(|e| TtsError::Synthesis(e.to_string()))?;
    let mut audio = Vec::new();
    for chunk in split_tokens(&tokens) {
        let mut ids = Vec::with_capacity(chunk.len() + 2);
        ids.push(0);
        ids.extend(chunk.iter().map(|(_, id)| *id));
        ids.push(0);
        // kokoro-onnx selects voice[min(token_count, rows) - 1]. The
        // sequence here includes BOS/EOS, so this is the last content token.
        let row = chunk.len().saturating_sub(1).min(509);
        let style = style_data[row * 256..(row + 1) * 256].to_vec();
        let ids_tensor = Tensor::from_array(([1usize, ids.len()], ids))
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let style_tensor = Tensor::from_array(([1usize, 256usize], style))
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let speed_tensor = Tensor::from_array(([1usize], vec![speed]))
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let outputs=session.run(ort::inputs! {"input_ids"=>ids_tensor, "style"=>style_tensor, "speed"=>speed_tensor}).map_err(|e|TtsError::Synthesis(e.to_string()))?;
        let (_, samples) = outputs["waveform"]
            .try_extract_tensor::<f32>()
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let mut part = samples.to_vec();
        trim_silence(&mut part);
        if !audio.is_empty() && !part.is_empty() {
            audio.extend(std::iter::repeat_n(0.0, 960));
        }
        audio.extend(part);
    }
    Ok(audio)
}
fn trim_silence(samples: &mut Vec<f32>) {
    if samples.is_empty() {
        return;
    }
    let threshold = 0.008f32;
    let first = samples
        .iter()
        .position(|s| s.abs() > threshold)
        .unwrap_or(0);
    let last = samples
        .iter()
        .rposition(|s| s.abs() > threshold)
        .map(|i| i + 1)
        .unwrap_or(samples.len());
    let pad = 960usize;
    let start = first.saturating_sub(pad);
    let end = (last + pad).min(samples.len());
    *samples = samples[start..end].to_vec();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_joiners_and_chunks() {
        assert_eq!(normalize_phonemes("t͡ʃe\u{200d}st\n"), "tʃest");
        let v = (0..1200).map(|i| ('a', i as i64)).collect::<Vec<_>>();
        assert!(split_tokens(&v).iter().all(|c| c.len() <= 510));
    }
    #[test]
    #[ignore = "requires model files and espeak-ng"]
    fn local_inference() {
        let dir =
            PathBuf::from(std::env::var("VOICE_TEST_KOKORO_DIR").expect("set model directory"));
        let model = KokoroTts::load(KokoroOptions {
            dir,
            model_file: "model_quantized.onnx".into(),
            espeak: None,
            threads: 2,
        })
        .unwrap();
        let samples = super::synthesize(
            &model,
            "Pronto, tudo certo.",
            &VoiceSpec {
                language: Language::PtBr,
                name: None,
                speed: 1.0,
            },
        )
        .unwrap();
        assert!(samples.len() > 2_400);
    }
}
