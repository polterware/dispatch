use anyhow::Result;
use std::path::PathBuf;
use voice_core::protocol::Language;

pub trait MicHandle: Send + Sync {
    fn set_muted(&self, muted: bool);
    fn stop(self: Box<Self>);
}
pub type CaptureCallback = Box<dyn FnMut(&[f32]) + Send>;
pub trait MicSource: Send + Sync {
    fn start(&self, on_chunk: CaptureCallback) -> Result<Box<dyn MicHandle>>;
}
pub trait SpeakerHandle: Send + Sync {
    fn push(&self, samples: &[f32]);
    fn finish(&self);
    fn cancel(&self);
}
pub trait Speaker: Send + Sync {
    fn start(
        &self,
        rate: u32,
        on_event: Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>,
    ) -> Result<Box<dyn SpeakerHandle>>;
}
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, pcm: &[f32], language: Language) -> Result<voice_stt::Transcript>;
}
pub trait VadFactory: Send + Sync {
    fn create(&self) -> Result<voice_stt::Vad>;
}
pub trait Synthesizer: Send + Sync {
    fn sample_rate(&self) -> u32;
    fn synthesize(&self, sentence: &str, voice: &voice_tts::VoiceSpec) -> Result<Vec<f32>>;
    fn name(&self) -> &'static str;
}

pub struct RealMicSource(pub Option<String>);
struct RealMicHandle(voice_audio::CaptureHandle);
impl MicHandle for RealMicHandle {
    fn set_muted(&self, muted: bool) {
        self.0.set_muted(muted);
    }
    fn stop(self: Box<Self>) {
        self.0.stop();
    }
}
impl MicSource for RealMicSource {
    fn start(&self, callback: Box<dyn FnMut(&[f32]) + Send>) -> Result<Box<dyn MicHandle>> {
        Ok(Box::new(RealMicHandle(voice_audio::start_capture(
            voice_audio::CaptureConfig {
                device: self.0.clone(),
            },
            callback,
        )?)))
    }
}
pub struct RealSpeaker(pub Option<String>);
struct RealSpeakerHandle(voice_audio::PlaybackHandle);
impl SpeakerHandle for RealSpeakerHandle {
    fn push(&self, samples: &[f32]) {
        self.0.push(samples);
    }
    fn finish(&self) {
        self.0.finish();
    }
    fn cancel(&self) {
        self.0.cancel();
    }
}
impl Speaker for RealSpeaker {
    fn start(
        &self,
        rate: u32,
        callback: Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>,
    ) -> Result<Box<dyn SpeakerHandle>> {
        Ok(Box::new(RealSpeakerHandle(voice_audio::start_playback(
            voice_audio::PlaybackConfig {
                device: self.0.clone(),
            },
            rate,
            callback,
        )?)))
    }
}
pub struct RealTranscriber(pub voice_stt::SttEngine);
impl Transcriber for RealTranscriber {
    fn transcribe(&self, pcm: &[f32], language: Language) -> Result<voice_stt::Transcript> {
        Ok(self.0.transcribe(pcm, language)?)
    }
}
pub struct RealVadFactory {
    pub model: Option<PathBuf>,
}
impl VadFactory for RealVadFactory {
    fn create(&self) -> Result<voice_stt::Vad> {
        Ok(match &self.model {
            Some(path) => {
                voice_stt::Vad::silero(path, Default::default()).unwrap_or_else(|error| {
                    tracing::warn!("Silero VAD unavailable ({error}); using energy VAD");
                    voice_stt::Vad::energy(Default::default())
                })
            }
            None => voice_stt::Vad::energy(Default::default()),
        })
    }
}
pub struct RealSynthesizer(pub Box<dyn voice_tts::TtsBackend>);
impl Synthesizer for RealSynthesizer {
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
    fn synthesize(&self, sentence: &str, voice: &voice_tts::VoiceSpec) -> Result<Vec<f32>> {
        Ok(self.0.synthesize(sentence, voice)?)
    }
    fn name(&self) -> &'static str {
        self.0.name()
    }
}

pub fn espeak() -> Option<PathBuf> {
    for p in [
        "espeak-ng",
        "/opt/homebrew/bin/espeak-ng",
        "/usr/local/bin/espeak-ng",
    ] {
        if p.contains('/') {
            if std::path::Path::new(p).exists() {
                return Some(p.into());
            }
        } else if let Ok(path) = std::env::var("PATH") {
            for dir in path.split(':') {
                let x = std::path::Path::new(dir).join(p);
                if x.exists() {
                    return Some(x);
                }
            }
        }
    }
    None
}
