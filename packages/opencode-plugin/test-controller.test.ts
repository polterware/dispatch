import { describe, expect, test } from "bun:test"
import { createVoiceController } from "./src/gui/controller.ts"
import { extractResponseText } from "./src/gui/response-text.ts"

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
