//! Settings › Voice's client of the local voice engine: its discovery file and token, its
//! HTTP endpoints (catalog, models, settings), and the little of its WebSocket this page
//! needs (a download's progress, and the test phrase a voice speaks). Everything here is
//! blocking and is called on the app's background executor.
//!
//! The chat's own client (the live microphone and playback socket) is separate; this file
//! keeps to what these settings ask of the engine, so both can be written apart.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::resolve_data_dir;
use crate::state::{read_preference, write_preference};

/// The languages the engine hears and speaks. The wire values are `voice-core`'s
/// `Language` in kebab case.
pub const LANGUAGES: [(&str, &str); 3] = [
    ("pt-br", "Portuguese (Brazil)"),
    ("en", "English"),
    ("auto", "Detect per utterance"),
];

/// The language the settings start on, as the engine does.
pub const DEFAULT_LANGUAGE: &str = "pt-br";

/// The engine's answer when its settings are being changed by another client.
const BUSY: &str = "The engine is busy. Try again.";

/// A language's label, or its code when nothing names it.
pub fn language_label(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(value, _)| *value == code)
        .map_or_else(|| code.to_string(), |(_, label)| (*label).to_string())
}

/// The speaking speeds the page offers, in the engine's 0.5–2.0 range.
pub const SPEEDS: [f32; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

/// A speed as people read it: `1×`, `1.25×`.
pub fn speed_label(speed: f32) -> String {
    let text = format!("{speed:.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}×")
}

/// One downloadable model, as `GET /v1/catalog` lists it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    /// `stt`, `tts`, or `vad`.
    pub kind: String,
    pub backend: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub installed: bool,
    /// The current settings use it: Delete is refused.
    #[serde(default)]
    pub active: bool,
}

/// A voice the engine can speak with.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct VoiceEntry {
    pub id: String,
    #[serde(default)]
    pub backend: String,
    pub name: String,
    pub language: String,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub installed: bool,
}

/// `GET /v1/catalog`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub models: Vec<CatalogEntry>,
    #[serde(default)]
    pub voices: Vec<VoiceEntry>,
}

impl Catalog {
    /// The voices that speak `language` (a piper or kokoro voice is one language's).
    pub fn voices_for(&self, language: &str) -> Vec<&VoiceEntry> {
        self.voices
            .iter()
            .filter(|voice| voice.language == language)
            .collect()
    }

    /// The model a voice needs, when the catalog lists it.
    pub fn model(&self, id: &str) -> Option<&CatalogEntry> {
        self.models.iter().find(|model| model.id == id)
    }

    /// The STT models settings may pick.
    pub fn stt_models(&self) -> Vec<&CatalogEntry> {
        self.models
            .iter()
            .filter(|model| model.kind == "stt")
            .collect()
    }
}

/// `GET /v1/settings`: what every client shares, stored by the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct EngineSettings {
    pub language: String,
    pub stt_model: String,
    pub voice_pt: String,
    pub voice_en: String,
    pub speed: f32,
    /// Live partial transcripts while listening.
    pub partials: bool,
    pub model_idle_secs: u64,
    pub exit_after_idle_secs: u64,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            language: DEFAULT_LANGUAGE.to_string(),
            stt_model: "whisper-small-q5_1".to_string(),
            voice_pt: "piper:pt_BR-faber-medium".to_string(),
            voice_en: "kokoro:af_heart".to_string(),
            speed: 1.0,
            partials: true,
            model_idle_secs: 600,
            exit_after_idle_secs: 1800,
        }
    }
}

impl EngineSettings {
    /// The voice the settings speak a language with.
    pub fn voice(&self, language: &str) -> &str {
        if language == "en" {
            &self.voice_en
        } else {
            &self.voice_pt
        }
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

/// A running engine: where it answers and the token it was discovered with.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceEngine {
    pub base_url: String,
    token: String,
}

impl VoiceEngine {
    /// The engine the discovery file names, when it is there and readable.
    pub fn discover() -> Option<Self> {
        let raw = std::fs::read(discovery_file()).ok()?;
        let info: Value = serde_json::from_slice(&raw).ok()?;
        let port = info.get("port")?.as_u64()?;
        if port == 0 || port > u16::MAX as u64 {
            return None;
        }
        let token = info.get("token")?.as_str()?.to_string();
        if token.is_empty() {
            return None;
        }
        Some(Self {
            base_url: format!("http://127.0.0.1:{port}"),
            token,
        })
    }

    /// `GET /v1/catalog`, with each model's installed and active flags.
    pub fn catalog(&self) -> Result<Catalog, String> {
        let body = self.query("GET", "/v1/catalog", None, BUSY)?;
        serde_json::from_str(&body).map_err(|err| format!("The catalog did not parse: {err}"))
    }

    /// `GET /v1/settings`.
    pub fn settings(&self) -> Result<EngineSettings, String> {
        let body = self.query("GET", "/v1/settings", None, BUSY)?;
        serde_json::from_str(&body).map_err(|err| format!("The settings did not parse: {err}"))
    }

    /// `PATCH /v1/settings`, answering the settings it stored.
    pub fn patch(&self, patch: Value) -> Result<EngineSettings, String> {
        let body = self.query("PATCH", "/v1/settings", Some(patch), BUSY)?;
        serde_json::from_str(&body).map_err(|err| format!("The settings did not parse: {err}"))
    }

    /// `POST /v1/models/{id}/download`: the download runs in the engine.
    pub fn download(&self, id: &str) -> Result<(), String> {
        self.query(
            "POST",
            &format!("/v1/models/{}/download", encode(id)),
            None,
            "This model is already downloading.",
        )
        .map(|_| ())
    }

