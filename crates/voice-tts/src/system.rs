use crate::{Result, TtsError, VoiceSpec};
#[cfg(target_os = "macos")]
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(target_os = "macos")]
use voice_core::protocol::Language;

pub struct SystemTts;
pub(super) fn new() -> SystemTts {
    SystemTts
}
#[cfg(target_os = "macos")]
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "macos")]
fn temp_path(suffix: &str) -> PathBuf {
    let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("voice-tts-{}-{t}-{n}.{suffix}", std::process::id()))
}
#[cfg(target_os = "macos")]
pub(super) fn synthesize(text: &str, voice: &VoiceSpec) -> Result<Vec<f32>> {
    let text_path = temp_path("txt");
    let wav_path = temp_path("wav");
    fs::write(&text_path, text).map_err(|e| TtsError::Synthesis(e.to_string()))?;
    let result = (|| {
        let language = if voice.language == Language::Auto {
            crate::text::detect_language(text)
        } else {
            voice.language
        };
        let language_default = if language == Language::PtBr {
            "Luciana"
        } else {
            "Samantha"
        };
        let requested = voice.name.as_deref().unwrap_or(language_default);
        let available = Command::new("say")
            .args(["-v", "?"])
            .output()
            .ok()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let selected = if available.iter().any(|v| v == requested) {
            requested
        } else if available.iter().any(|v| v == language_default) {
            language_default
        } else {
            ""
        };
        let mut cmd = Command::new("say");
        if !selected.is_empty() {
            cmd.args(["-v", selected]);
        }
        let output = cmd
            .args(["-o"])
            .arg(&wav_path)
            .args(["--file-format=WAVE", "--data-format=LEI16@22050", "-f"])
            .arg(&text_path)
            .output()
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        if !output.status.success() {
            return Err(TtsError::Synthesis(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let mut reader =
            hound::WavReader::open(&wav_path).map_err(|e| TtsError::Synthesis(e.to_string()))?;
        let spec = reader.spec();
        if spec.sample_rate != 22050 {
            return Err(TtsError::Synthesis(format!(
                "say returned {} Hz",
                spec.sample_rate
            )));
        }
        let channels = spec.channels as usize;
        let samples = reader
            .samples::<i16>()
            .map(|s| s.map(|x| x as f32 / 32768.0))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TtsError::Synthesis(e.to_string()))?;
        if samples.is_empty() {
            return Err(TtsError::Synthesis(
                "say returned an empty audio file".into(),
            ));
        }
        Ok(if channels == 1 {
            samples
        } else {
            samples
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        })
    })();
    let _ = fs::remove_file(text_path);
    let _ = fs::remove_file(wav_path);
    result
}
#[cfg(not(target_os = "macos"))]
pub(super) fn synthesize(_text: &str, _voice: &VoiceSpec) -> Result<Vec<f32>> {
    Err(TtsError::Synthesis(
        "system `say` is only available on macOS".into(),
    ))
}
