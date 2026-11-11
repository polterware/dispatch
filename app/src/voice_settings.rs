//! Settings › Voice's side of the local voice engine: its catalog, its settings, and its
//! models, the two chat switches this app keeps, and the little of the engine's WebSocket
//! this page needs (a download's progress, and the test phrase a voice speaks).
//!
//! The engine itself is `crate::voice`'s: the discovery file, the binary started on demand,
//! and the HTTP client are found there, and so is the only type in the app called
//! `VoiceEngine`. Everything here is blocking and is called on the app's background
//! executor.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use voice_core::catalog::{
    Catalog, CatalogEntry, EngineSettings, ModelKind, SettingsPatch, VoiceEntry,
};
use voice_core::discovery::EngineInfo;
pub use voice_core::protocol::Language;
use voice_core::protocol::{ClientMessage, EngineMessage, TextFormat};

use crate::client::resolve_data_dir;
use crate::state::{read_preference, write_preference};
use crate::voice::VoiceEngine;

/// The languages the engine hears and speaks, with the labels the page shows.
pub const LANGUAGES: [(Language, &str); 3] = [
    (Language::PtBr, "Portuguese (Brazil)"),
    (Language::En, "English"),
    (Language::Auto, "Detect per utterance"),
];

/// The engine's answer when its settings are being changed by another client.
const BUSY: &str = "The engine is busy. Try again.";

/// The id the test phrase speaks under, so its end is told apart from other speaks.
const TEST_SPEAK_ID: &str = "settings-test";

/// A language's label, or its code when nothing names it.
pub fn language_label(language: Language) -> String {
    LANGUAGES
        .iter()
        .find(|(value, _)| *value == language)
        .map_or_else(
            || language_code(language).to_string(),
            |(_, label)| (*label).to_string(),
        )
}

/// A language as the engine's wire names it.
fn language_code(language: Language) -> &'static str {
    match language {
        Language::PtBr => "pt-br",
        Language::En => "en",
        Language::Auto => "auto",
    }
}

/// The speaking speeds the page offers, in the engine's 0.5–2.0 range.
pub const SPEEDS: [f32; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

/// A speed as people read it: `1×`, `1.25×`.
pub fn speed_label(speed: f32) -> String {
    let text = format!("{speed:.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}×")
}

/// The voices that speak `language` (a piper or kokoro voice is one language's).
pub fn voices_for(catalog: &Catalog, language: Language) -> Vec<&VoiceEntry> {
    catalog
        .voices
        .iter()
        .filter(|voice| voice.language == language)
        .collect()
}

/// The model the catalog lists by `id`.
pub fn model<'a>(catalog: &'a Catalog, id: &str) -> Option<&'a CatalogEntry> {
    catalog.models.iter().find(|model| model.id == id)
}

/// The STT models settings may pick.
pub fn stt_models(catalog: &Catalog) -> Vec<&CatalogEntry> {
    catalog
        .models
        .iter()
        .filter(|model| model.kind == ModelKind::Stt)
        .collect()
}

/// The voice the settings speak a language with.
pub fn voice_for(settings: &EngineSettings, language: Language) -> &str {
    if language == Language::En {
        &settings.voice_en
    } else {
        &settings.voice_pt
    }
}

/// What the chat does with a voiced turn: the app's own, not the engine's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatVoice {
    /// Read a run's reply aloud when a voiced turn ends.
    pub read_replies_aloud: bool,
    /// Send a transcript as soon as the microphone stops.
    pub send_when_stop: bool,
}

/// The preference file these two switches are kept in.
pub const CHAT_PREFERENCE: &str = "voice-chat";

