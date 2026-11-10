use std::path::{Path, PathBuf};
use voice_core::{
    catalog::{Catalog, CatalogEntry, ModelKind, VoiceEntry},
    protocol::Language,
};

#[derive(Clone, Copy)]
pub struct FileSpec {
    pub url: &'static str,
    pub path: &'static str,
    pub size: u64,
}
#[derive(Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub kind: ModelKind,
    pub backend: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub languages: &'static [Language],
    pub size: u64,
    pub license: &'static str,
    pub files: &'static [FileSpec],
}

const PT_EN: &[Language] = &[Language::PtBr, Language::En];
const PT: &[Language] = &[Language::PtBr];
const STT_BASE: &[FileSpec] = &[FileSpec {
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    path: "whisper/ggml-base.bin",
    size: 147_951_465,
}];
const STT_SMALL: &[FileSpec] = &[FileSpec {
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
    path: "whisper/ggml-small-q5_1.bin",
    size: 190_085_487,
}];
const STT_TURBO: &[FileSpec] = &[FileSpec {
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    path: "whisper/ggml-large-v3-turbo-q5_0.bin",
    size: 574_000_000,
}];
const PIPER_FABER: &[FileSpec] = &[FileSpec { url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/faber/medium/pt_BR-faber-medium.onnx", path: "piper/pt_BR-faber-medium.onnx", size: 63_000_000 }, FileSpec { url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/faber/medium/pt_BR-faber-medium.onnx.json", path: "piper/pt_BR-faber-medium.onnx.json", size: 0 }];
const PIPER_CADU: &[FileSpec] = &[FileSpec { url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/cadu/medium/pt_BR-cadu-medium.onnx", path: "piper/pt_BR-cadu-medium.onnx", size: 63_000_000 }, FileSpec { url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/cadu/medium/pt_BR-cadu-medium.onnx.json", path: "piper/pt_BR-cadu-medium.onnx.json", size: 0 }];
const PIPER_JEFF: &[FileSpec] = &[FileSpec { url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/jeff/medium/pt_BR-jeff-medium.onnx", path: "piper/pt_BR-jeff-medium.onnx", size: 63_000_000 }, FileSpec { url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/jeff/medium/pt_BR-jeff-medium.onnx.json", path: "piper/pt_BR-jeff-medium.onnx.json", size: 0 }];
const KOKORO: &[FileSpec] = &[
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/onnx/model_quantized.onnx", path: "kokoro/model_quantized.onnx", size: 92_000_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/tokenizer.json", path: "kokoro/tokenizer.json", size: 1_000_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/config.json", path: "kokoro/config.json", size: 100_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/af_heart.bin", path: "kokoro/voices/af_heart.bin", size: 1_000_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/am_michael.bin", path: "kokoro/voices/am_michael.bin", size: 1_000_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/pf_dora.bin", path: "kokoro/voices/pf_dora.bin", size: 1_000_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/pm_alex.bin", path: "kokoro/voices/pm_alex.bin", size: 1_000_000 },
 FileSpec { url: "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/voices/pm_santa.bin", path: "kokoro/voices/pm_santa.bin", size: 1_000_000 },
];
const CHATTERBOX: &[FileSpec] = &[
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/tokenizer.json", path: "chatterbox/tokenizer.json", size: 0 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/tokenizer_config.json", path: "chatterbox/tokenizer_config.json", size: 0 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/generation_config.json", path: "chatterbox/generation_config.json", size: 0 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/default_voice.wav", path: "chatterbox/default_voice.wav", size: 0 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/speech_encoder.onnx", path: "chatterbox/onnx/speech_encoder.onnx", size: 1_184_608 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/speech_encoder.onnx_data", path: "chatterbox/onnx/speech_encoder.onnx_data", size: 591_274_880 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/embed_tokens.onnx", path: "chatterbox/onnx/embed_tokens.onnx", size: 13_286 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/embed_tokens.onnx_data", path: "chatterbox/onnx/embed_tokens.onnx_data", size: 68_390_912 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/language_model_q4.onnx", path: "chatterbox/onnx/language_model_q4.onnx", size: 227_911 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/language_model_q4.onnx_data", path: "chatterbox/onnx/language_model_q4.onnx_data", size: 353_621_248 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/conditional_decoder.onnx", path: "chatterbox/onnx/conditional_decoder.onnx", size: 6_350_448 },
    FileSpec { url: "https://huggingface.co/onnx-community/chatterbox-multilingual-ONNX/resolve/main/onnx/conditional_decoder.onnx_data", path: "chatterbox/onnx/conditional_decoder.onnx_data", size: 533_970_816 },
];
const VAD: &[FileSpec] = &[FileSpec { url: "https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx", path: "vad/silero_vad.onnx", size: 2_000_000 }];
pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "whisper-base",
        kind: ModelKind::Stt,
        backend: "whisper",
        name: "Whisper Base",
        description: "Speech recognition in Brazilian Portuguese and English",
        languages: PT_EN,
        size: 147_951_465,
        license: "MIT",
        files: STT_BASE,
    },
    ModelSpec {
        id: "whisper-small-q5_1",
        kind: ModelKind::Stt,
        backend: "whisper",
        name: "Whisper Small Q5_1",
        description: "Speech recognition in Brazilian Portuguese and English",
        languages: PT_EN,
        size: 190_085_487,
        license: "MIT",
        files: STT_SMALL,
    },
    ModelSpec {
        id: "whisper-large-v3-turbo-q5_0",
        kind: ModelKind::Stt,
        backend: "whisper",
        name: "Whisper Large V3 Turbo Q5_0",
        description: "Speech recognition in Brazilian Portuguese and English",
        languages: PT_EN,
        size: 574_000_000,
        license: "MIT",
        files: STT_TURBO,
    },
    ModelSpec {
        id: "piper-pt_BR-faber-medium",
        kind: ModelKind::Tts,
        backend: "piper",
        name: "Piper Faber Medium",
        description: "Brazilian Portuguese voice",
        languages: PT,
        size: 63_000_000,
        license: "CC0 dataset; Piper MIT",
        files: PIPER_FABER,
    },
    ModelSpec {
        id: "piper-pt_BR-cadu-medium",
        kind: ModelKind::Tts,
        backend: "piper",
        name: "Piper Cadu Medium",
        description: "Brazilian Portuguese voice",
        languages: PT,
        size: 63_000_000,
        license: "CC0 dataset; Piper MIT",
        files: PIPER_CADU,
    },
    ModelSpec {
        id: "piper-pt_BR-jeff-medium",
        kind: ModelKind::Tts,
        backend: "piper",
        name: "Piper Jeff Medium",
        description: "Brazilian Portuguese voice",
        languages: PT,
        size: 63_000_000,
        license: "CC0 dataset; Piper MIT",
        files: PIPER_JEFF,
    },
    ModelSpec {
        id: "kokoro-v1",
        kind: ModelKind::Tts,
        backend: "kokoro",
        name: "Kokoro v1",
        description: "Multilingual text-to-speech voices",
        languages: PT_EN,
        size: 96_000_000,
        license: "Apache-2.0",
        files: KOKORO,
    },
    ModelSpec {
        id: "chatterbox-multilingual",
        kind: ModelKind::Tts,
        backend: "chatterbox",
        name: "Chatterbox Multilingual (most natural, large)",
        description: "The most natural multilingual voice; about a 1.6 GB download with higher latency and RAM use",
        languages: PT_EN,
        size: 1_555_034_109,
        license: "MIT",
        files: CHATTERBOX,
    },
    ModelSpec {
        id: "silero-vad-v5",
        kind: ModelKind::Vad,
        backend: "silero",
        name: "Silero VAD v5",
        description: "Voice activity detection",
        languages: PT_EN,
        size: 2_000_000,
        license: "MIT",
        files: VAD,
    },
];
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|m| m.id == id)
}
pub fn is_active(model: &ModelSpec, settings: &voice_core::catalog::EngineSettings) -> bool {
    match model.kind {
        ModelKind::Stt => settings.stt_model == model.id,
        ModelKind::Tts => [settings.voice_pt.as_str(), settings.voice_en.as_str()]
            .iter()
            .any(|v| {
                v.strip_prefix("piper:")
                    .is_some_and(|id| model.id == format!("piper-{id}"))
                    || (v.starts_with("kokoro:") && model.id == "kokoro-v1")
                    || (v.starts_with("chatterbox:") && model.id == "chatterbox-multilingual")
            }),
        ModelKind::Vad => false,
    }
}
pub fn files_installed(spec: &ModelSpec, root: &Path) -> bool {
    spec.files.iter().all(|f| root.join(f.path).is_file())
}
pub fn file_paths(spec: &ModelSpec, root: &Path) -> Vec<PathBuf> {
    spec.files.iter().map(|f| root.join(f.path)).collect()
}
pub fn catalog(root: &Path, settings: &voice_core::catalog::EngineSettings) -> Catalog {
    catalog_with_vad(root, settings, false)
}
pub fn catalog_with_vad(
    root: &Path,
    settings: &voice_core::catalog::EngineSettings,
    vad_in_use: bool,
) -> Catalog {
    let models = MODELS
        .iter()
        .map(|m| CatalogEntry {
            id: m.id.into(),
            kind: m.kind,
            backend: m.backend.into(),
            name: m.name.into(),
            description: m.description.into(),
            languages: m.languages.to_vec(),
            size_bytes: m.size,
            license: m.license.into(),
            installed: if m.id == "chatterbox-multilingual" {
                voice_tts::chatterbox_installed(&root.join("chatterbox"))
            } else {
                files_installed(m, root)
            },
            active: is_active(m, settings) || (m.kind == ModelKind::Vad && vad_in_use),
        })
        .collect();
    let mut voices = vec![
        voice(
            "piper:pt_BR-faber-medium",
            "piper",
            "Faber (Piper)",
            Language::PtBr,
            Some("piper-pt_BR-faber-medium"),
            root,
        ),
        voice(
            "piper:pt_BR-cadu-medium",
            "piper",
            "Cadu (Piper)",
            Language::PtBr,
            Some("piper-pt_BR-cadu-medium"),
            root,
        ),
        voice(
            "piper:pt_BR-jeff-medium",
            "piper",
            "Jeff (Piper)",
            Language::PtBr,
            Some("piper-pt_BR-jeff-medium"),
            root,
        ),
        voice(
            "kokoro:pf_dora",
            "kokoro",
            "Dora (Kokoro)",
            Language::PtBr,
            Some("kokoro-v1"),
            root,
        ),
        voice(
            "kokoro:pm_alex",
            "kokoro",
            "Alex (Kokoro)",
            Language::PtBr,
            Some("kokoro-v1"),
            root,
        ),
        voice(
            "kokoro:af_heart",
            "kokoro",
            "Heart (Kokoro)",
            Language::En,
            Some("kokoro-v1"),
            root,
        ),
        voice(
            "kokoro:am_michael",
            "kokoro",
            "Michael (Kokoro)",
            Language::En,
            Some("kokoro-v1"),
            root,
        ),
        voice(
            "chatterbox:pt",
            "chatterbox",
            "Chatterbox · Português (Brasil)",
            Language::PtBr,
            Some("chatterbox-multilingual"),
            root,
        ),
        voice(
            "chatterbox:en",
            "chatterbox",
            "Chatterbox · English",
            Language::En,
            Some("chatterbox-multilingual"),
            root,
        ),
    ];
    voices.extend(system_voices());
    Catalog { models, voices }
}
fn voice(
    id: &str,
    backend: &str,
    name: &str,
    language: Language,
    model: Option<&str>,
    root: &Path,
) -> VoiceEntry {
    VoiceEntry {
        id: id.into(),
        backend: backend.into(),
        name: name.into(),
        language,
        model_id: model.map(str::to_owned),
        installed: model.and_then(find).is_some_and(|m| {
            if m.id == "chatterbox-multilingual" {
                voice_tts::chatterbox_installed(&root.join("chatterbox"))
            } else {
                files_installed(m, root)
            }
        }),
    }
}
fn system_voices() -> Vec<VoiceEntry> {
    #[cfg(target_os = "macos")]
    {
        let Ok(output) = std::process::Command::new("say")
            .arg("-v")
            .arg("?")
            .output()
        else {
            return vec![];
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let name = parts.next()?;
                let locale = parts.next()?;
                let language = match locale {
                    "pt_BR" => Language::PtBr,
                    "en_US" => Language::En,
                    _ => return None,
                };
                Some(VoiceEntry {
                    id: format!("system:{name}"),
                    backend: "system".into(),
                    name: name.into(),
                    language,
                    model_id: None,
                    installed: true,
                })
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voice_core::catalog::EngineSettings;
    #[test]
    fn installed_requires_every_file_and_active_tracks_settings() {
        let root = std::env::temp_dir().join(format!("voice-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let model = find("kokoro-v1").unwrap();
        for file in model.files {
            let path = root.join(file.path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        let settings = EngineSettings::default();
        let catalog = catalog(&root, &settings);
        let kokoro = catalog.models.iter().find(|m| m.id == "kokoro-v1").unwrap();
        assert!(kokoro.installed);
        assert!(kokoro.active);
        let piper = catalog
            .models
            .iter()
            .find(|m| m.id == "piper-pt_BR-faber-medium")
            .unwrap();
        assert!(!piper.installed);
        assert!(piper.active);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chatterbox_voices_are_installed_when_dummy_model_files_exist() {
        let root =
            std::env::temp_dir().join(format!("voice-catalog-chatterbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for path in [
            "chatterbox/tokenizer.json",
            "chatterbox/default_voice.wav",
            "chatterbox/onnx/speech_encoder.onnx",
            "chatterbox/onnx/speech_encoder.onnx_data",
            "chatterbox/onnx/embed_tokens.onnx",
            "chatterbox/onnx/embed_tokens.onnx_data",
            "chatterbox/onnx/language_model_q4.onnx",
            "chatterbox/onnx/language_model_q4.onnx_data",
            "chatterbox/onnx/conditional_decoder.onnx",
            "chatterbox/onnx/conditional_decoder.onnx_data",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"dummy").unwrap();
        }
        let catalog = catalog(&root, &EngineSettings::default());
        assert!(
            catalog
                .models
                .iter()
                .find(|m| m.id == "chatterbox-multilingual")
                .unwrap()
                .installed
        );
        let voices = catalog
            .voices
            .iter()
            .filter(|v| v.backend == "chatterbox")
            .collect::<Vec<_>>();
        assert_eq!(voices.len(), 2);
        assert!(voices
            .iter()
            .all(|v| v.installed && v.model_id.as_deref() == Some("chatterbox-multilingual")));
        assert_eq!(
            voices
                .iter()
                .find(|v| v.id == "chatterbox:pt")
                .unwrap()
                .language,
            Language::PtBr
        );
        assert_eq!(
            voices
                .iter()
                .find(|v| v.id == "chatterbox:en")
                .unwrap()
                .language,
            Language::En
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
