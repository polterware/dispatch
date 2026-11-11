//! Piper VITS voices through ONNX Runtime.
//!
//! A voice is loaded from its `.onnx` model and adjacent `.onnx.json` config.

use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use ort::{
    session::{Session, builder::AutoDevicePolicy},
    value::Tensor,
};
use serde_json::Value;

use crate::{Result, TtsBackend, TtsError, VoiceSpec};

#[derive(Debug, Clone)]
pub struct PiperOptions {
    /// Path of `<name>.onnx`; the config is the same path with `.json` appended.
    pub model: PathBuf,
    /// Explicit `espeak-ng` path; `None` searches like Kokoro does.
    pub espeak: Option<PathBuf>,
    /// Intra-op threads. Default 4.
    pub threads: usize,
}

#[derive(Debug)]
struct Config {
    sample_rate: u32,
    espeak_voice: String,
    phoneme_ids: HashMap<char, Vec<i64>>,
    phoneme_map: HashMap<char, Vec<char>>,
    pad_ids: Vec<i64>,
    bos_ids: Vec<i64>,
    eos_ids: Vec<i64>,
    noise_scale: f32,
    length_scale: f32,
    noise_w: f32,
    num_speakers: usize,
}

pub struct PiperTts {
    session: Mutex<Session>,
    config: Config,
    espeak: PathBuf,
}

impl PiperTts {
    pub fn load(options: PiperOptions) -> Result<Self> {
        if !options.model.is_file() {
            return Err(TtsError::ModelMissing(options.model));
        }
        let config_path = PathBuf::from(format!("{}.json", options.model.display()));
        if !config_path.is_file() {
            return Err(TtsError::ModelMissing(config_path));
        }
        let espeak = crate::kokoro::find_espeak(options.espeak.as_deref())
            .ok_or(TtsError::PhonemizerMissing)?;
        let json: Value = serde_json::from_slice(
            &std::fs::read(&config_path).map_err(|e| TtsError::Load(e.to_string()))?,
        )
        .map_err(|e| TtsError::Load(format!("invalid Piper config: {e}")))?;
        let config = parse_config(&json)?;
        let threads = if options.threads == 0 {
            4
        } else {
            options.threads
        };
        let environment = ort::init().build().map_err(load_err)?;
        // ort registers CoreML automatically on macOS; keep inference on CPU.
        let mut builder = Session::builder(&environment)
            .map_err(load_err)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(load_err)?
            .with_intra_threads(threads)
            .map_err(load_err)?;
        // Piper inputs have dynamic lengths, so avoid caching memory patterns.
        builder = builder.with_memory_pattern(false).map_err(load_err)?;
        let session = builder.commit_from_file(&options.model).map_err(load_err)?;
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
        tracing::info!(model = %options.model.display(), threads, ?inputs, ?outputs, "loaded Piper ONNX model");
        Ok(Self {
            session: Mutex::new(session),
            config,
            espeak,
        })
    }

    /// Returns normalized IPA for diagnostics and CLI tooling.
    pub fn phonemize(&self, text: &str) -> Result<String> {
        crate::text::sentences(text)
            .iter()
            .map(|sentence| {
                crate::kokoro::phonemize_with_voice(
                    &self.espeak,
                    sentence,
                    &self.config.espeak_voice,
                )
            })
            .collect::<Result<Vec<_>>>()
            .map(|parts| parts.join(" "))
    }
}