pub fn read_chat(data_dir: &Path) -> ChatVoice {
    read_preference::<ChatVoice>(data_dir, CHAT_PREFERENCE)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn write_chat(data_dir: &Path, chat: &ChatVoice) -> Result<(), String> {
    write_preference(data_dir, CHAT_PREFERENCE, chat)
}

/// The engine's failure in the page's words: its status alone says what happened for a token
/// it refuses, and `conflict` says what a 409 means for the call that got it.
fn failure(error: String, conflict: &str) -> String {
    match error.as_str() {
        "HTTP 401" | "HTTP 403" => {
            "The engine refused the discovery token. Restart it.".to_string()
        }
        "HTTP 409" if !conflict.is_empty() => conflict.to_string(),
        "HTTP 409" => BUSY.to_string(),
        _ => error,
    }
}

/// `PATCH /v1/settings`, answering the settings it stored.
pub fn patch(engine: &VoiceEngine, patch: &SettingsPatch) -> Result<EngineSettings, String> {
    engine
        .set_engine_settings(patch)
        .map_err(|error| failure(error, ""))
}

/// `DELETE /v1/models/{id}`; an in-use model answers 409, and the engine refuses it.
pub fn delete_model(engine: &VoiceEngine, id: &str) -> Result<(), String> {
    engine.remove_model(id).map_err(|error| {
        failure(
            error,
            "The engine is using this model. Pick another model or voice first.",
        )
    })
}

/// Speaks a test phrase and holds the socket until the engine says it ended.
pub fn speak(
    engine: &VoiceEngine,
    text: &str,
    language: Language,
    voice: &str,
) -> Result<(), String> {
    let info = engine
        .info()
        .ok_or_else(|| "The voice engine is not running.".to_string())?;
    let mut socket = Socket::connect(&info, Duration::from_secs(8))?;
    let message = ClientMessage::Speak {
        id: TEST_SPEAK_ID.to_string(),
        text: text.to_string(),
        format: TextFormat::Plain,
        language,
        voice: Some(voice.to_string()),
    };
    let body = serde_json::to_string(&message).map_err(|err| err.to_string())?;
    socket.send_text(&body)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let Some(message) = socket.read_message()? else {
            return Ok(());
        };
        if let Ok(EngineMessage::SpeakEnded { id, .. }) =
            serde_json::from_value::<EngineMessage>(message)
            && id == TEST_SPEAK_ID
        {
            return Ok(());
        }
    }
    Ok(())
}

/// A download's completion, as the WebSocket reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum Watched {
    Installed,
    Failed(String),
    /// The socket ended or never opened: the catalog is the only truth left.
    Unknown,
}

/// Live progress of the models downloading now, by id, 0 to 1. The page reads it while a
/// download runs and stops when the map empties.
pub type Progress = Arc<Mutex<BTreeMap<String, f64>>>;

pub fn new_progress() -> Progress {
    Arc::new(Mutex::new(BTreeMap::new()))
}

/// Asks the engine to download a model and follows its progress over a socket of its own
/// until the engine says it installed it, failed, or went quiet. A socket that never opens
/// leaves the engine downloading; the caller reloads the catalog to see.
pub fn download_and_watch(engine: &VoiceEngine, id: &str, progress: &Progress) -> Watched {
    if let Err(error) = engine.download_model(id) {
        return Watched::Failed(failure(error, "This model is already downloading."));
    }
    if let Ok(mut map) = progress.lock() {
        map.insert(id.to_string(), 0.0);
    }
    let Some(info) = engine.info() else {
        return Watched::Unknown;
    };
    let mut socket = match Socket::connect(&info, Duration::from_secs(10)) {
        Ok(socket) => socket,
        Err(_) => return Watched::Unknown,
    };
    let deadline = Instant::now() + Duration::from_secs(60 * 10);
    while Instant::now() < deadline {
        let message = match socket.read_message() {
            Ok(Some(message)) => message,
            _ => break,
        };
        let Ok(message) = serde_json::from_value::<EngineMessage>(message) else {
            continue;
        };
        match message {
            EngineMessage::ModelProgress {
                id: which,
                downloaded_bytes,
                total_bytes,
            } if which == id => {
                let fraction = if total_bytes == 0 {
                    0.0
                } else {
                    (downloaded_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0)
                };
                if let Ok(mut map) = progress.lock() {
                    map.insert(id.to_string(), fraction);
                }
            }
            EngineMessage::ModelInstalled { id: which } if which == id => {
                return Watched::Installed;
            }
            EngineMessage::ModelFailed { id: which, message } if which == id => {
                return Watched::Failed(message);
            }
            _ => {}
        }
    }
    Watched::Unknown
}

/// What the page found when it asked for the engine.
pub enum Loaded {
    Running(Box<VoiceData>),
    /// No discovery file: the engine is not running.
    Stopped,
    /// A discovery file, but no answer.
    Unreachable(String),
}

#[derive(Clone)]
pub struct VoiceData {
    pub engine: VoiceEngine,
    pub catalog: Catalog,
    pub settings: EngineSettings,
    pub chat: ChatVoice,
}

