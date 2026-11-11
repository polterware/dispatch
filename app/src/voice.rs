//! The voice engine's client: discovery, an engine started on demand, one WebSocket, and the
//! per-app preferences voice reads.
//!
//! The engine is its own process (`dispatch-voice`, from `crates/voice-server`). It writes a
//! discovery file (`voice_core::paths::discovery_file()`) with its loopback port and a token,
//! and speaks protocol v1 over HTTP and one WebSocket (`docs/voice/protocol.md`).
//!
//! [`VoiceEngine`] is installed as a GPUI global ([`init`]) and cloned by the views. Nothing
//! connects until something is asked of it: the microphone tapped, a reply read aloud, or
//! Settings asking for the catalog. A background thread then finds the engine (or starts it
//! beside this binary, in `Contents/Resources`, or on `PATH`), connects, and keeps a queue of
//! outgoing messages flushed. Audio levels land in atomics read by a view's paint, never in an
//! entity notify; engine events wait in a queue that the run page drains on its tick.
//!
//! Settings › Voice is another view: it uses [`VoiceEngine::engine_settings`],
//! [`VoiceEngine::set_engine_settings`], [`VoiceEngine::catalog`], [`VoiceEngine::download_model`],
//! and [`VoiceEngine::remove_model`] for the engine's side, and [`VoiceEngine::prefs`] and
//! [`VoiceEngine::set_prefs`] for the two switches the app keeps.
//!
//! Calls are short, as in `client.rs`: a local engine answers at once, and one that is starting
//! is waited for only when the person asked for it ([`VoiceEngine::ensure_running`]).

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{App, Global};
use serde::{Deserialize, Serialize};

pub use voice_core::catalog::{Catalog, EngineSettings, SettingsPatch};
pub use voice_core::discovery::EngineInfo;
pub use voice_core::protocol::{
    ClientMessage, EngineMessage, EngineState, ErrorCode, Language, LevelSource, ListenMode,
    MicState, SpeakEndReason, TextFormat,
};
use voice_core::{discovery, paths};

use crate::client::{DispatchRun, resolve_data_dir};
use crate::state::{read_preference, write_preference};

/// The preference file holding voice's own switches (`gpui-preferences/voice.json`).
pub const VOICE_PREFERENCE: &str = "voice";
/// The longest the app waits for an engine it started to write its discovery file.
const START_WAIT: Duration = Duration::from_secs(20);
/// The least time between two engines started from here, so one that fails is not retried on
/// every tick.
const START_GAP: Duration = Duration::from_secs(30);
/// How long the connection thread waits for a frame or for something to send, in turn.
const TICK: Duration = Duration::from_millis(20);
/// A local call's budgets: a quick connect and a short read, as `client.rs` keeps them.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(4);
const WRITE_TIMEOUT: Duration = Duration::from_secs(4);
/// How many engine events wait for the page to drain them.
const EVENT_BACKLOG: usize = 256;

/// Voice's own switches, kept on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoicePrefs {
    /// Send the message as soon as the dictation ends.
    pub send_when_i_stop: bool,
    /// Read a completed voiced run's reply aloud.
    pub read_replies: bool,
}

impl Default for VoicePrefs {
    fn default() -> Self {
        Self {
            send_when_i_stop: false,
            read_replies: true,
        }
    }
}

/// The last few smoothed audio levels, in 0..=1. The engine sends raw RMS and peak at about
/// 30 Hz; these are smoothed here so a disc drawn from them moves rather than flickers.
#[derive(Default)]
struct Levels {
    mic: AtomicU32,
    output: AtomicU32,
}

impl Levels {
    fn read(&self, source: LevelSource) -> f32 {
        let level = match source {
            LevelSource::Mic => &self.mic,
            LevelSource::Output => &self.output,
        };
        f32::from_bits(level.load(Ordering::Relaxed))
    }

    fn store(&self, source: LevelSource, peak: f32) {
        let level = match source {
            LevelSource::Mic => &self.mic,
            LevelSource::Output => &self.output,
        };
        let previous = f32::from_bits(level.load(Ordering::Relaxed));
        let next = (previous * 0.7 + peak.clamp(0.0, 1.0) * 0.3).clamp(0.0, 1.0);
        level.store(next.to_bits(), Ordering::Relaxed);
    }
}

