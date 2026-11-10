use crate::{
    backends::{RealSynthesizer, RealTranscriber, Synthesizer, Transcriber},
    config::{self, Config},
};
use anyhow::{bail, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};

type LoadedTts = (Arc<dyn Synthesizer>, Instant);

pub struct ModelManager {
    config: RwLock<Config>,
    root: PathBuf,
    stt: Mutex<Option<(Arc<dyn Transcriber>, Instant)>>,
    tts: Mutex<HashMap<String, LoadedTts>>,
}
impl ModelManager {
    pub fn new(config: Config, root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            config: RwLock::new(config),
            root,
            stt: Mutex::new(None),
            tts: Mutex::new(HashMap::new()),
        })
    }
    pub async fn apply_settings(&self, settings: &voice_core::catalog::EngineSettings) {
        let mut config = self.config.write().await;
        let old_stt = config.stt_model.clone();
        config.stt_model = config::stt_filename(&settings.stt_model)
            .unwrap_or(&settings.stt_model)
            .into();
        config.voice_pt = settings.voice_pt.clone();
        config.voice_en = settings.voice_en.clone();
        config.speed = settings.speed;
        config.partials = settings.partials;
        config.model_idle_secs = settings.model_idle_secs;
        config.exit_after_idle_secs = settings.exit_after_idle_secs;
        config.language = settings.language;
        let stt_changed = old_stt != config.stt_model;
        drop(config);
        if stt_changed {
            *self.stt.lock().await = None;
        }
    }
    pub async fn unload_model(&self, id: &str) {
        if id.starts_with("whisper-") {
            *self.stt.lock().await = None;
        }
        if id == "kokoro-v1" {
            self.tts
                .lock()
                .await
                .retain(|key, _| !key.starts_with("kokoro:"));
        }
        if id == "chatterbox-multilingual" {
            self.tts.lock().await.remove("chatterbox");
        }
        if id.starts_with("piper-") {
            self.tts.lock().await.retain(|key, _| {
                format!("piper-{}", key.strip_prefix("piper:").unwrap_or("")) != id
            });
        }
    }
    pub async fn stt(&self) -> Result<Arc<dyn Transcriber>> {
        let mut slot = self.stt.lock().await;
        if let Some((model, used)) = slot.as_mut() {
            *used = Instant::now();
            return Ok(model.clone());
        }
        let config = self.config.read().await.clone();
        let path = config::select_stt(&config, &self.root);
        anyhow::ensure!(path.is_file(), "model_missing: {}", path.display());
        let model = tokio::task::spawn_blocking(move || {
            voice_stt::SttEngine::load(&path, Default::default())
        })
        .await??;
        let model: Arc<dyn Transcriber> = Arc::new(RealTranscriber(model));
        *slot = Some((model.clone(), Instant::now()));
        Ok(model)
    }
    pub async fn tts(&self, voice_id: &str) -> Result<Arc<dyn Synthesizer>> {
        let mut cache = self.tts.lock().await;
        let key = tts_cache_key(voice_id);
        let cached_id = if cache.contains_key(key) {
            Some(key)
        } else if cache.contains_key("test") {
            Some("test")
        } else {
            None
        };
        if let Some((model, used)) = cached_id.and_then(|key| cache.get_mut(key)) {
            *used = Instant::now();
            return Ok(model.clone());
        }
        // Chatterbox can consume well over a gigabyte. Keep it exclusive with
        // every other TTS backend, including while a replacement is loading.
        prepare_tts_cache(&mut cache, key);
        let root = self.root.clone();
        let voice = voice_id.to_string();
        let espeak = crate::backends::espeak();
        let loaded =
            tokio::task::spawn_blocking(move || -> Result<Box<dyn voice_tts::TtsBackend>> {
                if let Some(name) = voice.strip_prefix("piper:") {
                    let model = root.join("piper").join(format!("{name}.onnx"));
                    anyhow::ensure!(model.is_file(), "model_missing: {}", model.display());
                    anyhow::ensure!(
                        espeak.is_some(),
                        "model_missing: espeak-ng is required for Piper"
                    );
                    return Ok(Box::new(voice_tts::PiperTts::load(voice_tts::PiperOptions {
                        model,
                        espeak,
                        threads: 2,
                    })?) as Box<dyn voice_tts::TtsBackend>);
                }
                if voice.starts_with("kokoro:") {
                    let dir = root.join("kokoro");
                    anyhow::ensure!(
                        voice_tts::kokoro_installed(&dir),
                        "model_missing: Kokoro files are incomplete"
                    );
                    anyhow::ensure!(
                        espeak.is_some(),
                        "model_missing: espeak-ng is required for Kokoro"
                    );
                    let model = voice_tts::KokoroTts::load(voice_tts::KokoroOptions {
                        dir,
                        model_file: "model_quantized.onnx".into(),
                        espeak,
                        threads: 2,
                    })?;
                    return Ok(Box::new(model) as Box<dyn voice_tts::TtsBackend>);
                }
                if voice.starts_with("chatterbox:") {
                    let dir = root.join("chatterbox");
                    anyhow::ensure!(
                        voice_tts::chatterbox_installed(&dir),
                        "model_missing: Chatterbox files are incomplete"
                    );
                    let model = voice_tts::ChatterboxTts::load(voice_tts::ChatterboxOptions {
                        dir,
                        reference_voice: None,
                        exaggeration: 0.5,
                        threads: 2,
                    })?;
                    return Ok(Box::new(model) as Box<dyn voice_tts::TtsBackend>);
                }
                if voice.starts_with("system:") && voice_tts::SystemTts::available() {
                    return Ok(
                        Box::new(voice_tts::SystemTts::new()) as Box<dyn voice_tts::TtsBackend>
                    );
                }
                bail!("model_missing: voice {voice} is unavailable")
            })
            .await??;
        let model: Arc<dyn Synthesizer> = Arc::new(RealSynthesizer(loaded));
        if cache.len() >= 2 {
            if let Some(old) = cache
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&old);
            }
        }
        cache.insert(key.to_owned(), (model.clone(), Instant::now()));
        Ok(model)
    }
    pub async fn warm(self: &Arc<Self>, stt: bool, tts: bool) -> Result<()> {
        if stt {
            self.stt().await?;
        }
        if tts {
            let config = self.config.read().await.clone();
            self.tts(&config.voice_pt).await?;
        }
        Ok(())
    }
    pub async fn stt_if_loaded(&self) -> Option<Arc<dyn Transcriber>> {
        self.stt.lock().await.as_mut().map(|(model, used)| {
            *used = Instant::now();
            model.clone()
        })
    }
    #[cfg(test)]
    pub async fn install_test_stt(&self, model: Arc<dyn Transcriber>) {
        *self.stt.lock().await = Some((model, Instant::now()));
    }
    #[cfg(test)]
    pub async fn install_test_tts(&self, model: Arc<dyn Synthesizer>) {
        self.tts
            .lock()
            .await
            .insert("test".into(), (model, Instant::now()));
    }
    #[cfg(test)]
    async fn install_test_tts_at(&self, key: &str, model: Arc<dyn Synthesizer>) {
        self.tts
            .lock()
            .await
            .insert(key.into(), (model, Instant::now()));
    }
    pub async fn status(&self) -> (bool, bool, String, String) {
        let stt = self.stt.lock().await;
        let tts = self.tts.lock().await;
        let config = self.config.read().await;
        let path = config::select_stt(&config, &self.root);
        let backend = tts
            .values()
            .next()
            .map(|(v, _)| v.name().to_string())
            .unwrap_or_else(|| {
                if voice_tts::SystemTts::available() {
                    "system".into()
                } else {
                    "missing".into()
                }
            });
        (
            stt.is_some(),
            !tts.is_empty(),
            path.display().to_string(),
            backend,
        )
    }
    pub fn loaded_flags(&self) -> (Option<bool>, Option<bool>) {
        (
            self.stt.try_lock().ok().map(|slot| slot.is_some()),
            self.tts.try_lock().ok().map(|slot| !slot.is_empty()),
        )
    }
    pub fn idle_unloader(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let idle = Duration::from_secs(this.config.read().await.model_idle_secs);
                if idle.is_zero() {
                    continue;
                }
                let mut stt = this.stt.lock().await;
                if stt.as_ref().is_some_and(|(_, at)| at.elapsed() >= idle) {
                    *stt = None;
                }
                drop(stt);
                this.tts
                    .lock()
                    .await
                    .retain(|_, (_, at)| at.elapsed() < idle);
            }
        });
    }
}

