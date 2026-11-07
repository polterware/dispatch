// End-to-end check of a running engine: push-to-talk over a WAV test input
// (start the engine with VOICE_ENGINE_TEST_INPUT=<wav>), then speech output.
// Usage: bun benchmarks/e2e.ts [--hold-ms 4800] [--speak "text"] [--hands-free]
import { connect, readEngineInfo } from "@local-voice/client"
import type { EngineMessage } from "@local-voice/client"

const args = process.argv.slice(2)
const flag = (name: string, fallback: string) => {
  const index = args.indexOf(name)
  return index >= 0 && args[index + 1] ? args[index + 1] : fallback
}
const holdMs = Number(flag("--hold-ms", "4800"))
const speakText = flag("--speak", "Pronto, criei o arquivo e todos os testes passaram. Encontrei dois avisos no componente de botão.")
const handsFree = args.includes("--hands-free")

const info = await readEngineInfo()
if (!info) throw new Error("no running engine (discovery file missing)")
const client = connect(`ws://127.0.0.1:${info.port}/v1/ws`, info.token)

const wait = <T extends EngineMessage["type"]>(type: T, timeoutMs = 30_000) =>
  new Promise<Extract<EngineMessage, { type: T }>>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timeout waiting for ${type}`)), timeoutMs)
    const off = client.on(type, (message: any) => {
      clearTimeout(timer)
      off()
      resolve(message)
    })
  })

const levels = { mic: 0, output: 0 }
client.on("level", (message: any) => void (levels[message.source as "mic" | "output"] += 1))
client.on("error", (message: any) => console.error("engine error:", message.code, message.message))

const hello = await wait("hello")
console.log(`engine ${hello.engine_version}, protocol ${hello.protocol}`)

client.send({ type: "models.warm", stt: true, tts: true })
await new Promise(r => setTimeout(r, 4000))

const id = crypto.randomUUID()
const startedAt = performance.now()
client.send({ type: "listen.start", id, language: "pt-br", mode: handsFree ? "hands-free" : "push-to-talk" })
if (!handsFree) {
  await new Promise(r => setTimeout(r, holdMs))
  client.send({ type: "listen.stop", id })
}
const stoppedAt = performance.now()
const final = await wait("transcript.final")
const finalAt = performance.now()
console.log(JSON.stringify({
  transcript: final.text,
  language: final.language,
  timings: final.timings,
  client_stop_to_final_ms: Math.round(finalAt - stoppedAt),
  client_total_ms: Math.round(finalAt - startedAt),
  mic_level_messages: levels.mic,
}, null, 2))

const speakId = crypto.randomUUID()
const requestAt = performance.now()
client.send({ type: "speak", id: speakId, text: speakText, format: "markdown", language: "pt-br" })
const started = await wait("speak.started", 60_000)
const startedClientMs = Math.round(performance.now() - requestAt)
const ended = await wait("speak.ended", 120_000)
console.log(JSON.stringify({
  speak_first_audio_ms: started.first_audio_ms,
  client_first_audio_ms: startedClientMs,
  speak_end_reason: ended.reason,
  speak_total_ms: Math.round(performance.now() - requestAt),
  output_level_messages: levels.output,
}, null, 2))
client.close()
process.exit(0)
