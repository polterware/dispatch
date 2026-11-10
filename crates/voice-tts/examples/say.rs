use std::{mem, path::PathBuf, time::Instant};
use voice_core::protocol::{Language, TextFormat};
use voice_tts::{text, KokoroOptions, KokoroTts, SystemTts, TtsBackend, VoiceSpec};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).peekable();
    let mut backend = "kokoro".to_string();
    let mut lang = Language::PtBr;
    let mut voice = None;
    let mut model_file = "model_quantized.onnx".to_string();
    while let Some(arg) = args.peek().cloned() {
        if !arg.starts_with("--") {
            break;
        }
        args.next();
        let value = args.next().ok_or("missing option value")?;
        match arg.as_str() {
            "--backend" => backend = value,
            "--lang" => {
                lang = if value == "en" {
                    Language::En
                } else {
                    Language::PtBr
                }
            }
            "--voice" => voice = Some(value),
            "--model-file" => model_file = value,
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    let text=args.next().ok_or("usage: say [--backend kokoro|system] [--lang pt-br|en] [--voice X] [--model-file F] <text> <out.wav>")?;
    let out = PathBuf::from(args.next().ok_or("missing output path")?);
    let prepared = text::speakable(&text, TextFormat::Plain, lang);
    let sentence = text::sentences(&prepared)
        .into_iter()
        .next()
        .ok_or("empty speech text")?;
    let voice = VoiceSpec {
        language: lang,
        name: voice,
        speed: 1.0,
    };
    let (model, load_ms, phonemes): (Box<dyn TtsBackend>, f64, String) = match backend.as_str() {
        "kokoro" => {
            let load_start = Instant::now();
            let t = KokoroTts::load(KokoroOptions {
                dir: PathBuf::from(std::env::var("VOICE_KOKORO_DIR").unwrap_or_else(|_| {
                    format!(
                        "{}/Library/Application Support/voice-engine/models/kokoro",
                        std::env::var("HOME").unwrap_or_default()
                    )
                })),
                model_file,
                espeak: None,
                threads: 4,
            })?;
            let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
            let p = t.phonemize(&sentence, lang)?;
            (Box::new(t), load_ms, p)
        }
        "system" => {
            let load_start = Instant::now();
            let t = SystemTts::new();
            let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
            (
                Box::new(t),
                load_ms,
                String::from("not available for system backend"),
            )
        }
        _ => return Err("backend must be kokoro or system".into()),
    };
    println!("rss_after_load_kb: {}", current_rss_kb());
    let cold_start = Instant::now();
    let cold_samples = model.synthesize(&sentence, &voice)?;
    let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;
    println!("rss_after_synthesis_1_kb: {}", current_rss_kb());
    let warm_start = Instant::now();
    let samples = model.synthesize(&sentence, &voice)?;
    let warm_ms = warm_start.elapsed().as_secs_f64() * 1000.0;
    println!("rss_after_synthesis_2_kb: {}", current_rss_kb());
    let third_start = Instant::now();
    let _third_samples = model.synthesize(&sentence, &voice)?;
    let third_ms = third_start.elapsed().as_secs_f64() * 1000.0;
    println!("rss_after_synthesis_3_kb: {}", current_rss_kb());
    let audio_ms = samples.len() as f64 / model.sample_rate() as f64 * 1000.0;
    let peak = samples.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: model.sample_rate(),
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&out, spec)?;
    for &s in &samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    writer.finalize()?;
    let cold_audio_ms = cold_samples.len() as f64 / model.sample_rate() as f64 * 1000.0;
    println!("phonemes: {phonemes}\nload_ms: {load_ms:.1}\ncold_synthesis_ms: {cold_ms:.1}\nwarm_synthesis_ms: {warm_ms:.1}\nthird_synthesis_ms: {third_ms:.1}\naudio_ms: {audio_ms:.1}\ncold_rtf: {:.3}\nwarm_rtf: {:.3}\npeak: {peak:.4}\noutput: {}",cold_ms/cold_audio_ms.max(1.0),warm_ms/audio_ms.max(1.0),out.display());
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
