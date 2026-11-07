use std::{
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use voice_audio::{
    start_capture, start_playback, CaptureConfig, LevelMeter, PlaybackConfig, PlaybackEvent,
    CAPTURE_RATE,
};

fn main() -> voice_audio::Result<()> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let levels = Arc::new(Mutex::new(LevelMeter::new(CAPTURE_RATE)));
    let capture_data = captured.clone();
    let capture_levels = levels.clone();
    let capture = start_capture(CaptureConfig::default(), move |chunk| {
        capture_data.lock().unwrap().extend_from_slice(chunk);
        for level in capture_levels.lock().unwrap().push(chunk) {
            println!("mic rms={:.3} peak={:.3}", level.rms, level.peak);
        }
    })?;
    thread::sleep(Duration::from_secs(3));
    capture.stop();

    let samples = captured.lock().unwrap().clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let playback = start_playback(PlaybackConfig::default(), CAPTURE_RATE, move |event| {
        println!("playback: {event:?}");
        if matches!(event, PlaybackEvent::Ended { .. }) {
            let _ = done_tx.send(());
        }
    })?;
    playback.push(&samples);
    playback.finish();
    let _ = done_rx.recv_timeout(Duration::from_secs(10));
    drop(playback);
    Ok(())
}
