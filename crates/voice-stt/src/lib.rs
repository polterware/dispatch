//! Speech-to-text using whisper.cpp, and streaming Silero or energy VAD.
//!
//! Silero v5 consumes a 512-sample frame prefixed by the previous frame's
//! 64-sample context (576 samples total), plus recurrent state `[2, 1, 128]`.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ort::{session::Session, value::Tensor};
use voice_core::protocol::Language;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("model file missing: {0}")]
    ModelMissing(PathBuf),
    #[error("model failed to load: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
}

pub type Result<T> = std::result::Result<T, SttError>;

#[derive(Debug, Clone)]
pub struct SttOptions {
    pub threads: u32,
    pub use_gpu: bool,
}
impl Default for SttOptions {
    fn default() -> Self {
        Self {
            threads: 4,
            use_gpu: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub language: Language,
    pub inference: Duration,
}

pub struct SttEngine {
    context: WhisperContext,
    state: Mutex<Option<WhisperState>>,
    threads: i32,
    bytes: u64,
}

impl SttEngine {
    pub fn load(model: &Path, options: SttOptions) -> Result<Self> {
        whisper_rs::install_logging_hooks();
        if !model.is_file() {
            return Err(SttError::ModelMissing(model.to_owned()));
        }
        let bytes = std::fs::metadata(model)
            .map_err(|e| SttError::Load(e.to_string()))?
            .len();
        let params = WhisperContextParameters {
            use_gpu: options.use_gpu,
            ..Default::default()
        };
        let context = WhisperContext::new_with_params(model.to_string_lossy().as_ref(), params)
            .map_err(|e| SttError::Load(e.to_string()))?;
        Ok(Self {
            context,
            state: Mutex::new(None),
            threads: options.threads.max(1).min(i32::MAX as u32) as i32,
            bytes,
        })
    }

    pub fn transcribe(&self, pcm: &[f32], language: Language) -> Result<Transcript> {
        let start = Instant::now();
        let silence = pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len().max(1) as f32;
        if pcm.len() < 4_800 || silence.sqrt() < 0.003 {
            return Ok(Transcript {
                text: String::new(),
                language,
                inference: start.elapsed(),
            });
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|e| SttError::Inference(e.to_string()))?;
        if guard.is_none() {
            *guard = Some(
                self.context
                    .create_state()
                    .map_err(|e| SttError::Inference(e.to_string()))?,
            );
        }
        let state = guard.as_mut().expect("state initialized");

        let detected = if language == Language::Auto {
            state
                .pcm_to_mel(pcm, self.threads)
                .map_err(|e| SttError::Inference(e.to_string()))?;
            let (_, probs) = state
                .lang_detect(0, self.threads as usize)
                .map_err(|e| SttError::Inference(e.to_string()))?;
            let pt_id = whisper_rs::get_lang_id("pt").unwrap_or(-1) as usize;
            let en_id = whisper_rs::get_lang_id("en").unwrap_or(-1) as usize;
            let code = match (probs.get(pt_id), probs.get(en_id)) {
                (Some(pt), Some(en)) if pt >= en => "pt",
                _ => "en",
            };
            code
        } else {
            language.whisper_code().unwrap_or("en")
        };

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(detected));
        params.set_n_threads(self.threads);
        params.set_no_timestamps(true);
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_temperature(0.0);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        state
            .full(params, pcm)
            .map_err(|e| SttError::Inference(e.to_string()))?;
        let text = (0..state.full_n_segments())
            .filter_map(|i| state.get_segment(i).map(|s| s.to_str_lossy().into_owned()))
            .collect::<String>()
            .trim()
            .to_owned();
        let lang = Language::from_whisper_code(detected);
        let text = if is_hallucination(&text) && (pcm.len() < 32_000 || silence.sqrt() < 0.015) {
            String::new()
        } else {
            text
        };
        Ok(Transcript {
            text,
            language: lang,
            inference: start.elapsed(),
        })
    }

    pub fn model_bytes(&self) -> u64 {
        self.bytes
    }
}

/// A deliberately short set of common Whisper music/credit fillers.
fn is_hallucination(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| matches!(c, '.' | '…' | '!' | '?')) {
        return true;
    }
    let normalized = trimmed
        .trim_matches(|c: char| c == '.' || c == '!' || c == '…')
        .trim()
        .to_lowercase();
    [
        "[música]",
        "[music]",
        "(música)",
        "legendas pela comunidade amara.org",
        "obrigado",
        "thank you",
        "you",
        "legendas por",
        "subtitles by",
    ]
    .iter()
    .any(|known| normalized == *known)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    SpeechStart,
    SpeechEnd,
}

