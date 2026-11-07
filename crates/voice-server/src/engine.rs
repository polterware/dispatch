use crate::{
    backends::{MicHandle, MicSource, Speaker, SpeakerHandle, Synthesizer, VadFactory},
    config::Config,
    models::ModelManager,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc, watch};
use voice_core::protocol::*;

pub type ClientId = u64;
pub enum Input {
    Connected {
        id: ClientId,
        replies: mpsc::Sender<EngineMessage>,
    },
    Disconnected(ClientId),
    Message(ClientId, ClientMessage),
    MicChunk(Vec<f32>),
    Playback(PlaybackInput),
    Partial {
        generation: u64,
        id: String,
        owner: ClientId,
        text: String,
    },
    Transcribed {
        id: String,
        owner: ClientId,
        pcm_len: usize,
        stopped: Instant,
        result: anyhow::Result<voice_stt::Transcript>,
    },
    SttStarted(u64),
    Synthesized {
        generation: u64,
        id: String,
        samples: anyhow::Result<Vec<f32>>,
        final_chunk: bool,
    },
    FirstSynthesized {
        generation: u64,
        owner: ClientId,
        id: String,
        requested: Instant,
        sample_rate: u32,
        result: anyhow::Result<Vec<f32>>,
        remaining: VecDeque<String>,
        synthesizer: Option<Arc<dyn Synthesizer>>,
        spec: voice_tts::VoiceSpec,
    },
    AdvanceSpeak,
    ModelsWarmDone {
        owner: ClientId,
        stt: bool,
        tts: bool,
        error: Option<String>,
    },
    Shutdown,
}
#[derive(Clone, Copy)]
pub enum PlaybackInput {
    Event(u64, voice_audio::PlaybackEvent),
}
#[derive(Clone)]
pub struct EngineHandle {
    pub tx: mpsc::Sender<Input>,
    state: watch::Receiver<EngineState>,
}
impl EngineHandle {
    pub fn spawn(
        config: Config,
        mic: Arc<dyn MicSource>,
        speaker: Arc<dyn Speaker>,
        vad: Arc<dyn VadFactory>,
        models: Arc<ModelManager>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let (events, _) = broadcast::channel(256);
        let initial = EngineState {
            mic: MicState::Idle,
            stt: SttState::Unloaded,
            tts: TtsState::Unloaded,
            listen_id: None,
            speak_id: None,
            clients: 0,
        };
        let (state_tx, state) = watch::channel(initial);
        let actor = Actor::new(
            config,
            mic,
            speaker,
            vad,
            models,
            tx.clone(),
            events.clone(),
            state_tx,
        );
        tokio::spawn(actor.run(rx));
        Self { tx, state }
    }
    pub fn state(&self) -> EngineState {
        self.state.borrow().clone()
    }
}
struct Client {
    replies: mpsc::Sender<EngineMessage>,
}
struct Listening {
    owner: ClientId,
    id: String,
    language: Language,
    pcm: Vec<f32>,
    started: Instant,
    vad: Option<voice_stt::Vad>,
    speech_started: bool,
    generation: u64,
    muted: bool,
}
struct Speaking {
    owner: ClientId,
    id: String,
    generation: u64,
    requested: Instant,
    playback: Box<dyn SpeakerHandle>,
    started: bool,
}
struct SpeakRequest {
    owner: ClientId,
    id: String,
    sentences: VecDeque<String>,
    language: Language,
    voice: Option<String>,
    requested: Instant,
}
struct Actor {
    config: Config,
    mic_source: Arc<dyn MicSource>,
    speaker: Arc<dyn Speaker>,
    vad_factory: Arc<dyn VadFactory>,
    models: Arc<ModelManager>,
    tx: mpsc::Sender<Input>,
    events: broadcast::Sender<EngineMessage>,
    state_tx: watch::Sender<EngineState>,
    clients: HashMap<ClientId, Client>,
    state: EngineState,
    listening: Option<Listening>,
    speaking: Option<Speaking>,
    pending_speak: Option<(ClientId, String, u64)>,
    queue: VecDeque<SpeakRequest>,
    mic: Option<Box<dyn MicHandle>>,
    meter: voice_audio::LevelMeter,
    generation: AtomicU64,
    partial_at: Instant,
    partial_busy: bool,
    last_activity: Instant,
    last_state: EngineState,
}
impl Actor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: Config,
        mic_source: Arc<dyn MicSource>,
        speaker: Arc<dyn Speaker>,
        vad_factory: Arc<dyn VadFactory>,
        models: Arc<ModelManager>,
        tx: mpsc::Sender<Input>,
        events: broadcast::Sender<EngineMessage>,
        state_tx: watch::Sender<EngineState>,
    ) -> Self {
        let state = EngineState {
            mic: MicState::Idle,
            stt: SttState::Unloaded,
            tts: TtsState::Unloaded,
            listen_id: None,
            speak_id: None,
            clients: 0,
        };
        Self {
            config,
            mic_source,
            speaker,
            vad_factory,
            models,
            tx,
            events,
            state_tx,
            clients: HashMap::new(),
            state: state.clone(),
            listening: None,
            speaking: None,
            pending_speak: None,
            queue: VecDeque::new(),
            mic: None,
            meter: voice_audio::LevelMeter::new(voice_audio::CAPTURE_RATE),
            generation: AtomicU64::new(1),
            partial_at: Instant::now(),
            partial_busy: false,
            last_activity: Instant::now(),
            last_state: state,
        }
    }
    async fn run(mut self, mut rx: mpsc::Receiver<Input>) {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! { input=rx.recv()=>match input {Some(Input::Shutdown)|None=>break,Some(input)=>self.handle(input).await}, _=tick.tick()=>self.on_tick().await }
        }
        self.cancel_all();
    }
    async fn handle(&mut self, input: Input) {
        match input {
            Input::Connected { id, replies } => {
                self.clients.insert(id, Client { replies });
                self.state.clients = self.clients.len() as u32;
                self.send_to(
                    id,
                    EngineMessage::Hello {
                        protocol: PROTOCOL_VERSION,
                        engine_version: env!("CARGO_PKG_VERSION").into(),
                        state: self.state.clone(),
                    },
                );
                self.changed();
            }
            Input::Disconnected(id) => {
                self.clients.remove(&id);
                if self.listening.as_ref().is_some_and(|s| s.owner == id) {
                    self.cancel_listen(None);
                }
                self.cancel_client_speaks(id);
                self.state.clients = self.clients.len() as u32;
                self.last_activity = Instant::now();
                self.changed();
            }
            Input::Message(id, msg) => {
                self.last_activity = Instant::now();
                self.message(id, msg).await;
            }
            Input::MicChunk(chunk) => self.mic_chunk(chunk).await,
            Input::Playback(PlaybackInput::Event(g, event)) => self.playback(g, event),
            Input::Partial {
                generation,
                id,
                owner,
                text,
            } => {
                self.partial_busy = false;
                if !text.is_empty()
                    && self
                        .listening
                        .as_ref()
                        .is_some_and(|s| s.generation == generation && s.id == id)
                {
                    self.send_to(owner, EngineMessage::TranscriptPartial { id, text });
                }
            }
            Input::Transcribed {
                id,
                owner,
                pcm_len,
                stopped,
                result,
            } => {
                self.state.stt = SttState::Ready;
                match result {
                    Ok(t) => self.send_to(
                        owner,
                        EngineMessage::TranscriptFinal {
                            id,
                            text: t.text,
                            language: t.language,
                            timings: TranscriptTimings {
                                audio_ms: (pcm_len as u64 * 1000) / 16000,
                                stop_to_final_ms: stopped.elapsed().as_millis() as u64,
                                inference_ms: t.inference.as_millis() as u64,
                            },
                        },
                    ),
                    Err(e) => {
                        let detail = e.to_string();
                        let code = if detail.contains("model_missing") {
                            ErrorCode::ModelMissing
                        } else {
                            ErrorCode::SttFailed
                        };
                        self.error(owner, code, &detail, Some(id));
                        if code == ErrorCode::ModelMissing {
                            self.state.stt = SttState::Unloaded;
                        }
                    }
                }
                self.changed();
            }
            Input::SttStarted(generation) => {
                let _ = generation;
                self.state.stt = SttState::Transcribing;
                self.changed();
            }
            Input::Synthesized {
                generation,
                id,
                samples,
                final_chunk,
            } => {
                if self
                    .speaking
                    .as_ref()
                    .is_some_and(|s| s.generation == generation && s.id == id)
                {
                    match samples {
                        Ok(samples) => {
                            if let Some(s) = self.speaking.as_ref() {
                                s.playback.push(&samples);
                                if final_chunk {
                                    s.playback.finish();
                                }
                            }
                        }
                        Err(e) => {
                            self.error(
                                self.speaking.as_ref().unwrap().owner,
                                ErrorCode::TtsFailed,
                                &e.to_string(),
                                Some(id),
                            );
                            self.finish_speak(SpeakEndReason::Error);
                        }
                    }
                }
            }
            Input::AdvanceSpeak => self.start_next_speak(),
            Input::FirstSynthesized {
                generation,
                owner,
                id,
                requested,
                sample_rate,
                result,
                remaining,
                synthesizer,
                spec,
            } => self.first_synthesized(
                generation,
                owner,
                id,
                requested,
                sample_rate,
                result,
                remaining,
                synthesizer,
                spec,
            ),
            Input::ModelsWarmDone {
                owner,
                stt,
                tts,
                error,
            } => {
                if let Some(error) = error {
                    self.error(owner, ErrorCode::ModelMissing, &error, None);
                } else {
                    if stt {
                        self.state.stt = SttState::Ready;
                    }
                    if tts {
                        self.state.tts = TtsState::Ready;
                    }
                    self.changed();
                }
            }
            Input::Shutdown => self.cancel_all(),
        }
    }
    async fn message(&mut self, owner: ClientId, msg: ClientMessage) {
        match msg {
            ClientMessage::Ping { nonce } => self.send_to(owner, EngineMessage::Pong { nonce }),
            ClientMessage::ListenStart { id, language, mode } => {
                if self.listening.as_ref().is_some_and(|s| s.owner != owner) {
                    self.error(owner, ErrorCode::Busy, "microphone is busy", Some(id));
                    return;
                }
                if self.listening.is_some() {
                    self.cancel_listen(None);
                }
                self.cancel_playback();
                let vad = if mode == ListenMode::HandsFree {
                    match self.vad_factory.create() {
                        Ok(v) => Some(v),
                        Err(e) => {
                            self.error(owner, ErrorCode::AudioDevice, &e.to_string(), Some(id));
                            return;
                        }
                    }
                } else {
                    None
                };
                let tx = self.tx.clone();
                let mic = match self.mic_source.start(Box::new(move |samples| {
                    static DROPPED: AtomicU64 = AtomicU64::new(0);
                    if tx.try_send(Input::MicChunk(samples.to_vec())).is_err() {
                        let count = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
                        if count % 100 == 0 {
                            tracing::warn!(
                                dropped_chunks = count,
                                "capture chunks dropped under actor backpressure"
                            );
                        }
                    }
                })) {
                    Ok(m) => m,
                    Err(e) => {
                        let code = if e.to_string().to_lowercase().contains("permission") {
                            ErrorCode::MicPermission
                        } else {
                            ErrorCode::AudioDevice
                        };
                        self.error(owner, code, &e.to_string(), Some(id));
                        return;
                    }
                };
                let generation = self.generation.fetch_add(1, Ordering::Relaxed);
                self.mic = Some(mic);
                self.listening = Some(Listening {
                    owner,
                    id: id.clone(),
                    language,
                    pcm: Vec::new(),
                    started: Instant::now(),
                    vad,
                    speech_started: false,
                    generation,
                    muted: false,
                });
                self.state.mic = MicState::Listening;
                self.state.listen_id = Some(id);
                self.partial_at = Instant::now();
                self.changed();
            }
            ClientMessage::ListenStop { id } => self.stop_listen(owner, &id).await,
            ClientMessage::ListenCancel { id } => {
                if self
                    .listening
                    .as_ref()
                    .is_some_and(|s| s.owner == owner && s.id == id)
                {
                    self.cancel_listen(None)
                }
            }
            ClientMessage::MicMute { muted } => {
                if let Some(s) = self.listening.as_mut().filter(|s| s.owner == owner) {
                    s.muted = muted;
                    if let Some(m) = self.mic.as_ref() {
                        m.set_muted(muted);
                    }
                    self.state.mic = if muted {
                        MicState::Muted
                    } else {
                        MicState::Listening
                    };
                    self.changed();
                }
            }
            ClientMessage::ModelsWarm { stt, tts } => {
                let models = self.models.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let error = models.warm(stt, tts).await.err().map(|e| e.to_string());
                    let _ = tx
                        .send(Input::ModelsWarmDone {
                            owner,
                            stt,
                            tts,
                            error,
                        })
                        .await;
                });
            }
            ClientMessage::Speak {
                id,
                text,
                format,
                language,
                voice,
            } => {
                let received = Instant::now();
                if text.len() > 64 * 1024 {
                    self.error(
                        owner,
                        ErrorCode::BadRequest,
                        "speak text exceeds 64 KiB",
                        Some(id),
                    );
                    return;
                }
                let language = if language == Language::Auto {
                    voice_tts::text::detect_language(&text)
                } else {
                    language
                };
                let prepared = voice_tts::text::speakable(&text, format, language);
                let sentences = voice_tts::text::sentences(&prepared);
                if sentences.is_empty() {
                    self.send_to(
                        owner,
                        EngineMessage::SpeakEnded {
                            id,
                            reason: SpeakEndReason::Done,
                        },
                    );
                    return;
                }
                self.queue.push_back(SpeakRequest {
                    owner,
                    id,
                    sentences: sentences.into(),
                    language,
                    voice,
                    requested: received,
                });
                self.start_next_speak();
            }
            ClientMessage::SpeakCancel { id } => {
                if self
                    .speaking
                    .as_ref()
                    .is_some_and(|s| id.as_ref().is_none_or(|id| id == &s.id) && s.owner == owner)
                {
                    if let Some(s) = self.speaking.as_ref() {
                        s.playback.cancel();
                    }
                }
                if self
                    .pending_speak
                    .as_ref()
                    .is_some_and(|(owner_id, pending_id, _)| {
                        *owner_id == owner && id.as_ref().is_none_or(|wanted| wanted == pending_id)
                    })
                {
                    if let Some((pending_owner, pending_id, _)) = self.pending_speak.take() {
                        self.send_to(
                            pending_owner,
                            EngineMessage::SpeakEnded {
                                id: pending_id,
                                reason: SpeakEndReason::Cancelled,
                            },
                        );
                    }
                    self.state.speak_id = None;
                    self.state.tts = TtsState::Unloaded;
                    self.changed();
                }
                let mut canceled = Vec::new();
                self.queue.retain(|s| {
                    let keep = !(s.owner == owner && id.as_ref().is_none_or(|id| id == &s.id));
                    if !keep {
                        canceled.push((s.owner, s.id.clone()));
                    }
                    keep
                });
                for (o, id) in canceled {
                    self.send_to(
                        o,
                        EngineMessage::SpeakEnded {
                            id,
                            reason: SpeakEndReason::Cancelled,
                        },
                    );
                }
                self.start_next_speak();
            }
        }
    }
    fn start_next_speak(&mut self) {
        if self.speaking.is_some() || self.pending_speak.is_some() {
            return;
        }
        let Some(mut item) = self.queue.pop_front() else {
            return;
        };
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);
        let owner = item.owner;
        let id = item.id.clone();
        let requested = item.requested;
        let sentence = item.sentences.pop_front().unwrap_or_default();
        let remaining = item.sentences;
        let voice = item.voice.unwrap_or_else(|| {
            if item.language == Language::PtBr {
                self.config.voice_pt.clone()
            } else {
                self.config.voice_en.clone()
            }
        });
        let spec = voice_tts::VoiceSpec {
            language: item.language,
            name: Some(voice),
            speed: self.config.speed,
        };
        self.pending_speak = Some((owner, id.clone(), generation));
        self.state.tts = TtsState::Loading;
        self.state.speak_id = Some(id.clone());
        self.changed();
        let models = self.models.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let loaded = models.tts().await;
            let (synthesizer, sample_rate, result) = match loaded {
                Ok(synth) => {
                    let rate = synth.sample_rate();
                    let work_synth = synth.clone();
                    let work_spec = spec.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        work_synth.synthesize(&sentence, &work_spec)
                    })
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|x| x);
                    (Some(synth), rate, result)
                }
                Err(error) => (None, 0, Err(error)),
            };
            let _ = tx
                .send(Input::FirstSynthesized {
                    generation,
                    owner,
                    id,
                    requested,
                    sample_rate,
                    result,
                    remaining,
                    synthesizer,
                    spec,
                })
                .await;
        });
    }
    fn first_synthesized(
        &mut self,
        generation: u64,
        owner: ClientId,
        id: String,
        requested: Instant,
        sample_rate: u32,
        result: anyhow::Result<Vec<f32>>,
        remaining: VecDeque<String>,
        synthesizer: Option<Arc<dyn Synthesizer>>,
        spec: voice_tts::VoiceSpec,
    ) {
        if !self.pending_speak.as_ref().is_some_and(
            |(pending_owner, pending_id, pending_generation)| {
                *pending_owner == owner && pending_id == &id && *pending_generation == generation
            },
        ) {
            return;
        }
        self.pending_speak = None;
        let (samples, synthesizer) = match (result, synthesizer) {
            (Ok(samples), Some(synth)) => (samples, synth),
            (Err(error), _) => {
                self.error(
                    owner,
                    ErrorCode::TtsFailed,
                    &error.to_string(),
                    Some(id.clone()),
                );
                self.send_to(
                    owner,
                    EngineMessage::SpeakEnded {
                        id,
                        reason: SpeakEndReason::Error,
                    },
                );
                self.state.speak_id = None;
                self.state.tts = TtsState::Unloaded;
                self.changed();
                self.start_next_speak();
                return;
            }
            _ => return,
        };
        let tx = self.tx.clone();
        let callback = Box::new(move |event| {
            let _ = tx.try_send(Input::Playback(PlaybackInput::Event(generation, event)));
        });
        let playback = match self.speaker.start(sample_rate, callback) {
            Ok(p) => p,
            Err(error) => {
                self.error(
                    owner,
                    ErrorCode::AudioDevice,
                    &error.to_string(),
                    Some(id.clone()),
                );
                self.send_to(
                    owner,
                    EngineMessage::SpeakEnded {
                        id,
                        reason: SpeakEndReason::Error,
                    },
                );
                self.state.speak_id = None;
                self.state.tts = TtsState::Ready;
                self.changed();
                self.start_next_speak();
                return;
            }
        };
        playback.push(&samples);
        if remaining.is_empty() {
            playback.finish();
        } else {
            self.prefetch(id.clone(), remaining, generation, synthesizer, spec);
        }
        self.speaking = Some(Speaking {
            owner,
            id,
            generation,
            requested,
            playback,
            started: false,
        });
        self.state.tts = TtsState::Speaking;
        self.changed();
    }
    fn prefetch(
        &self,
        id: String,
        mut remaining: VecDeque<String>,
        generation: u64,
        synth: Arc<dyn Synthesizer>,
        spec: voice_tts::VoiceSpec,
    ) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(sentence) = remaining.pop_front() {
                let synth = synth.clone();
                let spec = spec.clone();
                let result =
                    tokio::task::spawn_blocking(move || synth.synthesize(&sentence, &spec))
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|x| x);
                let final_chunk = remaining.is_empty();
                if tx
                    .send(Input::Synthesized {
                        generation,
                        id: id.clone(),
                        samples: result,
                        final_chunk,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    async fn stop_listen(&mut self, owner: ClientId, id: &str) {
        if !self
            .listening
            .as_ref()
            .is_some_and(|s| s.owner == owner && s.id == id)
        {
            self.error(
                owner,
                ErrorCode::BadRequest,
                "no matching active utterance",
                Some(id.into()),
            );
            return;
        }
        let s = self.listening.take().unwrap();
        self.close_mic();
        self.state.mic = MicState::Idle;
        self.state.listen_id = None;
        self.state.stt = SttState::Loading;
        self.changed();
        let generation = s.generation;
        let tx = self.tx.clone();
        let models = self.models.clone();
        let id = s.id.clone();
        let language = s.language;
        let pcm_len = s.pcm.len();
        let stopped = Instant::now();
        tokio::spawn(async move {
            let res = match models.stt().await {
                Ok(engine) => {
                    let _ = tx.send(Input::SttStarted(generation)).await;
                    tokio::task::spawn_blocking(move || engine.transcribe(&s.pcm, language))
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|x| x)
                }
                Err(e) => Err(e),
            };
            let _ = tx
                .send(Input::Transcribed {
                    id,
                    owner,
                    pcm_len,
                    stopped,
                    result: res,
                })
                .await;
        });
    }
    async fn mic_chunk(&mut self, chunk: Vec<f32>) {
        if self.listening.is_none() {
            return;
        }
        for level in self.meter.push(&chunk) {
            self.send_all(EngineMessage::Level {
                source: LevelSource::Mic,
                rms: level.rms,
                peak: level.peak,
            });
        }
        let mut vad_message = None;
        let mut end = None;
        {
            let Some(s) = self.listening.as_mut() else {
                return;
            };
            s.pcm.extend_from_slice(&chunk);
            if let Some(vad) = s.vad.as_mut() {
                if let Some(event) = vad.push(&chunk) {
                    match event {
                        voice_stt::VadEvent::SpeechStart => {
                            s.speech_started = true;
                            vad_message = Some((s.id.clone(), true));
                        }
                        voice_stt::VadEvent::SpeechEnd if s.speech_started => {
                            vad_message = Some((s.id.clone(), false));
                            end = Some((s.owner, s.id.clone()));
                        }
                        _ => {}
                    }
                }
            }
            if s.started.elapsed() >= Duration::from_secs(self.config.max_utterance_secs) {
                end = Some((s.owner, s.id.clone()));
            }
        }
        if let Some((id, speech)) = vad_message {
            self.send_all(EngineMessage::Vad { id, speech });
        }
        if let Some((owner, id)) = end {
            self.stop_listen(owner, &id).await;
        }
    }
    async fn on_tick(&mut self) {
        let (stt_loaded, tts_loaded) = self.models.loaded_flags();
        if stt_loaded == Some(false) && self.state.stt == SttState::Ready {
            self.state.stt = SttState::Unloaded;
            self.changed();
        }
        if tts_loaded == Some(false) && self.state.tts == TtsState::Ready {
            self.state.tts = TtsState::Unloaded;
            self.changed();
        }
        if self.config.partials
            && self.partial_at.elapsed() >= Duration::from_millis(800)
            && !self.partial_busy
        {
            self.partial_at = Instant::now();
            if let Some(model) = self.models.stt_if_loaded().await {
                if let Some(s) = self.listening.as_ref() {
                    if !s.pcm.is_empty() {
                        let generation = s.generation;
                        let pcm = s.pcm.clone();
                        let lang = s.language;
                        let id = s.id.clone();
                        let owner = s.owner;
                        let tx = self.tx.clone();
                        self.partial_busy = true;
                        tokio::spawn(async move {
                            let model = model.clone();
                            let text =
                                tokio::task::spawn_blocking(move || model.transcribe(&pcm, lang))
                                    .await
                                    .ok()
                                    .and_then(Result::ok)
                                    .map(|t| t.text);
                            if let Some(text) = text {
                                let _ = tx
                                    .send(Input::Partial {
                                        generation,
                                        id,
                                        owner,
                                        text,
                                    })
                                    .await;
                            } else {
                                let _ = tx
                                    .send(Input::Partial {
                                        generation,
                                        id,
                                        owner,
                                        text: String::new(),
                                    })
                                    .await;
                            }
                        });
                    }
                }
            }
        }
        if self.config.exit_after_idle_secs > 0
            && self.clients.is_empty()
            && self.listening.is_none()
            && self.speaking.is_none()
            && self.last_activity.elapsed() >= Duration::from_secs(self.config.exit_after_idle_secs)
        {
            let _ = self.tx.send(Input::Shutdown).await;
        }
    }
    fn playback(&mut self, g: u64, event: voice_audio::PlaybackEvent) {
        match event {
            voice_audio::PlaybackEvent::Started => {
                let message = self
                    .speaking
                    .as_mut()
                    .filter(|s| s.generation == g)
                    .and_then(|s| {
                        if !s.started {
                            s.started = true;
                            Some((
                                s.owner,
                                EngineMessage::SpeakStarted {
                                    id: s.id.clone(),
                                    first_audio_ms: s.requested.elapsed().as_millis() as u64,
                                },
                            ))
                        } else {
                            None
                        }
                    });
                if let Some((owner, message)) = message {
                    self.send_to(owner, message);
                }
            }
            voice_audio::PlaybackEvent::Level(level) => self.send_all(EngineMessage::Level {
                source: LevelSource::Output,
                rms: level.rms,
                peak: level.peak,
            }),
            voice_audio::PlaybackEvent::Ended { cancelled } => {
                if self.speaking.as_ref().is_some_and(|s| s.generation == g) {
                    self.finish_speak(if cancelled {
                        SpeakEndReason::Cancelled
                    } else {
                        SpeakEndReason::Done
                    });
                }
            }
        }
    }
    fn finish_speak(&mut self, reason: SpeakEndReason) {
        if let Some(s) = self.speaking.take() {
            self.send_to(s.owner, EngineMessage::SpeakEnded { id: s.id, reason });
        }
        self.state.speak_id = None;
        self.state.tts = TtsState::Ready;
        self.changed();
        let _ = self.tx.try_send(Input::AdvanceSpeak);
    }
    fn cancel_playback(&mut self) {
        if let Some((owner, id, _)) = self.pending_speak.take() {
            self.send_to(
                owner,
                EngineMessage::SpeakEnded {
                    id,
                    reason: SpeakEndReason::Cancelled,
                },
            );
            self.state.speak_id = None;
            self.state.tts = TtsState::Unloaded;
            self.changed();
        }
        if let Some(s) = self.speaking.as_ref() {
            s.playback.cancel();
        }
        let canceled = self.queue.drain(..).collect::<Vec<_>>();
        for item in canceled {
            self.send_to(
                item.owner,
                EngineMessage::SpeakEnded {
                    id: item.id,
                    reason: SpeakEndReason::Cancelled,
                },
            );
        }
    }
    fn cancel_listen(&mut self, _: Option<String>) {
        self.listening = None;
        self.close_mic();
        self.state.mic = MicState::Idle;
        self.state.listen_id = None;
        self.changed();
    }
    fn close_mic(&mut self) {
        if let Some(m) = self.mic.take() {
            m.stop();
        }
    }
    fn cancel_client_speaks(&mut self, owner: ClientId) {
        if self
            .pending_speak
            .as_ref()
            .is_some_and(|(pending_owner, _, _)| *pending_owner == owner)
        {
            if let Some((pending_owner, id, _)) = self.pending_speak.take() {
                self.send_to(
                    pending_owner,
                    EngineMessage::SpeakEnded {
                        id,
                        reason: SpeakEndReason::Cancelled,
                    },
                );
            }
            self.state.speak_id = None;
            self.state.tts = TtsState::Unloaded;
            self.changed();
        }
        if self.speaking.as_ref().is_some_and(|s| s.owner == owner) {
            if let Some(s) = self.speaking.as_ref() {
                s.playback.cancel();
            }
        }
        let mut canceled = Vec::new();
        self.queue.retain(|s| {
            if s.owner == owner {
                canceled.push(s.id.clone());
                false
            } else {
                true
            }
        });
        for id in canceled {
            self.send_to(
                owner,
                EngineMessage::SpeakEnded {
                    id,
                    reason: SpeakEndReason::Cancelled,
                },
            );
        }
    }
    fn cancel_all(&mut self) {
        self.cancel_listen(None);
        self.cancel_playback();
    }
    fn send_to(&self, id: ClientId, message: EngineMessage) {
        if let Some(c) = self.clients.get(&id) {
            let _ = c.replies.try_send(message);
        }
    }
    fn send_all(&self, message: EngineMessage) {
        let _ = self.events.send(message.clone());
        for client in self.clients.values() {
            let _ = client.replies.try_send(message.clone());
        }
    }
    fn error(&self, id: ClientId, code: ErrorCode, msg: &str, request: Option<String>) {
        self.send_to(
            id,
            EngineMessage::Error {
                code,
                message: msg.into(),
                id: request,
            },
        );
    }
    fn changed(&mut self) {
        if self.state != self.last_state {
            self.last_state = self.state.clone();
            self.state_tx.send_replace(self.state.clone());
            self.send_all(EngineMessage::State {
                state: self.state.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backends::{MicHandle, MicSource, Speaker, SpeakerHandle, VadFactory},
        config::Config,
        models::ModelManager,
    };

    struct FakeMic;
    struct FakeMicHandle;
    impl MicHandle for FakeMicHandle {
        fn set_muted(&self, _: bool) {}
        fn stop(self: Box<Self>) {}
    }
    impl MicSource for FakeMic {
        fn start(&self, _: crate::backends::CaptureCallback) -> anyhow::Result<Box<dyn MicHandle>> {
            Ok(Box::new(FakeMicHandle))
        }
    }
    struct FakeSpeaker;
    struct FakePlayback;
    impl SpeakerHandle for FakePlayback {
        fn push(&self, _: &[f32]) {}
        fn finish(&self) {}
        fn cancel(&self) {}
    }
    impl Speaker for FakeSpeaker {
        fn start(
            &self,
            _: u32,
            _: Box<dyn FnMut(voice_audio::PlaybackEvent) + Send>,
        ) -> anyhow::Result<Box<dyn SpeakerHandle>> {
            Ok(Box::new(FakePlayback))
        }
    }
    struct FakeVad;
    impl VadFactory for FakeVad {
        fn create(&self) -> anyhow::Result<voice_stt::Vad> {
            Ok(voice_stt::Vad::energy(Default::default()))
        }
    }

    #[tokio::test]
    async fn busy_listener_and_cancel_are_actor_owned() {
        let root = std::env::temp_dir().join(format!("voice-server-test-{}", std::process::id()));
        let models = ModelManager::new(Config::default(), root.clone());
        let handle = EngineHandle::spawn(
            Config {
                partials: false,
                exit_after_idle_secs: 0,
                ..Config::default()
            },
            Arc::new(FakeMic),
            Arc::new(FakeSpeaker),
            Arc::new(FakeVad),
            models,
        );
        let (tx1, mut rx1) = mpsc::channel(16);
        let (tx2, mut rx2) = mpsc::channel(16);
        handle
            .tx
            .send(Input::Connected {
                id: 1,
                replies: tx1,
            })
            .await
            .unwrap();
        assert!(matches!(
            rx1.recv().await,
            Some(EngineMessage::Hello { .. })
        ));
        handle
            .tx
            .send(Input::Connected {
                id: 2,
                replies: tx2,
            })
            .await
            .unwrap();
        assert!(matches!(
            rx2.recv().await,
            Some(EngineMessage::Hello { .. })
        ));
        handle
            .tx
            .send(Input::Message(
                1,
                ClientMessage::ListenStart {
                    id: "a".into(),
                    language: Language::PtBr,
                    mode: ListenMode::PushToTalk,
                },
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while handle.state().listen_id.as_deref() != Some("a") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        handle
            .tx
            .send(Input::Message(
                2,
                ClientMessage::ListenStart {
                    id: "b".into(),
                    language: Language::Auto,
                    mode: ListenMode::PushToTalk,
                },
            ))
            .await
            .unwrap();
        let busy = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    rx2.recv().await,
                    Some(EngineMessage::Error {
                        code: ErrorCode::Busy,
                        ..
                    })
                ) {
                    break;
                }
            }
        })
        .await;
        assert!(busy.is_ok());
        handle
            .tx
            .send(Input::Message(
                1,
                ClientMessage::ListenCancel { id: "a".into() },
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while handle.state().listen_id.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if matches!(
                    rx1.recv().await,
                    Some(EngineMessage::TranscriptFinal { .. })
                ) {
                    return true;
                }
            }
        })
        .await
        .is_err());
        handle.tx.send(Input::Shutdown).await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
