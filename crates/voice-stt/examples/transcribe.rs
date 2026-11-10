use std::{env, mem, path::Path, time::Instant};
use voice_core::protocol::Language;
use voice_stt::{SttEngine, SttOptions, Vad, VadEvent, VadOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "vad") {
        if args.len() != 4 {
            return Err("usage: transcribe vad <vad-model> <wav>".into());
        }
        return vad(&args[2], &args[3]);
    }
    if args.len() != 4 {
        return Err("usage: transcribe <model> <wav> <auto|pt|en>".into());
    }
    let language = match args[3].as_str() {
        "auto" => Language::Auto,
        "pt" => Language::PtBr,
        "en" => Language::En,
        _ => return Err("language must be auto, pt, or en".into()),
    };
    let started = Instant::now();
    let engine = SttEngine::load(
        Path::new(&args[1]),
        SttOptions {
            use_gpu: env::var_os("VOICE_STT_CPU").is_none(),
            flash_attn: env::var_os("VOICE_STT_FLASH_ATTN").is_some(),
            ..SttOptions::default()
        },
    )?;
    let load_ms = started.elapsed().as_millis();
    println!("rss_after_load_kb={}", current_rss_kb());
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
        let transcript = engine.transcribe(&pcm, language)?;
        println!(
            "run={run} inference_ms={} language={:?} text={}",
            transcript.inference.as_millis(),
            transcript.language,
            transcript.text
        );
        println!("rss_after_transcription_{run}_kb={}", current_rss_kb());
    }
    Ok(())
}

fn current_rss_kb() -> String {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct TaskBasicInfo64 {
            virtual_size: u64,
            resident_size: u64,
            resident_size_max: u64,
            user_time_seconds: i32,
            user_time_microseconds: i32,
            system_time_seconds: i32,
            system_time_microseconds: i32,
            policy: i32,
            suspend_count: i32,
        }
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn task_info(
                target_task: u32,
                flavor: i32,
                task_info_out: *mut i32,
                task_info_out_count: *mut u32,
            ) -> i32;
        }
        let mut info = mem::MaybeUninit::<TaskBasicInfo64>::uninit();
        let mut count = (mem::size_of::<TaskBasicInfo64>() / mem::size_of::<i32>()) as u32;
        let status = unsafe {
            task_info(
                mach_task_self_,
                20,
                info.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if status == 0 {
            let info = unsafe { info.assume_init() };
            (info.resident_size / 1024).to_string()
        } else {
            format!("unavailable({status})")
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        "unavailable".to_owned()
    }
}

fn vad(model: &str, wav: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(wav)?;
    let spec = reader.spec();
    let samples: Vec<f32> = if spec.sample_format == hound::SampleFormat::Float {
        reader.samples::<f32>().collect::<Result<_, _>>()?
    } else if spec.bits_per_sample <= 16 {
        reader
            .samples::<i16>()
            .map(|x| x.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<_, _>>()?
    } else {
        reader
            .samples::<i32>()
            .map(|x| x.map(|v| v as f32 / i32::MAX as f32))
            .collect::<Result<_, _>>()?
    };
    let mono: Vec<f32> = if spec.channels == 1 {
        samples
    } else {
        samples
            .chunks_exact(spec.channels as usize)
            .map(|f| f.iter().sum::<f32>() / f.len() as f32)
            .collect()
    };
    let pcm = resample(&mono, spec.sample_rate, 16_000);
    let mut vad = Vad::silero(Path::new(model), VadOptions::default())?;
    let mut speech_start = None;
    for (index, chunk) in pcm.chunks(512).enumerate() {
        if let Some(event) = vad.push(chunk) {
            let timestamp = (index + 1) * 512;
            match event {
                VadEvent::SpeechStart => speech_start = Some(timestamp),
                VadEvent::SpeechEnd => {
                    println!(
                        "speech={:.3}-{:.3}s",
                        speech_start.unwrap_or(timestamp) as f64 / 16_000.0,
                        timestamp as f64 / 16_000.0
                    );
                    speech_start = None;
                }
            }
        }
    }
    if let Some(start) = speech_start {
        println!(
            "speech={:.3}-{:.3}s",
            start as f64 / 16_000.0,
            pcm.len() as f64 / 16_000.0
        );
    }
    println!("audio_seconds={:.3}", pcm.len() as f64 / 16_000.0);
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
