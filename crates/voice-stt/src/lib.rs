//! Speech-to-text (whisper.cpp through whisper-rs, Metal on Apple Silicon) and
//! voice activity detection (Silero VAD through ONNX Runtime, with an energy
//! fallback).
//!
//! CONTRACT (owned by the architect; keep these signatures):
//!
//! - All calls are synchronous and CPU/GPU-bound: the server runs them on
//!   `spawn_blocking`. Types are `Send + Sync` where noted so one loaded model
//!   serves every client.
//! - Input audio is mono `f32` at 16 kHz (`voice_audio::CAPTURE_RATE`).
//! - Loading never downloads anything. A missing file is `SttError::ModelMissing`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use voice_core::protocol::Language;

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
    /// Threads for the CPU parts of inference. Default: 4 (M1 performance cores).
    pub threads: u32,
    /// Use the GPU (Metal) when the build supports it. Default: true.
    pub use_gpu: bool,
}

impl Default for SttOptions {
    fn default() -> Self {
        Self { threads: 4, use_gpu: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    /// Trimmed text; empty when nothing was said. Whisper's non-speech
    /// hallucinations ("[Música]", "Legendas pela comunidade...", "Thank you.")
    /// on near-silent input are filtered to empty.
    pub text: String,
    /// The detected (or forced) language.
    pub language: Language,
    pub inference: Duration,
}

/// A loaded whisper model. `Send + Sync`; concurrent `transcribe` calls are
/// serialized internally (one whisper state at a time is enough for one mic).
pub struct SttEngine {
    _private: (),
}

impl SttEngine {
    pub fn load(model: &Path, options: SttOptions) -> Result<Self> {
        let _ = (model, options);
        unimplemented!("voice-stt: SttEngine::load")
    }

    /// Transcribes a whole utterance. `language: Auto` detects it, restricted
    /// to the supported languages (pt or en) when detection is ambiguous.
    pub fn transcribe(&self, pcm: &[f32], language: Language) -> Result<Transcript> {
        let _ = (pcm, language);
        unimplemented!("voice-stt: SttEngine::transcribe")
    }

    /// Model file size in bytes (for status and benchmarks).
    pub fn model_bytes(&self) -> u64 {
        unimplemented!("voice-stt: SttEngine::model_bytes")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEvent {
    SpeechStart,
    /// Silence lasted `VadOptions::min_silence` after speech.
    SpeechEnd,
}

#[derive(Debug, Clone)]
pub struct VadOptions {
    /// Speech probability threshold. Default 0.5.
    pub threshold: f32,
    /// Silence that ends an utterance. Default 700 ms.
    pub min_silence: Duration,
    /// Speech shorter than this is ignored. Default 200 ms.
    pub min_speech: Duration,
}

impl Default for VadOptions {
    fn default() -> Self {
        Self { threshold: 0.5, min_silence: Duration::from_millis(700), min_speech: Duration::from_millis(200) }
    }
}

/// A streaming VAD for one utterance. Not shared: create one per listen (cheap
/// once the model is loaded; the ONNX session itself may be shared inside).
pub struct Vad {
    _private: (),
}

impl Vad {
    /// Silero VAD from an ONNX file.
    pub fn silero(model: &Path, options: VadOptions) -> Result<Self> {
        let _ = (model, options);
        unimplemented!("voice-stt: Vad::silero")
    }

    /// RMS-energy VAD with an adaptive noise floor, no model needed.
    pub fn energy(options: VadOptions) -> Self {
        let _ = options;
        unimplemented!("voice-stt: Vad::energy")
    }

    /// Feeds one chunk of `voice_audio::CAPTURE_CHUNK` (512) samples.
    pub fn push(&mut self, chunk: &[f32]) -> Option<VadEvent> {
        let _ = chunk;
        unimplemented!("voice-stt: Vad::push")
    }

    /// True between `SpeechStart` and `SpeechEnd`.
    pub fn in_speech(&self) -> bool {
        unimplemented!("voice-stt: Vad::in_speech")
    }

    pub fn reset(&mut self) {
        unimplemented!("voice-stt: Vad::reset")
    }
}
