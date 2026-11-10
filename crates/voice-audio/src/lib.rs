//! Audio I/O for the voice engine.
//!
//! Capture and playback streams are owned by worker threads and exist only while
//! their handles are active. Realtime CPAL callbacks must not allocate or lock;
//! they exchange samples with workers through bounded lock-free ring buffers.
//! Capture start returns after its stream is playing. Playback emits `Started`
//! when its first non-silent sample reaches the output callback, then zero or
//! more `Level` events, and exactly one `Ended`. Playback cancellation fades
//! output before `Ended`; the output stream is released before that event is
//! delivered. Dropping either handle stops and joins its worker.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

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

/// Accumulates samples and yields a `Level` every `LEVEL_INTERVAL` worth of audio at `sample_rate`.
pub struct LevelMeter {
    window: usize,
    sum_sq: f64,
    peak: f32,
    count: usize,
}

impl LevelMeter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            window: ((sample_rate as u64 * LEVEL_INTERVAL.as_nanos() as u64) / 1_000_000_000).max(1)
                as usize,
            sum_sq: 0.0,
            peak: 0.0,
            count: 0,
        }
    }
    pub fn push(&mut self, samples: &[f32]) -> Vec<Level> {
        let mut levels = Vec::new();
        for &sample in samples {
            let value = sample.clamp(-1.0, 1.0);
            self.sum_sq += f64::from(value) * f64::from(value);
            self.peak = self.peak.max(value.abs());
            self.count += 1;
            if self.count == self.window {
                levels.push(Level {
                    rms: (self.sum_sq / self.count as f64).sqrt() as f32,
                    peak: self.peak,
                });
                self.sum_sq = 0.0;
                self.peak = 0.0;
                self.count = 0;
            }
        }
        levels
    }
}

#[derive(Debug, Clone, Default)]
pub struct CaptureConfig {
    pub device: Option<String>,
}

pub struct CaptureHandle {
    stop: mpsc::Sender<()>,
    muted: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl CaptureHandle {
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start_capture<F>(config: CaptureConfig, mut on_chunk: F) -> Result<CaptureHandle>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let (stop_tx, stop_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let muted = Arc::new(AtomicBool::new(false));
    let muted_worker = muted.clone();
    let worker = thread::Builder::new().name("voice-audio-capture".into()).spawn(move || {
        let setup = setup_capture(config, ready_tx);
        let (stream, mut samples, channels, rate) = match setup { Ok(v) => v, Err(()) => return };
        let _stream = stream;
        let mut chunk = Vec::with_capacity(CAPTURE_CHUNK);
        let mut pending = Vec::new();
        let mut phase = 0.0f64;
        let mut previous_mono = None;
        let started_at = Instant::now();
        let mut had_nonzero = false;
        loop {
            if stop_rx.try_recv().is_ok() { break; }
            while let Some(sample) = samples.try_pop() {
                if started_at.elapsed() < Duration::from_millis(1500) && sample != 0.0 { had_nonzero = true; }
                pending.push(sample);
            }
            let frames = pending.len() / channels;
            if frames > 1 {
                for frame in 0..frames {
                    let base = frame * channels;
                    let mono = downmix_frame(&pending[base..base + channels]);
                    resample_frame(previous_mono, mono, &mut phase, CAPTURE_RATE, rate, |sample| {
                        chunk.push(if muted_worker.load(Ordering::Relaxed) { 0.0 } else { sample });
                    });
                    previous_mono = Some(mono);
                    if chunk.len() == CAPTURE_CHUNK { on_chunk(&chunk); chunk.clear(); }
                }
                pending.drain(..(frames * channels));
            }
            if started_at.elapsed() >= Duration::from_millis(1500) && !had_nonzero {
                tracing::warn!("microphone input has produced only zeros for 1.5 seconds; check microphone permission");
                had_nonzero = true;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }).map_err(|e| AudioError::Device(e.to_string()))?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(CaptureHandle {
            stop: stop_tx,
            muted,
            thread: Some(worker),
        }),
        Ok(Err(err)) => {
            let _ = worker.join();
            Err(err)
        }
        Err(_) => {
            let _ = stop_tx.send(());
            let _ = worker.join();
            Err(AudioError::Device(
                "timed out starting capture stream".into(),
            ))
        }
    }
}

fn setup_capture(
    config: CaptureConfig,
    ready: mpsc::SyncSender<Result<()>>,
) -> std::result::Result<(cpal::Stream, ringbuf::HeapCons<f32>, usize, u32), ()> {
    let host = cpal::default_host();
    let device = match config.device {
        Some(name) => host
            .input_devices()
            .ok()
            .and_then(|mut ds| ds.find(|d| d.name().is_ok_and(|n| n == name)))
            .ok_or(AudioError::NoDevice("input")),
        None => host
            .default_input_device()
            .ok_or(AudioError::NoDevice("input")),
    };
    let device = match device {
        Ok(d) => d,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Err(());
        }
    };
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(map_device_error(e.to_string())));
            return Err(());
        }
    };
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate().0;
    let config = supported.config();
    let (mut prod, cons) = HeapRb::<f32>::new((rate as usize * channels).max(8192)).split();
    let error_callback = |_| {};
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_input::<f32, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(v);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::I16 => build_input::<i16, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(v as f32 / 32768.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::U16 => build_input::<u16, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push((v as f32 - 32768.0) / 32768.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::I8 => build_input::<i8, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(v as f32 / 128.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::U8 => build_input::<u8, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push((v as f32 - 128.0) / 128.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::I32 => build_input::<i32, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(v as f32 / 2_147_483_648.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::U32 => build_input::<u32, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push((v as f32 - 2_147_483_648.0) / 2_147_483_648.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::I64 => build_input::<i64, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(v as f64 as f32 / 9_223_372_036_854_775_808.0);
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::U64 => build_input::<u64, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(
                        ((v as f64 - 9_223_372_036_854_775_808.0) / 9_223_372_036_854_775_808.0)
                            as f32,
                    );
                }
            },
            error_callback,
        ),
        cpal::SampleFormat::F64 => build_input::<f64, _>(
            &device,
            &config,
            move |data| {
                for &v in data {
                    let _ = prod.try_push(v as f32);
                }
            },
            error_callback,
        ),
        fmt => {
            let _ = ready.send(Err(AudioError::Device(format!(
                "unsupported input sample format: {fmt:?}"
            ))));
            return Err(());
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(map_device_error(e.to_string())));
            return Err(());
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(map_device_error(e.to_string())));
        return Err(());
    }
    let _ = ready.send(Ok(()));
    Ok((stream, cons, channels, rate))
}

