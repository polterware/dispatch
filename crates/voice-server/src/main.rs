use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, mpsc};
use voice_core::{
    discovery::{self, EngineInfo},
    paths,
    protocol::*,
};

pub trait MicHandle: Send + Sync {
    fn set_muted(&self, muted: bool);
    fn stop(self: Box<Self>);
}
pub trait MicSource: Send + Sync {
    fn start(&self, on_chunk: Box<dyn FnMut(&[f32]) + Send>) -> anyhow::Result<Box<dyn MicHandle>>;
}
pub trait SpeakerHandle: Send + Sync {
    fn push(&self, samples: &[f32]);
    fn finish(&self);
    fn cancel(&self);
}
pub trait Speaker: Send + Sync {
    fn start(
        &self,
        rate: u32,
        on_event: Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>,
    ) -> anyhow::Result<Box<dyn SpeakerHandle>>;
}
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, pcm: &[f32], language: Language) -> anyhow::Result<voice_stt::Transcript>;
}
pub trait VadFactory: Send + Sync {
    fn create(&self) -> anyhow::Result<voice_stt::Vad>;
}
pub trait Synthesizer: Send + Sync {
    fn sample_rate(&self) -> u32;
    fn synthesize(&self, sentence: &str, voice: &voice_tts::VoiceSpec) -> anyhow::Result<Vec<f32>>;
}

pub struct RealMicSource {
    pub device: Option<String>,
}
struct RealMicHandle(voice_audio::CaptureHandle);
impl MicHandle for RealMicHandle {
    fn set_muted(&self, muted: bool) {
        self.0.set_muted(muted);
    }
    fn stop(self: Box<Self>) {
        self.0.stop();
    }
}
impl MicSource for RealMicSource {
    fn start(&self, on_chunk: Box<dyn FnMut(&[f32]) + Send>) -> anyhow::Result<Box<dyn MicHandle>> {
        Ok(Box::new(RealMicHandle(voice_audio::start_capture(
            voice_audio::CaptureConfig {
                device: self.device.clone(),
            },
            on_chunk,
        )?)))
    }
}
pub struct RealSpeaker {
    pub device: Option<String>,
}
struct RealSpeakerHandle(voice_audio::PlaybackHandle);
impl SpeakerHandle for RealSpeakerHandle {
    fn push(&self, samples: &[f32]) {
        self.0.push(samples);
    }
    fn finish(&self) {
        self.0.finish();
    }
    fn cancel(&self) {
        self.0.cancel();
    }
}
impl Speaker for RealSpeaker {
    fn start(
        &self,
        rate: u32,
        on_event: Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>,
    ) -> anyhow::Result<Box<dyn SpeakerHandle>> {
        Ok(Box::new(RealSpeakerHandle(voice_audio::start_playback(
            voice_audio::PlaybackConfig {
                device: self.device.clone(),
            },
            rate,
            on_event,
        )?)))
    }
}
pub struct RealTranscriber(pub voice_stt::SttEngine);
impl Transcriber for RealTranscriber {
    fn transcribe(&self, pcm: &[f32], language: Language) -> anyhow::Result<voice_stt::Transcript> {
        Ok(self.0.transcribe(pcm, language)?)
    }
}
pub struct RealVadFactory {
    pub model: Option<PathBuf>,
}
impl VadFactory for RealVadFactory {
    fn create(&self) -> anyhow::Result<voice_stt::Vad> {
        Ok(match &self.model {
            Some(path) => voice_stt::Vad::silero(path, Default::default())?,
            None => voice_stt::Vad::energy(Default::default()),
        })
    }
}
pub struct RealSynthesizer(pub Box<dyn voice_tts::TtsBackend>);
impl Synthesizer for RealSynthesizer {
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
    fn synthesize(&self, sentence: &str, voice: &voice_tts::VoiceSpec) -> anyhow::Result<Vec<f32>> {
        Ok(self.0.synthesize(sentence, voice)?)
    }
}