fn tts_cache_key(voice_id: &str) -> &str {
    if voice_id.starts_with("chatterbox:") {
        "chatterbox"
    } else {
        voice_id
    }
}

fn prepare_tts_cache(cache: &mut HashMap<String, LoadedTts>, key: &str) {
    if key == "chatterbox" || cache.contains_key("chatterbox") {
        cache.clear();
    }
}

pub fn list() -> anyhow::Result<()> {
    let root = voice_core::paths::models_dir();
    let catalog = crate::catalog::catalog(&root, &crate::settings::load());
    for model in catalog.models {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            model.id,
            if model.installed {
                "installed"
            } else {
                "missing"
            },
            if model.active { "active" } else { "" },
            model.size_bytes,
            model.license
        );
    }
    Ok(())
}
pub fn remove(id: &str) -> anyhow::Result<()> {
    let spec = crate::catalog::find(id).ok_or_else(|| anyhow::anyhow!("unknown model id: {id}"))?;
    let root = voice_core::paths::models_dir();
    let settings = crate::settings::load();
    anyhow::ensure!(
        !crate::catalog::is_active(spec, &settings),
        "model is active in current settings"
    );
    for path in crate::catalog::file_paths(spec, &root) {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTts;
    impl Synthesizer for FakeTts {
        fn sample_rate(&self) -> u32 {
            24_000
        }
        fn synthesize(&self, _: &str, _: &voice_tts::VoiceSpec) -> Result<Vec<f32>> {
            Ok(vec![])
        }
        fn name(&self) -> &'static str {
            "chatterbox"
        }
    }

    #[tokio::test]
    async fn chatterbox_voices_share_one_cached_backend() {
        let manager = ModelManager::new(Config::default(), PathBuf::from("unused"));
        let backend: Arc<dyn Synthesizer> = Arc::new(FakeTts);
        manager
            .install_test_tts_at("chatterbox", backend.clone())
            .await;
        let portuguese = manager.tts("chatterbox:pt").await.unwrap();
        let english = manager.tts("chatterbox:en").await.unwrap();
        assert!(Arc::ptr_eq(&portuguese, &english));
        assert_eq!(manager.tts.lock().await.len(), 1);
    }

    #[test]
    fn chatterbox_cache_policy_evicts_other_backends() {
        let mut cache = HashMap::new();
        let backend: Arc<dyn Synthesizer> = Arc::new(FakeTts);
        cache.insert("kokoro:af_heart".into(), (backend.clone(), Instant::now()));
        prepare_tts_cache(&mut cache, "chatterbox");
        assert!(cache.is_empty());
        cache.insert("chatterbox".into(), (backend.clone(), Instant::now()));
        prepare_tts_cache(&mut cache, "piper:pt_BR-faber-medium");
        assert!(cache.is_empty());
    }
}