struct Inner {
    discovery: PathBuf,
    data_dir: PathBuf,
    /// Start `dispatch-voice` when none answers (off for a test's fake engine).
    start_engine: bool,
    /// Check `/v1/health` before connecting (off for a test's fake engine).
    health_check: bool,
    started: AtomicBool,
    connected: AtomicBool,
    /// The microphone is open, or is about to be: from the tap until the final transcript.
    listening: AtomicBool,
    speaking: AtomicBool,
    /// What this app is saying, so pressing Read again stops it.
    speaking_id: Mutex<Option<String>>,
    /// The utterance the microphone was opened for.
    listening_id: Mutex<Option<String>>,
    language: Mutex<Language>,
    outbox: Mutex<VecDeque<ClientMessage>>,
    wake: Condvar,
    events: Mutex<VecDeque<EngineMessage>>,
    levels: Levels,
    prefs: Mutex<VoicePrefs>,
    last_start: Mutex<Option<Instant>>,
}

impl Inner {
    fn push_event(&self, message: EngineMessage) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() >= EVENT_BACKLOG {
            events.pop_front();
        }
        events.push_back(message);
    }

    fn enqueue(&self, message: ClientMessage) {
        self.outbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(message);
        self.wake.notify_all();
    }

    fn next_message(&self) -> Option<ClientMessage> {
        self.outbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }
}

/// The engine client. Cloning is cheap: all clones share one connection and one queue.
#[derive(Clone)]
pub struct VoiceEngine {
    inner: Arc<Inner>,
}

impl Global for VoiceEngine {}

impl Default for VoiceEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceEngine {
    /// The app's engine: the discovery file under this machine's data folder.
    pub fn new() -> Self {
        Self::with(resolve_data_dir(), paths::discovery_file(), true, true)
    }

    /// An engine at an explicit discovery file, which is never started (a test's fake engine).
    pub fn at(discovery: PathBuf) -> Self {
        Self::with(std::env::temp_dir(), discovery, false, false)
    }