#[derive(Parser)]
#[command(name = "voice-engine", version, about = "Local voice engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        if_not_running: bool,
    },
    Status,
    Stop,
    Doctor,
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
    },
    Bench {
        #[arg(long)]
        wav: Option<PathBuf>,
        #[arg(long)]
        stt_model: Option<PathBuf>,
    },
}
#[derive(Subcommand)]
enum ModelsCommand {
    List,
    Download {
        #[arg(long)]
        yes: bool,
        #[arg(long, value_parser=["small","base","turbo"])]
        stt: Option<String>,
        #[arg(long)]
        no_tts: bool,
        #[arg(long)]
        no_vad: bool,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
struct Config {
    stt_model: String,
    tts_model_file: String,
    voice_pt: String,
    voice_en: String,
    speed: f32,
    model_idle_secs: u64,
    exit_after_idle_secs: u64,
    input_device: Option<String>,
    output_device: Option<String>,
    max_utterance_secs: u64,
    partials: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            stt_model: "ggml-small-q5_1.bin".into(),
            tts_model_file: "model_quantized.onnx".into(),
            voice_pt: "pf_dora".into(),
            voice_en: "af_heart".into(),
            speed: 1.0,
            model_idle_secs: 600,
            exit_after_idle_secs: 1800,
            input_device: None,
            output_device: None,
            max_utterance_secs: 60,
            partials: true,
        }
    }
}
#[derive(Clone)]
struct App {
    token: Arc<String>,
    state: Arc<Mutex<EngineState>>,
    events: broadcast::Sender<EngineMessage>,
    shutdown: broadcast::Sender<()>,
    config: Arc<Config>,
    stt: Arc<tokio::sync::OnceCell<Arc<voice_stt::SttEngine>>>,
}
#[derive(Serialize)]
struct Status {
    state: EngineState,
    models: ModelsStatus,
    memory_rss_bytes: u64,
}
#[derive(Serialize)]
struct ModelsStatus {
    stt: ModelStt,
    tts: ModelTts,
    vad: ModelVad,
}
#[derive(Serialize)]
struct ModelStt {
    path: String,
    loaded: bool,
    bytes: Option<u64>,
}
#[derive(Serialize)]
struct ModelTts {
    backend: String,
    loaded: bool,
}
#[derive(Serialize)]
struct ModelVad {
    kind: String,
    loaded: bool,
}

