//! Chatterbox Multilingual (Resemble AI, MIT) through ONNX Runtime.
//!
//! CONTRACT (owned by the architect; keep these signatures):
//! - Files under `dir` (`models/chatterbox`), as published by onnx-community/chatterbox-multilingual-ONNX:
//!   `tokenizer.json`, `default_voice.wav`, and `onnx/{speech_encoder,embed_tokens,
//!   language_model_q4,conditional_decoder}.onnx` with their `.onnx_data` files.
//! - Pipeline (see the model card's reference script): text -> `[pt]` language tag + tokenizer
//!   -> embed_tokens -> 0.5B Llama language model with a KV cache, greedy decoding with a 1.2
//!   repetition penalty, from START_SPEECH_TOKEN 6561 until STOP_SPEECH_TOKEN 6562 -> speech
//!   tokens -> conditional_decoder -> 24 kHz audio.
//! - The reference voice's conditioning (speech_encoder outputs) is computed once per voice
//!   and cached; the speech encoder session can then be dropped to save memory.
//! - No watermarking is applied (the reference script's Perth watermark is optional and
//!   Python-only).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use ort::{
    ep::CPU,
    session::{builder::AutoDevicePolicy, Session, SessionInputValue},
    value::{DynValue, Tensor, ValueType},
};
use tokenizers::Tokenizer;
use voice_core::protocol::Language;

use crate::{Result, TtsBackend, TtsError, VoiceSpec};

const SAMPLE_RATE: u32 = 24_000;
const START_SPEECH_TOKEN: i64 = 6561;
const STOP_SPEECH_TOKEN: i64 = 6562;
const LAYERS: usize = 30;
const KV_HEADS: usize = 16;
const HEAD_DIM: usize = 64;
const HARD_MAX_TOKENS: usize = 1000;

#[derive(Debug, Clone)]
pub struct ChatterboxOptions {
    pub dir: PathBuf,
    /// A 24 kHz-or-any-rate mono WAV of the voice to imitate; `None` uses `default_voice.wav`.
    pub reference_voice: Option<PathBuf>,
    /// 0.25..=2.0, default 0.5 (the model card's default).
    pub exaggeration: f32,
    /// Intra-op threads. Default 4.
    pub threads: usize,
}

struct Conditioning {
    cond_emb: DynValue,
    prompt_token: DynValue,
    speaker_embeddings: DynValue,
    speaker_features: DynValue,
}

pub struct ChatterboxTts {
    tokenizer: Tokenizer,
    embed_tokens: Mutex<Session>,
    language_model: Mutex<Session>,
    conditional_decoder: Mutex<Session>,
    conditioning: Conditioning,
    exaggeration: f32,
}

impl ChatterboxTts {
    pub fn load(options: ChatterboxOptions) -> Result<Self> {
        if !chatterbox_installed(&options.dir) {
            return Err(TtsError::ModelMissing(options.dir));
        }
        let tokenizer_path = options.dir.join("tokenizer.json");
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| TtsError::Load(format!("cannot load tokenizer: {e}")))?;
        let voice_path = options
            .reference_voice
            .unwrap_or_else(|| options.dir.join("default_voice.wav"));
        let audio = read_wav_mono_24k(&voice_path)?;
        let threads = if options.threads == 0 { 4 } else { options.threads };
        let environment = ort::init().build().map_err(load_err)?;
        let speech_path = options.dir.join("onnx/speech_encoder.onnx");
        let mut speech_encoder = make_session(&environment, &speech_path, threads)?;
        log_io("Chatterbox speech encoder", &speech_encoder);

        let encoder_inputs = bind_named(&speech_encoder, HashMap::from([
            ("audio_values", Tensor::from_array(([1usize, audio.len()], audio)).map_err(load_err)?.into()),
        ]))?;
        let started = std::time::Instant::now();
        let mut encoded = speech_encoder.run(encoder_inputs).map_err(load_err)?;
        tracing::debug!(ms = started.elapsed().as_millis() as u64, "Chatterbox reference voice encoded");
        let cond_emb = take_output(&mut encoded, &["cond_emb", "cond_embeds", "audio_features"])?;
        let prompt_token = take_output(&mut encoded, &["prompt_token", "prompt_tokens", "audio_tokens"])?;
        let speaker_embeddings = take_output(&mut encoded, &["ref_x_vector", "speaker_embeddings"])?;
        let speaker_features = take_output(&mut encoded, &["prompt_feat", "speaker_features"])?;
        drop(encoded);
        drop(speech_encoder);

