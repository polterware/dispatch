//! The process's one ONNX Runtime environment. `ort` allows a single environment per process
//! and refuses a second `build()` even after the first is dropped, so the Silero VAD and every
//! TTS voice (Piper, Kokoro, Chatterbox) take theirs from here: whichever loads first creates
//! it, and it lives as long as the engine.

use std::sync::OnceLock;

use ort::environment::Environment;

/// The shared environment, created on first use. Answers why it could not be created, the same
/// way to every caller.
pub fn environment() -> Result<Environment, String> {
    static ENVIRONMENT: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| ort::init().build().map_err(|error| error.to_string()))
        .clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_caller_gets_the_one_environment() {
        assert!(super::environment().is_ok());
        assert!(super::environment().is_ok(), "a second caller is refused");
    }
}