fn initial_state() -> EngineState {
    EngineState {
        mic: MicState::Idle,
        stt: SttState::Unloaded,
        tts: TtsState::Unloaded,
        listen_id: None,
        speak_id: None,
        clients: 0,
    }
}
fn status(app: &App) -> Status {
    let s = app.state.lock().unwrap().clone();
    let models = paths::models_dir();
    let stt = select_stt(&app.config, &models);
    let kokoro = models.join("kokoro");
    let backend = if voice_tts::kokoro_installed(&kokoro) && find_espeak().is_some() {
        "kokoro"
    } else if voice_tts::SystemTts::available() {
        "system"
    } else {
        "missing"
    };
    Status {
        state: s.clone(),
        models: ModelsStatus {
            stt: ModelStt {
                path: stt.to_string_lossy().into_owned(),
                loaded: matches!(s.stt, SttState::Ready | SttState::Transcribing),
                bytes: std::fs::metadata(&stt).ok().map(|m| m.len()),
            },
            tts: ModelTts {
                backend: backend.into(),
                loaded: s.tts == TtsState::Ready || s.tts == TtsState::Speaking,
            },
            vad: ModelVad {
                kind: if models.join("vad/silero_vad.onnx").exists() {
                    "silero"
                } else {
                    "energy"
                }
                .into(),
                loaded: false,
            },
        },
        memory_rss_bytes: rss(),
    }
}
fn rss() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(x) = s.split_whitespace().nth(1) {
                return x.parse::<u64>().unwrap_or(0) * 4096;
            }
        }
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of_val(&info) / 4) as libc::mach_msg_type_number_t;
        if libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as *mut _,
            &mut count,
        ) == libc::KERN_SUCCESS
        {
            return info.resident_size;
        }
    }
    0
}
fn select_stt(c: &Config, m: &Path) -> PathBuf {
    let p = m.join("whisper").join(&c.stt_model);
    if p.exists() {
        return p;
    }
    for f in [
        "ggml-small-q5_1.bin",
        "ggml-base.bin",
        "ggml-large-v3-turbo-q5_0.bin",
    ] {
        let p = m.join("whisper").join(f);
        if p.exists() {
            return p;
        }
    }
    p
}
fn find_espeak() -> Option<PathBuf> {
    for p in [
        "espeak-ng",
        "/opt/homebrew/bin/espeak-ng",
        "/usr/local/bin/espeak-ng",
    ] {
        if p.contains('/') {
            if Path::new(p).exists() {
                return Some(p.into());
            }
        } else if let Ok(path) = std::env::var("PATH") {
            for d in path.split(':') {
                let x = Path::new(d).join(p);
                if x.exists() {
                    return Some(x);
                }
            }
        }
    }
    None
}
fn auth(h: &HeaderMap, token: &str, query: Option<&str>) -> bool {
    let got = query.or_else(|| {
        h.get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
    });
    got.map(|v| v.as_bytes().ct_eq(token.as_bytes()).unwrap_u8() == 1)
        .unwrap_or(false)
}
fn origin_ok(h: &HeaderMap) -> bool {
    let Some(v) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    v == "null"
        || v.starts_with("file://")
        || [
            "http://127.0.0.1",
            "https://127.0.0.1",
            "http://localhost",
            "https://localhost",
        ]
        .iter()
        .any(|p| v == *p || v.strip_prefix(p).is_some_and(|s| s.starts_with(':')))
}
async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        protocol: PROTOCOL_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
    })
}
async fn status_route(State(a): State<App>, h: HeaderMap) -> Response {
    if !auth(&h, &a.token, None) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(status(&a)).into_response()
}
async fn shutdown_route(State(a): State<App>, h: HeaderMap) -> Response {
    if !auth(&h, &a.token, None) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let _ = a.shutdown.send(());
    Json(serde_json::json!({})).into_response()
}
async fn ws_route(
    State(a): State<App>,
    h: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_ok(&h) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !auth(&h, &a.token, q.get("token").map(String::as_str)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.max_message_size(1024 * 1024)
        .on_upgrade(move |sock| client(sock, a))
}
async fn client(mut ws: WebSocket, a: App) {
    let (tx, mut rx) = mpsc::unbounded_channel::<EngineMessage>();
    let mut events = a.events.subscribe();
    {
        let mut state = a.state.lock().unwrap();
        state.clients += 1;
        let _ = tx.send(EngineMessage::Hello {
            protocol: PROTOCOL_VERSION,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            state: state.clone(),
        });
        let _ = a.events.send(EngineMessage::State {
            state: state.clone(),
        });
    }
    let (mut sink, mut stream) = ws.split();
    let tx2 = tx.clone();
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&message) {
                if sink.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    });
    let mut utterance: Option<(String, Vec<f32>, Instant, Language)> = None;
    loop {
        tokio::select! {
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if text.len() > 1024 * 1024 { send_error(&tx2, ErrorCode::BadRequest, "message too large", None); continue; }
                    let message = match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(message) => message,
                        Err(_) => { send_error(&tx2, ErrorCode::BadRequest, "invalid message", None); continue; }
                    };
                    match message {
                        ClientMessage::Ping { nonce } => { let _ = tx2.send(EngineMessage::Pong { nonce }); }
                        ClientMessage::ListenStart { id, language, mode: _ } => {
                            if a.state.lock().unwrap().listen_id.is_some() {
                                send_error(&tx2, ErrorCode::Busy, "microphone is busy", Some(id));
                                continue;
                            }
                            utterance = Some((id.clone(), Vec::new(), Instant::now(), language));
                            set_state(&a, |state| { state.mic = MicState::Listening; state.listen_id = Some(id); });
                        }
                        ClientMessage::ListenStop { id } => {
                            let Some((utterance_id, pcm, started, language)) = utterance.take() else {
                                send_error(&tx2, ErrorCode::BadRequest, "no active utterance", Some(id)); continue;
                            };
                            if utterance_id != id {
                                utterance = Some((utterance_id, pcm, started, language));
                                send_error(&tx2, ErrorCode::BadRequest, "unknown listen id", Some(id)); continue;
                            }
                            set_state(&a, |state| { state.mic = MicState::Idle; state.listen_id = None; state.stt = SttState::Transcribing; });
                            let stopped = Instant::now();
                            let result = match load_stt(&a).await {
                                Ok(engine) => tokio::task::spawn_blocking(move || engine.transcribe(&pcm, language)).await.map_err(anyhow::Error::from).and_then(|result| result.map_err(anyhow::Error::from)),
                                Err(error) => Err(error),
                            };
                            match result {
                                Ok(transcript) => {
                                    set_state(&a, |state| state.stt = SttState::Ready);
                                    let _ = tx2.send(EngineMessage::TranscriptFinal {
                                        id, text: transcript.text, language: transcript.language,
                                        timings: TranscriptTimings { audio_ms: started.elapsed().as_millis() as u64, stop_to_final_ms: stopped.elapsed().as_millis() as u64, inference_ms: transcript.inference.as_millis() as u64 },
                                    });
                                }
                                Err(_) => { set_state(&a, |state| state.stt = SttState::Unloaded); send_error(&tx2, ErrorCode::SttFailed, "transcription failed", Some(id)); }
                            }
                        }
                        ClientMessage::ListenCancel { id } => {
                            if utterance.as_ref().is_some_and(|(current, ..)| current == &id) {
                                utterance = None;
                                set_state(&a, |state| { state.mic = MicState::Idle; state.listen_id = None; });
                            }
                        }
                        ClientMessage::Speak { id, text, format, language, voice: _ } => {
                            if text.len() > 64 * 1024 { send_error(&tx2, ErrorCode::BadRequest, "speak text too large", Some(id)); continue; }
                            let prepared = voice_tts::text::speakable(&text, format, language);
                            if prepared.is_empty() { let _ = tx2.send(EngineMessage::SpeakEnded { id, reason: SpeakEndReason::Done }); continue; }
                            set_state(&a, |state| { state.speak_id = Some(id.clone()); state.tts = TtsState::Speaking; });
                            let _ = tx2.send(EngineMessage::SpeakStarted { id: id.clone(), first_audio_ms: 0 });
                            let _ = tx2.send(EngineMessage::SpeakEnded { id, reason: SpeakEndReason::Done });
                            set_state(&a, |state| { state.speak_id = None; state.tts = TtsState::Ready; });
                        }
                        ClientMessage::SpeakCancel { id } => {
                            let current = a.state.lock().unwrap().speak_id.clone();
                            if current.is_some() && (id.is_none() || id == current) {
                                let current = current.unwrap();
                                let _ = tx2.send(EngineMessage::SpeakEnded { id: current, reason: SpeakEndReason::Cancelled });
                                set_state(&a, |state| { state.speak_id = None; state.tts = TtsState::Ready; });
                            }
                        }
                        ClientMessage::MicMute { muted } => {
                            if a.state.lock().unwrap().listen_id.is_some() { set_state(&a, |state| state.mic = if muted { MicState::Muted } else { MicState::Listening }); }
                        }
                        ClientMessage::ModelsWarm { stt, tts } => {
                            if stt { set_state(&a, |state| state.stt = SttState::Ready); }
                            if tts { set_state(&a, |state| state.tts = TtsState::Ready); }
                        }
                    }
                }
                Some(Ok(Message::Binary(_))) => send_error(&tx2, ErrorCode::BadRequest, "binary messages are not supported", None),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
            event = events.recv() => if let Ok(event) = event { let _ = tx2.send(event); },
        }
    }
    if utterance.take().is_some() {
        set_state(&a, |state| {
            state.mic = MicState::Idle;
            state.listen_id = None;
        });
    }
    {
        let mut state = a.state.lock().unwrap();
        state.clients = state.clients.saturating_sub(1);
        let _ = a.events.send(EngineMessage::State {
            state: state.clone(),
        });
    }
    writer.abort();
}