        let embed_tokens = make_session(&environment, &options.dir.join("onnx/embed_tokens.onnx"), threads)?;
        let language_model = make_session(&environment, &options.dir.join("onnx/language_model_q4.onnx"), threads)?;
        let conditional_decoder = make_session(&environment, &options.dir.join("onnx/conditional_decoder.onnx"), threads)?;
        log_io("Chatterbox embed tokens", &embed_tokens);
        log_io("Chatterbox language model", &language_model);
        log_io("Chatterbox conditional decoder", &conditional_decoder);
        let exaggeration = if options.exaggeration == 0.0 { 0.5 } else { options.exaggeration };
        if !exaggeration.is_finite() || !(0.25..=2.0).contains(&exaggeration) {
            return Err(TtsError::Load("Chatterbox exaggeration must be between 0.25 and 2.0".into()));
        }
        Ok(Self {
            tokenizer,
            embed_tokens: Mutex::new(embed_tokens),
            language_model: Mutex::new(language_model),
            conditional_decoder: Mutex::new(conditional_decoder),
            conditioning: Conditioning { cond_emb, prompt_token, speaker_embeddings, speaker_features },
            exaggeration,
        })
    }

    fn synthesize_inner(&self, text: &str, language: Language) -> Result<Vec<f32>> {
        let tag = if language == Language::En { "[en]" } else { "[pt]" };
        let tokenized = self.tokenizer.encode(format!("{tag}{text}"), true)
            .map_err(|e| TtsError::Synthesis(format!("tokenization failed: {e}")))?;
        let ids = tokenized.get_ids().iter().map(|&id| id as i64).collect::<Vec<_>>();
        if ids.is_empty() { return Err(TtsError::Synthesis("text produced no tokens".into())); }
        let positions = ids.iter().enumerate().map(|(i, id)| if *id >= START_SPEECH_TOKEN { 0 } else { i as i64 - 1 }).collect::<Vec<_>>();
        let mut embed = self.embed_tokens.lock().map_err(|e| synth_err(e.to_string()))?;
        let embed_inputs = bind_named(&embed, HashMap::from([
            ("input_ids", Tensor::from_array(([1usize, ids.len()], ids.clone())).map_err(synth_err)?.into()),
            ("position_ids", Tensor::from_array(([1usize, positions.len()], positions)).map_err(synth_err)?.into()),
            ("exaggeration", Tensor::from_array(([1usize], vec![self.exaggeration])).map_err(synth_err)?.into()),
        ]))?;
        let mut embedded = embed.run(embed_inputs).map_err(synth_err)?;
        let text_embeds = take_output(&mut embedded, &["inputs_embeds", "embeddings"])?;
        let text_shape = tensor_shape(&text_embeds)?;
        let cond_shape = tensor_shape(&self.conditioning.cond_emb)?;
        let text_values = tensor_f32(&text_embeds)?;
        let cond_values = tensor_f32(&self.conditioning.cond_emb)?;
        if text_shape.len() != 3 || cond_shape.len() != 3 || text_shape[0] != 1 || cond_shape[0] != 1 || text_shape[2] != cond_shape[2] {
            return Err(TtsError::Synthesis(format!("incompatible text and reference conditioning shapes {text_shape:?} and {cond_shape:?}")));
        }
        let width = text_shape[2];
        let mut prompt_embeds = Vec::with_capacity((text_shape[1] + cond_shape[1]) * width);
        prompt_embeds.extend_from_slice(cond_values);
        prompt_embeds.extend_from_slice(text_values);
        let prompt_len = text_shape[1] + cond_shape[1];
        drop(embedded);
        drop(embed);

        let mut lm = self.language_model.lock().map_err(|e| synth_err(e.to_string()))?;
        let mut inputs = vec![
            ("inputs_embeds".to_owned(), Tensor::from_array(([1usize, prompt_len, width], prompt_embeds)).map_err(synth_err)?.into()),
            ("attention_mask".to_owned(), Tensor::from_array(([1usize, prompt_len], vec![1i64; prompt_len])).map_err(synth_err)?.into()),
        ];
        let mut cache_names = Vec::with_capacity(LAYERS * 2);
        for layer in 0..LAYERS {
            for kind in ["key", "value"] {
                let name = format!("past_key_values.{layer}.{kind}");
                cache_names.push(name.clone());
                inputs.push((name, Tensor::from_array(([1usize, KV_HEADS, 0usize, HEAD_DIM], Vec::<f32>::new())).map_err(synth_err)?.into()));
            }
        }
        let first_inputs = bind_named_owned(&lm, inputs)?;
        let started = std::time::Instant::now();
        let first = lm.run(first_inputs).map_err(synth_err)?;
        tracing::debug!(prompt_len, ms = started.elapsed().as_millis() as u64, "Chatterbox prompt prefill");
        let (mut logits, mut past) = split_lm_outputs(first, &cache_names)?;
        let max_new_tokens = (60 + text.chars().count() * 4).min(HARD_MAX_TOKENS);
        let mut generated = vec![START_SPEECH_TOKEN];
        for step in 0..max_new_tokens {
            let token = greedy_with_repetition_penalty(&logits, &generated, 1.2)?;
            generated.push(token);
            if token == STOP_SPEECH_TOKEN { break; }
            if step + 1 == max_new_tokens { break; }
            let mut embed = self.embed_tokens.lock().map_err(|e| synth_err(e.to_string()))?;
            let one = bind_named(&embed, HashMap::from([
                ("input_ids", Tensor::from_array(([1usize, 1usize], vec![token])).map_err(synth_err)?.into()),
                ("position_ids", Tensor::from_array(([1usize, 1usize], vec![(step + 1) as i64])).map_err(synth_err)?.into()),
                ("exaggeration", Tensor::from_array(([1usize], vec![self.exaggeration])).map_err(synth_err)?.into()),
            ]))?;
            let mut output = embed.run(one).map_err(synth_err)?;
            let token_embeds = take_output(&mut output, &["inputs_embeds", "embeddings"])?;
            drop(output);
            drop(embed);
            let mut named = Vec::with_capacity(past.len() + 2);
            named.push(("inputs_embeds".to_owned(), (&token_embeds).into()));
            named.push(("attention_mask".to_owned(), Tensor::from_array(([1usize, prompt_len + step + 1], vec![1i64; prompt_len + step + 1])).map_err(synth_err)?.into()));
            for (name, value) in &past {
                named.push((name.clone(), value.into()));
            }
            let named = bind_named_owned(&lm, named)?;
            let output = lm.run(named).map_err(synth_err)?;
            let (new_logits, new_past) = split_lm_outputs(output, &cache_names)?;
            logits = new_logits;
            past = new_past;
            tracing::trace!(step, token, "Chatterbox speech token");
        }
        tracing::debug!(tokens = generated.len(), stopped = generated.last() == Some(&STOP_SPEECH_TOKEN), "Chatterbox generation finished");
        drop(lm);
        if generated.last() != Some(&STOP_SPEECH_TOKEN) { generated.push(STOP_SPEECH_TOKEN); }
        let speech_tokens = generated.get(1..generated.len().saturating_sub(1)).unwrap_or(&[]).to_vec();
        if speech_tokens.is_empty() { return Err(TtsError::Synthesis("Chatterbox generated no speech tokens".into())); }
        let mut speech_tokens = tensor_i64(&self.conditioning.prompt_token)?.to_vec();
        speech_tokens.extend(generated.get(1..generated.len().saturating_sub(1)).unwrap_or(&[]));
        let mut decoder = self.conditional_decoder.lock().map_err(|e| synth_err(e.to_string()))?;
        let decoder_inputs = bind_named(&decoder, HashMap::from([
            ("speech_tokens", Tensor::from_array(([1usize, speech_tokens.len()], speech_tokens)).map_err(synth_err)?.into()),
            ("speaker_embeddings", self.conditioning.speaker_embeddings.view().into()),
            ("speaker_features", self.conditioning.speaker_features.view().into()),
        ]))?;
        let started = std::time::Instant::now();
        let mut decoded = decoder.run(decoder_inputs).map_err(synth_err)?;
        tracing::debug!(ms = started.elapsed().as_millis() as u64, "Chatterbox decoder");
        let wav = take_output(&mut decoded, &["waveform", "wav", "audio"])?;
        let mut samples = tensor_f32(&wav)?.to_vec();
        trim_silence(&mut samples, SAMPLE_RATE);
        Ok(samples)
    }
}

