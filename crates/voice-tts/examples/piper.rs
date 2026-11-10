use std::{env, path::PathBuf, time::Instant};

use voice_core::protocol::Language;
use voice_tts::{PiperOptions, PiperTts, TtsBackend, VoiceSpec};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let model = PathBuf::from(
        args.next()
            .ok_or("usage: piper <model.onnx> <text> <out.wav>")?,
    );
    let text = args
        .next()
        .ok_or("missing text")?
        .to_string_lossy()
        .into_owned();
    let output = PathBuf::from(args.next().ok_or("missing output WAV")?);
    if args.next().is_some() {
        return Err("usage: piper <model.onnx> <text> <out.wav>".into());
    }

    let start = Instant::now();
    let piper = PiperTts::load(PiperOptions {
        model,
        espeak: None,
        threads: 4,
    })?;
    let load_time = start.elapsed();
    let phonemes = piper.phonemize(&text)?;
    println!("Phonemes: {phonemes}");

    let start = Instant::now();
    let samples = piper.synthesize(
        &text,
        &VoiceSpec {
            language: Language::PtBr,
            name: None,
            speed: 1.0,
        },
    )?;
    let synth_time = start.elapsed();
    let audio_time =
        std::time::Duration::from_secs_f64(samples.len() as f64 / piper.sample_rate() as f64);
    let peak = samples
        .iter()
        .fold(0.0f32, |max, sample| max.max(sample.abs()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: piper.sample_rate(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output, spec)?;
    for sample in samples {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    println!(
        "Load: {:.1} ms; synthesis: {:.1} ms; audio: {:.1} ms; RTF: {:.3}; peak: {:.4}",
        load_time.as_secs_f64() * 1000.0,
        synth_time.as_secs_f64() * 1000.0,
        audio_time.as_secs_f64() * 1000.0,
        synth_time.as_secs_f64() / audio_time.as_secs_f64(),
        peak,
    );
    Ok(())
}