#[derive(Debug, Clone)]
pub struct VadOptions {
    pub threshold: f32,
    pub min_silence: Duration,
    pub min_speech: Duration,
}
impl Default for VadOptions {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_silence: Duration::from_millis(700),
            min_speech: Duration::from_millis(200),
        }
    }
}

struct VadModelInner {
    session: Mutex<Session>,
}
/// Loaded reusable Silero model. Sessions are serialized because ONNX Runtime
/// requires mutable access; each utterance keeps its own recurrent state.
pub struct VadModel {
    inner: Arc<VadModelInner>,
}
impl VadModel {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(SttError::ModelMissing(path.to_owned()));
        }
        let session = Session::builder()
            .map_err(|e| SttError::Load(e.to_string()))?
            .with_intra_threads(1)
            .map_err(|e| SttError::Load(e.to_string()))?
            .with_inter_threads(1)
            .map_err(|e| SttError::Load(e.to_string()))?
            .commit_from_file(path)
            .map_err(|e| SttError::Load(e.to_string()))?;
        let inputs: Vec<_> = session.inputs.iter().map(|x| x.name.as_str()).collect();
        let outputs: Vec<_> = session.outputs.iter().map(|x| x.name.as_str()).collect();
        tracing::info!(?inputs, ?outputs, "Loaded Silero VAD ONNX model");
        if inputs != ["input", "state", "sr"] || outputs != ["output", "stateN"] {
            return Err(SttError::Load(format!(
                "unexpected Silero model tensors: inputs={inputs:?}, outputs={outputs:?}"
            )));
        }
        Ok(Self {
            inner: Arc::new(VadModelInner {
                session: Mutex::new(session),
            }),
        })
    }
}

enum VadBackend {
    Energy {
        noise_floor: f32,
    },
    Silero {
        model: Arc<VadModelInner>,
        state: Vec<f32>,
        context: [f32; 64],
    },
}
struct Hysteresis {
    options: VadOptions,
    speech: bool,
    positive: Duration,
    quiet: Duration,
}
impl Hysteresis {
    fn new(options: VadOptions) -> Self {
        Self {
            options,
            speech: false,
            positive: Duration::ZERO,
            quiet: Duration::ZERO,
        }
    }
    fn push(&mut self, probability: f32, duration: Duration) -> Option<VadEvent> {
        if !self.speech {
            self.positive = if probability >= self.options.threshold {
                self.positive + duration
            } else {
                Duration::ZERO
            };
            if self.positive >= self.options.min_speech {
                self.speech = true;
                self.quiet = Duration::ZERO;
                return Some(VadEvent::SpeechStart);
            }
        } else {
            self.quiet = if probability < self.options.threshold - 0.15 {
                self.quiet + duration
            } else {
                Duration::ZERO
            };
            if self.quiet >= self.options.min_silence {
                self.speech = false;
                self.positive = Duration::ZERO;
                return Some(VadEvent::SpeechEnd);
            }
        }
        None
    }
    fn reset(&mut self) {
        self.speech = false;
        self.positive = Duration::ZERO;
        self.quiet = Duration::ZERO;
    }
}

