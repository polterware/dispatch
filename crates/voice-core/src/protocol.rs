//! Wire protocol v1. JSON over a WebSocket at `GET /v1/ws`, plus a few HTTP
//! endpoints. See `docs/protocol.md` for the normative description.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// A language hint. `Auto` lets the STT model detect it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    Auto,
    PtBr,
    En,
}

impl Language {
    /// The two-letter code whisper.cpp expects, or `None` for auto-detect.
    pub fn whisper_code(self) -> Option<&'static str> {
        match self {
            Language::Auto => None,
            Language::PtBr => Some("pt"),
            Language::En => Some("en"),
        }
    }

    /// Maps a whisper language code back to a supported language.
    pub fn from_whisper_code(code: &str) -> Language {
        match code {
            "pt" => Language::PtBr,
            "en" => Language::En,
            _ => Language::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ListenMode {
    /// The client says when the utterance ends (`listen.stop`).
    #[default]
    PushToTalk,
    /// The engine ends the utterance itself when VAD detects silence.
    HandsFree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TextFormat {
    /// Markdown from a coding agent: the engine filters code blocks, diffs,
    /// tables, and long paths before speaking.
    #[default]
    Markdown,
    Plain,
}

/// Messages a client sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    /// Opens the microphone and starts an utterance. Only one client may
    /// listen at a time; another gets `error { code: "busy" }`.
    #[serde(rename = "listen.start")]
    ListenStart {
        id: String,
        #[serde(default)]
        language: Language,
        #[serde(default)]
        mode: ListenMode,
    },
    /// Ends the utterance and transcribes it.
    #[serde(rename = "listen.stop")]
    ListenStop { id: String },
    /// Drops the utterance without transcribing it.
    #[serde(rename = "listen.cancel")]
    ListenCancel { id: String },
    /// Queues text to speak. Utterances with the same `id` prefix are not
    /// special; each `speak` is its own playback item.
    #[serde(rename = "speak")]
    Speak {
        id: String,
        text: String,
        #[serde(default)]
        format: TextFormat,
        #[serde(default)]
        language: Language,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
    },
    /// Stops playback and drops the queue. `id: None` cancels everything.
    #[serde(rename = "speak.cancel")]
    SpeakCancel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Mutes the microphone while keeping the utterance open (levels read 0).
    #[serde(rename = "mic.mute")]
    MicMute { muted: bool },
    /// Loads models ahead of use so the first utterance is fast.
    #[serde(rename = "models.warm")]
    ModelsWarm {
        #[serde(default)]
        stt: bool,
        #[serde(default)]
        tts: bool,
    },
    #[serde(rename = "ping")]
    Ping {
        #[serde(default)]
        nonce: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MicState {
    Idle,
    Listening,
    Muted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SttState {
    Unloaded,
    Loading,
    Ready,
    Transcribing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TtsState {
    Unloaded,
    Loading,
    Ready,
    Speaking,
}

/// The engine's state, sent on connect and whenever it changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineState {
    pub mic: MicState,
    pub stt: SttState,
    pub tts: TtsState,
    /// Id of the utterance being captured, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_id: Option<String>,
    /// Id of the speak item playing, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speak_id: Option<String>,
    pub clients: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LevelSource {
    Mic,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakEndReason {
    Done,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptTimings {
    /// Captured audio length.
    pub audio_ms: u64,
    /// From `listen.stop` (or VAD end of speech) to the final transcript.
    pub stop_to_final_ms: u64,
    /// Inference time alone.
    pub inference_ms: u64,
}

/// Messages the engine sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EngineMessage {
    #[serde(rename = "hello")]
    Hello {
        protocol: u32,
        engine_version: String,
        state: EngineState,
    },
    #[serde(rename = "state")]
    State { state: EngineState },
    /// Audio level, ~30 per second per source, only while that source is
    /// active. `rms` and `peak` are linear in 0..=1 (full scale), not smoothed.
    #[serde(rename = "level")]
    Level {
        source: LevelSource,
        rms: f32,
        peak: f32,
    },
    /// VAD saw speech start or stop inside the current utterance.
    #[serde(rename = "vad")]
    Vad { id: String, speech: bool },
    /// A provisional transcript of the utterance so far.
    #[serde(rename = "transcript.partial")]
    TranscriptPartial { id: String, text: String },
    /// The utterance's final transcript. Empty `text` means nothing was heard.
    #[serde(rename = "transcript.final")]
    TranscriptFinal {
        id: String,
        text: String,
        language: Language,
        timings: TranscriptTimings,
    },
    #[serde(rename = "speak.started")]
    SpeakStarted {
        id: String,
        /// From the `speak` request to the first audible sample.
        first_audio_ms: u64,
    },
    #[serde(rename = "speak.ended")]
    SpeakEnded { id: String, reason: SpeakEndReason },
    #[serde(rename = "pong")]
    Pong { nonce: u64 },
    #[serde(rename = "error")]
    Error {
        /// Stable machine code, see `ErrorCode`.
        code: ErrorCode,
        message: String,
        /// The request id it refers to, when it refers to one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Another client holds the microphone.
    Busy,
    /// A required model file is missing; run `voice-engine models download`.
    ModelMissing,
    /// No input or output device, or the device failed.
    AudioDevice,
    /// The OS refused microphone access.
    MicPermission,
    SttFailed,
    TtsFailed,
    /// A message that did not parse or an id that is unknown.
    BadRequest,
    Internal,
}

/// `GET /v1/health` (no auth).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub protocol: u32,
    pub engine_version: String,
    pub pid: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_wire_shape() {
        let msg: ClientMessage =
            serde_json::from_str(r#"{"type":"listen.start","id":"u1","language":"pt-br"}"#)
                .unwrap();
        assert_eq!(
            msg,
            ClientMessage::ListenStart {
                id: "u1".into(),
                language: Language::PtBr,
                mode: ListenMode::PushToTalk
            }
        );
        let speak: ClientMessage =
            serde_json::from_str(r#"{"type":"speak","id":"s1","text":"hi"}"#).unwrap();
        assert!(matches!(
            speak,
            ClientMessage::Speak {
                format: TextFormat::Markdown,
                ..
            }
        ));
    }

    #[test]
    fn engine_message_wire_shape() {
        let json = serde_json::to_value(EngineMessage::Level {
            source: LevelSource::Mic,
            rms: 0.5,
            peak: 1.0,
        })
        .unwrap();
        assert_eq!(json["type"], "level");
        assert_eq!(json["source"], "mic");
        let err = serde_json::to_value(EngineMessage::Error {
            code: ErrorCode::ModelMissing,
            message: "x".into(),
            id: None,
        })
        .unwrap();
        assert_eq!(err["code"], "model_missing");
    }
}
