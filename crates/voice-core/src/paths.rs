//! On-disk layout. macOS: `~/Library/Application Support/Polterware Dispatch/voice`,
//! or `$PWDISPATCH_DATA_DIR/voice` when the environment names a data folder, as
//! the core reads it. Linux: `$XDG_DATA_HOME/Polterware Dispatch/voice` for data
//! and `$XDG_RUNTIME_DIR` (else the data dir) for the discovery file.
//! `VOICE_ENGINE_HOME` overrides the root (tests use it).

use std::path::{Path, PathBuf};

/// The app's data folder, which the engine shares with the core and the app.
pub const APP_DIR: &str = "Polterware Dispatch";
/// The engine's own folder inside it.
pub const VOICE_DIR: &str = "voice";
/// Where the standalone engine kept its data, now read once to move its models.
pub const LEGACY_APP_DIR: &str = "voice-engine";

/// The root data directory.
pub fn data_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("VOICE_ENGINE_HOME") {
        return PathBuf::from(home);
    }
    if let Some(data) = std::env::var_os("PWDISPATCH_DATA_DIR")
        && !data.is_empty()
    {
        return PathBuf::from(data).join(VOICE_DIR);
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().join(APP_DIR).join(VOICE_DIR))
        .unwrap_or_else(|| std::env::temp_dir().join(APP_DIR).join(VOICE_DIR))
}

/// Where model files live: `models/whisper`, `models/kokoro`, `models/vad`.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Moves the models the standalone engine downloaded
/// (`~/Library/Application Support/voice-engine/models`) into the app's data
/// folder, once. It does nothing when the data folder is given by
/// `VOICE_ENGINE_HOME` or `PWDISPATCH_DATA_DIR`, or when the new models folder
/// already exists.
pub fn migrate_legacy_models() {
    if std::env::var_os("VOICE_ENGINE_HOME").is_some()
        || std::env::var_os("PWDISPATCH_DATA_DIR").is_some()
    {
        return;
    }
    let Some(base) = directories::BaseDirs::new() else {
        return;
    };
    let legacy = base.data_dir().join(LEGACY_APP_DIR).join("models");
    move_models(&legacy, &models_dir());
}

fn move_models(legacy: &Path, models: &Path) {
    if !legacy.is_dir() || models.exists() {
        return;
    }
    if let Some(parent) = models.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(%error, path = %parent.display(), "could not create the voice data folder");
        return;
    }
    match std::fs::rename(legacy, models) {
        Ok(()) => tracing::info!(
            from = %legacy.display(),
            to = %models.display(),
            "moved the voice models into the app's data folder"
        ),
        Err(error) => tracing::warn!(
            %error,
            from = %legacy.display(),
            path = %models.display(),
            "could not move the voice models into the app's data folder"
        ),
    }
}

/// The discovery file a running engine writes (see `discovery`).
pub fn discovery_file() -> PathBuf {
    if std::env::var_os("VOICE_ENGINE_HOME").is_none() {
        if let Some(runtime) =
            directories::BaseDirs::new().and_then(|d| d.runtime_dir().map(|p| p.to_path_buf()))
        {
            return runtime.join(APP_DIR).join(VOICE_DIR).join("engine.json");
        }
    }
    data_dir().join("engine.json")
}

/// The engine's log file when it runs detached.
pub fn log_file() -> PathBuf {
    data_dir().join("engine.log")
}

/// User configuration (optional JSON).
pub fn config_file() -> PathBuf {
    data_dir().join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_move_once() {
        let dir = std::env::temp_dir().join(format!("voice-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let legacy = dir.join("legacy/models");
        let models = dir.join("new/models");
        std::fs::create_dir_all(legacy.join("kokoro")).unwrap();
        std::fs::write(legacy.join("kokoro/tokenizer.json"), b"{}").unwrap();

        move_models(&legacy, &models);
        assert!(models.join("kokoro/tokenizer.json").exists());
        assert!(!legacy.exists());

        // A second start finds the new folder and leaves it alone.
        std::fs::create_dir_all(&legacy).unwrap();
        move_models(&legacy, &models);
        assert!(legacy.exists());
        assert!(models.join("kokoro/tokenizer.json").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
