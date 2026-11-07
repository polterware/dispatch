use crate::{
    backends::{RealSynthesizer, RealTranscriber, Synthesizer, Transcriber},
    config::{self, Config},
};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub struct ModelManager {
    config: Config,
    root: PathBuf,
    stt: Mutex<Option<(Arc<dyn Transcriber>, Instant)>>,
    tts: Mutex<Option<(Arc<dyn Synthesizer>, Instant)>>,
}
impl ModelManager {
    pub fn new(config: Config, root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            config,
            root,
            stt: Mutex::new(None),
            tts: Mutex::new(None),
        })
    }
    pub async fn stt(&self) -> Result<Arc<dyn Transcriber>> {
        let mut slot = self.stt.lock().await;
        if let Some((model, used)) = slot.as_mut() {
            *used = Instant::now();
            return Ok(model.clone());
        }
        let path = config::select_stt(&self.config, &self.root);
        anyhow::ensure!(path.is_file(), "model_missing: {}", path.display());
        let model = tokio::task::spawn_blocking(move || {
            voice_stt::SttEngine::load(&path, Default::default())
        })
        .await??;
        let model: Arc<dyn Transcriber> = Arc::new(RealTranscriber(model));
        *slot = Some((model.clone(), Instant::now()));
        Ok(model)
    }
    pub async fn tts(&self) -> Result<Arc<dyn Synthesizer>> {
        let mut slot = self.tts.lock().await;
        if let Some((model, used)) = slot.as_mut() {
            *used = Instant::now();
            return Ok(model.clone());
        }
        let dir = self.root.join("kokoro");
        let model_file = self.config.tts_model_file.clone();
        let espeak = crate::backends::espeak();
        let loaded = tokio::task::spawn_blocking(move || {
            if voice_tts::kokoro_installed(&dir) && espeak.is_some() {
                if let Ok(model) = voice_tts::KokoroTts::load(voice_tts::KokoroOptions {
                    dir,
                    model_file,
                    espeak,
                    threads: 2,
                }) {
                    return Box::new(model) as Box<dyn voice_tts::TtsBackend>;
                }
            }
            Box::new(voice_tts::SystemTts::new()) as Box<dyn voice_tts::TtsBackend>
        })
        .await?;
        let model: Arc<dyn Synthesizer> = Arc::new(RealSynthesizer(loaded));
        *slot = Some((model.clone(), Instant::now()));
        Ok(model)
    }
    pub async fn warm(self: &Arc<Self>, stt: bool, tts: bool) -> Result<()> {
        if stt {
            self.stt().await?;
        }
        if tts {
            self.tts().await?;
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
        *self.tts.lock().await = Some((model, Instant::now()));
    }
    pub async fn status(&self) -> (bool, bool, String, String) {
        let stt = self.stt.lock().await;
        let tts = self.tts.lock().await;
        let path = config::select_stt(&self.config, &self.root);
        let backend = tts
            .as_ref()
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
            tts.is_some(),
            path.display().to_string(),
            backend,
        )
    }
    pub fn loaded_flags(&self) -> (Option<bool>, Option<bool>) {
        (
            self.stt.try_lock().ok().map(|slot| slot.is_some()),
            self.tts.try_lock().ok().map(|slot| slot.is_some()),
        )
    }
    pub fn idle_unloader(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let idle = Duration::from_secs(this.config.model_idle_secs);
                if idle.is_zero() {
                    continue;
                }
                let mut stt = this.stt.lock().await;
                if stt.as_ref().is_some_and(|(_, at)| at.elapsed() >= idle) {
                    *stt = None;
                }
                drop(stt);
                let mut tts = this.tts.lock().await;
                if tts.as_ref().is_some_and(|(_, at)| at.elapsed() >= idle) {
                    *tts = None;
                }
            }
        });
    }
}
pub fn list() -> Result<()> {
    let root = voice_core::paths::models_dir();
    for name in ["whisper", "kokoro", "vad"] {
        println!("{}:", root.join(name).display());
        if let Ok(entries) = std::fs::read_dir(root.join(name)) {
            for e in entries.flatten() {
                if let Ok(md) = e.metadata() {
                    println!("  {} ({} bytes)", e.path().display(), md.len());
                }
            }
        }
    }
    Ok(())
}