/// Reads the engine's catalog and settings, and the app's chat choices, in one blocking
/// call on the background executor.
pub fn load(engine: &VoiceEngine) -> Loaded {
    if engine.info().is_none() {
        return Loaded::Stopped;
    }
    let catalog = match engine.catalog().map_err(|error| failure(error, "")) {
        Ok(catalog) => catalog,
        Err(err) => return Loaded::Unreachable(err),
    };
    let settings = match engine.engine_settings().map_err(|error| failure(error, "")) {
        Ok(settings) => settings,
        Err(err) => return Loaded::Unreachable(err),
    };
    Loaded::Running(Box::new(VoiceData {
        engine: engine.clone(),
        catalog,
        settings,
        chat: read_chat(&resolve_data_dir()),
    }))
}

/// Which setting a voice menu changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceField {
    Language,
    VoicePt,
    VoiceEn,
    Speed,
    Stt,
}

impl VoiceField {
    /// Its anchor slot in the page, so each menu hangs from its own trigger.
    pub fn index(self) -> usize {
        match self {
            VoiceField::Language => 0,
            VoiceField::VoicePt => 1,
            VoiceField::VoiceEn => 2,
            VoiceField::Speed => 3,
            VoiceField::Stt => 4,
        }
    }

    /// The language whose voices it picks, if it picks one.
    pub fn voice_language(self) -> Option<Language> {
        match self {
            VoiceField::VoicePt => Some(Language::PtBr),
            VoiceField::VoiceEn => Some(Language::En),
            _ => None,
        }
    }
}

/// A byte count as people read it: `151 MB`, `1.3 GB`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes / MB)
    } else {
        format!("{:.0} KB", (bytes / KB).max(1.0))
    }
}

/// The phrase a voice speaks when Test is pressed: its own name, then a count, in the
/// language the settings are set to.
pub fn sample_text(voice_name: &str) -> String {
    format!("This is {voice_name}. Dispatch, one, two, three.")
}

/// A minimal WebSocket client for the engine's local socket: one text frame out, JSON
/// frames back. The chat's live socket is another client's business.
struct Socket {
    reader: BufReader<TcpStream>,
}

