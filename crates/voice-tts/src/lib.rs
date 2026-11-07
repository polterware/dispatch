//! Text-to-speech: Kokoro-82M through ONNX Runtime (primary) and the macOS
//! system voice through `say` (zero-download fallback), plus the text
//! preparation every backend shares.
//!
//! CONTRACT (owned by the architect; keep these signatures):
//!
//! - Synthesis is synchronous and CPU-bound; the server runs it on
//!   `spawn_blocking`, one sentence at a time, and starts playback as soon as
//!   the first sentence is ready (time to first audio is the metric).
//! - Output is mono `f32` at `TtsBackend::sample_rate()` (Kokoro: 24 kHz).
//! - Kokoro phonemizes with the `espeak-ng` executable (GPL-3.0), called as a
//!   separate process (`espeak-ng -q --ipa=3 -v <voice>` style) so the engine
//!   never links GPL code. Its path comes from `KokoroOptions::espeak` or PATH
//!   (also try /opt/homebrew/bin and /usr/local/bin, since a GUI-spawned
//!   process has a minimal PATH).
//! - Loading never downloads anything. A missing file is `TtsError::ModelMissing`.

use std::path::{Path, PathBuf};

use voice_core::protocol::{Language, TextFormat};

pub mod text {
    //! Turning a coding agent's Markdown into something worth hearing.
    use super::*;

    /// Maximum characters spoken for one response before it is cut with a
    /// short notice (in the response's language).
    pub const MAX_SPOKEN_CHARS: usize = 1200;

    /// Prepares text for speech.
    ///
    /// For `Markdown`: drops fenced code blocks, indented code, diffs, tables,
    /// HTML, and images, replacing each dropped block with one short phrase
    /// ("code omitted" / "código omitido"); keeps link text and drops URLs;
    /// shortens file paths to their file name; strips emphasis, headings, and
    /// list markers; reads inline code only when short (≤ 24 chars, else drops
    /// it); collapses whitespace; and cuts at `MAX_SPOKEN_CHARS` on a sentence
    /// boundary. `Plain` only collapses whitespace and applies the cap.
    pub fn speakable(input: &str, format: TextFormat, language: Language) -> String {
        let _ = (input, format, language);
        unimplemented!("voice-tts: text::speakable")
    }

    /// Splits prepared text into sentences for incremental synthesis. The
    /// first piece should be short (cut a long first sentence at a comma
    /// after ~60 chars) so audio starts early; no piece exceeds ~300 chars.
    pub fn sentences(text: &str) -> Vec<String> {
        let _ = text;
        unimplemented!("voice-tts: text::sentences")
    }

    /// Guesses pt-BR vs English for text whose language is `Auto` (stopword
    /// and diacritic counts are enough).
    pub fn detect_language(text: &str) -> Language {
        let _ = text;
        unimplemented!("voice-tts: text::detect_language")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("model file missing: {0}")]
    ModelMissing(PathBuf),
    #[error("espeak-ng not found; install it (brew install espeak-ng)")]
    PhonemizerMissing,
    #[error("model failed to load: {0}")]
    Load(String),
    #[error("synthesis failed: {0}")]
    Synthesis(String),
}

pub type Result<T> = std::result::Result<T, TtsError>;

/// A voice choice. `name` is backend-specific (`pf_dora`, `Luciana`); `None`
/// takes the backend's default voice for `language`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceSpec {
    pub language: Language,
    pub name: Option<String>,
    /// 1.0 is normal speed.
    pub speed: f32,
}

pub trait TtsBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn sample_rate(&self) -> u32;
    /// Synthesizes one sentence of prepared text.
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>>;
}

#[derive(Debug, Clone)]
pub struct KokoroOptions {
    /// `models/kokoro`: holds the `.onnx` model, `tokenizer.json`, and `voices/<name>.bin`.
    pub dir: PathBuf,
    /// Model file inside `dir`. Default `model_quantized.onnx`.
    pub model_file: String,
    /// Explicit `espeak-ng` path; `None` searches.
    pub espeak: Option<PathBuf>,
    /// Intra-op threads. Default 4.
    pub threads: usize,
}

/// Default voices: pt-BR `pf_dora`, English `af_heart`.
pub struct KokoroTts {
    _private: (),
}

impl KokoroTts {
    pub fn load(options: KokoroOptions) -> Result<Self> {
        let _ = options;
        unimplemented!("voice-tts: KokoroTts::load")
    }
}

impl TtsBackend for KokoroTts {
    fn name(&self) -> &'static str {
        "kokoro"
    }
    fn sample_rate(&self) -> u32 {
        24_000
    }
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        let _ = (text, voice);
        unimplemented!("voice-tts: KokoroTts::synthesize")
    }
}

/// macOS `say` writing 16-bit PCM WAV to a temp file at 22.05 kHz, read back
/// and deleted. Default voices: pt-BR `Luciana`, English `Samantha`.
/// `available()` is false off macOS.
pub struct SystemTts {
    _private: (),
}

impl SystemTts {
    pub fn new() -> Self {
        unimplemented!("voice-tts: SystemTts::new")
    }

    pub fn available() -> bool {
        cfg!(target_os = "macos")
    }
}

impl Default for SystemTts {
    fn default() -> Self {
        Self::new()
    }
}

impl TtsBackend for SystemTts {
    fn name(&self) -> &'static str {
        "system"
    }
    fn sample_rate(&self) -> u32 {
        22_050
    }
    fn synthesize(&self, text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
        let _ = (text, voice);
        unimplemented!("voice-tts: SystemTts::synthesize")
    }
}

/// Helper used by the server: is a Kokoro model present in `dir`?
pub fn kokoro_installed(dir: &Path) -> bool {
    dir.join("tokenizer.json").exists() && dir.join("voices").is_dir()
}
