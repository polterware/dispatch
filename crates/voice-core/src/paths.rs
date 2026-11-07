//! On-disk layout. macOS: `~/Library/Application Support/voice-engine`.
//! Linux: `$XDG_DATA_HOME/voice-engine` for data and `$XDG_RUNTIME_DIR` (else
//! the data dir) for the discovery file. `VOICE_ENGINE_HOME` overrides the root
//! (tests use it).

use std::path::PathBuf;

pub const APP_DIR: &str = "voice-engine";

/// The root data directory.
pub fn data_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("VOICE_ENGINE_HOME") {
        return PathBuf::from(home);
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().join(APP_DIR))
        .unwrap_or_else(|| std::env::temp_dir().join(APP_DIR))
}

/// Where model files live: `models/whisper`, `models/kokoro`, `models/vad`.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// The discovery file a running engine writes (see `discovery`).
pub fn discovery_file() -> PathBuf {
    if std::env::var_os("VOICE_ENGINE_HOME").is_none() {
        if let Some(runtime) = directories::BaseDirs::new().and_then(|d| d.runtime_dir().map(|p| p.to_path_buf())) {
            return runtime.join(APP_DIR).join("engine.json");
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