impl TtsBackend for ChatterboxTts {
    fn name(&self) -> &'static str { "chatterbox" }
    fn sample_rate(&self) -> u32 { SAMPLE_RATE }
    /// `voice.language` selects the language tag (`pt` for PtBr, `en` for En); `voice.speed` is ignored.
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> { self.synthesize_inner(text, voice.language) }
}

fn make_session(environment: &ort::environment::Environment, path: &Path, threads: usize) -> Result<Session> {
    let started = std::time::Instant::now();
    // ort registers CoreML automatically on macOS; it splits these graphs into thousands of
    // partitions and session setup never finishes, so stay on CPU.
    let session = Session::builder(environment).map_err(load_err)?
        .with_auto_device(AutoDevicePolicy::PreferCPU).map_err(load_err)?
        .with_intra_threads(threads).map_err(load_err)?
        .with_inter_threads(1).map_err(load_err)?
        .with_memory_pattern(false).map_err(load_err)?
        .with_execution_providers([CPU::default().with_arena_allocator(false).build()]).map_err(load_err)?
        .commit_from_file(path).map_err(load_err)?;
    tracing::debug!(path = %path.display(), ms = started.elapsed().as_millis() as u64, "Chatterbox session loaded");
    Ok(session)
}

fn log_io(label: &str, session: &Session) {
    let inputs = session.inputs().iter().map(|v| format!("{}: {:?}", v.name(), v.dtype())).collect::<Vec<_>>();
    let outputs = session.outputs().iter().map(|v| format!("{}: {:?}", v.name(), v.dtype())).collect::<Vec<_>>();
    tracing::info!(?inputs, ?outputs, "{label} ONNX tensors");
}