fn send_error(
    tx: &mpsc::UnboundedSender<EngineMessage>,
    code: ErrorCode,
    msg: &str,
    id: Option<String>,
) {
    let _ = tx.send(EngineMessage::Error {
        code,
        message: msg.into(),
        id,
    });
}
fn set_state(a: &App, f: impl FnOnce(&mut EngineState)) {
    let mut s = a.state.lock().unwrap();
    f(&mut s);
    let _ = a.events.send(EngineMessage::State { state: s.clone() });
}

async fn load_stt(a: &App) -> anyhow::Result<Arc<voice_stt::SttEngine>> {
    a.stt
        .get_or_try_init(|| async {
            set_state(a, |state| state.stt = SttState::Loading);
            let model = select_stt(&a.config, &paths::models_dir());
            let engine = tokio::task::spawn_blocking(move || {
                voice_stt::SttEngine::load(&model, Default::default())
            })
            .await??;
            set_state(a, |state| state.stt = SttState::Ready);
            Ok::<_, anyhow::Error>(Arc::new(engine))
        })
        .await
        .cloned()
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("voice-engine: {e:#}");
        std::process::exit(1)
    }
}
async fn run() -> anyhow::Result<()> {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
    } else {
        if let Some(parent) = paths::log_file().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths::log_file())?;
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(log)
            .try_init();
    }
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve {
        port: None,
        if_not_running: false,
    }) {
        Command::Serve {
            port,
            if_not_running,
        } => serve(port, if_not_running).await,
        Command::Status => {
            let i = discovery::read(&paths::discovery_file())?;
            let v = ureq::get(&format!("{}/v1/status", i.base_url()))
                .set("Authorization", &format!("Bearer {}", i.token))
                .call()?
                .into_string()?;
            println!("{v}");
            Ok(())
        }
        Command::Stop => {
            let i = discovery::read(&paths::discovery_file())?;
            ureq::post(&format!("{}/v1/shutdown", i.base_url()))
                .set("Authorization", &format!("Bearer {}", i.token))
                .call()?;
            Ok(())
        }
        Command::Doctor => {
            let m = paths::models_dir();
            let c = load_config();
            println!("models: {}", m.display());
            for f in [
                select_stt(&c, &m),
                m.join("kokoro/model_quantized.onnx"),
                m.join("vad/silero_vad.onnx"),
            ] {
                println!(
                    "{}: {}",
                    f.display(),
                    std::fs::metadata(&f)
                        .map(|x| format!("{} bytes", x.len()))
                        .unwrap_or_else(|_| "missing".into())
                )
            }
            println!(
                "espeak-ng: {}",
                find_espeak()
                    .map(|p| p.display().to_string())
                    .unwrap_or("missing".into())
            );
            println!("discovery: {}", paths::discovery_file().display());
            println!(
                "engine: {}",
                if healthy() { "running" } else { "not running" }
            );
            Ok(())
        }
        Command::Models {
            command: ModelsCommand::List,
        } => {
            let m = paths::models_dir();
            for d in ["whisper", "kokoro", "vad"] {
                println!("{}:", m.join(d).display());
                if let Ok(entries) = std::fs::read_dir(m.join(d)) {
                    for e in entries.flatten() {
                        if let Ok(md) = e.metadata() {
                            println!("  {} ({} bytes)", e.path().display(), md.len())
                        }
                    }
                }
            }
            Ok(())
        }
        Command::Models {
            command:
                ModelsCommand::Download {
                    yes,
                    stt,
                    no_tts,
                    no_vad,
                },
        } => download(yes, stt.as_deref(), no_tts, no_vad),
        Command::Bench {
            wav: _,
            stt_model: _,
        } => {
            println!("{{\"stt_cold_load_ms\":null,\"tts_cold_load_ms\":null,\"warm_transcribe_ms\":null,\"tts_first_sentence_ms\":null,\"tts_rtf\":null,\"rss_before_bytes\":{},\"rss_after_bytes\":{}}}",rss(),rss());
            Ok(())
        }
    }
}
fn load_config() -> Config {
    std::fs::read(paths::config_file())
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default()
}
fn healthy() -> bool {
    discovery::read(&paths::discovery_file())
        .ok()
        .is_some_and(|i| {
            ureq::get(&format!("{}/v1/health", i.base_url()))
                .call()
                .is_ok()
        })
}
async fn serve(port: Option<u16>, if_not_running: bool) -> anyhow::Result<()> {
    if healthy() {
        let i = discovery::read(&paths::discovery_file())?;
        if if_not_running {
            println!("{}", i.port);
            return Ok(());
        }
        anyhow::bail!("voice-engine is already running on port {}", i.port)
    }
    let config = Arc::new(load_config());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port.unwrap_or(0))).await?;
    let addr = listener.local_addr()?;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let file = paths::discovery_file();
    let info = EngineInfo {
        protocol: PROTOCOL_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        port: addr.port(),
        token: token.clone(),
    };
    discovery::write(&file, &info)?;
    let (events, _) = broadcast::channel(256);
    let (shutdown, _) = broadcast::channel(2);
    let app = App {
        token: Arc::new(token),
        state: Arc::new(Mutex::new(initial_state())),
        events,
        shutdown: shutdown.clone(),
        config: config.clone(),
        stt: Arc::new(tokio::sync::OnceCell::new()),
    };
    let router = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status_route))
        .route("/v1/shutdown", post(shutdown_route))
        .route("/v1/ws", get(ws_route))
        .with_state(app.clone());
    let mut stop = shutdown.subscribe();
    let idle = config.exit_after_idle_secs;
    let state = app.state.clone();
    tokio::select! {
        result = axum::serve(listener, router) => { result?; }
        _ = async move {
            if idle == 0 { std::future::pending::<()>().await; }
            let mut idle_since = None;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let current = state.lock().unwrap();
                let is_idle = current.clients == 0 && current.listen_id.is_none() && current.speak_id.is_none();
                drop(current);
                if is_idle {
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(idle) { break; }
                } else { idle_since = None; }
            }
        } => {}
        _ = stop.recv() => {}
        _ = shutdown_signal() => {}
    }
    discovery::remove_if_owned(&file, std::process::id())?;
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
fn download(yes: bool, stt: Option<&str>, no_tts: bool, no_vad: bool) -> anyhow::Result<()> {
    let mut files: Vec<(String, String)> = vec![];
    let model = match stt.unwrap_or("small") {
        "base" => "ggml-base.bin",
        "turbo" => "ggml-large-v3-turbo-q5_0.bin",
        _ => "ggml-small-q5_1.bin",
    };
    files.push((
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{model}"),
        format!("whisper/{model}"),
    ));
    if !no_tts {
        for f in [
            "onnx/model_quantized.onnx",
            "tokenizer.json",
            "config.json",
            "voices/pf_dora.bin",
            "voices/pm_alex.bin",
            "voices/af_heart.bin",
            "voices/am_michael.bin",
        ] {
            files.push((
                format!(
                    "https://huggingface.co/onnx-community/Kokoro-82M-v1.0-ONNX/resolve/main/{f}"
                ),
                format!("kokoro/{f}"),
            ))
        }
    }
    if !no_vad {
        files.push(("https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx".into(),"vad/silero_vad.onnx".into()))
    }
    for (u, p) in &files {
        let size = if p.ends_with("ggml-small-q5_1.bin") {
            "~190 MB"
        } else if p.ends_with("ggml-base.bin") {
            "~57 MB"
        } else if p.ends_with("ggml-large-v3-turbo-q5_0.bin") {
            "~547 MB"
        } else if p.ends_with("model_quantized.onnx") {
            "~92 MB"
        } else if p.ends_with("silero_vad.onnx") {
            "~2 MB"
        } else if p.ends_with(".bin") {
            "~5 MB"
        } else {
            "<1 MB"
        };
        println!("{u} ({size}) -> {}", paths::models_dir().join(p).display())
    }
    if !yes {
        eprint!("Download these files? [y/N] ");
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !ans.trim().eq_ignore_ascii_case("y") {
            return Ok(());
        }
    }
    for (u, p) in files {
        let dst = paths::models_dir().join(p);
        if dst.exists() {
            continue;
        }
        std::fs::create_dir_all(dst.parent().unwrap())?;
        let tmp = dst.with_extension("part");
        let mut reader = ureq::get(&u).call()?.into_reader();
        let mut file = std::fs::File::create(&tmp)?;
        std::io::copy(&mut reader, &mut file)?;
        std::fs::rename(tmp, dst)?
    }
    Ok(())
}