fn parse_config(json: &Value) -> Result<Config> {
    let sample_rate = json
        .pointer("/audio/sample_rate")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n > 0)
        .ok_or_else(|| TtsError::Load("Piper config missing audio.sample_rate".into()))?;
    let espeak_voice = json
        .pointer("/espeak/voice")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TtsError::Load("Piper config missing espeak.voice".into()))?
        .to_owned();
    let inference = json
        .get("inference")
        .and_then(Value::as_object)
        .ok_or_else(|| TtsError::Load("Piper config missing inference".into()))?;
    let scale = |name: &str| -> Result<f32> {
        inference
            .get(name)
            .and_then(Value::as_f64)
            .map(|n| n as f32)
            .filter(|n| n.is_finite() && *n >= 0.0)
            .ok_or_else(|| TtsError::Load(format!("Piper config has invalid inference.{name}")))
    };
    let length_scale = scale("length_scale")?;
    if length_scale == 0.0 {
        return Err(TtsError::Load(
            "Piper config inference.length_scale must be positive".into(),
        ));
    }
    let id_map = json
        .get("phoneme_id_map")
        .and_then(Value::as_object)
        .ok_or_else(|| TtsError::Load("Piper config missing phoneme_id_map".into()))?;
    let mut phoneme_ids = HashMap::new();
    for (key, value) in id_map {
        let Some(ch) = one_char(key) else { continue };
        let Some(ids) = value.as_array() else {
            continue;
        };
        let ids = ids.iter().filter_map(Value::as_i64).collect::<Vec<_>>();
        if !ids.is_empty() {
            phoneme_ids.insert(ch, ids);
        }
    }
    let special = |symbol: char, label: &str| {
        phoneme_ids
            .get(&symbol)
            .cloned()
            .ok_or_else(|| TtsError::Load(format!("Piper config missing {label} phoneme id")))
    };
    let pad_ids = special('_', "PAD")?;
    let bos_ids = special('^', "BOS")?;
    let eos_ids = special('$', "EOS")?;
    let mut phoneme_map = HashMap::new();
    if let Some(mapping) = json.get("phoneme_map").and_then(Value::as_object) {
        for (from, to) in mapping {
            let Some(from) = one_char(from) else { continue };
            let Some(to) = to.as_array() else { continue };
            phoneme_map.insert(
                from,
                to.iter()
                    .filter_map(Value::as_str)
                    .flat_map(str::chars)
                    .collect(),
            );
        }
    }
    Ok(Config {
        sample_rate,
        espeak_voice,
        phoneme_ids,
        phoneme_map,
        pad_ids,
        bos_ids,
        eos_ids,
        noise_scale: scale("noise_scale")?,
        length_scale,
        noise_w: scale("noise_w")?,
        num_speakers: json
            .get("num_speakers")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(1),
    })
}

fn one_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

fn load_err(error: impl std::fmt::Display) -> TtsError {
    TtsError::Load(error.to_string())
}

fn input_ids(phonemes: &str, config: &Config) -> Vec<i64> {
    let mut ids = config.bos_ids.clone();
    for source in phonemes.chars() {
        let expanded = config
            .phoneme_map
            .get(&source)
            .cloned()
            .unwrap_or_else(|| vec![source]);
        for phoneme in expanded {
            if let Some(mapped) = config.phoneme_ids.get(&phoneme) {
                ids.extend_from_slice(mapped);
                ids.extend_from_slice(&config.pad_ids);
            } else {
                tracing::debug!(phoneme = %phoneme, "Piper phoneme has no id; skipping");
            }
        }
    }
    ids.extend_from_slice(&config.eos_ids);
    ids
}