pub struct Vad {
    backend: VadBackend,
    core: Hysteresis,
    pending: Vec<f32>,
}
impl Vad {
    pub fn silero(model: &Path, options: VadOptions) -> Result<Self> {
        Self::from_model(&VadModel::load(model)?, options)
    }
    pub fn from_model(model: &VadModel, options: VadOptions) -> Result<Self> {
        Ok(Self {
            backend: VadBackend::Silero {
                model: model.inner.clone(),
                state: vec![0.0; 2 * 128],
                context: [0.0; 64],
            },
            core: Hysteresis::new(options),
            pending: Vec::with_capacity(512),
        })
    }
    pub fn energy(options: VadOptions) -> Self {
        Self {
            backend: VadBackend::Energy { noise_floor: 0.002 },
            core: Hysteresis::new(options),
            pending: Vec::new(),
        }
    }
    pub fn push(&mut self, chunk: &[f32]) -> Option<VadEvent> {
        let mut event = None;
        match &mut self.backend {
            VadBackend::Energy { noise_floor } => {
                let rms = rms(chunk);
                let threshold = (noise_floor * 3.0).max(0.008);
                if rms < *noise_floor {
                    *noise_floor += (rms - *noise_floor) * 0.08;
                } else {
                    *noise_floor += (rms - *noise_floor) * 0.002;
                }
                event = self.core.push(
                    if rms > threshold { 1.0 } else { 0.0 },
                    Duration::from_secs_f32(chunk.len() as f32 / 16_000.0),
                );
            }
            VadBackend::Silero {
                model,
                state,
                context,
            } => {
                self.pending.extend_from_slice(chunk);
                while self.pending.len() >= 512 {
                    let frame: Vec<f32> = self.pending.drain(..512).collect();
                    let mut input = Vec::with_capacity(576);
                    input.extend_from_slice(context);
                    input.extend_from_slice(&frame);
                    context.copy_from_slice(&frame[448..512]);
                    let mut session = match model.session.lock() {
                        Ok(s) => s,
                        Err(_) => return event,
                    };
                    let input_t = match Tensor::from_array(([1usize, 576], input)) {
                        Ok(v) => v,
                        Err(_) => return event,
                    };
                    let state_t = match Tensor::from_array(([2usize, 1, 128], state.clone())) {
                        Ok(v) => v,
                        Err(_) => return event,
                    };
                    let sr_t = match Tensor::from_array(([1usize], vec![16_000i64])) {
                        Ok(v) => v,
                        Err(_) => return event,
                    };
                    let outputs = match session
                        .run(ort::inputs!["input" => input_t, "state" => state_t, "sr" => sr_t])
                    {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(error=%e, "Silero VAD inference failed");
                            return event;
                        }
                    };
                    if let Ok((_, prob)) = outputs["output"].try_extract_tensor::<f32>() {
                        if let Some(p) = prob.first() {
                            event = self.core.push(*p, Duration::from_millis(32)).or(event);
                        }
                    }
                    if let Ok((_, next)) = outputs["stateN"].try_extract_tensor::<f32>() {
                        state.copy_from_slice(next);
                    }
                }
            }
        }
        event
    }
    pub fn in_speech(&self) -> bool {
        self.core.speech
    }
    pub fn reset(&mut self) {
        self.core.reset();
        self.pending.clear();
        if let VadBackend::Silero { state, context, .. } = &mut self.backend {
            state.fill(0.0);
            context.fill(0.0);
        }
        if let VadBackend::Energy { noise_floor } = &mut self.backend {
            *noise_floor = 0.002;
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hallucination_filter_matches_known_fillers() {
        for phrase in [
            "[Música]",
            "[Music]",
            "(música)",
            "Legendas pela comunidade Amara.org",
            "Obrigado.",
            "Thank you.",
            "you",
            "...",
        ] {
            assert!(is_hallucination(phrase), "{phrase}");
        }
        assert!(!is_hallucination("Crie um arquivo teste."));
    }
    #[test]
    fn hysteresis_requires_sustained_speech_and_silence() {
        let mut core = Hysteresis::new(VadOptions::default());
        for _ in 0..6 {
            assert_eq!(core.push(0.8, Duration::from_millis(32)), None);
        }
        assert_eq!(
            core.push(0.8, Duration::from_millis(32)),
            Some(VadEvent::SpeechStart)
        );
        assert!(core.speech);
        for _ in 0..21 {
            assert_eq!(core.push(0.1, Duration::from_millis(32)), None);
        }
        assert_eq!(
            core.push(0.1, Duration::from_millis(32)),
            Some(VadEvent::SpeechEnd)
        );
    }
    #[test]
    fn energy_vad_detects_synthetic_burst() {
        let mut vad = Vad::energy(VadOptions {
            min_speech: Duration::from_millis(64),
            min_silence: Duration::from_millis(96),
            ..Default::default()
        });
        let quiet = vec![0.001; 512];
        let loud = vec![0.06; 512];
        assert_eq!(vad.push(&quiet), None);
        assert_eq!(vad.push(&loud), None);
        assert_eq!(vad.push(&loud), Some(VadEvent::SpeechStart));
        assert_eq!(vad.push(&quiet), None);
        assert_eq!(vad.push(&quiet), None);
        assert_eq!(vad.push(&quiet), Some(VadEvent::SpeechEnd));
    }
    #[test]
    #[ignore = "requires local whisper model"]
    fn load_whisper_model() {
        let path = PathBuf::from(
            std::env::var("VOICE_TEST_WHISPER_MODEL").expect("VOICE_TEST_WHISPER_MODEL"),
        );
        let model = SttEngine::load(&path, SttOptions::default()).unwrap();
        assert!(model.model_bytes() > 0);
    }
    #[test]
    #[ignore = "requires local Silero ONNX model"]
    fn load_vad_model() {
        let path =
            PathBuf::from(std::env::var("VOICE_TEST_VAD_MODEL").expect("VOICE_TEST_VAD_MODEL"));
        VadModel::load(&path).unwrap();
    }
}