fn bind_named<'a>(session: &Session, mut values: HashMap<&'static str, SessionInputValue<'a>>) -> Result<Vec<(String, SessionInputValue<'a>)>> {
    let mut out = Vec::with_capacity(session.inputs().len());
    for input in session.inputs() {
        let name = input.name();
        let value = values.remove(name).ok_or_else(|| TtsError::Synthesis(format!("missing ONNX input {name}")))?;
        out.push((name.to_owned(), value));
    }
    if let Some((name, _)) = values.into_iter().next() { return Err(TtsError::Synthesis(format!("unexpected ONNX input {name}"))); }
    Ok(out)
}

fn bind_named_owned<'a>(session: &Session, mut values: Vec<(String, SessionInputValue<'a>)>) -> Result<Vec<(String, SessionInputValue<'a>)>> {
    let mut out = Vec::with_capacity(session.inputs().len());
    for input in session.inputs() {
        let name = input.name();
        let index = values.iter().position(|(candidate, _)| candidate == name)
            .ok_or_else(|| TtsError::Synthesis(format!("missing ONNX input {name}")))?;
        out.push(values.swap_remove(index));
    }
    if !values.is_empty() { return Err(TtsError::Synthesis(format!("unexpected ONNX input {}", values[0].0))); }
    Ok(out)
}

fn take_output(outputs: &mut ort::session::SessionOutputs<'_>, candidates: &[&str]) -> Result<DynValue> {
    for name in candidates { if let Some(value) = outputs.remove(*name) { return Ok(value); } }
    let available = outputs.keys().map(str::to_owned).collect::<Vec<_>>();
    Err(TtsError::Load(format!("missing expected ONNX output {candidates:?}; got {available:?}")))
}

fn split_lm_outputs(mut outputs: ort::session::SessionOutputs<'_>, cache_names: &[String]) -> Result<(Vec<f32>, Vec<(String, DynValue)>)> {
    let logits = take_output(&mut outputs, &["logits"])?;
    let values = tensor_f32(&logits)?;
    let shape = tensor_shape(&logits)?;
    let vocab = *shape.last().ok_or_else(|| synth_err("language model logits have no dimensions"))?;
    let time = shape.get(shape.len().saturating_sub(2)).copied().unwrap_or(1);
    let start = values.len().checked_sub(vocab).ok_or_else(|| synth_err("empty language model logits"))?;
    let _ = time;
    let logits = values[start..].to_vec();
    let mut cache = Vec::with_capacity(cache_names.len());
    for name in cache_names {
        let output_name = format!("present.{}", name.strip_prefix("past_key_values.").unwrap_or(name));
        let value = take_output(&mut outputs, &[&output_name, name])?;
        cache.push((name.clone(), value));
    }
    Ok((logits, cache))
}