    fn with(data_dir: PathBuf, discovery: PathBuf, start_engine: bool, health_check: bool) -> Self {
        let prefs = read_preference::<VoicePrefs>(&data_dir, VOICE_PREFERENCE)
            .ok()
            .flatten()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                discovery,
                data_dir,
                start_engine,
                health_check,
                started: AtomicBool::new(false),
                connected: AtomicBool::new(false),
                listening: AtomicBool::new(false),
                speaking: AtomicBool::new(false),
                speaking_id: Mutex::new(None),
                listening_id: Mutex::new(None),
                language: Mutex::new(Language::default()),
                outbox: Mutex::new(VecDeque::new()),
                wake: Condvar::new(),
                events: Mutex::new(VecDeque::new()),
                levels: Levels::default(),
                prefs: Mutex::new(prefs),
                last_start: Mutex::new(None),
            }),
        }
    }

    /// Voice's own switches.
    pub fn prefs(&self) -> VoicePrefs {
        *self
            .inner
            .prefs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stores voice's switches and answers what was stored.
    pub fn set_prefs(&self, prefs: VoicePrefs) -> Result<VoicePrefs, String> {
        *self
            .inner
            .prefs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = prefs;
        write_preference(&self.inner.data_dir, VOICE_PREFERENCE, &prefs)?;
        Ok(prefs)
    }

    /// Whether the engine is connected right now.
    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::Relaxed)
    }

    /// Queues one message and makes sure the connection thread runs.
    fn send_command(&self, message: ClientMessage) {
        self.inner.enqueue(message);
        self.start_connection();
    }

    fn start_connection(&self) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("dispatch-voice".into())
            .spawn(move || connection_loop(inner))
            .ok();
    }

    /// Whether the microphone is open, or opening.
    pub fn is_listening(&self) -> bool {
        self.inner.listening.load(Ordering::Relaxed)
    }

    /// Whether a reply is being read aloud.
    pub fn is_speaking(&self) -> bool {
        self.inner.speaking.load(Ordering::Relaxed)
    }

    /// The smoothed level of the microphone or of playback, in 0..=1.
    pub fn level(&self, source: LevelSource) -> f32 {
        self.inner.levels.read(source)
    }

    /// The language the engine is configured for, as its settings said at connect.
    pub fn language(&self) -> Language {
        *self
            .inner
            .language
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opens the microphone in push-to-talk mode, with the engine's language. Answers the
    /// utterance's id, which its final transcript carries.
    pub fn listen_start(&self) -> String {
        let id = format!("utterance-{}", next_id());
        *self
            .inner
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id.clone());
        self.inner.listening.store(true, Ordering::SeqCst);
        self.send_command(ClientMessage::ListenStart {
            id: id.clone(),
            language: self.language(),
            mode: ListenMode::PushToTalk,
        });
        id
    }

    /// Opens the microphone hands-free: the engine ends the utterance when the speaking
    /// stops. Answers the utterance's id, which its final transcript carries.
    pub fn listen_start_hands_free(&self) -> String {
        let id = format!("utterance-{}", next_id());
        *self
            .inner
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id.clone());
        self.inner.listening.store(true, Ordering::SeqCst);
        self.send_command(ClientMessage::ListenStart {
            id: id.clone(),
            language: self.language(),
            mode: ListenMode::HandsFree,
        });
        id
    }

    /// Ends the utterance: the engine transcribes what it heard and answers
    /// `transcript.final`.
    pub fn listen_stop(&self, id: &str) {
        self.inner.listening.store(false, Ordering::SeqCst);
        self.send_command(ClientMessage::ListenStop { id: id.to_string() });
    }

    /// Drops the utterance without transcribing it.
    pub fn listen_cancel(&self, id: &str) {
        self.inner.listening.store(false, Ordering::SeqCst);
        *self
            .inner
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.send_command(ClientMessage::ListenCancel { id: id.to_string() });
    }

    /// Reads `text` aloud as a coding agent's markdown. Reading again stops it.
    pub fn read(&self, text: &str) {
        let id = read_id(text);
        let speaking = self
            .inner
            .speaking_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if speaking.as_deref() == Some(id.as_str()) {
            self.cancel_speaking();
            return;
        }
        self.cancel_speaking();
        self.speak(id, text, TextFormat::Markdown);
    }

    /// Reads a stored reply aloud, as a run's end does.
    pub fn speak_reply(&self, run_id: &str, text: &str) {
        let id = format!("reply-{run_id}");
        self.speak(id, text, TextFormat::Markdown);
    }

    /// Speaks `text`, with the engine's voice for its language.
    pub fn speak(&self, id: String, text: &str, format: TextFormat) {
        if text.trim().is_empty() {
            return;
        }
        *self
            .inner
            .speaking_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id.clone());
        self.inner.speaking.store(true, Ordering::SeqCst);
        self.send_command(ClientMessage::Speak {
            id,
            text: text.to_string(),
            format,
            language: self.language(),
            voice: None,
        });
    }

    /// Stops whatever is being read aloud.
    pub fn cancel_speaking(&self) {
        self.inner.speaking.store(false, Ordering::SeqCst);
        *self
            .inner
            .speaking_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.send_command(ClientMessage::SpeakCancel { id: None });
    }

    /// The engine messages that arrived since the last drain, oldest first.
    pub fn drain(&self) -> Vec<EngineMessage> {
        let mut events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        events.drain(..).collect()
    }

    /// Reads the discovery file of a running engine, without checking that it answers.
    pub fn info(&self) -> Option<EngineInfo> {
        discovery::read(&self.inner.discovery).ok()
    }

    /// Makes sure an engine answers, starting one when none does. It waits at most
    /// [`START_WAIT`], so call it from the background executor.
    pub fn ensure_running(&self) -> bool {
        if let Some(info) = self.info()
            && (!self.inner.health_check || healthy(&info))
        {
            self.start_connection();
            return true;
        }
        if !self.inner.start_engine {
            return false;
        }
        let Some(info) = start_engine(&self.inner.discovery, &self.inner.last_start) else {
            return false;
        };
        self.start_connection();
        !self.inner.health_check || healthy(&info)
    }

    /// The engine's settings (language, models, voice, speed).
    pub fn engine_settings(&self) -> Result<EngineSettings, String> {
        let body = self.request("GET", "/v1/settings", None)?;
        serde_json::from_value(body).map_err(|error| error.to_string())
    }

    /// Changes the engine's settings; absent fields keep their values.
    pub fn set_engine_settings(&self, patch: &SettingsPatch) -> Result<EngineSettings, String> {
        let body = serde_json::to_value(patch).map_err(|error| error.to_string())?;
        let answer = self.request("PATCH", "/v1/settings", Some(&body))?;
        serde_json::from_value(answer).map_err(|error| error.to_string())
    }

    /// Every model and voice the engine knows, with what is installed and in use.
    pub fn catalog(&self) -> Result<Catalog, String> {
        let body = self.request("GET", "/v1/catalog", None)?;
        serde_json::from_value(body).map_err(|error| error.to_string())
    }

    /// Asks the engine to download a model; progress arrives as `model.progress` events.
    pub fn download_model(&self, id: &str) -> Result<(), String> {
        self.request("POST", &format!("/v1/models/{id}/download"), None)?;
        Ok(())
    }

    /// Deletes an installed model; the engine refuses (409) while the settings use it.
    pub fn remove_model(&self, id: &str) -> Result<(), String> {
        self.request("DELETE", &format!("/v1/models/{id}"), None)?;
        Ok(())
    }

    /// The engine's status: its state, models, and memory.
    pub fn status(&self) -> Result<serde_json::Value, String> {
        self.request("GET", "/v1/status", None)
    }

    /// One authenticated call to the engine. Settings › Voice and future callers use it for
    /// endpoints this client has no typed wrapper for.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let info = self
            .info()
            .ok_or_else(|| "The voice engine is not running.".to_string())?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(READ_TIMEOUT)
            .timeout_write(WRITE_TIMEOUT)
            .build();
        let url = format!("{}{}", info.base_url(), path);
        let request = agent
            .request(method, &url)
            .set("Authorization", &format!("Bearer {}", info.token))
            .set("Content-Type", "application/json");
        let response = match body {
            Some(body) => request.send_string(&body.to_string()),
            None => request.call(),
        };
        match response {
            Ok(response) => {
                let text = response.into_string().map_err(|error| error.to_string())?;
                if text.trim().is_empty() {
                    return Ok(serde_json::Value::Null);
                }
                serde_json::from_str(&text).map_err(|error| error.to_string())
            }
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                let message = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("message")
                            .and_then(|message| message.as_str())
                            .map(str::to_string)
                    })
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or_else(|| format!("HTTP {code}"));
                Err(message)
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The app's engine, installed as a global the first time a view asks for it.
pub fn init(cx: &mut App) {
    engine(cx);
}

