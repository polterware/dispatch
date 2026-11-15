//! The voice blob over the composer: polterware-ui's [`VoiceBlob`], fed by the engine's level
//! of the microphone while the person speaks, of playback while a reply is read aloud, and
//! breathing quietly while a conversation waits for its next turn.

use gpui::{AnyElement, IntoElement};
use polterware_ui::VoiceBlob;

use crate::theme::Theme;
use crate::voice::{LevelSource, VoiceEngine};

/// What the conversation is doing, which picks the blob's level and ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceMark {
    /// The microphone is open.
    Listening,
    /// A reply is being read aloud.
    Speaking,
    /// A conversation is on and waits for the agent's reply.
    Waiting,
}

/// The blob for `mark`, `side` pixels wide.
pub fn voice_blob(engine: &VoiceEngine, mark: VoiceMark, side: f32, theme: &Theme) -> AnyElement {
    let engine = engine.clone();
    let (color, source) = match mark {
        VoiceMark::Listening => (theme.color_accent, Some(LevelSource::Mic)),
        VoiceMark::Speaking => (theme.color_brand, Some(LevelSource::Output)),
        VoiceMark::Waiting => (theme.color_text_muted, None),
    };
    VoiceBlob::new("dispatch-voice-blob", move || {
        source.map_or(0.0, |source| engine.level(source))
    })
    .size(side)
    .color(color)
    .into_any_element()
}
