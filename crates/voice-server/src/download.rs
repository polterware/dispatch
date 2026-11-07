use anyhow::Result;
use voice_core::paths;
pub fn download(yes: bool, stt: Option<&str>, no_tts: bool, no_vad: bool) -> Result<()> {
    let model = match stt.unwrap_or("small") {
        "base" => "ggml-base.bin",
        "turbo" => "ggml-large-v3-turbo-q5_0.bin",
        _ => "ggml-small-q5_1.bin",
    };
    let mut files = vec![(
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{model}"),
        format!("whisper/{model}"),
    )];
    if !no_tts {
        for f in [
            "onnx/model_quantized.onnx",
            "tokenizer.json",
            "config.json",
            "voices/pf_dora.bin",
            "voices/pm_alex.bin",
            "voices/af_heart.bin",
            "voices/am_michael.bin",
        ] {
            files.push((
                format!(
                    "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/{f}"
                ),
                format!("kokoro/{f}"),
            ));
        }
    }
    if !no_vad {
        files.push(("https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx".into(),"vad/silero_vad.onnx".into()));
    }
    for (u, p) in &files {
        println!("{u} -> {}", paths::models_dir().join(p).display());
    }
    if !yes {
        eprint!("Download these model files? [y/N] ");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            return Ok(());
        }
    }
    for (url, path) in files {
        let target = paths::models_dir().join(path);
        if target.exists() {
            continue;
        }
        std::fs::create_dir_all(target.parent().expect("model parent"))?;
        let temp = target.with_extension("part");
        let mut input = ureq::get(&url).call()?.into_reader();
        let mut output = std::fs::File::create(&temp)?;
        std::io::copy(&mut input, &mut output)?;
        std::fs::rename(temp, target)?;
    }
    Ok(())
}
