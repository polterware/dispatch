//! Piper (VITS) voices through ONNX Runtime: Brazilian Portuguese voices trained on
//! Brazilian speakers (`pt_BR-faber-medium`, `pt_BR-cadu-medium`, `pt_BR-jeff-medium`).
//!
//! CONTRACT (owned by the architect; keep these signatures):
//! - A voice is a pair of files: `<name>.onnx` and `<name>.onnx.json` (Piper's config:
//!   `audio.sample_rate`, `espeak.voice`, `phoneme_id_map`, `inference` noise/length scales).
//! - Phonemes come from the `espeak-ng` executable as a separate process (same rules as
//!   Kokoro: GPL stays out of the binary), using the config's `espeak.voice`.
//! - Output is mono f32 at the config's sample rate (22050 Hz for medium voices).

use std::path::PathBuf;

use crate::{Result, TtsBackend, VoiceSpec};

#[derive(Debug, Clone)]
pub struct PiperOptions {
    /// Path of `<name>.onnx`; the config is the same path with `.json` appended.
    pub model: PathBuf,
    /// Explicit `espeak-ng` path; `None` searches like Kokoro does.
    pub espeak: Option<PathBuf>,
    /// Intra-op threads. Default 4.
    pub threads: usize,
}

pub struct PiperTts {
    _private: (),
}

impl PiperTts {
    pub fn load(options: PiperOptions) -> Result<Self> {
        let _ = options;
        unimplemented!("voice-tts: PiperTts::load")
    }
}

impl TtsBackend for PiperTts {
    fn name(&self) -> &'static str {
        "piper"
    }
    fn sample_rate(&self) -> u32 {
        unimplemented!("voice-tts: PiperTts::sample_rate")
    }
    /// `voice.speed` maps to Piper's length scale (1/speed); `voice.name` is ignored
    /// (a PiperTts is one voice).
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        let _ = (text, voice);
        unimplemented!("voice-tts: PiperTts::synthesize")
    }
}