/// The app's engine. Every view reads the same one.
pub fn engine(cx: &mut App) -> VoiceEngine {
    if let Some(engine) = cx.try_global::<VoiceEngine>() {
        return engine.clone();
    }
    let engine = VoiceEngine::new();
    cx.set_global(engine.clone());
    engine
}

/// Reads `text` aloud on demand, when the app has an engine.
pub fn read_aloud(text: &str, cx: &mut App) {
    if let Some(engine) = cx.try_global::<VoiceEngine>() {
        engine.read(text);
    }
}

/// The composer's text with a finished dictation added after what is there.
pub fn with_transcript(existing: &str, said: &str) -> String {
    let said = said.trim();
    if said.is_empty() {
        return existing.to_string();
    }
    let existing = existing.trim_end();
    if existing.trim().is_empty() {
        said.to_string()
    } else {
        format!("{existing} {said}")
    }
}

/// The id a reply read from the transcript speaks under: the same words speak under the same
/// id, so pressing Read again stops them.
pub fn read_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("read-{hash:016x}")
}

/// Which runs were started from a dictation, and which of them have been read aloud: a turn
/// spoken into the composer is heard back once its run completes.
#[derive(Clone, Debug, Default)]
pub struct VoicedTurns {
    voiced: std::collections::HashSet<String>,
    spoken: std::collections::HashSet<String>,
}

impl VoicedTurns {
    /// Notes that `run_id` started from a dictation.
    pub fn mark_voiced(&mut self, run_id: &str) {
        if run_id.is_empty() {
            return;
        }
        self.voiced.insert(run_id.to_string());
    }

    /// Whether `run_id` started from a dictation.
    pub fn is_voiced(&self, run_id: &str) -> bool {
        self.voiced.contains(run_id)
    }

