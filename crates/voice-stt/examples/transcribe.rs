use std::{env, path::Path, time::Instant};
use voice_core::protocol::Language;
use voice_stt::{SttEngine, SttOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("usage: transcribe <model> <wav>".into());
    }
    let started = Instant::now();
    let engine = SttEngine::load(Path::new(&args[1]), SttOptions::default())?;
    let load_ms = started.elapsed().as_millis();
    let mut reader = hound::WavReader::open(&args[2])?;
    let spec = reader.spec();
    let samples: Vec<f32> = if spec.sample_format == hound::SampleFormat::Float {
        reader.samples::<f32>().map(|x| x.unwrap()).collect()
    } else if spec.bits_per_sample <= 16 {
        reader
            .samples::<i16>()
            .map(|x| x.unwrap() as f32 / i16::MAX as f32)
            .collect()
    } else {
        reader
            .samples::<i32>()
            .map(|x| x.unwrap() as f32 / i32::MAX as f32)
            .collect()
    };
    let mono: Vec<f32> = if spec.channels == 1 {
        samples
    } else {
        samples
            .chunks_exact(spec.channels as usize)
            .map(|f| f.iter().sum::<f32>() / spec.channels as f32)
            .collect()
    };
    let pcm = resample(&mono, spec.sample_rate, 16_000);
    println!("load_ms={load_ms}");
    for run in 1..=2 {
        let transcript = engine.transcribe(&pcm, Language::Auto)?;
        println!(
            "run={run} inference_ms={} language={:?} text={}",
            transcript.inference.as_millis(),
            transcript.language,
            transcript.text
        );
    }
    Ok(())
}

fn resample(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || input.is_empty() {
        return input.to_vec();
    }
    let out_len = input.len() as u64 * target_rate as u64 / source_rate as u64;
    (0..out_len)
        .map(|i| {
            let position = i as f64 * source_rate as f64 / target_rate as f64;
            let left = position.floor() as usize;
            let right = (left + 1).min(input.len() - 1);
            let fraction = (position - left as f64) as f32;
            input[left.min(input.len() - 1)] * (1.0 - fraction) + input[right] * fraction
        })
        .collect()
}
