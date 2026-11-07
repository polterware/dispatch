import type { EngineMessage, Language } from "@local-voice/client"
import type { VoiceController } from "./controller.ts"
import { extractResponseText } from "./response-text.ts"

export interface VoiceSession {
  key: string
  id: string
  server: {
    client: { session: { switchModel(input: { sessionID: string; model: { id: string; providerID: string; variant?: string } }): Promise<unknown>; prompt(input: { sessionID: string; text: string }): Promise<unknown> } }
    data: {
      on(type: "session.execution.started" | "session.execution.succeeded" | "session.execution.failed" | "permission.asked", handler: (event: any) => void): () => void
      session: { message: { list(sessionID: string): ReadonlyArray<{ type: string }>; sync(sessionID: string): Promise<void> } }
    }
  }
}

export interface VoiceRuntimeOptions {
  controller: VoiceController
  send(message: Record<string, unknown>): void
  currentSession(): VoiceSession | undefined
  currentModel(): { providerID: string; modelID: string; variant?: string } | undefined
  insert(text: string): boolean
  notice(message: string): void
  speak(text: string, language: Language, id: string): void
  onFailure?(error: unknown): void
}
export type VoiceTransportMessage = { type: "connection"; connected: boolean }

export function createVoiceRuntime(options: VoiceRuntimeOptions) {
  let listeningID: string | undefined
  let speakID: string | undefined
  let submittedSession: VoiceSession | undefined
  let started = false
  let removeEvents: Array<() => void> = []

  const clearEvents = () => { removeEvents.forEach(remove => remove()); removeEvents = []; started = false; submittedSession = undefined }
  const send = (message: Record<string, unknown>) => { try { options.send(message); return true } catch (error) { options.onFailure?.(error); options.controller.dispatch({ type: "disconnected" }); return false } }
  const cancelSpeech = () => { send({ type: "speak.cancel", ...(speakID ? { id: speakID } : {}) }); speakID = undefined }
  const listen = () => {
    listeningID = crypto.randomUUID()
    if (send({ type: "listen.start", id: listeningID, mode: options.controller.handsFree ? "hands-free" : "push-to-talk", language: "auto" })) options.controller.dispatch({ type: "listen.start" })
    else listeningID = undefined
  }
  const processMessage = (message: EngineMessage | VoiceTransportMessage) => {
    if (message.type === "connection") {
      if (message.connected && options.controller.state === "disconnected") options.controller.dispatch({ type: "connected" })
      else if (!message.connected && options.controller.state !== "off") {
        options.controller.dispatch({ type: "disconnected" })
        const error = new Error("Voice engine connection lost")
        if (options.onFailure) options.onFailure(error)
        else options.notice("Voice engine connection lost. Start the engine again from the panel.")
      }
      return
    }
    switch (message.type) {
      case "level": options.controller.levels[message.source === "mic" ? "mic" : "out"] = message.rms; break
      case "state": if (message.state.mic === "idle" && options.controller.state === "disconnected") options.controller.dispatch({ type: "connected" }); break
      case "transcript.partial": options.controller.dispatch({ type: "transcript.partial", text: message.text }); break
      case "transcript.final": {
        listeningID = undefined
        options.controller.dispatch({ type: "transcript.final", text: message.text })
        const text = message.text.trim()
        if (!text) return
        void submit(text, message.language)
        break
      }
      case "speak.started": speakID = message.id; options.controller.dispatch({ type: "speak.started" }); break
      case "speak.ended": speakID = undefined; options.controller.dispatch({ type: "speak.ended", reason: message.reason }); if (message.reason === "done" && options.controller.handsFree) listen(); break
      case "error": options.controller.dispatch({ type: "failure" }); options.notice(`Voice engine: ${message.message}`); break
    }
  }
  const submit = async (text: string, language: Language) => {
    const session = options.currentSession()
    if (!session) {
      const inserted = options.insert(text)
      options.notice(inserted ? "No routed session; transcript inserted into the composer." : "Transcript ready. Open a session to add it to the composer.")
      options.controller.dispatch({ type: "turn.ended" })
      return
    }
    clearEvents()
    submittedSession = session
    const onStarted = (event: { data: { sessionID: string } }) => {
      if (event.data.sessionID === session.id) { started = true; options.controller.dispatch({ type: "turn.started" }) }
    }
    const onSucceeded = (event: { data: { sessionID: string } }) => {
      if (!started || event.data.sessionID !== session.id) return
      void (async () => {
        try {
          await session.server.data.session.message.sync(session.id)
          const last = [...session.server.data.session.message.list(session.id)].reverse().find(item => item.type === "assistant") as
            | { type: string; content?: Array<{ type?: string; text?: string }> }
            | undefined
          const response = extractResponseText({ role: last?.type, parts: last?.content })
          options.controller.dispatch({ type: "turn.ended" })
          clearEvents()
          if (response) {
            const id = crypto.randomUUID(); speakID = id
            options.speak(response, language, id)
          }
        } catch (error) { options.onFailure?.(error); options.controller.dispatch({ type: "failure" }) }
      })()
    }
    const onFailed = (event: { data: { sessionID: string } }) => {
      if (event.data.sessionID === session.id) { clearEvents(); options.controller.dispatch({ type: "failure" }) }
    }
    const onPermission = (event: { data: { sessionID: string } }) => {
      if (event.data.sessionID !== session.id) return
      const id = crypto.randomUUID(); speakID = id
      options.speak(language === "pt-br" ? "O agente está aguardando sua aprovação." : "The agent is waiting for your approval.", language, id)
    }
    removeEvents = [
      session.server.data.on("session.execution.started", onStarted),
      session.server.data.on("session.execution.succeeded", onSucceeded),
      session.server.data.on("session.execution.failed", onFailed),
      session.server.data.on("permission.asked", onPermission),
    ]
    try {
      const model = options.currentModel()
      if (model) await session.server.client.session.switchModel({ sessionID: session.id, model: { id: model.modelID, providerID: model.providerID, variant: model.variant } })
      await session.server.client.session.prompt({ sessionID: session.id, text })
      options.controller.dispatch({ type: "prompt.submitted" })
    } catch (error) {
      clearEvents()
      options.onFailure?.(error)
      options.controller.dispatch({ type: "failure" })
      options.notice(`Could not submit voice transcript: ${String(error)}`)
    }
  }
  return {
    processMessage,
    listen,
    stopListening() { if (listeningID) send({ type: "listen.stop", id: listeningID }); options.controller.dispatch({ type: "listen.stop" }) },
    cancelListening() { if (listeningID) send({ type: "listen.cancel", id: listeningID }); listeningID = undefined; options.controller.dispatch({ type: "listen.cancel" }) },
    mute(value: boolean) { options.controller.dispatch({ type: "mute", value }); send({ type: "mic.mute", muted: value }) },
    cancelSpeech,
    sessionChanged() { clearEvents(); cancelSpeech(); if (options.controller.state === "listening" && listeningID) send({ type: "listen.cancel", id: listeningID }); listeningID = undefined },
    dispose() { clearEvents(); cancelSpeech() },
  }
}
