import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js"
import { Schema } from "effect"
import { Command, ComposerPill, Panel, Plugin, SettingsPage, Slot, Store, Style } from "@opencode/plugin/gui"
import type { Command as GUICommand } from "@opencode/plugin/gui"
import type { EngineMessage, Language } from "@local-voice/client"
import css from "./gui/style.css?inline"
import blobCss from "../../voice-blob/src/voice-blob.css?inline"
import { VoiceBlob } from "./gui/blob.ts"
import { createVoiceController } from "./gui/controller.ts"
import { EngineBridge } from "./gui/contract.ts"
import { createEngineLink } from "./gui/engine-api.ts"
import { assistantReplies, timeLabel } from "./gui/history.ts"
import { createVoiceRuntime, type VoiceSession } from "./gui/runtime.ts"
import { VoiceSettings } from "./gui/settings.tsx"
import { main } from "./gui/main.ts"

const STATE_TEXT: Record<string, string> = {
  off: "Voice off",
  connecting: "Starting…",
  idle: "Ready",
  listening: "Listening",
  transcribing: "Transcribing…",
  submitting: "Sending…",
  processing: "Agent working",
  speaking: "Speaking",
  error: "Error",
  disconnected: "Engine offline",
}
const SAMPLE: Record<string, string> = {
  "pt-br": "Olá! Esta é a minha voz. Pronto, criei o arquivo e todos os testes passaram.",
  en: "Hi! This is my voice. Done, I created the file and all tests passed.",
}