    /// `DELETE /v1/models/{id}`; an in-use model answers 409.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.query(
            "DELETE",
            &format!("/v1/models/{}", encode(id)),
            None,
            "The engine is using this model. Pick another model or voice first.",
        )
        .map(|_| ())
    }

    /// Speaks a test phrase and holds the socket until the engine says it ended.
    pub fn speak(&self, text: &str, language: &str, voice: &str) -> Result<(), String> {
        let mut socket = Socket::connect(self, Duration::from_secs(8))?;
        socket.send_text(
            &json!({
                "type": "speak",
                "id": "settings-test",
                "text": text,
                "format": "plain",
                "language": language,
                "voice": voice,
            })
            .to_string(),
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let Some(message) = socket.read_message()? else {
                return Ok(());
            };
            if message.get("type").and_then(Value::as_str) == Some("speak.ended")
                && message.get("id").and_then(Value::as_str) == Some("settings-test")
            {
                return Ok(());
            }
        }
        Ok(())
    }

    /// One HTTP call, answering its body; a status outside the 2xx range is an error.
    fn query(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        conflict: &str,
    ) -> Result<String, String> {
        let url = format!("{}{path}", self.base_url);
        let request = match method {
            "GET" => ureq::get(&url),
            "POST" => ureq::post(&url),
            "DELETE" => ureq::delete(&url),
            other => ureq::request(other, &url),
        }
        .set("authorization", &format!("Bearer {}", self.token))
        .timeout(Duration::from_secs(30));
        let result = match body {
            Some(body) => request
                .set("content-type", "application/json")
                .send_string(&body.to_string()),
            None => request.call(),
        };
        match result {
            Ok(response) => Ok(response.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(match (code, detail.trim()) {
                    (409, _) => conflict.to_string(),
                    (401, _) | (403, _) => {
                        "The engine refused the discovery token. Restart it.".to_string()
                    }
                    (_, "") => format!("The engine answered {code}."),
                    (_, detail) => detail.to_string(),
                })
            }
            Err(err) => Err(format!("The voice engine did not answer: {err}")),
        }
    }
}

/// Percent-encodes a model id for a path (ids are plain, so only `%`, `/`, `?`, and `#`
/// change).
fn encode(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Where the engine writes its discovery file, read as `voice_core::paths` computes it.
pub fn discovery_file() -> PathBuf {
    if let Some(root) = std::env::var_os("VOICE_ENGINE_HOME")
        && !root.is_empty()
    {
        return PathBuf::from(root).join("engine.json");
    }
    if !cfg!(target_os = "macos")
        && let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
        && !runtime.is_empty()
    {
        return PathBuf::from(runtime)
            .join("Polterware Dispatch")
            .join("voice")
            .join("engine.json");
    }
    resolve_data_dir().join("voice").join("engine.json")
}

/// Starts `dispatch-voice` detached, as the app starts its core, so the page's Start can
/// bring it up.
pub fn start_engine() -> Result<(), String> {
    let binary = engine_binary();
    std::process::Command::new(&binary)
        .args(["serve", "--if-not-running"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Could not start the voice engine: {err}"))
}

/// The engine's binary: the one the environment names, the bundle's resource, one beside
/// the app, else the one on the PATH.
fn engine_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("VOICE_ENGINE_BIN")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate in [
            dir.join("dispatch-voice"),
            dir.join("../Resources/dispatch-voice"),
            dir.join("Resources/dispatch-voice"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("dispatch-voice")
}

/// Waits for a just-started engine to write its discovery file and answer, up to about
/// twenty seconds.
pub fn wait_for_engine() -> Option<VoiceEngine> {
    for _ in 0..100 {
        if let Some(engine) = VoiceEngine::discover()
            && engine.catalog().is_ok()
        {
            return Some(engine);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
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
    if let Err(err) = engine.download(id) {
        return Watched::Failed(err);
    }
    if let Ok(mut map) = progress.lock() {
        map.insert(id.to_string(), 0.0);
    }
    let mut socket = match Socket::connect(engine, Duration::from_secs(10)) {
        Ok(socket) => socket,
        Err(_) => return Watched::Unknown,
    };
    let deadline = Instant::now() + Duration::from_secs(60 * 10);
    while Instant::now() < deadline {
        let message = match socket.read_message() {
            Ok(Some(message)) => message,
            _ => break,
        };
        let kind = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let which = message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if which != id {
            continue;
        }
        match kind {
            "model.progress" => {
                let done = message
                    .get("downloaded_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let total = message
                    .get("total_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let fraction = if total == 0 {
                    0.0
                } else {
                    (done as f64 / total as f64).clamp(0.0, 1.0)
                };
                if let Ok(mut map) = progress.lock() {
                    map.insert(id.to_string(), fraction);
                }
            }
            "model.installed" => return Watched::Installed,
            "model.failed" => {
                let reason = message
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The download failed.");
                return Watched::Failed(reason.to_string());
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
pub fn load() -> Loaded {
    let Some(engine) = VoiceEngine::discover() else {
        return Loaded::Stopped;
    };
    let catalog = match engine.catalog() {
        Ok(catalog) => catalog,
        Err(err) => return Loaded::Unreachable(err),
    };
    let settings = match engine.settings() {
        Ok(settings) => settings,
        Err(err) => return Loaded::Unreachable(err),
    };
    Loaded::Running(Box::new(VoiceData {
        engine,
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
    pub fn voice_language(self) -> Option<&'static str> {
        match self {
            VoiceField::VoicePt => Some("pt-br"),
            VoiceField::VoiceEn => Some("en"),
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
    fn connect(engine: &VoiceEngine, timeout: Duration) -> Result<Self, String> {
        let host = engine
            .base_url
            .strip_prefix("http://")
            .unwrap_or(&engine.base_url)
            .to_string();
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
            engine.token
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
