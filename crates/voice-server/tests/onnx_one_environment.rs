//! The VAD and a TTS voice load in the same engine process. ONNX Runtime allows one
//! environment per process, and each model used to build its own, so whichever loaded second
//! failed ("only one environment is allowed per process") and replies could not be read
//! aloud. Uses the models on this computer and skips when they are not downloaded.

use voice_core::protocol::Language;
use voice_tts::{PiperOptions, PiperTts, TtsBackend, VoiceSpec};

#[test]
fn vad_then_piper_load_and_speak_in_one_process() {
    let models = voice_core::paths::models_dir();
    let vad = models.join("vad/silero_vad.onnx");
    let Some(piper) = std::fs::read_dir(models.join("piper"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "onnx"))
    else {
        eprintln!("skipped: no Piper voice downloaded");
        return;
    };
    if !vad.is_file() {
        eprintln!("skipped: no Silero VAD downloaded");
        return;
    }
    voice_stt::VadModel::load(&vad).expect("the VAD loads first");
    let tts = PiperTts::load(PiperOptions {
        model: piper,
        espeak: None,
        threads: 2,
    })
    .expect("a voice loads after the VAD");
    let samples = tts
        .synthesize(
            "Teste.",
            &VoiceSpec {
                language: Language::PtBr,
                name: None,
                speed: 1.0,
            },
        )
        .expect("the voice speaks");
    assert!(!samples.is_empty());
}