export default Plugin.define({
  stores: {
    prefs: Store.global(Schema.Struct({ autoRead: Schema.Boolean }), { autoRead: true }),
  },
  main,
  setup(ctx) {
    const voice = createVoiceController()
    // Plugin option `language` is the fallback until the engine's settings load.
    const option = ctx.options.language
    const [language, setLanguage] = createSignal<Language>(option === "en" || option === "auto" ? option : "pt-br")
    const [revision, setRevision] = createSignal(0)
    const unsubscribeController = voice.subscribe(() => setRevision(value => value + 1))
    const engine = createEngineLink(ctx.main(EngineBridge))
    const [active, setActive] = createSignal(false)
    const [playing, setPlaying] = createSignal<{ message: string; speak: string }>()
    const autoRead = () => ctx.stores.prefs.value.autoRead
    const setAutoRead = (value: boolean) => ctx.stores.prefs.update(prefs => { prefs.autoRead = value })
    let release: (() => void) | undefined
    let unsubscribeEngine: (() => void) | undefined
    let initializedSession: string | undefined

    const runtime = createVoiceRuntime({
      controller: voice,
      send: message => void engine.send(message).catch(failed),
      currentSession: (): VoiceSession | undefined => ctx.sessions.current(),
      currentModel: () => ctx.screen.current()?.composer.model(),
      insert: text => {
        const screen = ctx.screen.current()
        if (!screen) return false
        screen.composer.insert(text)
        return true
      },
      notice: title => ctx.toast.show({ title }),
      speak: (text, lang, id) => void engine.send({ type: "speak", id, text, format: "markdown", language: lang }).catch(failed),
      language,
      autoRead,
      onFailure: failed,
    })

    const onEngineMessage = (message: EngineMessage) => {
      if (message.type === "settings.changed") setLanguage(message.settings.language)
      if (message.type === "speak.ended" && playing()?.speak === message.id) setPlaying(undefined)
      runtime.processMessage(message)
    }

    function failed(error: unknown) {
      teardown()
      voice.dispatch({ type: "disconnected" })
      ctx.toast.show({ title: `Voice engine unavailable: ${error instanceof Error ? error.message : String(error)}`, variant: "error" })
    }
    function teardown() {
      setActive(false)
      setPlaying(undefined)
      unsubscribeEngine?.()
      unsubscribeEngine = undefined
      release?.()
      release = undefined
    }
    const deactivate = () => {
      if (!active()) return
      runtime.dispose()
      void engine.send({ type: "listen.cancel", id: crypto.randomUUID() }).catch(() => {})
      teardown()
      voice.dispatch({ type: "disable" })
    }
    const activate = async () => {
      if (active()) return
      setActive(true)
      voice.dispatch({ type: "enable" })
      try {
        unsubscribeEngine = engine.subscribe(onEngineMessage)
        release = await engine.acquire()
        voice.dispatch({ type: "connected" })
        runtime.watch(ctx.sessions.current())
        void engine.settings().then(settings => setLanguage(settings.language)).catch(() => {})
        await engine.send({ type: "models.warm", stt: true, tts: true })
      } catch (error) {
        failed(error)
      }
    }
    const toggle = () => (active() ? deactivate() : void activate())
    const openPanel = () => {
      const session = ctx.sessions.current()
      if (session) ctx.layout.open("local-voice:main", session, { tab: "select" })
      else ctx.toast.show({ title: "Open a session to use voice controls." })
    }
    const startTalk = () => {
      if (!active()) void activate().then(() => { if (voice.state === "idle") runtime.listen() })
      else runtime.listen()
    }
    const stopTalk = () => runtime.stopListening()
    const stopAll = () => {
      runtime.cancelSpeech()
      setPlaying(undefined)
      if (voice.state === "listening") runtime.stopListening()
    }
    const toggleHandsFree = () => {
      const enabled = !voice.handsFree
      voice.dispatch({ type: "hands-free", value: enabled })
      if (enabled && active() && voice.state === "idle") runtime.listen()
      else if (!enabled && voice.state === "listening") runtime.cancelListening()
    }
    const playReply = async (message: string, text: string) => {
      if (playing()?.message === message) {
        stopAll()
        return
      }
      if (!active()) await activate()
      if (!active()) return
      setPlaying({ message, speak: runtime.speakText(text) })
    }

    // Tap to start and tap again to send; holding longer than TAP_MS sends on release instead.
    const TAP_MS = 350
    let pressedAt = 0
    let pressStartedListening = false
    const onTalkDown = () => {
      pressedAt = performance.now()
      pressStartedListening = voice.state !== "listening"
      if (pressStartedListening) startTalk()
      else stopTalk()
    }
    const onTalkUp = () => {
      if (pressStartedListening && performance.now() - pressedAt > TAP_MS && voice.state === "listening") stopTalk()
      pressStartedListening = false
    }

    const stateText = () => {
      revision()
      const text = active() ? STATE_TEXT[voice.state] ?? voice.state : STATE_TEXT.off
      return voice.micMuted && active() ? `${text} · muted` : text
    }
    const listening = () => (revision(), voice.state === "listening")
    const theme = () => ctx.appearance.mode()
    const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches
    const blobState = () => (revision(), voice.blobState())

    createEffect(() => {
      const current = ctx.sessions.current()
      const key = current?.key
      if (initializedSession !== undefined && key !== initializedSession && active()) {
        setPlaying(undefined)
        runtime.sessionChanged(current)
      }
      initializedSession = key
    })
    onCleanup(() => {
      unsubscribeController()
      deactivate()
      runtime.dispose()
    })

    ctx.add(Style, `${css}\n${blobCss}`)
    ctx.add(Command, { id: "toggle", title: "Voice mode", description: "Turn local voice input and spoken replies on or off", bind: "mod+shift+v", slash: { name: "voice" }, run: () => { toggle() } })
    ctx.add(Command, (): GUICommand => ({ id: "talk", title: "Voice: talk", description: "Start listening, or send what you said", bind: "mod+shift+space", section: "session", enabled: !!ctx.sessions.current(), run: () => { if (voice.state === "listening") stopTalk(); else startTalk() } }))
    ctx.add(Command, (): GUICommand => ({ id: "stop", title: "Voice: stop", section: "session", enabled: active(), run: stopAll }))
    ctx.add(Command, (): GUICommand => ({ id: "mute", title: "Voice: mute microphone", section: "session", enabled: active(), run: () => runtime.mute(!voice.micMuted) }))
    ctx.add(Command, (): GUICommand => ({ id: "hands-free", title: "Voice: hands-free", section: "session", enabled: active(), run: toggleHandsFree }))
    ctx.add(Command, (): GUICommand => ({
      id: "read",
      title: "Voice: read a reply",
      description: "Pick an earlier reply of this session to hear",
      section: "session",
      enabled: !!ctx.sessions.current(),
      run: async () => {
        const session = ctx.sessions.current()
        if (!session) return
        await session.server.data.session.message.sync(session.id).catch(() => {})
        const replies = assistantReplies(session.server.data.session.message.list(session.id))
        if (!replies.length) return void ctx.toast.show({ title: "No replies to read yet." })
        const picked = await ctx.dialogs.select({
          title: "Read a reply",
          placeholder: "Search replies",
          options: replies.map(reply => ({ value: reply.id, title: reply.text.replace(/\s+/g, " ").slice(0, 90), description: timeLabel(reply.created) })),
        })
        const reply = replies.find(item => item.id === picked)
        if (reply) void playReply(reply.id, reply.text)
      },
    }))
    ctx.add(Command, { id: "settings", title: "Voice: settings", run: () => ctx.layout.settings("local-voice") })

    ctx.add(SettingsPage, {
      id: "local-voice",
      icon: "waveform",
      get title() {
        return "Local Voice"
      },
      get entries() {
        return [
          { id: "language", title: "Language", keywords: "voice language portuguese english idioma" },
          { id: "auto-read", title: "Read replies aloud", keywords: "voice speak auto read" },
          { id: "voice_pt", title: "Portuguese voice", keywords: "voice tts piper kokoro" },
          { id: "voice_en", title: "English voice", keywords: "voice tts" },
          { id: "stt", title: "Speech recognition model", keywords: "whisper stt transcription" },
        ]
      },
      render: () => (
        <VoiceSettings
          ui={ctx.ui as never}
          engine={engine}
          theme={theme}
          autoRead={autoRead}
          setAutoRead={setAutoRead}
          confirm={input => ctx.dialogs.confirm(input)}
          notify={(title, error) => ctx.toast.show({ title, variant: error ? "error" : undefined })}
          test={(voiceID, lang) => {
            void engine.acquire().then(done => {
              const id = crypto.randomUUID()
              const off = engine.subscribe(message => {
                if (message.type === "speak.ended" && message.id === id) {
                  off()
                  done()
                }
              })
              void engine.send({ type: "speak", id, text: SAMPLE[lang] ?? SAMPLE.en, format: "plain", language: lang, voice: voiceID })
            }).catch(error => ctx.toast.show({ title: String(error), variant: "error" }))
          }}
        />
      ),
    })

    const panelTab = { id: "main", title: "Voice" }
    ctx.add(Panel, {
      id: "main",
      region: "side",
      width: 360,
      list: input => (input.open.includes("main") ? [panelTab] : []),
      render: props => {
        const replies = createMemo(() => {
          const session = props.session
          return assistantReplies(session.server.data.session.message.list(session.id), 20)
        })
        createEffect(() => void props.session.server.data.session.message.sync(props.session.id).catch(() => {}))
        return (
          <div
            class="lv lv-panel"
            data-theme={theme()}
            tabIndex={0}
            onKeyDown={event => {
              if (event.key === " " && event.target === event.currentTarget) {
                event.preventDefault()
                listening() ? stopTalk() : startTalk()
              }
            }}
          >
            <div class="lv-toolbar">
              <ctx.ui.IconButton icon="settings-gear" label="Voice settings" variant="ghost-muted" onClick={() => ctx.layout.settings("local-voice")} />
              <button
                class="lv-btn lv-power"
                aria-pressed={active()}
                onClick={() => {
                  if (!active()) return void activate()
                  deactivate()
                  ctx.screen.current()?.composer.focus()
                }}
              >
                <span class="lv-power__dot" />
                {active() ? "Voice on" : "Start voice"}
              </button>
            </div>
            <div class="lv-panel__stage">
              <VoiceBlob state={blobState} levels={voice.levels} mode="expanded" size={180} theme={theme()} reducedMotion={reducedMotion()} />
            </div>
            <Show when={(revision(), voice.partial || voice.final)}>
              <div class="lv-card">
                {(revision(), voice.partial) ? <span class="lv-card__partial">{voice.partial}</span> : voice.final}
              </div>
            </Show>
            <button
              class="lv-btn lv-btn--block"
              classList={{ "lv-btn--brand": !listening(), "lv-btn--live": listening() }}
              onPointerDown={event => { event.currentTarget.setPointerCapture(event.pointerId); onTalkDown() }}
              onPointerUp={onTalkUp}
              onPointerCancel={onTalkUp}
              disabled={(revision(), active() && voice.state === "disconnected")}
            >
              {listening() ? "Tap to send" : "Tap or hold to talk"}
            </button>
            <div class="lv-row">
              <button class="lv-btn" aria-pressed={(revision(), voice.micMuted)} onClick={() => runtime.mute(!voice.micMuted)} disabled={!active()}>
                {(revision(), voice.micMuted) ? "Unmute" : "Mute"}
              </button>
              <button class="lv-btn" onClick={stopAll} disabled={!active()}>Stop</button>
              <button class="lv-btn" aria-pressed={(revision(), voice.handsFree)} onClick={toggleHandsFree} disabled={!active()}>Hands-free</button>
            </div>
            <Show when={(revision(), voice.state === "disconnected")}>
              <div class="lv-hint">Install voice-engine (or set VOICE_ENGINE_BIN), then start the engine.</div>
            </Show>
            <div class="lv-section-title">
              <span>Replies</span>
              <button class="lv-flat" aria-pressed={autoRead()} onClick={() => setAutoRead(!autoRead())} title="Read every finished reply aloud">
                Auto-read: {autoRead() ? "on" : "off"}
              </button>
            </div>
            <div class="lv-history">
              <For each={replies()} fallback={<div class="lv-history__empty">Replies of this session show up here.</div>}>
                {reply => (
                  <div class="lv-history__item" data-playing={playing()?.message === reply.id}>
                    <button
                      class="lv-btn lv-btn--icon"
                      aria-label={playing()?.message === reply.id ? "Stop reading" : "Read aloud"}
                      onClick={() => void playReply(reply.id, reply.text)}
                    >
                      {playing()?.message === reply.id ? "■" : "▶"}
                    </button>
                    <div>
                      <div class="lv-history__text">{reply.text}</div>
                      <div class="lv-history__meta">{timeLabel(reply.created)}</div>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </div>
        )
      },
    })
    ctx.add(Slot, {
      at: "session.header",
      order: 50,
      render: () => (
        <button class="lv lv-header" data-theme={theme()} onClick={openPanel} aria-label="Open voice panel">
          <VoiceBlob state={blobState} levels={voice.levels} mode="compact" size={16} theme={theme()} reducedMotion={reducedMotion()} />
          <span>{active() ? stateText() : "Voice"}</span>
        </button>
      ),
    })
    ctx.add(ComposerPill, () => ({
      id: "status",
      show: () => (active() ? { label: stateText(), title: "Local voice", tone: (voice.state === "error" || voice.state === "disconnected" ? "danger" : undefined) as "danger" | undefined, run: openPanel } : undefined),
    }))
  },
})
