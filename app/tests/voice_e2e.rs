//! The voice engine client against a fake engine: a WebSocket server on 127.0.0.1 that answers
//! the handshake, the settings call, and the protocol's messages. It covers what the chat does
//! with voice: a final transcript reaching the composer, and a voiced run's reply being spoken
//! as the run ends.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use dispatch_app::client::DispatchRun;
use dispatch_app::voice::{EngineMessage, VoiceEngine, VoicedTurns, with_transcript};
use serde_json::{Value, json};

/// The fake engine's own home and discovery file, kept alive for the test.
struct Fake {
    discovery: PathBuf,
    data: PathBuf,
    _home: tempfile::TempDir,
    sent: Arc<Mutex<Vec<Value>>>,
    clients: Arc<Mutex<Vec<mpsc::Sender<Value>>>>,
}

impl Fake {
    fn start() -> Self {
        let home = tempfile::tempdir().expect("a temp folder");
        let data = home.path().join("data");
        std::fs::create_dir_all(&data).expect("the data folder");
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("the port").port();
        let discovery = home.path().join("engine.json");
        std::fs::write(
            &discovery,
            json!({
                "protocol": 1,
                "engine_version": "0.1.0",
                "pid": std::process::id(),
                "port": port,
                "token": "test-token",
            })
            .to_string(),
        )
        .expect("the discovery file");
        let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
        let clients: Arc<Mutex<Vec<mpsc::Sender<Value>>>> = Arc::default();
        let server_sent = sent.clone();
        let server_clients = clients.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let sent = server_sent.clone();
                let clients = server_clients.clone();
                std::thread::spawn(move || serve(stream, sent, clients));
            }
        });
        Self {
            discovery,
            data,
            _home: home,
            sent,
            clients,
        }
    }

    /// Sends `message` to every connected client.
    fn send(&self, message: Value) {
        let clients = self.clients.lock().expect("the clients");
        for client in clients.iter() {
            let _ = client.send(message.clone());
        }
    }

    /// Waits for a message the client sent that `matches`.
    fn wait_for(&self, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(found) = self
                .sent
                .lock()
                .expect("the sent messages")
                .iter()
                .find(|message| matches(message))
                .cloned()
            {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "the client sent no matching message"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn serve(
    mut stream: TcpStream,
    sent: Arc<Mutex<Vec<Value>>>,
    clients: Arc<Mutex<Vec<mpsc::Sender<Value>>>>,
) {
    let mut peek = [0_u8; 2048];
    let peeked = stream.peek(&mut peek).unwrap_or(0);
    let head = String::from_utf8_lossy(&peek[..peeked]);
    if !head.contains("/v1/ws") {
        // The client's settings call: answer it, then let the connection go.
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let body = json!({
            "language": "pt-br",
            "stt_model": "whisper-small-q5_1",
            "voice_pt": "piper:pt_BR-faber-medium",
            "voice_en": "kokoro:af_heart",
            "speed": 1.0,
            "partials": true,
            "model_idle_secs": 600,
            "exit_after_idle_secs": 1800,
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        return;
    }
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    let _ = socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(20)));
    let hello = json!({
        "type": "hello",
        "protocol": 1,
        "engine_version": "0.1.0",
        "state": {"mic": "idle", "stt": "unloaded", "tts": "unloaded", "clients": 1},
    });
    let _ = socket.send(tungstenite::Message::Text(hello.to_string()));
    let (tx, rx) = mpsc::channel();
    clients.lock().expect("the clients").push(tx);
    loop {
        while let Ok(message) = rx.try_recv() {
            if socket
                .send(tungstenite::Message::Text(message.to_string()))
                .is_err()
            {
                return;
            }
        }
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                if let Ok(message) = serde_json::from_str::<Value>(&text) {
                    sent.lock().expect("the sent messages").push(message);
                }
            }
            Ok(tungstenite::Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return,
        }
    }
}

/// Drains the client's events until one of `kind` arrives.
fn wait_event(engine: &VoiceEngine, kind: &str) -> EngineMessage {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for message in engine.drain() {
            let tag = match &message {
                EngineMessage::TranscriptFinal { .. } => "transcript.final",
                EngineMessage::TranscriptPartial { .. } => "transcript.partial",
                EngineMessage::SpeakEnded { .. } => "speak.ended",
                EngineMessage::SpeakStarted { .. } => "speak.started",
                EngineMessage::Error { .. } => "error",
                _ => "",
            };
            if tag == kind {
                return message;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the engine sent no {kind} to the client"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_final_transcript_reaches_the_composer_and_a_voiced_run_is_read() {
    let fake = Fake::start();
    let engine = VoiceEngine::at(fake.discovery.clone());
    let utterance = engine.listen_start();
    let start = fake.wait_for(|message| message["type"] == "listen.start");
    assert_eq!(start["id"], utterance);
    assert_eq!(start["language"], "pt-br");
    assert_eq!(start["mode"], "push-to-talk");
    engine.listen_stop(&utterance);
    fake.wait_for(|message| message["type"] == "listen.stop");

    fake.send(json!({
        "type": "transcript.final",
        "id": utterance,
        "text": " open the file ",
        "language": "pt-br",
        "timings": {"audio_ms": 10, "stop_to_final_ms": 5, "inference_ms": 5},
    }));
    let EngineMessage::TranscriptFinal { text, .. } = wait_event(&engine, "transcript.final")
    else {
        unreachable!()
    };
    assert_eq!(with_transcript("", &text), "open the file");
    assert_eq!(
        with_transcript("review this", &text),
        "review this open the file"
    );

    // A run started from that dictation is read aloud when it completes.
    let mut turns = VoicedTurns::default();
    turns.mark_voiced("run-1");
    let completed = DispatchRun {
        id: "run-1".into(),
        status: "completed".into(),
        ..DispatchRun::default()
    };
    assert!(turns.due(&completed));
    engine.speak_reply("run-1", "The file is open.\n\n```sh\nrm -rf /\n```");
    let speak = fake.wait_for(|message| message["type"] == "speak");
    assert_eq!(speak["id"], "reply-run-1");
    assert_eq!(speak["format"], "markdown");
    assert!(
        speak["text"]
            .as_str()
            .unwrap_or_default()
            .contains("The file is open")
    );
    turns.mark_spoken("run-1");
    assert!(!turns.due(&completed), "a turn is read once");
}

#[test]
fn reading_the_same_reply_again_stops_it() {
    let fake = Fake::start();
    let engine = VoiceEngine::at(fake.discovery.clone());
    engine.read("a reply");
    let speak = fake.wait_for(|message| message["type"] == "speak");
    assert_eq!(speak["format"], "markdown");
    assert!(
        speak["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("read-")
    );
    engine.read("a reply");
    fake.wait_for(|message| message["type"] == "speak.cancel");
}

/// The app's window with the fake engine installed: a tap on the microphone, and the engine's
/// final transcript lands in the composer.
#[gpui::test]
fn a_dictation_lands_in_the_composer(cx: &mut gpui::TestAppContext) {
    use dispatch_app::app::AppView;
    use gpui::Modifiers;

    let fake = Fake::start();
    // SAFETY: this test binary runs one test at a time through this environment variable.
    unsafe { std::env::set_var("PWDISPATCH_DATA_DIR", &fake.data) };
    cx.update(|cx| {
        cx.set_global(dispatch_app::theme::Theme::dark());
        polterware_ui::init(cx);
        dispatch_app::app::register_key_bindings(cx);
        cx.set_global(VoiceEngine::at(fake.discovery.clone()));
    });
    let (view, cx) = cx.add_window_view(AppView::new);
    cx.simulate_resize(gpui::size(gpui::px(1400.0), gpui::px(2000.0)));
    let mut bounds = None;
    for _ in 0..20 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        bounds = cx.debug_bounds("voice-mic");
        if bounds.is_some() {
            break;
        }
    }
    let bounds = bounds.expect("the composer's microphone");
    cx.simulate_click(bounds.center(), Modifiers::none());
    let start = fake.wait_for(|message| message["type"] == "listen.start");
    let utterance = start["id"].as_str().unwrap_or_default().to_string();
    fake.send(json!({
        "type": "transcript.final",
        "id": utterance,
        "text": "open the file",
        "language": "pt-br",
        "timings": {"audio_ms": 10, "stop_to_final_ms": 5, "inference_ms": 5},
    }));
    let mut text = String::new();
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(10));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        let page = view.read_with(cx, |view, _| view.run_page());
        text = page.read_with(cx, |page, app| page.composer_text(app));
        if !text.is_empty() {
            break;
        }
    }
    assert_eq!(text, "open the file");
}
