use anyhow::{Context, Result};
use std::{path::PathBuf, time::Instant};
use voice_core::{paths, protocol::Language};
pub fn run(wav: Option<PathBuf>, stt_model: Option<PathBuf>) -> Result<()> {
    let before = crate::rss::resident_bytes();
    let model = stt_model
        .unwrap_or_else(|| crate::config::select_stt(&crate::config::load(), &paths::models_dir()));
    let load_start = Instant::now();
    let stt =
        voice_stt::SttEngine::load(&model, Default::default()).context("loading STT model")?;
    let stt_load = load_start.elapsed().as_millis();
    let wav = wav.unwrap_or_else(|| PathBuf::from("benchmarks/clips/ptbr-command.wav"));
    let pcm = read_wav(&wav)?;
    let transcribe = Instant::now();
    let transcript = stt.transcribe(&pcm, Language::Auto)?;
    let transcribe_ms = transcribe.elapsed().as_millis();
    let tts_start = Instant::now();
    let models = paths::models_dir().join("kokoro");
    let tts: Box<dyn voice_tts::TtsBackend> =
        if voice_tts::kokoro_installed(&models) && crate::backends::espeak().is_some() {
            match voice_tts::KokoroTts::load(voice_tts::KokoroOptions {
                dir: models,
                model_file: crate::config::load().tts_model_file,
                espeak: crate::backends::espeak(),
                threads: 2,
            }) {
                Ok(m) => Box::new(m),
                Err(_) => Box::new(voice_tts::SystemTts::new()),
            }
        } else {
            Box::new(voice_tts::SystemTts::new())
        };
    let tts_load = tts_start.elapsed().as_millis();
    let prep = voice_tts::text::speakable(
        &transcript.text,
        voice_core::protocol::TextFormat::Plain,
        transcript.language,
    );
    let sentence = voice_tts::text::sentences(&prep)
        .into_iter()
        .next()
        .unwrap_or_default();
    let start = Instant::now();
    let samples = tts.synthesize(
        &sentence,
        &voice_tts::VoiceSpec {
            language: transcript.language,
            name: None,
            speed: 1.0,
        },
    )?;
    let first = start.elapsed().as_millis();
    let duration = samples.len() as f64 / tts.sample_rate() as f64;
    let rtf = if duration > 0.0 {
        first as f64 / 1000.0 / duration
    } else {
        0.0
    };
    let after = crate::rss::resident_bytes();
    println!(
        "{}",
        serde_json::json!({"stt_load_ms":stt_load,"warm_transcribe_ms":transcribe_ms,"tts_load_ms":tts_load,"first_sentence_ms":first,"rtf":rtf,"rss_before_bytes":before,"rss_after_bytes":after,"wav":wav.display().to_string()})
    );
    Ok(())
}
fn read_wav(path: &std::path::Path) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() > 44 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "unsupported WAV file"
    );
    let mut pos = 12;
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut data = &bytes[44..];
    while pos + 8 <= bytes.len() {
        let n = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?) as usize;
        let start = pos + 8;
        if start + n > bytes.len() {
            break;
        }
        match &bytes[pos..pos + 4] {
            b"fmt " => {
                format = u16::from_le_bytes(bytes[start..start + 2].try_into()?);
                channels = u16::from_le_bytes(bytes[start + 2..start + 4].try_into()?);
                rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into()?);
                bits = u16::from_le_bytes(bytes[start + 14..start + 16].try_into()?);
            }
            b"data" => data = &bytes[start..start + n],
            _ => {}
        }
        pos = start + n + (n & 1);
    }
    anyhow::ensure!(
        format == 1 && bits == 16 && channels > 0,
        "expected PCM16 WAV"
    );
    let raw = data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    let mono = raw
        .chunks_exact(channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect::<Vec<_>>();
    if rate == 16000 {
        return Ok(mono);
    }
    let n = (mono.len() as u64 * 16000 / rate.max(1) as u64) as usize;
    Ok((0..n)
        .map(|i| {
            let pos = i as f64 * rate as f64 / 16000.0;
            let a = pos.floor() as usize;
            let b = (a + 1).min(mono.len().saturating_sub(1));
            let f = (pos - a as f64) as f32;
            mono.get(a).copied().unwrap_or(0.0) * (1.0 - f)
                + mono.get(b).copied().unwrap_or(0.0) * f
        })
        .collect())
}