    /// Whether `run` is a voiced turn that completed and has not been read aloud yet.
    pub fn due(&self, run: &DispatchRun) -> bool {
        self.is_voiced(&run.id)
            && !self.spoken.contains(&run.id)
            && run.status.eq_ignore_ascii_case("completed")
    }

    /// Notes that `run_id`'s reply was read aloud (or would not be, since reading is off).
    pub fn mark_spoken(&mut self, run_id: &str) {
        self.spoken.insert(run_id.to_string());
    }

    /// Forgets runs the core no longer lists.
    pub fn retain(&mut self, runs: &[DispatchRun]) {
        let live: std::collections::HashSet<&str> =
            runs.iter().map(|run| run.id.as_str()).collect();
        self.voiced.retain(|id| live.contains(id.as_str()));
        self.spoken.retain(|id| live.contains(id.as_str()));
    }
}

/// A process-wide counter for utterance and speak ids.
fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn healthy(info: &EngineInfo) -> bool {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();
    let Ok(response) = agent.get(&format!("{}/v1/health", info.base_url())).call() else {
        return false;
    };
    let Ok(text) = response.into_string() else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("protocol").and_then(|protocol| protocol.as_u64()))
        == Some(u64::from(voice_core::protocol::PROTOCOL_VERSION))
}

