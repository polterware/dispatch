use crate::{
    backends::{self, MicSource, Speaker, VadFactory},
    config,
    engine::{EngineHandle, Input},
    models::ModelManager,
    rss,
};
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
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, mpsc};
use voice_core::{
    discovery::{self, EngineInfo},
    paths,
    protocol::*,
};

#[derive(Clone)]
struct App {
    token: Arc<String>,
    engine: EngineHandle,
    models: Arc<ModelManager>,
    stop: broadcast::Sender<()>,
}
#[derive(serde::Serialize)]
struct Status {
    state: EngineState,
    models: ModelSummary,
    memory_rss_bytes: u64,
}
#[derive(serde::Serialize)]
struct ModelSummary {
    stt: ModelStt,
    tts: ModelTts,
    vad: ModelVad,
}
#[derive(serde::Serialize)]
struct ModelStt {
    path: String,
    loaded: bool,
    bytes: Option<u64>,
}
#[derive(serde::Serialize)]
struct ModelTts {
    backend: String,
    loaded: bool,
}
#[derive(serde::Serialize)]
struct ModelVad {
    kind: String,
    loaded: bool,
}
fn authorized(headers: &HeaderMap, token: &str, query: Option<&str>) -> bool {
    let candidate = query.or_else(|| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
    });
    candidate.is_some_and(|v| {
        v.len() == token.len() && v.as_bytes().ct_eq(token.as_bytes()).unwrap_u8() == 1
    })
}
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    if origin == "null" || origin.starts_with("file://") {
        return true;
    }
    ["http://", "https://"].iter().any(|scheme| {
        origin.strip_prefix(scheme).is_some_and(|host| {
            host.split('/')
                .next()
                .unwrap_or("")
                .split(':')
                .next()
                .is_some_and(|h| h == "localhost" || h == "127.0.0.1")
        })
    })
}
async fn health(headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(Health {
        ok: true,
        protocol: PROTOCOL_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
    })
    .into_response()
}
async fn status(State(app): State<App>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authorized(&headers, &app.token, None) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (loaded_stt, loaded_tts, stt_path, backend) = app.models.status().await;
    let bytes = std::fs::metadata(&stt_path).ok().map(|m| m.len());
    let state = app.engine.state();
    Json(Status {
        state,
        models: ModelSummary {
            stt: ModelStt {
                path: stt_path,
                loaded: loaded_stt,
                bytes,
            },
            tts: ModelTts {
                backend,
                loaded: loaded_tts,
            },
            vad: ModelVad {
                kind: if paths::models_dir().join("vad/silero_vad.onnx").exists() {
                    "silero"
                } else {
                    "energy"
                }
                .into(),
                loaded: false,
            },
        },
        memory_rss_bytes: rss::resident_bytes(),
    })
    .into_response()
}
async fn shutdown(State(app): State<App>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authorized(&headers, &app.token, None) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let _ = app.stop.send(());
    Json(serde_json::json!({})).into_response()
}
async fn websocket(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authorized(&headers, &app.token, query.get("token").map(String::as_str)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.max_message_size(2 * 1024 * 1024)
        .max_frame_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| client(socket, app))
}
fn router(app: App) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/shutdown", post(shutdown))
        .route("/v1/ws", get(websocket))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(app)
}
async fn client(socket: WebSocket, app: App) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<EngineMessage>(64);
    let _ = app
        .engine
        .tx
        .send(Input::Connected {
            id,
            replies: tx.clone(),
        })
        .await;
    if let Some(hello) = rx.recv().await {
        if sink
            .send(Message::Text(
                serde_json::to_string(&hello).unwrap_or_default(),
            ))
            .await
            .is_err()
        {
            return;
        }
    } else {
        return;
    }
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink
                .send(Message::Text(
                    serde_json::to_string(&message).unwrap_or_default(),
                ))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Text(text) => {
                if text.len() > 1024 * 1024 {
                    let _ = tx.try_send(EngineMessage::Error {
                        code: ErrorCode::BadRequest,
                        message: "message too large".into(),
                        id: None,
                    });
                    continue;
                }
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(msg) => {
                        if let ClientMessage::Speak {
                            id: ref request_id,
                            text: ref body,
                            ..
                        } = msg
                        {
                            if body.len() > 64 * 1024 {
                                let _ = tx.try_send(EngineMessage::Error {
                                    code: ErrorCode::BadRequest,
                                    message: "speak text exceeds 64 KiB".into(),
                                    id: Some(request_id.clone()),
                                });
                                continue;
                            }
                        }
                        let _ = app.engine.tx.send(Input::Message(id, msg)).await;
                    }
                    Err(_) => {
                        let _ = tx.try_send(EngineMessage::Error {
                            code: ErrorCode::BadRequest,
                            message: "invalid message".into(),
                            id: None,
                        });
                    }
                }
            }
            Message::Binary(_) => {
                let _ = tx.try_send(EngineMessage::Error {
                    code: ErrorCode::BadRequest,
                    message: "binary messages are not supported".into(),
                    id: None,
                });
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = app.engine.tx.send(Input::Disconnected(id)).await;
    writer.abort();
}
pub async fn serve(port: Option<u16>, if_not_running: bool) -> anyhow::Result<()> {
    if let Some(info) = healthy() {
        if if_not_running {
            println!("{}", info.port);
            return Ok(());
        }
        anyhow::bail!("voice-engine is already running on port {}", info.port);
    }
    let cfg = config::load();
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port.unwrap_or(0))).await {
        Ok(l) => l,
        Err(e) => return Err(e.into()),
    };
    let port = listener.local_addr()?.port();
    let mut random = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut random);
    let token = random
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let file = paths::discovery_file();
    let info = EngineInfo {
        protocol: PROTOCOL_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        pid: std::process::id(),
        port,
        token: token.clone(),
    };
    discovery::write(&file, &info)?;
    let models = ModelManager::new(cfg.clone(), paths::models_dir());
    models.idle_unloader();
    let mic: Arc<dyn MicSource> = match std::env::var_os("VOICE_ENGINE_TEST_INPUT") {
        Some(wav) => {
            tracing::warn!("test input enabled: microphone replaced by {wav:?}");
            Arc::new(backends::WavMicSource(wav.into()))
        }
        None => Arc::new(backends::RealMicSource(cfg.input_device.clone())),
    };
    let speaker: Arc<dyn Speaker> = Arc::new(backends::RealSpeaker(cfg.output_device.clone()));
    let vad: Arc<dyn VadFactory> = Arc::new(backends::RealVadFactory {
        model: {
            let p = paths::models_dir().join("vad/silero_vad.onnx");
            p.exists().then_some(p)
        },
    });
    let engine = EngineHandle::spawn(cfg.clone(), mic, speaker, vad, models.clone());
    let (stop, _) = broadcast::channel(2);
    let app = App {
        token: Arc::new(token),
        engine: engine.clone(),
        models,
        stop: stop.clone(),
    };
    let router = router(app);
    let mut stopped = stop.subscribe();
    let idle = cfg.exit_after_idle_secs;
    let engine_for_idle = engine.clone();
    let result = tokio::select! {r=axum::serve(listener,router)=>r.map_err(anyhow::Error::from),_=stopped.recv()=>Ok(()),_=shutdown_signal()=>Ok(()),_=async move{if idle==0{std::future::pending::<()>().await;}let mut since:Option<Instant>=None;loop{tokio::time::sleep(Duration::from_secs(1)).await;let state=engine_for_idle.state();if state.clients==0&&state.listen_id.is_none()&&state.speak_id.is_none(){let start=since.get_or_insert_with(Instant::now);if start.elapsed()>=Duration::from_secs(idle){break;}}else{since=None;}}}=>Ok(())};
    let _ = engine.tx.send(Input::Shutdown).await;
    discovery::remove_if_owned(&file, std::process::id())?;
    result
}
fn healthy() -> Option<EngineInfo> {
    discovery::read(&paths::discovery_file()).ok().filter(|i| {
        ureq::get(&format!("{}/v1/health", i.base_url()))
            .call()
            .is_ok()
    })
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sig) = signal(SignalKind::terminate()) {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=sig.recv()=>{}}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
pub fn doctor() -> anyhow::Result<()> {
    let models = paths::models_dir();
    let cfg = config::load();
    println!("models: {}", models.display());
    let stt = config::select_stt(&cfg, &models);
    for p in [
        stt,
        models.join("kokoro/model_quantized.onnx"),
        models.join("vad/silero_vad.onnx"),
    ] {
        println!(
            "{}: {}",
            p.display(),
            std::fs::metadata(&p)
                .map(|m| format!("{} bytes", m.len()))
                .unwrap_or_else(|_| "missing".into())
        );
    }
    println!(
        "espeak-ng: {}",
        backends::espeak()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "missing".into())
    );
    println!("discovery: {}", paths::discovery_file().display());
    println!(
        "engine: {}",
        if healthy().is_some() {
            "running"
        } else {
            "not running"
        }
    );
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => {
            for device in devices {
                println!(
                    "input: {}",
                    device.name().unwrap_or_else(|_| "unknown device".into())
                );
            }
        }
        Err(error) => println!("input devices unavailable: {error}"),
    }
    match host.output_devices() {
        Ok(devices) => {
            for device in devices {
                println!(
                    "output: {}",
                    device.name().unwrap_or_else(|_| "unknown device".into())
                );
            }
        }
        Err(error) => println!("output devices unavailable: {error}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backends::{
            CaptureCallback, MicHandle, MicSource, Speaker, SpeakerHandle, Synthesizer,
            Transcriber, VadFactory,
        },
        config::Config,
    };
    use futures_util::{SinkExt, StreamExt};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    struct FakeMic;
    struct FakeMicHandle;
    impl MicHandle for FakeMicHandle {
        fn set_muted(&self, _: bool) {}
        fn stop(self: Box<Self>) {}
    }
    impl MicSource for FakeMic {
        fn start(&self, _: CaptureCallback) -> anyhow::Result<Box<dyn MicHandle>> {
            Ok(Box::new(FakeMicHandle))
        }
    }
    #[derive(Default)]
    struct SpeakerState {
        manual_end: AtomicBool,
    }
    struct FakeSpeaker(Arc<SpeakerState>);
    struct FakePlayback {
        callback: Mutex<Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>>,
        started: AtomicBool,
        manual_end: Arc<SpeakerState>,
    }
    impl FakePlayback {
        fn emit(&self, event: voice_audio::PlaybackEvent) {
            (self.callback.lock().unwrap())(event);
        }
    }
    impl SpeakerHandle for FakePlayback {
        fn push(&self, _: &[f32]) {
            if !self.started.swap(true, Ordering::Relaxed) {
                self.emit(voice_audio::PlaybackEvent::Started);
            }
            self.emit(voice_audio::PlaybackEvent::Level(voice_audio::Level {
                rms: 0.25,
                peak: 0.5,
            }));
        }
        fn finish(&self) {
            if !self.manual_end.manual_end.load(Ordering::Relaxed) {
                self.emit(voice_audio::PlaybackEvent::Ended { cancelled: false });
            }
        }
        fn cancel(&self) {
            self.emit(voice_audio::PlaybackEvent::Ended { cancelled: true });
        }
    }
    impl Speaker for FakeSpeaker {
        fn start(
            &self,
            _: u32,
            callback: Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>,
        ) -> anyhow::Result<Box<dyn SpeakerHandle>> {
            Ok(Box::new(FakePlayback {
                callback: Mutex::new(callback),
                started: AtomicBool::new(false),
                manual_end: self.0.clone(),
            }))
        }
    }
    struct FakeVad;
    impl VadFactory for FakeVad {
        fn create(&self) -> anyhow::Result<voice_stt::Vad> {
            Ok(voice_stt::Vad::energy(Default::default()))
        }
    }
    struct FakeTranscriber;
    impl Transcriber for FakeTranscriber {
        fn transcribe(
            &self,
            _: &[f32],
            language: Language,
        ) -> anyhow::Result<voice_stt::Transcript> {
            Ok(voice_stt::Transcript {
                text: "fake transcript".into(),
                language: if language == Language::Auto {
                    Language::PtBr
                } else {
                    language
                },
                inference: Duration::from_millis(7),
            })
        }
    }
    struct FakeSynth(Arc<Mutex<Vec<String>>>);
    impl Synthesizer for FakeSynth {
        fn sample_rate(&self) -> u32 {
            16000
        }
        fn synthesize(&self, sentence: &str, _: &voice_tts::VoiceSpec) -> anyhow::Result<Vec<f32>> {
            self.0.lock().unwrap().push(sentence.to_string());
            Ok(vec![0.1; 320])
        }
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    #[test]
    fn authorization_and_origin_policy() {
        let mut headers = HeaderMap::new();
        assert!(!authorized(&headers, "012345", None));
        headers.insert(header::AUTHORIZATION, "Bearer 012345".parse().unwrap());
        assert!(authorized(&headers, "012345", None));
        assert!(!authorized(&headers, "012346", None));
        for origin in [
            "null",
            "file://",
            "file:///tmp/app.html",
            "http://localhost:3000",
            "https://127.0.0.1:4321",
        ] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(origin_allowed(&headers), "{origin}");
        }
        for origin in [
            "https://example.com",
            "http://localhost.evil",
            "http://127.0.0.2:3000",
        ] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert!(!origin_allowed(&headers), "{origin}");
        }
        headers.remove(header::ORIGIN);
        assert!(origin_allowed(&headers));
    }

    #[test]
    fn discovery_file_uses_test_home_owner_permissions_and_is_removed() {
        let home = std::env::temp_dir().join(format!("voice-server-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let old_home = std::env::var_os("VOICE_ENGINE_HOME");
        std::env::set_var("VOICE_ENGINE_HOME", &home);
        let path = paths::discovery_file();
        let info = EngineInfo {
            protocol: PROTOCOL_VERSION,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            port: 49152,
            token: "fake-token".into(),
        };
        discovery::write(&path, &info).unwrap();
        assert_eq!(discovery::read(&path).unwrap(), info);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        discovery::remove_if_owned(&path, std::process::id()).unwrap();
        assert!(!path.exists());
        match old_home {
            Some(value) => std::env::set_var("VOICE_ENGINE_HOME", value),
            None => std::env::remove_var("VOICE_ENGINE_HOME"),
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn websocket_auth_hello_busy_and_cancel() {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(l) => l,
            Err(error) => {
                eprintln!("skipping WebSocket test: sandbox refused local bind: {error}");
                return;
            }
        };
        let addr = listener.local_addr().unwrap();
        let token = "test-token".to_string();
        let config = Config {
            partials: false,
            exit_after_idle_secs: 0,
            ..Config::default()
        };
        let models = ModelManager::new(config.clone(), paths::models_dir());
        let engine = EngineHandle::spawn(
            config,
            Arc::new(FakeMic),
            Arc::new(FakeSpeaker(Arc::new(SpeakerState::default()))),
            Arc::new(FakeVad),
            models,
        );
        let (stop, _) = broadcast::channel(1);
        let app = App {
            token: Arc::new(token.clone()),
            engine: engine.clone(),
            models: ModelManager::new(Config::default(), paths::models_dir()),
            stop,
        };
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router(app)).await;
        });
        let health_addr = addr;
        let health = tokio::task::spawn_blocking(move || {
            ureq::get(&format!("http://{health_addr}/v1/health")).call()
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(health.status(), 200);
        let endpoint = format!("ws://{addr}/v1/ws");
        let unauth = tokio_tungstenite::connect_async(&endpoint)
            .await
            .unwrap_err();
        assert!(
            matches!(unauth,tokio_tungstenite::tungstenite::Error::Http(ref response) if response.status()==StatusCode::UNAUTHORIZED)
        );
        let wrong_token = tokio_tungstenite::connect_async(format!("{endpoint}?token=wrong"))
            .await
            .unwrap_err();
        assert!(
            matches!(wrong_token,tokio_tungstenite::tungstenite::Error::Http(ref response) if response.status()==StatusCode::UNAUTHORIZED)
        );
        let mut foreign = endpoint.clone().into_client_request().unwrap();
        foreign
            .headers_mut()
            .insert(header::ORIGIN, "https://example.com".parse().unwrap());
        let denied = tokio_tungstenite::connect_async(foreign).await.unwrap_err();
        assert!(
            matches!(denied,tokio_tungstenite::tungstenite::Error::Http(ref response) if response.status()==StatusCode::FORBIDDEN)
        );
        let (url_a, resp_a) = tokio_tungstenite::connect_async(format!("{endpoint}?token={token}"))
            .await
            .unwrap();
        assert_eq!(resp_a.status(), StatusCode::SWITCHING_PROTOCOLS);
        let (url_b, _) = tokio_tungstenite::connect_async(format!("{endpoint}?token={token}"))
            .await
            .unwrap();
        let (mut a, mut b) = (url_a, url_b);
        for socket in [&mut a, &mut b] {
            let msg = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let parsed: EngineMessage = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            assert!(matches!(parsed, EngineMessage::Hello { .. }));
        }
        a.send(ClientMessage::Text(
            r#"{"type":"listen.start","id":"a","language":"pt-br"}"#.into(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine.state().listen_id.as_deref() != Some("a") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        b.send(ClientMessage::Text(
            r#"{"type":"listen.start","id":"b"}"#.into(),
        ))
        .await
        .unwrap();
        let busy = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = b.next().await.unwrap().unwrap();
                let parsed: EngineMessage = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                if matches!(
                    parsed,
                    EngineMessage::Error {
                        code: ErrorCode::Busy,
                        ..
                    }
                ) {
                    break parsed;
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            busy,
            EngineMessage::Error {
                code: ErrorCode::Busy,
                ..
            }
        ));
        a.send(ClientMessage::Text(
            r#"{"type":"listen.cancel","id":"a"}"#.into(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine.state().listen_id.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let no_final = tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                if let Some(Ok(msg)) = a.next().await {
                    if let Ok(parsed) =
                        serde_json::from_str::<EngineMessage>(msg.to_text().unwrap_or(""))
                    {
                        if matches!(parsed, EngineMessage::TranscriptFinal { .. }) {
                            break true;
                        }
                    }
                }
            }
        })
        .await;
        assert!(no_final.is_err());
        let _ = engine.tx.send(Input::Shutdown).await;
        server.abort();
    }

    #[tokio::test]
    async fn websocket_fake_transcription_vad_and_streamed_speech() {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(l) => l,
            Err(error) => {
                eprintln!(
                    "skipping model-backed WebSocket test: sandbox refused local bind: {error}"
                );
                return;
            }
        };
        let addr = listener.local_addr().unwrap();
        let token = "fake-test-token".to_string();
        let config = Config {
            partials: false,
            exit_after_idle_secs: 0,
            ..Config::default()
        };
        let models = ModelManager::new(config.clone(), paths::models_dir());
        models.install_test_stt(Arc::new(FakeTranscriber)).await;
        let synthesized = Arc::new(Mutex::new(Vec::new()));
        models
            .install_test_tts(Arc::new(FakeSynth(synthesized.clone())))
            .await;
        let speaker_state = Arc::new(SpeakerState::default());
        let engine = EngineHandle::spawn(
            config.clone(),
            Arc::new(FakeMic),
            Arc::new(FakeSpeaker(speaker_state.clone())),
            Arc::new(FakeVad),
            models.clone(),
        );
        let (stop, _) = broadcast::channel(1);
        let app = App {
            token: Arc::new(token.clone()),
            engine: engine.clone(),
            models,
            stop,
        };
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router(app)).await;
        });
        let endpoint = format!("ws://{addr}/v1/ws?token={token}");
        let (mut socket, _) = tokio_tungstenite::connect_async(endpoint).await.unwrap();
        let first = socket.next().await.unwrap().unwrap();
        assert!(matches!(
            serde_json::from_str::<EngineMessage>(first.to_text().unwrap()).unwrap(),
            EngineMessage::Hello { .. }
        ));
        socket
            .send(ClientMessage::Text(
                r#"{"type":"listen.start","id":"ptt","language":"pt-br","mode":"push-to-talk"}"#
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while engine.state().listen_id.as_deref() != Some("ptt") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        engine
            .tx
            .send(Input::MicChunk(vec![0.1; 16000]))
            .await
            .unwrap();
        socket
            .send(ClientMessage::Text(
                r#"{"type":"listen.stop","id":"ptt"}"#.into(),
            ))
            .await
            .unwrap();
        let final_ptt = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = socket.next().await.unwrap().unwrap();
                if let Ok(EngineMessage::TranscriptFinal {
                    id,
                    text,
                    language,
                    timings,
                }) = serde_json::from_str(msg.to_text().unwrap())
                {
                    if id == "ptt" {
                        break (text, language, timings);
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(final_ptt.0, "fake transcript");
        assert_eq!(final_ptt.1, Language::PtBr);
        assert_eq!(final_ptt.2.audio_ms, 1000);
        assert_eq!(final_ptt.2.inference_ms, 7);
        socket
            .send(ClientMessage::Text(
                r#"{"type":"listen.start","id":"hands","language":"pt-br","mode":"hands-free"}"#
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while engine.state().listen_id.as_deref() != Some("hands") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..8 {
            engine
                .tx
                .send(Input::MicChunk(vec![0.1; 512]))
                .await
                .unwrap();
        }
        for _ in 0..23 {
            engine
                .tx
                .send(Input::MicChunk(vec![0.0; 512]))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = socket.next().await.unwrap().unwrap();
                if let Ok(EngineMessage::TranscriptFinal { id, .. }) =
                    serde_json::from_str(msg.to_text().unwrap())
                {
                    if id == "hands" {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        socket.send(ClientMessage::Text(r#"{"type":"speak","id":"speech","text":"First sentence. Second sentence. Third sentence.","format":"plain","language":"en"}"#.into())).await.unwrap();
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(2),async{while !got.iter().any(|m|matches!(m,EngineMessage::SpeakEnded{id,reason:SpeakEndReason::Done} if id=="speech")){let msg=socket.next().await.unwrap().unwrap();if let Ok(parsed)=serde_json::from_str::<EngineMessage>(msg.to_text().unwrap()){got.push(parsed);}}}).await.unwrap();
        let started = got
            .iter()
            .position(|m| matches!(m,EngineMessage::SpeakStarted{id,..} if id=="speech"))
            .unwrap();
        let level = got
            .iter()
            .position(|m| {
                matches!(
                    m,
                    EngineMessage::Level {
                        source: LevelSource::Output,
                        ..
                    }
                )
            })
            .unwrap();
        let ended=got.iter().position(|m|matches!(m,EngineMessage::SpeakEnded{id,reason:SpeakEndReason::Done} if id=="speech")).unwrap();
        assert!(started < level && level < ended);
        assert_eq!(synthesized.lock().unwrap().len(), 3);
        for id in ["fifo-1", "fifo-2"] {
            socket.send(ClientMessage::Text(format!(r#"{{"type":"speak","id":"{id}","text":"One sentence.","format":"plain","language":"en"}}"#))).await.unwrap();
        }
        let fifo_ended = tokio::time::timeout(Duration::from_secs(2), async {
            let mut ended = Vec::new();
            while ended.len() < 2 {
                let msg = socket.next().await.unwrap().unwrap();
                if let Ok(EngineMessage::SpeakEnded {
                    id,
                    reason: SpeakEndReason::Done,
                }) = serde_json::from_str(msg.to_text().unwrap())
                {
                    if id.starts_with("fifo-") {
                        ended.push(id);
                    }
                }
            }
            ended
        })
        .await
        .unwrap();
        assert_eq!(fifo_ended, ["fifo-1", "fifo-2"]);
        speaker_state.manual_end.store(true, Ordering::Relaxed);
        socket.send(ClientMessage::Text(r#"{"type":"speak","id":"cancel-me","text":"Cancel this item.","format":"plain","language":"en"}"#.into())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while engine
                .state()
                .speak_id
                .as_deref()
                .is_none_or(|id| id != "cancel-me")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        socket
            .send(ClientMessage::Text(
                r#"{"type":"speak.cancel","id":"cancel-me"}"#.into(),
            ))
            .await
            .unwrap();
        let canceled = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = socket.next().await.unwrap().unwrap();
                if let Ok(
                    parsed @ EngineMessage::SpeakEnded {
                        reason: SpeakEndReason::Cancelled,
                        ..
                    },
                ) = serde_json::from_str(msg.to_text().unwrap())
                {
                    break parsed;
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(canceled,EngineMessage::SpeakEnded{id,..} if id=="cancel-me"));
        socket.send(ClientMessage::Text(r#"{"type":"speak","id":"barge-speak","text":"Wait here.","format":"plain","language":"en"}"#.into())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while engine.state().speak_id.as_deref() != Some("barge-speak") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        socket
            .send(ClientMessage::Text(
                r#"{"type":"listen.start","id":"barge-listen","language":"en"}"#.into(),
            ))
            .await
            .unwrap();
        let barge_end = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = socket.next().await.unwrap().unwrap();
                if let Ok(EngineMessage::SpeakEnded {
                    id,
                    reason: SpeakEndReason::Cancelled,
                }) = serde_json::from_str(msg.to_text().unwrap())
                {
                    if id == "barge-speak" {
                        break id;
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(barge_end, "barge-speak");
        socket
            .send(ClientMessage::Text(
                r#"{"type":"listen.cancel","id":"barge-listen"}"#.into(),
            ))
            .await
            .unwrap();
        let _ = engine.tx.send(Input::Shutdown).await;
        server.abort();
    }
}
