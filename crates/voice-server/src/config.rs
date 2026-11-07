use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}
impl Default for Config {
    fn default() -> Self {
        Self {
            stt_model: "ggml-small-q5_1.bin".into(),
            tts_model_file: "model_quantized.onnx".into(),
            voice_pt: "pf_dora".into(),
            voice_en: "af_heart".into(),
            speed: 1.0,
            model_idle_secs: 600,
            exit_after_idle_secs: 1800,
            input_device: None,
            output_device: None,
            max_utterance_secs: 60,
            partials: true,
        }
    }
}
pub fn load() -> Config {
    std::fs::read(voice_core::paths::config_file())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}
pub fn select_stt(config: &Config, root: &std::path::Path) -> PathBuf {
    let desired = root.join("whisper").join(&config.stt_model);
    if desired.is_file() {
        return desired;
    }
    for name in [
        "ggml-small-q5_1.bin",
        "ggml-base.bin",
        "ggml-large-v3-turbo-q5_0.bin",
    ] {
        let candidate = root.join("whisper").join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    desired
}
