//! Model catalog, voices, and engine settings (protocol v1 additions).
//!
//! HTTP (all authenticated, see docs/protocol.md):
//! - `GET /v1/catalog` -> [`Catalog`]
//! - `POST /v1/models/{id}/download` -> 202; progress arrives on every WebSocket as
//!   `model.progress`, then `model.installed` or `model.failed`.
//! - `DELETE /v1/models/{id}` -> 204, or 409 when the model is in use by the settings.
//! - `GET /v1/settings` -> [`EngineSettings`]
//! - `PATCH /v1/settings` with [`SettingsPatch`] -> the new [`EngineSettings`]; persisted to
//!   `config.json` and broadcast as `settings.changed`. Changing a model or voice takes
//!   effect on the next use (the old model unloads).

use serde::{Deserialize, Serialize};

use crate::protocol::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelKind {
    Stt,
    Tts,
    Vad,
}

/// One downloadable model. Ids are stable, e.g. `whisper-small-q5_1`, `kokoro-v1`,
/// `piper-pt_BR-faber-medium`, `silero-vad-v5`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub kind: ModelKind,
    /// `whisper`, `kokoro`, `piper`, or `silero`.
    pub backend: String,
    pub name: String,
    pub description: String,
    pub languages: Vec<Language>,
    /// Total download size of every file of the model.
    pub size_bytes: u64,
    pub license: String,
    pub installed: bool,
    /// The current settings use it (deleting it is refused).
    pub active: bool,
}

/// A voice the engine can speak with. Ids: `piper:pt_BR-faber-medium`,
/// `kokoro:pf_dora`, `system:Luciana`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceEntry {
    pub id: String,
    pub backend: String,
    pub name: String,
    pub language: Language,
    /// The catalog model it needs, if any (`None` for system voices).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub models: Vec<CatalogEntry>,
    pub voices: Vec<VoiceEntry>,
}

/// Settings every client shares, stored in the engine's `config.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSettings {
    /// The language users speak and hear; `auto` detects per utterance.
    pub language: Language,
    /// Catalog id of the STT model.
    pub stt_model: String,
    /// Voice id for Brazilian Portuguese.
    pub voice_pt: String,
    /// Voice id for English.
    pub voice_en: String,
    /// 0.5..=2.0, 1.0 is normal.
    pub speed: f32,
    pub partials: bool,
    pub model_idle_secs: u64,
    pub exit_after_idle_secs: u64,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            language: Language::PtBr,
            stt_model: "whisper-small-q5_1".into(),
            voice_pt: "piper:pt_BR-faber-medium".into(),
            voice_en: "kokoro:af_heart".into(),
            speed: 1.0,
            partials: true,
            model_idle_secs: 600,
            exit_after_idle_secs: 1800,
        }
    }
}

/// A partial update; absent fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_pt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_en: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partials: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_idle_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_after_idle_secs: Option<u64>,
}

impl EngineSettings {
    pub fn apply(&mut self, patch: SettingsPatch) {
        if let Some(v) = patch.language {
            self.language = v;
        }
        if let Some(v) = patch.stt_model {
            self.stt_model = v;
        }
        if let Some(v) = patch.voice_pt {
            self.voice_pt = v;
        }
        if let Some(v) = patch.voice_en {
            self.voice_en = v;
        }
        if let Some(v) = patch.speed {
            self.speed = v.clamp(0.5, 2.0);
        }
        if let Some(v) = patch.partials {
            self.partials = v;
        }
        if let Some(v) = patch.model_idle_secs {
            self.model_idle_secs = v;
        }
        if let Some(v) = patch.exit_after_idle_secs {
            self.exit_after_idle_secs = v;
        }
    }
}
