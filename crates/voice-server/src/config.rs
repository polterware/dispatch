use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use voice_core::protocol::Language;

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub stt_model: String,
    pub tts_model_file: String,
    pub voice_pt: String,
    pub voice_en: String,
    pub speed: f32,
    pub model_idle_secs: u64,
    pub exit_after_idle_secs: u64,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub max_utterance_secs: u64,
    pub partials: bool,
    pub language: Language,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            stt_model: "ggml-small-q5_1.bin".into(),
            tts_model_file: "model_quantized.onnx".into(),
            voice_pt: "piper:pt_BR-faber-medium".into(),
            voice_en: "kokoro:af_heart".into(),
            speed: 1.0,
            model_idle_secs: 600,
            exit_after_idle_secs: 1800,
            input_device: None,
            output_device: None,
            max_utterance_secs: 60,
            partials: true,
            language: Language::PtBr,
        }
    }
}
pub fn load() -> Config {
    let mut config: Config = std::fs::read(voice_core::paths::config_file())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    let settings = crate::settings::load();
    config.apply_settings(&settings);
    config
}
impl Config {
    pub fn apply_settings(&mut self, settings: &voice_core::catalog::EngineSettings) {
        self.stt_model = stt_filename(&settings.stt_model)
            .unwrap_or(&settings.stt_model)
            .into();
        self.voice_pt = settings.voice_pt.clone();
        self.voice_en = settings.voice_en.clone();
        self.speed = settings.speed;
        self.partials = settings.partials;
        self.model_idle_secs = settings.model_idle_secs;
        self.exit_after_idle_secs = settings.exit_after_idle_secs;
        self.language = settings.language;
    }
}
pub fn stt_filename(id: &str) -> Option<&'static str> {
    match id {
        "whisper-base" => Some("ggml-base.bin"),
        "whisper-small-q5_1" => Some("ggml-small-q5_1.bin"),
        "whisper-large-v3-turbo-q5_0" => Some("ggml-large-v3-turbo-q5_0.bin"),
        _ => None,
    }
}
pub fn select_stt(config: &Config, root: &std::path::Path) -> PathBuf {
    root.join("whisper").join(&config.stt_model)
}
