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
    const toggleHandsFree = () => {
      const enabled = !voice.handsFree
      voice.dispatch({ type: "hands-free", value: enabled })
      if (enabled && active() && voice.state === "idle") runtime.listen()
      else if (!enabled && voice.state === "listening") runtime.cancelListening()
    }
    const stateLabel = () => { revision(); return voice.state }
    const theme = () => ctx.appearance.mode()
    const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches

    createEffect(() => {
      const current = ctx.sessions.current()
      const key = current?.key
      if (initializedSession !== undefined && key !== initializedSession) runtime.sessionChanged()
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
      render: () => <div class="local-voice-panel" tabIndex={0} onKeyDown={event => { if (event.key === " " && event.target === event.currentTarget) { event.preventDefault(); voice.state === "listening" ? stopTalk() : startTalk() } }}>
        <VoiceBlob state={() => { revision(); return voice.blobState() }} levels={voice.levels} mode="expanded" size={220} theme={theme()} reducedMotion={reducedMotion()} />
        <div class="local-voice-panel__status">{active() ? stateLabel() : "Voice mode off"}{voice.micMuted ? " · mic muted" : ""}</div>
        <div class="local-voice-panel__transcript">{voice.partial || voice.final || (voice.state === "disconnected" ? "Engine unavailable. Start the engine to use voice." : "Your transcript will appear here.")}</div>
        <button class="local-voice-panel__talk" onPointerDown={event => { event.currentTarget.setPointerCapture(event.pointerId); startTalk() }} onPointerUp={stopTalk} onPointerCancel={stopTalk} onPointerLeave={event => { if (event.currentTarget.hasPointerCapture(event.pointerId)) stopTalk() }} disabled={!active() || voice.state === "disconnected"}>Hold to talk</button>
        <div class="local-voice-panel__controls">
          <button onClick={() => runtime.mute(!voice.micMuted)} disabled={!active()}>{voice.micMuted ? "Unmute" : "Mute"}</button>
          <button onClick={() => { runtime.cancelSpeech(); if (voice.state === "listening") runtime.stopListening() }} disabled={!active()}>Stop</button>
          <button onClick={toggleHandsFree} disabled={!active()}>{voice.handsFree ? "Hands-free on" : "Hands-free off"}</button>
          <button onClick={toggle}>{active() ? "Turn voice off" : "Start engine"}</button>
          <button onClick={() => { deactivate(); ctx.screen.current()?.composer.focus() }}>Back to text</button>
        </div>
        {voice.state === "disconnected" && <div class="local-voice-panel__hint">Install voice-engine or set VOICE_ENGINE_BIN, then select Start engine.</div>}
      </div>,
    })
    ctx.add(Slot, { at: "session.header", order: 50, render: () => <button class="local-voice-header" onClick={openPanel} aria-label="Open voice panel"><VoiceBlob state={() => { revision(); return voice.blobState() }} levels={voice.levels} mode="compact" size={20} theme={theme()} reducedMotion={reducedMotion()} /><span>{active() ? stateLabel() : "Voice"}</span></button> })
    ctx.add(ComposerPill, () => ({ id: "status", show: () => active() ? { label: stateLabel(), title: "Local voice", tone: (voice.state === "error" || voice.state === "disconnected" ? "danger" : undefined) as "danger" | undefined, run: openPanel } : undefined }))
  },
})