fn build_input<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut convert: F,
    error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> std::result::Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    F: FnMut(&[T]) + Send + 'static,
{
    device.build_input_stream(config, move |data: &[T], _| convert(data), error, None)
}
fn map_device_error(message: String) -> AudioError {
    let lower = message.to_lowercase();
    if lower.contains("permission") || lower.contains("not authorized") || lower.contains("denied")
    {
        AudioError::PermissionDenied
    } else {
        AudioError::Device(message)
    }
}

fn downmix_frame(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        0.0
    } else {
        frame.iter().copied().sum::<f32>() / frame.len() as f32
    }
}

fn resample_frame(
    previous: Option<f32>,
    current: f32,
    phase: &mut f64,
    output_rate: u32,
    input_rate: u32,
    mut emit: impl FnMut(f32),
) -> usize {
    let step = f64::from(output_rate) / f64::from(input_rate.max(1));
    *phase += step;
    let mut emitted = 0;
    while *phase >= 1.0 - 1e-12 {
        *phase = (*phase - 1.0).max(0.0);
        let fraction = (1.0 - *phase / step).clamp(0.0, 1.0) as f32;
        let sample = previous.map_or(current, |last| last + (current - last) * fraction);
        emit(sample);
        emitted += 1;
    }
    emitted
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackConfig {
    pub device: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackEvent {
    Started,
    Level(Level),
    Ended { cancelled: bool },
}

enum PlayCommand {
    Push(Vec<f32>),
    Finish,
    Cancel,
}

#[derive(Default)]
struct PlaybackState {
    started: bool,
    finished: bool,
    cancelled: bool,
}

impl PlaybackState {
    fn cancel(&mut self) {
        self.cancelled = true;
        self.finished = true;
    }

    fn should_end(&self, queues_drained: bool, fade_drained: bool) -> bool {
        self.finished && queues_drained && fade_drained
    }

    fn ended_event(&self) -> PlaybackEvent {
        PlaybackEvent::Ended {
            cancelled: self.cancelled,
        }
    }
}
pub struct PlaybackHandle {
    tx: mpsc::Sender<PlayCommand>,
    thread: Option<thread::JoinHandle<()>>,
}
impl PlaybackHandle {
    pub fn push(&self, samples: &[f32]) {
        let _ = self.tx.send(PlayCommand::Push(samples.to_vec()));
    }
    pub fn finish(&self) {
        let _ = self.tx.send(PlayCommand::Finish);
    }
    pub fn cancel(&self) {
        let _ = self.tx.send(PlayCommand::Cancel);
    }
}
impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(PlayCommand::Cancel);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start_playback<F>(
    config: PlaybackConfig,
    input_rate: u32,
    on_event: F,
) -> Result<PlaybackHandle>
where
    F: FnMut(PlaybackEvent) + Send + 'static,
{
    if input_rate == 0 {
        return Err(AudioError::Device(
            "input sample rate must be greater than zero".into(),
        ));
    }
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("voice-audio-playback".into())
        .spawn(move || run_playback(config, input_rate, rx, ready_tx, on_event))
        .map_err(|e| AudioError::Device(e.to_string()))?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(PlaybackHandle {
            tx,
            thread: Some(worker),
        }),
        Ok(Err(e)) => {
            let _ = worker.join();
            Err(e)
        }
        Err(_) => {
            let _ = tx.send(PlayCommand::Cancel);
            let _ = worker.join();
            Err(AudioError::Device(
                "timed out starting playback stream".into(),
            ))
        }
    }
}

fn run_playback<F: FnMut(PlaybackEvent)>(
    config: PlaybackConfig,
    input_rate: u32,
    rx: mpsc::Receiver<PlayCommand>,
    ready: mpsc::SyncSender<Result<()>>,
    mut on_event: F,
) {
    let host = cpal::default_host();
    let device = match config.device {
        Some(name) => host
            .output_devices()
            .ok()
            .and_then(|mut ds| ds.find(|d| d.name().is_ok_and(|n| n == name))),
        None => host.default_output_device(),
    };
    let Some(device) = device else {
        let _ = ready.send(Err(AudioError::NoDevice("output")));
        return;
    };
    let supported = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(AudioError::Device(e.to_string())));
            return;
        }
    };
    let channels = supported.channels() as usize;
    let rate = supported.sample_rate().0;
    let config = supported.config();
    let (mut prod, mut cons) = HeapRb::<f32>::new((rate as usize * channels).max(8192)).split();
    let (mut played_prod, mut played_cons) = HeapRb::<f32>::new((rate as usize).max(8192)).split();
    let fade = Arc::new(AtomicUsize::new(0));
    let cancelling = Arc::new(AtomicBool::new(false));
    let fade_cb = fade.clone();
    let cancelling_cb = cancelling.clone();
    let fade_total = (rate as usize / 200).max(1);
    macro_rules! output_stream {
        ($sample:ty) => {
            device.build_output_stream::<$sample, _, _>(
                &config,
                move |data, _| {
                    output_callback(
                        data,
                        channels,
                        &mut cons,
                        &mut played_prod,
                        &fade_cb,
                        &cancelling_cb,
                        fade_total,
                    )
                },
                |_| {},
                None,
            )
        };
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => output_stream!(f32),
        cpal::SampleFormat::I16 => output_stream!(i16),
        cpal::SampleFormat::U16 => output_stream!(u16),
        cpal::SampleFormat::I8 => output_stream!(i8),
        cpal::SampleFormat::U8 => output_stream!(u8),
        cpal::SampleFormat::I32 => output_stream!(i32),
        cpal::SampleFormat::U32 => output_stream!(u32),
        cpal::SampleFormat::I64 => output_stream!(i64),
        cpal::SampleFormat::U64 => output_stream!(u64),
        cpal::SampleFormat::F64 => output_stream!(f64),
        fmt => {
            let _ = ready.send(Err(AudioError::Device(format!(
                "unsupported output sample format: {fmt:?}"
            ))));
            return;
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(AudioError::Device(e.to_string())));
            return;
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(AudioError::Device(e.to_string())));
        return;
    }
    let _ = ready.send(Ok(()));
    let mut meter = LevelMeter::new(rate);
    let mut state = PlaybackState::default();
    let mut input = VecDeque::<f32>::new();
    let mut resampled = VecDeque::<f32>::new();
    let mut phase = 0.0f64;
    let mut previous_input = None;
    let mut meter_samples = Vec::with_capacity(rate as usize / 15);
    loop {
        match rx.recv_timeout(Duration::from_millis(4)) {
            Ok(PlayCommand::Push(data)) => input.extend(data),
            Ok(PlayCommand::Finish) => state.finished = true,
            Ok(PlayCommand::Cancel) => {
                state.cancel();
                input.clear();
                resampled.clear();
                cancelling.store(true, Ordering::Relaxed);
                fade.store(fade_total, Ordering::Relaxed);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                state.cancel();
                input.clear();
                resampled.clear();
                cancelling.store(true, Ordering::Relaxed);
                fade.store(fade_total, Ordering::Relaxed);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while resampled.len() < 2048 {
            let Some(current) = input.pop_front() else {
                break;
            };
            resample_frame(
                previous_input,
                current,
                &mut phase,
                rate,
                input_rate,
                |sample| resampled.push_back(sample),
            );
            previous_input = Some(current);
        }
        while !prod.is_full() {
            let Some(sample) = resampled.pop_front() else {
                break;
            };
            let _ = prod.try_push(sample);
        }
        meter_samples.clear();
        while let Some(sample) = played_cons.try_pop() {
            meter_samples.push(sample);
        }
        if meter_samples.iter().any(|s| *s != 0.0) && !state.started {
            state.started = true;
            on_event(PlaybackEvent::Started);
        }
        for level in meter.push(&meter_samples) {
            on_event(PlaybackEvent::Level(level));
        }
        let queues_drained =
            input.is_empty() && resampled.is_empty() && prod.is_empty() && played_cons.is_empty();
        if state.should_end(queues_drained, fade.load(Ordering::Relaxed) == 0) {
            break;
        }
    }
    drop(stream);
    on_event(state.ended_event());
}

fn output_callback<T>(
    data: &mut [T],
    channels: usize,
    consumer: &mut impl Consumer<Item = f32>,
    played: &mut impl Producer<Item = f32>,
    fade: &AtomicUsize,
    cancelling: &AtomicBool,
    fade_total: usize,
) where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    for frame in data.chunks_mut(channels) {
        let mut sample = consumer.try_pop().unwrap_or(0.0);
        let remaining = fade.load(Ordering::Relaxed);
        if cancelling.load(Ordering::Relaxed) && remaining > 0 {
            sample *= remaining as f32 / fade_total as f32;
            fade.fetch_sub(1, Ordering::Relaxed);
        } else if cancelling.load(Ordering::Relaxed) {
            sample = 0.0;
        }
        let _ = played.try_push(sample);
        let output = T::from_sample(sample);
        for out in frame {
            *out = output;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CaptureHandle>();
        assert_send_sync::<PlaybackHandle>();
    }

    #[test]
    fn level_meter_reports_window_rms_and_peak() {
        let mut meter = LevelMeter::new(1000);
        assert!(meter.push(&[0.5; 32]).is_empty());
        let levels = meter.push(&[0.5; 1]);
        assert_eq!(
            levels,
            vec![Level {
                rms: 0.5,
                peak: 0.5
            }]
        );
    }

    #[test]
    fn playback_state_ends_after_drain_or_cancel_fade() {
        let drained = PlaybackState {
            finished: true,
            ..PlaybackState::default()
        };
        assert!(!drained.should_end(false, true));
        assert!(!drained.should_end(true, false));
        assert!(drained.should_end(true, true));
        assert_eq!(
            drained.ended_event(),
            PlaybackEvent::Ended { cancelled: false }
        );

        let mut cancelled = PlaybackState::default();
        cancelled.cancel();
        assert!(!cancelled.should_end(true, false));
        assert!(cancelled.should_end(true, true));
        assert_eq!(
            cancelled.ended_event(),
            PlaybackEvent::Ended { cancelled: true }
        );
    }

    #[test]
    fn resampling_downmix_and_chunking_contract() {
        assert_eq!(downmix_frame(&[0.25, 0.75]), 0.5);
        let mut phase = 0.0;
        let mut samples = Vec::new();
        let mut previous = None;
        for _ in 0..48_000 {
            resample_frame(previous, 0.25, &mut phase, CAPTURE_RATE, 48_000, |sample| {
                samples.push(sample)
            });
            previous = Some(0.25);
        }
        assert_eq!(samples.len(), 16_000);
        assert_eq!(samples.chunks_exact(CAPTURE_CHUNK).len(), 31);
        assert!(samples
            .chunks_exact(CAPTURE_CHUNK)
            .all(|chunk| chunk.iter().all(|v| *v == 0.25)));

        let mut interpolated = Vec::new();
        let mut phase = 0.0;
        resample_frame(Some(0.0), 1.0, &mut phase, 2, 1, |sample| {
            interpolated.push(sample)
        });
        assert_eq!(interpolated, vec![0.5, 1.0]);
    }

    #[test]
    #[ignore = "requires a microphone device and permission"]
    fn capture_device_available() {
        start_capture(CaptureConfig::default(), |_| {})
            .unwrap()
            .stop();
    }

    #[test]
    #[ignore = "requires an output device"]
    fn playback_device_available() {
        let (ended_tx, ended_rx) = mpsc::channel();
        let playback = start_playback(PlaybackConfig::default(), CAPTURE_RATE, move |event| {
            if matches!(event, PlaybackEvent::Ended { cancelled: false }) {
                let _ = ended_tx.send(());
            }
        })
        .unwrap();
        playback.push(&[0.1; 1600]);
        playback.finish();
        ended_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    }
}
