use std::{f32::consts::TAU, sync::mpsc, time::Instant};
use voice_audio::{PlaybackConfig, PlaybackEvent, start_playback};

fn main() -> voice_audio::Result<()> {
    const SAMPLE_RATE: u32 = 48_000;
    const DURATION_SECONDS: f32 = 0.5;
    const AMPLITUDE: f32 = 0.1;

    let samples = (0..(SAMPLE_RATE as f32 * DURATION_SECONDS) as usize)
        .map(|index| {
            let time = index as f32 / SAMPLE_RATE as f32;
            AMPLITUDE * (TAU * 440.0 * time).sin()
        })
        .collect::<Vec<_>>();
    let started_at = Instant::now();
    let (ended_tx, ended_rx) = mpsc::channel();
    let playback = start_playback(PlaybackConfig::default(), SAMPLE_RATE, move |event| {
        let elapsed = started_at.elapsed();
        println!("{elapsed:?} {event:?}");
        if matches!(event, PlaybackEvent::Ended { .. }) {
            let _ = ended_tx.send(());
        }
    })?;
    playback.push(&samples);
    playback.finish();
    ended_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| voice_audio::AudioError::Device(error.to_string()))?;
    drop(playback);
    Ok(())
}