/// The `dispatch-voice` binary: beside this executable, in a bundle's `Resources`, or on
/// `PATH`.
fn engine_binary() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    // `VOICE_ENGINE_BIN` names the engine first, as the voice repository's docs say.
    if let Some(path) = std::env::var_os("VOICE_ENGINE_BIN") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("dispatch-voice"));
        candidates.push(dir.join("../Resources/dispatch-voice"));
        candidates.push(dir.join("../../target/debug/dispatch-voice"));
        candidates.push(dir.join("../../target/release/dispatch-voice"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("dispatch-voice"));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// The last engine Settings › Voice started, which has no client of its own to keep the time
/// in.
static SETTINGS_START: Mutex<Option<Instant>> = Mutex::new(None);

/// The running engine's discovery, starting one when none answers (Settings › Voice's
/// Start), and waiting at most [`START_WAIT`] for it. Blocking: call it from the background
/// executor. It reads the same discovery file and starts the same binary as the app's engine,
/// without needing a [`VoiceEngine`].
pub(crate) fn ensure_engine() -> Result<EngineInfo, String> {
    let discovery = paths::discovery_file();
    if let Ok(info) = discovery::read(&discovery)
        && healthy(&info)
    {
        return Ok(info);
    }
    // Say why nothing could start: a missing binary is not an engine that went quiet.
    if engine_binary().is_none() {
        return Err(
            "The voice engine (dispatch-voice) is not next to the app or on PATH; set \
             VOICE_ENGINE_BIN to it, or build it with `cargo build -p voice-server`."
                .to_string(),
        );
    }
    let started_lately = SETTINGS_START
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some_and(|at| at.elapsed() < START_GAP);
    if started_lately {
        return Err("The voice engine was started moments ago. Try again shortly.".to_string());
    }
    start_engine(&discovery, &SETTINGS_START)
        .ok_or_else(|| "The voice engine did not answer after starting.".to_string())
}

/// Starts the engine (it exits at once when one already runs) and waits for its discovery
/// file.
fn start_engine(discovery_file: &Path, last_start: &Mutex<Option<Instant>>) -> Option<EngineInfo> {
    {
        let mut last = last_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < START_GAP) {
            return None;
        }
        *last = Some(Instant::now());
    }
    let binary = engine_binary()?;
    Command::new(binary)
        .args(["serve", "--if-not-running"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if let Ok(info) = discovery::read(discovery_file) {
            return Some(info);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn set_read_timeout(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) {
    if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
        let _ = stream.set_read_timeout(Some(TICK));
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    }
}

/// The one connection, kept up for as long as the app lives: it connects (starting the engine
/// when it must), sends what the views queued, and puts what the engine says in the event queue.
fn connection_loop(inner: Arc<Inner>) {
    let mut misses = 0_u32;
    loop {
        let info = match inner.ensure_discovery() {
            Some(info) => {
                misses = 0;
                info
            }
            None => {
                misses += 1;
                // A tap that cannot reach an engine says so instead of waiting for one.
                inner.fail_pending("The voice engine is not running.");
                // A fake engine that never came up (a test's) does not keep a thread awake.
                if !inner.start_engine && misses > 20 {
                    break;
                }
                inner.wait(Duration::from_millis(500));
                continue;
            }
        };
        let url = format!("ws://127.0.0.1:{}/v1/ws?token={}", info.port, info.token);
        match tungstenite::connect(&url) {
            Ok((mut socket, _)) => {
                set_read_timeout(&mut socket);
                inner.fetched_language(&info);
                inner.connected.store(true, Ordering::SeqCst);
                loop {
                    while let Some(message) = inner.next_message() {
                        let message = inner.with_language(message);
                        let Ok(text) = serde_json::to_string(&message) else {
                            continue;
                        };
                        if socket.send(tungstenite::Message::Text(text)).is_err() {
                            break;
                        }
                    }
                    match socket.read() {
                        Ok(tungstenite::Message::Text(text)) => inner.handle(&text),
                        Ok(tungstenite::Message::Ping(payload)) => {
                            let _ = socket.send(tungstenite::Message::Pong(payload));
                        }
                        Ok(tungstenite::Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(tungstenite::Error::Io(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            // The tick passed with nothing to read: send anything queued.
                        }
                        Err(_) => break,
                    }
                }
                inner.connected.store(false, Ordering::SeqCst);
                inner.disconnected();
            }
            Err(_) => {
                inner.connected.store(false, Ordering::SeqCst);
                inner.wait(Duration::from_millis(500));
            }
        }
    }
}

impl Inner {
    /// A queued message with the language the engine is configured for, which may be known
    /// only after connecting.
    fn with_language(&self, message: ClientMessage) -> ClientMessage {
        let language = *self
            .language
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match message {
            ClientMessage::ListenStart { id, mode, .. } => {
                ClientMessage::ListenStart { id, language, mode }
            }
            ClientMessage::Speak {
                id, text, format, ..
            } => ClientMessage::Speak {
                id,
                text,
                format,
                language,
                voice: None,
            },
            other => other,
        }
    }

    fn wait(&self, timeout: Duration) {
        let guard = self
            .outbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_empty() {
            let _ = self
                .wake
                .wait_timeout(guard, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn ensure_discovery(&self) -> Option<EngineInfo> {
        if let Ok(info) = discovery::read(&self.discovery)
            && (!self.health_check || healthy(&info))
        {
            return Some(info);
        }
        if !self.start_engine {
            return None;
        }
        start_engine(&self.discovery, &self.last_start)
    }

    fn fetched_language(&self, info: &EngineInfo) {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(READ_TIMEOUT)
            .build();
        let url = format!("{}/v1/settings", info.base_url());
        let Ok(response) = agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", info.token))
            .call()
        else {
            return;
        };
        let Ok(text) = response.into_string() else {
            return;
        };
        let Ok(settings) = serde_json::from_str::<EngineSettings>(&text) else {
            return;
        };
        *self
            .language
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = settings.language;
    }

    fn disconnected(&self) {
        self.listening.store(false, Ordering::SeqCst);
        self.speaking.store(false, Ordering::SeqCst);
        *self
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .speaking_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn handle(&self, text: &str) {
        let Ok(message) = serde_json::from_str::<EngineMessage>(text) else {
            return;
        };
        match &message {
            EngineMessage::Hello { state, .. } | EngineMessage::State { state } => {
                self.apply_state(state);
            }
            EngineMessage::Level { source, peak, .. } => {
                self.levels.store(*source, *peak);
                return;
            }
            EngineMessage::SettingsChanged { settings } => {
                *self
                    .language
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = settings.language;
            }
            EngineMessage::TranscriptFinal { id, .. } => {
                self.finish_utterance(id);
            }
            EngineMessage::SpeakEnded { id, .. } => {
                let mut speaking = self
                    .speaking_id
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if speaking.as_deref() == Some(id.as_str()) {
                    *speaking = None;
                    self.speaking.store(false, Ordering::SeqCst);
                }
            }
            EngineMessage::Error { id: Some(id), .. } => {
                self.finish_utterance(id);
            }
            _ => {}
        }
        self.push_event(message);
    }

    /// Follows the engine's microphone for this client's own utterance only: another
    /// client listening is not this composer's dictation, and a microphone the engine says is
    /// closed ends one this client thought was open.
    fn apply_state(&self, state: &EngineState) {
        let ours = self
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        let open = ours && state.mic == MicState::Listening;
        self.listening.store(open, Ordering::SeqCst);
    }

    fn finish_utterance(&self, id: &str) {
        let mut listening = self
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if listening.as_deref() == Some(id) {
            *listening = None;
            self.listening.store(false, Ordering::SeqCst);
        }
    }

    /// A request that cannot reach an engine ends as an error event, so the composer stops
    /// listening and the person reads why.
    fn fail_pending(&self, message: &str) {
        let utterance = self
            .listening_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(id) = utterance {
            self.listening.store(false, Ordering::SeqCst);
            self.push_event(EngineMessage::Error {
                code: ErrorCode::Internal,
                message: message.to_string(),
                id: Some(id),
            });
        }
        if self.speaking.swap(false, Ordering::SeqCst) {
            let id = self
                .speaking_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .unwrap_or_default();
            self.push_event(EngineMessage::SpeakEnded {
                id,
                reason: SpeakEndReason::Error,
            });
        }
    }
}

/// The notice a person reads for an engine error.
pub fn error_message(code: ErrorCode, message: &str) -> String {
    match code {
        ErrorCode::Busy => "The microphone is in use by another Dispatch conversation.".to_string(),
        ErrorCode::ModelMissing => {
            "The voice models are not downloaded. Open Settings › Voice to download them."
                .to_string()
        }
        ErrorCode::AudioDevice => "No microphone or speaker is available.".to_string(),
        ErrorCode::MicPermission => {
            "Dispatch needs microphone access. Allow it in System Settings › Privacy & Security › Microphone."
                .to_string()
        }
        ErrorCode::SttFailed => "The speech could not be transcribed.".to_string(),
        ErrorCode::TtsFailed => "The reply could not be read aloud.".to_string(),
        ErrorCode::BadRequest => {
            if message.trim().is_empty() {
                "The voice engine did not understand the request.".to_string()
            } else {
                message.to_string()
            }
        }
        ErrorCode::Internal => {
            if message.trim().is_empty() {
                "The voice engine failed.".to_string()
            } else {
                message.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, status: &str) -> DispatchRun {
        DispatchRun {
            id: id.to_string(),
            status: status.to_string(),
            ..DispatchRun::default()
        }
    }

    #[test]
    fn a_dictation_is_added_after_what_the_composer_holds() {
        assert_eq!(with_transcript("", " hello "), "hello");
        assert_eq!(
            with_transcript("take a look", "at this"),
            "take a look at this"
        );
        assert_eq!(with_transcript("one\n", "two"), "one two");
        assert_eq!(with_transcript("kept", "  "), "kept");
    }

    #[test]
    fn only_a_voiced_turn_that_completed_is_read_once() {
        let mut turns = VoicedTurns::default();
        turns.mark_voiced("a");
        assert!(turns.is_voiced("a"));
        assert!(!turns.due(&run("a", "running")));
        assert!(turns.due(&run("a", "COMPLETED")));
        assert!(!turns.due(&run("b", "completed")), "never voiced");
        turns.mark_spoken("a");
        assert!(!turns.due(&run("a", "completed")), "read once");
    }

    #[test]
    fn turns_forget_runs_the_core_no_longer_lists() {
        let mut turns = VoicedTurns::default();
        turns.mark_voiced("a");
        turns.mark_voiced("b");
        turns.mark_spoken("b");
        turns.retain(&[run("a", "completed")]);
        assert!(turns.is_voiced("a"));
        assert!(!turns.is_voiced("b"));
    }

    #[test]
    fn reading_the_same_words_again_has_the_same_id() {
        assert_eq!(read_id("hello"), read_id("hello"));
        assert_ne!(read_id("hello"), read_id("hello!"));
        assert!(read_id("hello").starts_with("read-"));
    }

    #[test]
    fn engine_errors_read_as_sentences() {
        assert!(error_message(ErrorCode::Busy, "").contains("microphone"));
        assert!(error_message(ErrorCode::MicPermission, "").contains("Privacy & Security"));
        assert_eq!(error_message(ErrorCode::Internal, "boom"), "boom");
    }
}
