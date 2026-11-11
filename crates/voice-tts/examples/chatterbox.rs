use std::{env, path::PathBuf, time::Instant};

use voice_core::protocol::Language;
use voice_tts::{ChatterboxOptions, ChatterboxTts, TtsBackend, VoiceSpec};

fn rss_mib() -> Option<f64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .map(|kb| kb / 1024.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let mut args = env::args().skip(1);
    let mut language = Language::PtBr;
    let mut exaggeration = 0.5f32;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lang" => {
                language = match args.next().as_deref() {
                    Some("pt-br") => Language::PtBr,
                    Some("en") => Language::En,
                    _ => return Err("--lang must be pt-br or en".into()),
                }
            }
            "--exaggeration" => {
                exaggeration = args.next().ok_or("missing exaggeration")?.parse()?
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}").into()),
            _ => positional.push(arg),
        }
    }
    if positional.len() != 2 {
        return Err(
            "usage: chatterbox [--lang pt-br|en] [--exaggeration 0.5] <text> <out.wav>".into(),
        );
    }
    let text = &positional[0];
    let output = PathBuf::from(&positional[1]);
    let dir = PathBuf::from(env::var_os("VOICE_CHATTERBOX_DIR").ok_or("set VOICE_CHATTERBOX_DIR")?);
    let started = Instant::now();
    let tts = ChatterboxTts::load(ChatterboxOptions {
        dir,
        reference_voice: None,
        exaggeration,
        threads: 4,
    })?;
    let load_time = started.elapsed();
    let rss_loaded = rss_mib();
    let started = Instant::now();
    let samples = tts.synthesize(
        text,
        &VoiceSpec {
            language,
            name: None,
            speed: 1.0,
        },
    )?;
    let synth_time = started.elapsed();
    if env::var_os("CHATTERBOX_WARM").is_some() {
        let started = Instant::now();
        tts.synthesize(
            text,
            &VoiceSpec {
                language,
                name: None,
                speed: 1.0,
            },
        )?;
        println!("Warm synthesis: {:.1} s", started.elapsed().as_secs_f64());
    }
    let rss_synth = rss_mib();
    let duration = samples.len() as f64 / tts.sample_rate() as f64;
    let peak = samples
        .iter()
        .fold(0.0f32, |max, value| max.max(value.abs()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: tts.sample_rate(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output, spec)?;
    for sample in samples {
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    println!(
        "Load: {:.1} s; first audio: {:.1} s; total synthesis: {:.1} s; audio: {:.2} s; RTF: {:.3}; peak: {:.4}; RSS after load: {}; RSS after synthesis: {}",
        load_time.as_secs_f64(),
        synth_time.as_secs_f64(),
        synth_time.as_secs_f64(),
        duration,
        synth_time.as_secs_f64() / duration.max(0.001),
        peak,
        rss_loaded
            .map(|v| format!("{v:.1} MiB"))
            .unwrap_or_else(|| "unavailable".into()),
        rss_synth
            .map(|v| format!("{v:.1} MiB"))
            .unwrap_or_else(|| "unavailable".into())
    );
    Ok(())
}