fn synthesize(tts: &PiperTts, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
    let speed = if voice.speed == 0.0 { 1.0 } else { voice.speed };
    if !speed.is_finite() || speed <= 0.0 {
        return Err(TtsError::Synthesis("voice speed must be positive".into()));
    }
    let mut session = tts
        .session
        .lock()
        .map_err(|e| TtsError::Synthesis(e.to_string()))?;
    let mut audio = Vec::new();
    for sentence in crate::text::sentences(text) {
        let phonemes =
            crate::kokoro::phonemize_with_voice(&tts.espeak, &sentence, &tts.config.espeak_voice)?;
        let ids = input_ids(&phonemes, &tts.config);
        if ids.len() <= tts.config.bos_ids.len() + tts.config.eos_ids.len() {
            continue;
        }
        let len = i64::try_from(ids.len())
            .map_err(|_| TtsError::Synthesis("Piper input is too long".into()))?;
        let mut inputs: Vec<(String, ort::session::SessionInputValue<'_>)> = vec![
            (
                "input".to_owned(),
                Tensor::from_array(([1usize, ids.len()], ids))
                    .map_err(|e| TtsError::Synthesis(e.to_string()))?
                    .into(),
            ),
            (
                "input_lengths".to_owned(),
                Tensor::from_array(([1usize], vec![len]))
                    .map_err(|e| TtsError::Synthesis(e.to_string()))?
                    .into(),
            ),
            (
                "scales".to_owned(),
                Tensor::from_array((
                    [3usize],
                    vec![
                        tts.config.noise_scale,
                        tts.config.length_scale / speed,
                        tts.config.noise_w,
                    ],
                ))
                .map_err(|e| TtsError::Synthesis(e.to_string()))?
                .into(),
            ),
        ];
        if tts.config.num_speakers > 1 {
            inputs.push((
                "sid".to_owned(),
                Tensor::from_array(([1usize], vec![0i64]))
                    .map_err(|e| TtsError::Synthesis(e.to_string()))?
                    .into(),
            ));
        }
        let output_name = session.outputs().first().map(|o| o.name().to_owned());
        let outputs = session
            .run(inputs)
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let output_name = output_name.as_deref().unwrap_or("output");
        let (_, samples) = outputs[output_name]
            .try_extract_tensor::<f32>()
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let mut part = samples.to_vec();
        trim_silence(&mut part, tts.config.sample_rate);
        if !audio.is_empty() && !part.is_empty() {
            audio.extend(std::iter::repeat_n(
                0.0,
                (tts.config.sample_rate / 25) as usize,
            ));
        }
        audio.extend(part);
    }
    if audio.is_empty() {
        return Err(TtsError::Synthesis("no supported phonemes in text".into()));
    }
    Ok(audio)
}

fn trim_silence(samples: &mut Vec<f32>, sample_rate: u32) {
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
    let pad = (sample_rate / 25) as usize;
    *samples = samples[first.saturating_sub(pad)..(last + pad).min(samples.len())].to_vec();
}

impl TtsBackend for PiperTts {
    fn name(&self) -> &'static str {
        "piper"
    }
    fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }
    /// `voice.speed` maps to Piper's length scale (1/speed); `voice.name` is ignored.
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        synthesize(self, text, voice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            sample_rate: 22_050,
            espeak_voice: "pt-br".into(),
            phoneme_ids: HashMap::from([
                ('_', vec![0]),
                ('^', vec![1]),
                ('$', vec![2]),
                ('a', vec![4, 5]),
            ]),
            phoneme_map: HashMap::new(),
            pad_ids: vec![0],
            bos_ids: vec![1],
            eos_ids: vec![2],
            noise_scale: 0.667,
            length_scale: 1.0,
            noise_w: 0.8,
            num_speakers: 1,
        }
    }

    #[test]
    fn adds_piper_boundaries_and_padding_after_each_phoneme() {
        assert_eq!(input_ids("aa", &config()), vec![1, 4, 5, 0, 4, 5, 0, 2]);
    }

    #[test]
    fn applies_model_phoneme_remaps_and_skips_unknown_symbols() {
        let mut cfg = config();
        cfg.phoneme_map.insert('c', vec!['a']);
        assert_eq!(input_ids("c?", &cfg), vec![1, 4, 5, 0, 2]);
    }

    #[test]
    #[ignore = "requires Piper model files and espeak-ng"]
    fn local_inference() {
        let model = PathBuf::from(std::env::var("VOICE_TEST_PIPER_MODEL").expect("set model path"));
        let piper = PiperTts::load(PiperOptions {
            model,
            espeak: None,
            threads: 2,
        })
        .unwrap();
        let samples = piper
            .synthesize(
                "Pronto, criei o arquivo e todos os testes passaram.",
                &VoiceSpec {
                    language: voice_core::protocol::Language::PtBr,
                    name: None,
                    speed: 1.0,
                },
            )
            .unwrap();
        assert!(samples.len() > 2_400);
    }
}
