import type { EngineMessage, Language } from "@local-voice/client"
import type { VoiceController } from "./controller.ts"
import { extractResponseText } from "./response-text.ts"

type SessionEvent = { data: { sessionID: string } }
type WatchedEvent = "session.execution.started" | "session.execution.succeeded" | "session.execution.failed" | "permission.asked"

/** The slice of the host's MountedSession the runtime uses (structurally checked against the real type). */
export interface VoiceSession {
  key: string
  id: string
  server: {
    client: {
      session: {
        switchModel(input: { sessionID: string; model: { id: string; providerID: string; variant?: string } }): Promise<unknown>
        prompt(input: { sessionID: string; text: string }): Promise<unknown>
      }
    }
    data: {
      on(type: WatchedEvent, handler: (event: any) => void): () => void
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
  /** The language the user speaks and wants to hear. Defaults to pt-BR. */
  language?(): Language
  /** Read every finished turn aloud. Defaults to true. */
  autoRead?(): boolean
  onFailure?(error: unknown): void
}
export type VoiceTransportMessage = { type: "connection"; connected: boolean }

type AssistantContent = Array<{ type?: string; text?: string }>

/** The text the agent wrote in its latest turn: every assistant message after the last user message. */
export function turnText(messages: ReadonlyArray<{ type: string }>): string {
  let start = 0
  for (let index = messages.length - 1; index >= 0; index--) {
    if (messages[index]!.type === "user") {
      start = index + 1
      break
    }
  }
  return messages
    .slice(start)
    .filter(message => message.type === "assistant")
    .map(message => extractResponseText({ role: "assistant", parts: (message as { content?: AssistantContent }).content }))
    .filter(Boolean)
    .join("\n\n")
}

export function createVoiceRuntime(options: VoiceRuntimeOptions) {
  const language = () => options.language?.() ?? "pt-br"
  let listeningID: string | undefined
  let speakID: string | undefined
  let watched: { session: VoiceSession; remove: Array<() => void> } | undefined

  const send = (message: Record<string, unknown>) => {
    try {
      options.send(message)
      return true
    } catch (error) {
      options.onFailure?.(error)
      options.controller.dispatch({ type: "disconnected" })
      return false
    }
  }
  const cancelSpeech = () => {
    send({ type: "speak.cancel", ...(speakID ? { id: speakID } : {}) })
    speakID = undefined
  }
  const speak = (text: string) => {
    const id = crypto.randomUUID()
    speakID = id
    options.speak(text, language(), id)
  }
  const listen = () => {
    listeningID = crypto.randomUUID()
    const mode = options.controller.handsFree ? "hands-free" : "push-to-talk"
    if (send({ type: "listen.start", id: listeningID, mode, language: language() })) options.controller.dispatch({ type: "listen.start" })
    else listeningID = undefined
  }

  /** Follows one session's turns while voice mode is on: every finished turn is read aloud, typed or spoken. */
  const watch = (session: VoiceSession | undefined) => {
    if (watched?.session.key === session?.key) return
    unwatch()
    if (!session) return
    const mine = (event: SessionEvent) => event.data.sessionID === session.id
    const onStarted = (event: SessionEvent) => {
      if (!mine(event)) return
      if (options.controller.state === "idle" || options.controller.state === "submitting") options.controller.dispatch({ type: "turn.started" })
    }
    const onSucceeded = (event: SessionEvent) => {
      if (!mine(event)) return
      void (async () => {
        try {
          await session.server.data.session.message.sync(session.id)
          const text = turnText(session.server.data.session.message.list(session.id))
          if (options.controller.state === "processing" || options.controller.state === "submitting") options.controller.dispatch({ type: "turn.ended" })
          if (text && (options.autoRead?.() ?? true)) speak(text)
        } catch (error) {
          options.onFailure?.(error)
          options.controller.dispatch({ type: "failure" })
        }
      })()
    }
    const onFailed = (event: SessionEvent) => {
      if (mine(event) && options.controller.state === "processing") options.controller.dispatch({ type: "turn.ended" })
    }
    const onPermission = (event: SessionEvent) => {
      if (!mine(event)) return
      speak(language() === "en" ? "The agent is waiting for your approval." : "O agente está aguardando sua aprovação.")
    }
    watched = {
      session,
      remove: [
        session.server.data.on("session.execution.started", onStarted),
        session.server.data.on("session.execution.succeeded", onSucceeded),
        session.server.data.on("session.execution.failed", onFailed),
        session.server.data.on("permission.asked", onPermission),
      ],
    }
  }
  const unwatch = () => {
    watched?.remove.forEach(remove => remove())
    watched = undefined
  }

  const processMessage = (message: EngineMessage | VoiceTransportMessage) => {
    if (message.type === "connection") {
      if (message.connected && options.controller.state === "disconnected") options.controller.dispatch({ type: "connected" })
      else if (!message.connected && options.controller.state !== "off") {
        // The socket reconnects by itself; voice mode stays on and resumes when it is back.
        listeningID = undefined
        speakID = undefined
        options.controller.dispatch({ type: "disconnected" })
      }
      return
    }
    switch (message.type) {
      case "level":
        options.controller.levels[message.source === "mic" ? "mic" : "out"] = message.rms
        break
      case "state":
        if (message.state.mic === "idle" && options.controller.state === "disconnected") options.controller.dispatch({ type: "connected" })
        break
      case "transcript.partial":
        options.controller.dispatch({ type: "transcript.partial", text: message.text })
        break
      case "transcript.final": {
        listeningID = undefined
        options.controller.dispatch({ type: "transcript.final", text: message.text })
        const text = message.text.trim()
        if (text) void submit(text)
        break
      }
      case "speak.started":
        speakID = message.id
        options.controller.dispatch({ type: "speak.started" })
        break
      case "speak.ended":
        speakID = undefined
        options.controller.dispatch({ type: "speak.ended", reason: message.reason })
        if (message.reason === "done" && options.controller.handsFree) listen()
        break
      case "error":
        options.controller.dispatch({ type: "failure" })
        options.notice(`Voice engine: ${message.message}`)
        break
    }
  }

  const submit = async (text: string) => {
    const session = options.currentSession()
    if (!session) {
      const inserted = options.insert(text)
      options.notice(inserted ? "No routed session; transcript inserted into the composer." : "Transcript ready. Open a session to add it to the composer.")
      options.controller.dispatch({ type: "turn.ended" })
      return
    }
    watch(session)
    try {
      const model = options.currentModel()
      if (model) await session.server.client.session.switchModel({ sessionID: session.id, model: { id: model.modelID, providerID: model.providerID, variant: model.variant } })
      await session.server.client.session.prompt({ sessionID: session.id, text })
      options.controller.dispatch({ type: "prompt.submitted" })
    } catch (error) {
      options.onFailure?.(error)
      options.controller.dispatch({ type: "failure" })
      options.notice(`Could not submit voice transcript: ${String(error)}`)
    }
  }

  return {
    processMessage,
    listen,
    watch,
    unwatch,
    get listening() {
      return listeningID !== undefined
    },
    stopListening() {
      if (listeningID) send({ type: "listen.stop", id: listeningID })
      options.controller.dispatch({ type: "listen.stop" })
    },
    cancelListening() {
      if (listeningID) send({ type: "listen.cancel", id: listeningID })
      listeningID = undefined
      options.controller.dispatch({ type: "listen.cancel" })
    },
    mute(value: boolean) {
      options.controller.dispatch({ type: "mute", value })
      send({ type: "mic.mute", muted: value })
    },
    cancelSpeech,
    /** Speaks any text now (e.g. an earlier reply the user picked), replacing what is playing. */
    speakText(text: string) {
      // Drop everything queued or playing, so the picked reply starts now.
      send({ type: "speak.cancel" })
      speakID = undefined
      speak(text)
      return speakID!
    },
    sessionChanged(session: VoiceSession | undefined) {
      cancelSpeech()
      if (options.controller.state === "listening" && listeningID) send({ type: "listen.cancel", id: listeningID })
      listeningID = undefined
      watch(session)
    },
    dispose() {
      unwatch()
      cancelSpeech()
    },
  }
}
