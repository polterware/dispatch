use anyhow::{Context, Result};
use voice_core::{
    catalog::{EngineSettings, SettingsPatch},
    paths,
};

pub fn load() -> EngineSettings {
    let data = std::fs::read(paths::config_file()).ok();
    let mut settings = data
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<EngineSettings>(bytes).ok())
        .unwrap_or_default();
    normalize(&mut settings);
    settings
}
pub fn normalize(settings: &mut EngineSettings) {
    settings.stt_model = match settings.stt_model.as_str() {
        "ggml-base.bin" => "whisper-base",
        "ggml-small-q5_1.bin" => "whisper-small-q5_1",
        "ggml-large-v3-turbo-q5_0.bin" => "whisper-large-v3-turbo-q5_0",
        other => other,
    }
    .into();
    settings.voice_pt = normalize_voice(&settings.voice_pt);
    settings.voice_en = normalize_voice(&settings.voice_en);
}
fn normalize_voice(value: &str) -> String {
    match value {
        "pf_dora" => "kokoro:pf_dora".into(),
        "pm_alex" => "kokoro:pm_alex".into(),
        "af_heart" => "kokoro:af_heart".into(),
        "am_michael" => "kokoro:am_michael".into(),
        v if v.starts_with("piper:") || v.starts_with("kokoro:") || v.starts_with("system:") => {
            v.into()
        }
        v => format!("kokoro:{v}"),
    }
}
pub fn persist_to(settings: &EngineSettings, path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("config path has no parent")?)?;
    let temp = path.with_file_name("config.json.part");
    let mut document = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    for (key, value) in serde_json::to_value(settings)?
        .as_object()
        .context("settings are not an object")?
    {
        document
            .as_object_mut()
            .unwrap()
            .insert(key.clone(), value.clone());
    }
    let bytes = serde_json::to_vec_pretty(&document)?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(temp, path)?;
    Ok(())
}
pub fn apply(
    current: &EngineSettings,
    patch: SettingsPatch,
    root: &std::path::Path,
) -> Result<EngineSettings, String> {
    let mut next = current.clone();
    next.apply(patch);
    normalize(&mut next);
    if crate::catalog::find(&next.stt_model)
        .is_none_or(|m| m.kind != voice_core::catalog::ModelKind::Stt)
    {
        return Err("unknown STT model id".into());
    }
    let catalog = crate::catalog::catalog(root, &next);
    let valid_voice = |id: &str| catalog.voices.iter().any(|v| v.id == id);
    if !valid_voice(&next.voice_pt) {
        return Err("unknown Portuguese voice id".into());
    }
    if !valid_voice(&next.voice_en) {
        return Err("unknown English voice id".into());
    }
    if !next.speed.is_finite() || !(0.5..=2.0).contains(&next.speed) {
        return Err("speed must be between 0.5 and 2.0".into());
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voice_core::catalog::SettingsPatch;
    #[test]
    fn patch_accepts_known_missing_voice_and_rejects_unknown_ids() {
        let root = std::env::temp_dir().join(format!("voice-settings-{}", std::process::id()));
        let base = EngineSettings::default();
        let patch = SettingsPatch {
            voice_pt: Some("piper:pt_BR-cadu-medium".into()),
            ..Default::default()
        };
        assert_eq!(
            apply(&base, patch, &root).unwrap().voice_pt,
            "piper:pt_BR-cadu-medium"
        );
        let bad_model = SettingsPatch {
            stt_model: Some("../../file".into()),
            ..Default::default()
        };
        assert!(apply(&base, bad_model, &root).is_err());
        let bad_voice = SettingsPatch {
            voice_en: Some("system:fake".into()),
            ..Default::default()
        };
        assert!(apply(&base, bad_voice, &root).is_err());
    }
}
