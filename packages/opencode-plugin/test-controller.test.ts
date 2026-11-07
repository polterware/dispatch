import { describe, expect, test } from "bun:test"
import { createVoiceController } from "./src/gui/controller.ts"
import { extractResponseText } from "./src/gui/response-text.ts"
import { createVoiceRuntime, type VoiceSession } from "./src/gui/runtime.ts"

describe("voice controller", () => {
  test("tracks listen, transcript, turn and hands-free transitions", () => {
    const voice = createVoiceController()
    voice.dispatch({ type: "enable" }); expect(voice.state).toBe("connecting")
    voice.dispatch({ type: "connected" }); voice.dispatch({ type: "listen.start" }); expect(voice.state).toBe("listening")
    voice.dispatch({ type: "transcript.partial", text: "hello" }); expect(voice.partial).toBe("hello")
    voice.dispatch({ type: "listen.stop" }); expect(voice.state).toBe("transcribing")
    voice.dispatch({ type: "transcript.final", text: "hello world" }); expect(voice.final).toBe("hello world")
    voice.dispatch({ type: "prompt.submitted" }); voice.dispatch({ type: "turn.ended" }); expect(voice.state).toBe("idle")
    voice.dispatch({ type: "hands-free", value: true }); voice.dispatch({ type: "speak.ended", reason: "done" }); expect(voice.state).toBe("listening")
  })
})

describe("assistant response extraction", () => {
  test("includes text parts and skips tools and reasoning", () => {
    expect(extractResponseText({ info: { role: "assistant" }, parts: [
      { type: "text", text: " First " }, { type: "reasoning", text: "hidden" }, { type: "tool", text: "ignored" }, { type: "text", text: "Second" },
    ] })).toBe("First\nSecond")
  })
})

describe("voice runtime integration", () => {
  test("submits with the selected composer model, speaks the successful response, announces permissions and cancels on session switch", async () => {
    const voice = createVoiceController()
    const handlers = new Map<string, Array<(event: any) => void>>()
    const messages = [{ type: "assistant", content: [{ type: "text", text: "The answer." }, { type: "reasoning", text: "secret" }, { type: "tool", text: "ignored" }] }]
    const calls: Array<{ type: string; [key: string]: unknown }> = []
    const spoken: Array<{ text: string; language: string; id: string }> = []
    const session = {
      key: "server\nsession-1", id: "session-1",
      server: {
        client: { session: {
          switchModel: async (input: unknown) => { calls.push({ type: "switchModel", input }) },
          prompt: async (input: unknown) => { calls.push({ type: "prompt", input }) },
        } },
        data: {
          on: (type: string, handler: (event: any) => void) => { handlers.set(type, [...(handlers.get(type) ?? []), handler]); return () => handlers.set(type, (handlers.get(type) ?? []).filter(item => item !== handler)) },
          session: { message: { list: () => messages, sync: async () => {} } },
        },
      },
    } satisfies VoiceSession
    const runtime = createVoiceRuntime({
      controller: voice,
      send: message => { calls.push(message as { type: string }) },
      currentSession: () => session,
      currentModel: () => ({ providerID: "anthropic", modelID: "sonnet", variant: "high" }),
      insert: () => true, notice: () => {},
      speak: (text, language, id) => spoken.push({ text, language, id }),
    })
    runtime.processMessage({ type: "transcript.final", id: "transcript", text: "Hello", language: "pt-br", timings: { audio_ms: 10, stop_to_final_ms: 2, inference_ms: 8 } })
    await Promise.resolve(); await Promise.resolve()
    expect(calls.find(call => call.type === "switchModel")?.input).toEqual({ sessionID: "session-1", model: { id: "sonnet", providerID: "anthropic", variant: "high" } })
    expect(calls.find(call => call.type === "prompt")?.input).toEqual({ sessionID: "session-1", text: "Hello" })
    for (const handler of handlers.get("session.execution.started") ?? []) handler({ data: { sessionID: "session-1" } })
    for (const handler of handlers.get("permission.asked") ?? []) handler({ data: { sessionID: "session-1" } })
    expect(spoken[0]?.text).toBe("O agente está aguardando sua aprovação.")
    for (const handler of handlers.get("session.execution.succeeded") ?? []) handler({ data: { sessionID: "session-1" } })
    await Promise.resolve(); await Promise.resolve()
    expect(spoken[1]).toMatchObject({ text: "The answer.", language: "pt-br" })
    runtime.sessionChanged()
    expect(calls.some(call => call.type === "speak.cancel")).toBe(true)
  })

  test("turns an engine send failure into disconnected state without throwing", () => {
    const voice = createVoiceController()
    const runtime = createVoiceRuntime({ controller: voice, send: () => { throw new Error("engine down") }, currentSession: () => undefined,
      currentModel: () => undefined, insert: () => true, notice: () => {}, speak: () => {} })
    expect(() => runtime.listen()).not.toThrow()
    expect(voice.state).toBe("disconnected")
  })
})