fn greedy_with_repetition_penalty(logits: &[f32], previous: &[i64], penalty: f32) -> Result<i64> {
    if logits.is_empty() { return Err(synth_err("empty language model logits")); }
    let mut best = 0usize;
    let mut best_score = f32::NEG_INFINITY;
    for (i, &score) in logits.iter().enumerate() {
        let adjusted = if previous.contains(&(i as i64)) { if score < 0.0 { score * penalty } else { score / penalty } } else { score };
        if adjusted > best_score { best = i; best_score = adjusted; }
    }
    Ok(best as i64)
}

fn tensor_shape(value: &DynValue) -> Result<Vec<usize>> {
    match value.dtype() {
        ValueType::Tensor { shape, .. } => Ok(shape.iter().map(|&d| d as usize).collect()),
        _ => Err(synth_err("expected ONNX tensor")),
    }
}
fn tensor_f32(value: &DynValue) -> Result<&[f32]> { value.try_extract_tensor::<f32>().map(|(_, v)| v).map_err(synth_err) }
fn tensor_i64(value: &DynValue) -> Result<&[i64]> { value.try_extract_tensor::<i64>().map(|(_, v)| v).map_err(synth_err) }

fn read_wav_mono_24k(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).map_err(|e| TtsError::Load(e.to_string()))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let mut samples = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.map_err(|e| TtsError::Load(e.to_string()))).collect::<Result<Vec<_>>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader.samples::<i16>().map(|s| s.map(|v| v as f32 / i16::MAX as f32).map_err(|e| TtsError::Load(e.to_string()))).collect::<Result<Vec<_>>>()?,
        hound::SampleFormat::Int => reader.samples::<i32>().map(|s| s.map(|v| v as f32 / i32::MAX as f32).map_err(|e| TtsError::Load(e.to_string()))).collect::<Result<Vec<_>>>()?,
    };
    if channels == 0 || samples.len() < channels { return Err(TtsError::Load("reference WAV has no audio samples".into())); }
    if channels > 1 { samples = samples.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect(); }
    if spec.sample_rate == SAMPLE_RATE { return Ok(samples); }
    let output_len = ((samples.len() as u64 * SAMPLE_RATE as u64) / spec.sample_rate as u64).max(1) as usize;
    let ratio = spec.sample_rate as f64 / SAMPLE_RATE as f64;
    Ok((0..output_len).map(|i| {
        let source = i as f64 * ratio;
        let left = (source.floor() as usize).min(samples.len() - 1);
        let right = (left + 1).min(samples.len() - 1);
        let frac = (source - left as f64) as f32;
        samples[left] * (1.0 - frac) + samples[right] * frac
    }).collect())
}

fn trim_silence(samples: &mut Vec<f32>, sample_rate: u32) {
    if samples.is_empty() { return; }
    let first = samples.iter().position(|s| s.abs() > 0.008).unwrap_or(0);
    let last = samples.iter().rposition(|s| s.abs() > 0.008).map(|i| i + 1).unwrap_or(samples.len());
    let pad = (sample_rate / 25) as usize;
    *samples = samples[first.saturating_sub(pad)..(last + pad).min(samples.len())].to_vec();
}

fn load_err(error: impl std::fmt::Display) -> TtsError { TtsError::Load(error.to_string()) }
fn synth_err(error: impl std::fmt::Display) -> TtsError { TtsError::Synthesis(error.to_string()) }

/// Helper for the server: are all Chatterbox files present in `dir`?
pub fn chatterbox_installed(dir: &Path) -> bool {
    ["tokenizer.json", "default_voice.wav"].iter().all(|f| dir.join(f).exists())
        && ["speech_encoder", "embed_tokens", "language_model_q4", "conditional_decoder"]
            .iter().all(|m| dir.join("onnx").join(format!("{m}.onnx")).exists() && dir.join("onnx").join(format!("{m}.onnx_data")).exists())
}