impl Socket {
    fn connect(info: &EngineInfo, timeout: Duration) -> Result<Self, String> {
        let url = info.base_url();
        let host = url.strip_prefix("http://").unwrap_or(&url).to_string();
        let addrs = host
            .to_socket_addrs()
            .map_err(|err| format!("The voice engine's address did not resolve: {err}"))?;
        let stream = addrs
            .filter_map(|addr| TcpStream::connect_timeout(&addr, timeout).ok())
            .next()
            .ok_or_else(|| "The voice engine did not accept the connection.".to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .map_err(|err| err.to_string())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|err| err.to_string())?;
        let request = format!(
            "GET /v1/ws?token={} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            info.token
        );
        let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
        writer
            .write_all(request.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|err| format!("The voice engine did not take the request: {err}"))?;
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader
            .read_line(&mut status)
            .map_err(|err| err.to_string())?;
        if !status.contains(" 101 ") {
            return Err("The voice engine refused the socket.".to_string());
        }
        let mut header = String::new();
        loop {
            header.clear();
            let read = reader
                .read_line(&mut header)
                .map_err(|err| err.to_string())?;
            if read == 0 || header == "\r\n" {
                break;
            }
        }
        Ok(Self { reader })
    }

    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let payload = text.as_bytes();
        let mask = [0x51u8, 0x7a, 0x2c, 0x11];
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x81);
        match payload.len() {
            len if len < 126 => frame.push(0x80 | len as u8),
            len if len <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(len as u16).to_be_bytes());
            }
            len => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(len as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ mask[index % 4]),
        );
        self.stream_write(&frame)
    }

    fn stream_write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let stream = self.reader.get_mut();
        stream
            .write_all(bytes)
            .and_then(|_| stream.flush())
            .map_err(|err| format!("The voice engine did not take the message: {err}"))
    }

    /// The next text message, `None` when the engine closes the socket.
    fn read_message(&mut self) -> Result<Option<Value>, String> {
        loop {
            let mut head = [0u8; 2];
            if !self.read_exact(&mut head)? {
                return Ok(None);
            }
            let opcode = head[0] & 0x0f;
            let masked = head[1] & 0x80 != 0;
            let mut length = u64::from(head[1] & 0x7f);
            if length == 126 {
                let mut extended = [0u8; 2];
                self.read_exact(&mut extended)?;
                length = u64::from(u16::from_be_bytes(extended));
            } else if length == 127 {
                let mut extended = [0u8; 8];
                self.read_exact(&mut extended)?;
                length = u64::from_be_bytes(extended);
            }
            if length > 2 * 1024 * 1024 {
                return Err("The voice engine sent an oversized message.".to_string());
            }
            let mut mask = [0u8; 4];
            if masked {
                self.read_exact(&mut mask)?;
            }
            let mut payload = vec![0u8; length as usize];
            self.read_exact(&mut payload)?;
            if masked {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
            }
            match opcode {
                0x1 => {
                    let text = String::from_utf8_lossy(&payload);
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        return Ok(Some(value));
                    }
                }
                0x8 => return Ok(None),
                0x9 => {
                    // A ping is answered with its payload as a pong.
                    let mut frame = Vec::with_capacity(payload.len() + 6);
                    frame.push(0x8a);
                    frame.push(0x80 | payload.len() as u8);
                    frame.extend_from_slice(&[0x51, 0x7a, 0x2c, 0x11]);
                    frame.extend(
                        payload
                            .iter()
                            .enumerate()
                            .map(|(index, byte)| byte ^ [0x51u8, 0x7a, 0x2c, 0x11][index % 4]),
                    );
                    self.stream_write(&frame)?;
                }
                _ => {}
            }
        }
    }

    /// Fills `buffer`; `Ok(false)` when the engine closed before a byte.
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<bool, String> {
        match self.reader.read_exact(buffer) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
            Err(err) => Err(format!("The voice engine's socket stopped: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, kind: ModelKind, installed: bool) -> CatalogEntry {
        CatalogEntry {
            id: id.to_string(),
            kind,
            backend: "whisper".to_string(),
            name: id.to_string(),
            description: String::new(),
            languages: vec![Language::En],
            size_bytes: 151 * 1024 * 1024,
            license: "MIT".to_string(),
            installed,
            active: false,
        }
    }

    fn voice(id: &str, language: Language) -> VoiceEntry {
        VoiceEntry {
            id: id.to_string(),
            backend: "kokoro".to_string(),
            name: id.to_string(),
            language,
            model_id: None,
            installed: true,
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            models: vec![
                entry("whisper-small", ModelKind::Stt, true),
                entry("kokoro-v1", ModelKind::Tts, false),
            ],
            voices: vec![
                voice("kokoro:af_heart", Language::En),
                voice("piper:pt_BR-faber-medium", Language::PtBr),
            ],
        }
    }

    #[test]
    fn the_catalog_lists_a_languages_voices_and_the_speech_models() {
        let catalog = catalog();
        let english: Vec<&str> = voices_for(&catalog, Language::En)
            .iter()
            .map(|voice| voice.id.as_str())
            .collect();
        assert_eq!(english, ["kokoro:af_heart"]);
        let speech: Vec<&str> = stt_models(&catalog)
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(speech, ["whisper-small"]);
        assert_eq!(
            model(&catalog, "kokoro-v1").map(|model| model.kind),
            Some(ModelKind::Tts)
        );
        assert!(model(&catalog, "missing").is_none());
    }

    #[test]
    fn a_language_speaks_with_its_own_voice_setting() {
        let settings = EngineSettings::default();
        assert_eq!(voice_for(&settings, Language::En), "kokoro:af_heart");
        assert_eq!(
            voice_for(&settings, Language::PtBr),
            "piper:pt_BR-faber-medium"
        );
        assert_eq!(
            voice_for(&settings, Language::Auto),
            "piper:pt_BR-faber-medium"
        );
        assert_eq!(language_label(Language::PtBr), "Portuguese (Brazil)");
        assert_eq!(language_label(Language::En), "English");
    }

    #[test]
    fn sizes_and_speeds_read_as_people_say_them() {
        assert_eq!(human_size(151 * 1024 * 1024), "151 MB");
        assert_eq!(human_size(1024), "1 KB");
        assert_eq!(speed_label(1.0), "1×");
        assert_eq!(speed_label(1.25), "1.25×");
        assert_eq!(speed_label(2.0), "2×");
    }
}
