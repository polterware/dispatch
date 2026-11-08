//! Chatterbox Multilingual (Resemble AI, MIT) through ONNX Runtime: the most natural voice
//! the engine offers, including Brazilian Portuguese, at a much higher cost than Piper.
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

use std::path::PathBuf;

use crate::{Result, TtsBackend, VoiceSpec};

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

pub struct ChatterboxTts {
    _private: (),
}

impl ChatterboxTts {
    pub fn load(options: ChatterboxOptions) -> Result<Self> {
        let _ = options;
        unimplemented!("voice-tts: ChatterboxTts::load")
    }
}

impl TtsBackend for ChatterboxTts {
    fn name(&self) -> &'static str {
        "chatterbox"
    }
    fn sample_rate(&self) -> u32 {
        24_000
    }
    /// `voice.language` selects the language tag (`pt` for PtBr, `en` for En);
    /// `voice.speed` is not supported by the model and is ignored.
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        let _ = (text, voice);
        unimplemented!("voice-tts: ChatterboxTts::synthesize")
    }
}

/// Helper for the server: are all Chatterbox files present in `dir`?
pub fn chatterbox_installed(dir: &std::path::Path) -> bool {
    ["tokenizer.json", "default_voice.wav"].iter().all(|f| dir.join(f).exists())
        && ["speech_encoder", "embed_tokens", "language_model_q4", "conditional_decoder"]
            .iter()
            .all(|m| dir.join("onnx").join(format!("{m}.onnx")).exists() && dir.join("onnx").join(format!("{m}.onnx_data")).exists())
}
