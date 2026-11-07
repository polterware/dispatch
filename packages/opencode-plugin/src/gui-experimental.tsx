import { createEffect, createSignal, onCleanup } from "solid-js"
import { Command, ComposerPill, Panel, Plugin, Slot, Style } from "@opencode/plugin/gui"
import type { Command as GUICommand } from "@opencode/plugin/gui"
import css from "./gui/style.css?inline"
import blobCss from "../../voice-blob/src/voice-blob.css?inline"
import type { EngineMessage } from "@local-voice/client"
import { VoiceBlob } from "./gui/blob.ts"
import { createVoiceController } from "./gui/controller.ts"
import { EngineBridge } from "./gui/contract.ts"
import { createVoiceRuntime, type VoiceSession } from "./gui/runtime.ts"
import { main } from "./gui/main.ts"

export default Plugin.define({
  main,
  setup(ctx) {
    const voice = createVoiceController()
    // Plugin option `language` in opencode.json: "pt-br" (default), "en", or "auto".
    const option = ctx.options.language
    const configuredLanguage: "auto" | "pt-br" | "en" = option === "en" || option === "auto" ? option : "pt-br"
    const [revision, setRevision] = createSignal(0)
    const refresh = voice.subscribe(() => setRevision(value => value + 1))
    const bridge = ctx.main(EngineBridge)
    const [active, setActive] = createSignal(false)
    let initializedSession: string | undefined
    let removeMessage: (() => void) | undefined
    const runtime = createVoiceRuntime({
      controller: voice,
      send: message => { void bridge.send({ json: JSON.stringify(message) }).catch(failed) },
      currentSession: (): VoiceSession | undefined => ctx.sessions.current(),
      currentModel: () => ctx.screen.current()?.composer.model(),
      insert: text => { const screen = ctx.screen.current(); if (!screen) return false; screen.composer.insert(text); return true },
      notice: title => ctx.toast.show({ title }),
      speak: (text, language, id) => runtimeSendSpeak(text, language, id),
      language: () => configuredLanguage,
      onFailure: failed,
    })
    function failed(error: unknown) {
      setActive(false)
      removeMessage?.()
      removeMessage = undefined
      void bridge.disconnect().catch(() => {})
      voice.dispatch({ type: "disconnected" })
      ctx.toast.show({ title: `Voice engine unavailable: ${String(error)}`, variant: "error" })
    }
    function runtimeSendSpeak(text: string, language: "auto" | "pt-br" | "en", id: string) {
      void bridge.send({ json: JSON.stringify({ type: "speak", id, text, format: "markdown", language }) }).catch(failed)
    }
    const deactivate = () => {
      if (!active()) return
      setActive(false)
      removeMessage?.()
      removeMessage = undefined
      runtime.dispose()
      void bridge.send({ json: JSON.stringify({ type: "listen.cancel", id: crypto.randomUUID() }) }).catch(() => {})
      void bridge.send({ json: JSON.stringify({ type: "speak.cancel" }) }).catch(() => {})
      void bridge.disconnect().catch(() => {})
      voice.dispatch({ type: "disable" })
    }
    const activate = async () => {
      if (active()) return
      setActive(true)
      voice.dispatch({ type: "enable" })
      try {
        await bridge.ensure()
        removeMessage?.()
        removeMessage = bridge.on("message", json => {
          try { runtime.processMessage(JSON.parse(json) as EngineMessage) } catch (error) { failed(error) }
        })
        await bridge.connect()
        voice.dispatch({ type: "connected" })
        runtime.watch(ctx.sessions.current())
        await bridge.send({ json: JSON.stringify({ type: "models.warm", stt: true, tts: true }) })
      } catch (error) { failed(error); voice.dispatch({ type: "disconnected" }) }
    }
    const toggle = () => active() ? deactivate() : void activate()
    const openPanel = () => {
      const session = ctx.sessions.current()
      if (session) ctx.layout.open("local-voice:main", session, { tab: "select" })
      else ctx.toast.show({ title: "Open a session to use voice controls." })
    }
    const startTalk = () => { if (!active()) void activate().then(() => { if (voice.state === "idle") runtime.listen() }); else runtime.listen() }
    const stopTalk = () => runtime.stopListening()
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
    const talkLabel = () => {
      revision()
      return voice.state === "listening" ? "Tap to send" : "Tap or hold to talk"
    }
    const toggleHandsFree = () => {
      const enabled = !voice.handsFree
      voice.dispatch({ type: "hands-free", value: enabled })
      if (enabled && active() && voice.state === "idle") runtime.listen()
      else if (!enabled && voice.state === "listening") runtime.cancelListening()
    }
    const stateLabel = () => { revision(); return voice.state }
    const STATE_TEXT: Record<string, string> = {
      off: "Voice off", connecting: "Starting…", idle: "Ready", listening: "Listening", transcribing: "Transcribing…",
      submitting: "Sending…", processing: "Agent working", speaking: "Speaking", error: "Error", disconnected: "Engine offline",
    }
    const stateText = () => {
      revision()
      const text = active() ? STATE_TEXT[voice.state] ?? voice.state : STATE_TEXT.off
      return voice.micMuted && active() ? `${text} · muted` : text
    }
    const theme = () => ctx.appearance.mode()
    const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches

    createEffect(() => {
      const current = ctx.sessions.current()
      const key = current?.key
      if (initializedSession !== undefined && key !== initializedSession && active()) runtime.sessionChanged(current)
      initializedSession = key
    })
    onCleanup(() => { refresh(); deactivate(); runtime.dispose() })

    ctx.add(Style, `${css}\n${blobCss}`)
    ctx.add(Command, { id: "toggle", title: "Voice mode", description: "Turn local voice input and spoken replies on or off", bind: "mod+shift+v", slash: { name: "voice" }, run: () => { toggle() } })
    ctx.add(Command, (): GUICommand => ({ id: "talk", title: "Voice: talk", description: "Start or stop push-to-talk", bind: "mod+shift+space", section: "session", enabled: !!ctx.sessions.current(), run: () => { if (voice.state === "listening") stopTalk(); else startTalk() } }))
    ctx.add(Command, (): GUICommand => ({ id: "stop", title: "Stop voice", section: "session", enabled: active(), run: () => { runtime.cancelSpeech(); if (voice.state === "listening") runtime.stopListening() } }))
    ctx.add(Command, (): GUICommand => ({ id: "mute", title: "Mute voice microphone", section: "session", enabled: active(), run: () => runtime.mute(!voice.micMuted) }))
    ctx.add(Command, (): GUICommand => ({ id: "hands-free", title: "Toggle hands-free voice", section: "session", enabled: active(), run: toggleHandsFree }))
    const panelTab = { id: "main", title: "Voice" }
    ctx.add(Panel, {
      id: "main", region: "side", width: 350,
      list: input => (input.open.includes("main") ? [panelTab] : []),
      render: () => (
        <div
          class="lv lv-panel"
          data-theme={theme()}
          tabIndex={0}
          onKeyDown={event => {
            if (event.key === " " && event.target === event.currentTarget) {
              event.preventDefault()
              voice.state === "listening" ? stopTalk() : startTalk()
            }
          }}
        >
          <div class="lv-panel__stage">
            <VoiceBlob state={() => { revision(); return voice.blobState() }} levels={voice.levels} mode="expanded" size={200} theme={theme()} reducedMotion={reducedMotion()} />
            <span class="lv-chip" data-state={(revision(), active() ? voice.state : "off")}>
              <span class="lv-chip__dot" />
              {stateText()}
            </span>
          </div>
          <div class="lv-card" data-empty={(revision(), !voice.partial && !voice.final)}>
            {(revision(), voice.partial)
              ? <span class="lv-card__partial">{voice.partial}</span>
              : voice.final || (voice.state === "disconnected" ? "Voice engine unavailable." : "Your words will appear here.")}
          </div>
          <button
            class="lv-btn lv-btn--lg lv-btn--block"
            classList={{ "lv-btn--brand": (revision(), voice.state !== "listening"), "lv-btn--live": voice.state === "listening" }}
            onPointerDown={event => { event.currentTarget.setPointerCapture(event.pointerId); onTalkDown() }}
            onPointerUp={onTalkUp}
            onPointerCancel={onTalkUp}
            disabled={(revision(), active() && voice.state === "disconnected")}
          >
            {talkLabel()}
          </button>
          <div class="lv-row">
            <button class="lv-btn" aria-pressed={(revision(), voice.micMuted)} onClick={() => runtime.mute(!voice.micMuted)} disabled={!active()}>
              {voice.micMuted ? "Unmute" : "Mute"}
            </button>
            <button class="lv-btn" onClick={() => { runtime.cancelSpeech(); if (voice.state === "listening") runtime.stopListening() }} disabled={!active()}>
              Stop
            </button>
            <button class="lv-btn" aria-pressed={(revision(), voice.handsFree)} onClick={toggleHandsFree} disabled={!active()}>
              Hands-free
            </button>
          </div>
          {(revision(), voice.state === "disconnected") && (
            <div class="lv-hint">Install voice-engine (or set VOICE_ENGINE_BIN), then start the engine.</div>
          )}
          <div class="lv-footer">
            <button class="lv-flat" onClick={() => { deactivate(); ctx.screen.current()?.composer.focus() }}>Back to text</button>
            <button class="lv-flat" onClick={toggle}>{active() ? "Turn voice off" : "Start engine"}</button>
          </div>
        </div>
      ),
    })
    ctx.add(Slot, {
      at: "session.header",
      order: 50,
      render: () => (
        <button class="lv lv-header" data-theme={theme()} onClick={openPanel} aria-label="Open voice panel">
          <VoiceBlob state={() => { revision(); return voice.blobState() }} levels={voice.levels} mode="compact" size={16} theme={theme()} reducedMotion={reducedMotion()} />
          <span>{active() ? stateText() : "Voice"}</span>
        </button>
      ),
    })
    ctx.add(ComposerPill, () => ({ id: "status", show: () => active() ? { label: stateText(), title: "Local voice", tone: (voice.state === "error" || voice.state === "disconnected" ? "danger" : undefined) as "danger" | undefined, run: openPanel } : undefined }))
  },
})
