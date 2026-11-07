//! Audio I/O for the voice engine: microphone capture, playback, and level
//! metering, on CPAL.
//!
//! CONTRACT (owned by the architect; implementations may add private items and
//! extra public helpers, but must keep these signatures):
//!
//! - CPAL streams are `!Send` on macOS, so each stream lives on a dedicated OS
//!   thread owned by its handle. Handles are `Send + Sync` and stop their
//!   thread on `stop()`/`Drop`. Nothing here touches tokio.
//! - Capture delivers mono `f32` frames at 16 kHz (whisper's rate), resampled
//!   from the device's native rate and format, in chunks of `CAPTURE_CHUNK`
//!   samples (32 ms). The callback runs on the audio thread side: it must not
//!   block (use it to push into a channel).
//! - Playback accepts mono `f32` at any sample rate and resamples to the
//!   device. Audio is streamed: `PlaybackHandle::push` appends samples while
//!   it plays, so TTS can start before synthesis ends.
//! - The device is opened only while capturing or playing: no background audio
//!   work when idle.

use std::time::Duration;

/// Capture sample rate delivered to callers.
pub const CAPTURE_RATE: u32 = 16_000;
/// Samples per delivered capture chunk (32 ms at 16 kHz; also Silero VAD's frame size).
pub const CAPTURE_CHUNK: usize = 512;
/// How often meters report a level (≈30 Hz).
pub const LEVEL_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no {0} audio device")]
    NoDevice(&'static str),
    /// macOS refused microphone access (or delivered only silence because of it).
    #[error("microphone permission denied")]
    PermissionDenied,
    #[error("audio device error: {0}")]
    Device(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// A level reading in linear full scale (0..=1).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Level {
    pub rms: f32,
    pub peak: f32,
}

/// Accumulates samples and yields a `Level` every `LEVEL_INTERVAL` worth of
/// audio at `sample_rate`.
pub struct LevelMeter {
    _private: (),
}

impl LevelMeter {
    pub fn new(sample_rate: u32) -> Self {
        let _ = sample_rate;
        unimplemented!("voice-audio: LevelMeter::new")
    }

    /// Feeds samples; returns the levels completed by them (usually 0 or 1).
    pub fn push(&mut self, samples: &[f32]) -> Vec<Level> {
        let _ = samples;
        unimplemented!("voice-audio: LevelMeter::push")
    }
}

#[derive(Debug, Clone, Default)]
pub struct CaptureConfig {
    /// Device name; `None` is the system default input.
    pub device: Option<String>,
}

/// A running capture. Dropping it stops the stream and releases the device.
pub struct CaptureHandle {
    _private: (),
}

impl CaptureHandle {
    /// Mutes delivery: the callback receives zeros while muted.
    pub fn set_muted(&self, muted: bool) {
        let _ = muted;
        unimplemented!("voice-audio: CaptureHandle::set_muted")
    }

    pub fn stop(self) {}
}

/// Opens the input device and calls `on_chunk` with 16 kHz mono chunks of
/// `CAPTURE_CHUNK` samples until the handle is stopped or dropped. Returns
/// once the stream is actually running (or failed to start).
pub fn start_capture<F>(config: CaptureConfig, on_chunk: F) -> Result<CaptureHandle>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let _ = (config, on_chunk);
    unimplemented!("voice-audio: start_capture")
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackConfig {
    /// Device name; `None` is the system default output.
    pub device: Option<String>,
}

/// Events a playback reports from its audio thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackEvent {
    /// The first non-silent buffer reached the device.
    Started,
    /// Output level of what is being heard, every `LEVEL_INTERVAL`.
    Level(Level),
    /// Everything pushed before `finish()` has played, or `cancel()` ran.
    Ended { cancelled: bool },
}

/// One playback stream.
pub struct PlaybackHandle {
    _private: (),
}

impl PlaybackHandle {
    /// Appends mono samples at the stream's input rate.
    pub fn push(&self, samples: &[f32]) {
        let _ = samples;
        unimplemented!("voice-audio: PlaybackHandle::push")
    }

    /// No more samples will come; `Ended { cancelled: false }` follows once the
    /// buffer drains.
    pub fn finish(&self) {
        unimplemented!("voice-audio: PlaybackHandle::finish")
    }

    /// Stops at once (with a ~5 ms fade to avoid a click) and drops the buffer.
    pub fn cancel(&self) {
        unimplemented!("voice-audio: PlaybackHandle::cancel")
    }
}

/// Opens the output device for samples at `input_rate` and reports events
/// through `on_event` (called from a non-realtime thread; it may block
/// briefly). The device is released after `Ended`.
pub fn start_playback<F>(config: PlaybackConfig, input_rate: u32, on_event: F) -> Result<PlaybackHandle>
where
    F: FnMut(PlaybackEvent) + Send + 'static,
{
    let _ = (config, input_rate, on_event);
    unimplemented!("voice-audio: start_playback")
}
