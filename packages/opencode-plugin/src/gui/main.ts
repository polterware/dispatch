import { Plugin } from "@opencode/plugin/gui"
import { ensureEngine } from "@local-voice/client"
import { connect, type VoiceClient } from "@local-voice/client"
import type { ClientMessage, EngineMessage } from "@local-voice/client"
import { EngineBridge } from "./contract.ts"

// Caller.window and provider.emit(..., window) are documented GUI IPC APIs.
export const main: Plugin.MainSetup = ctx => {
  const sockets = new Map<number, VoiceClient>()
  const close = (window: number) => { sockets.get(window)?.close(); sockets.delete(window) }
  const provider = ctx.provide(EngineBridge, {
    ensure: async (_input, _caller) => {
      const engine = await ensureEngine()
      return JSON.stringify(engine)
    },
    connect: async (_input, caller) => {
      close(caller.window)
      const engine = await ensureEngine()
      const client = connect(`ws://127.0.0.1:${engine.port}/v1/ws`, engine.token)
      sockets.set(caller.window, client)
      client.on("hello", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("state", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("level", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("vad", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("transcript.partial", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("transcript.final", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("speak.started", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("speak.ended", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("pong", message => provider.emit("message", JSON.stringify(message), caller.window))
      client.on("error", message => provider.emit("message", JSON.stringify(message), caller.window))
      return JSON.stringify(engine)
    },
    disconnect: (_input, caller) => close(caller.window),
    send: (input, caller) => {
      const message = JSON.parse(input.json) as ClientMessage
      sockets.get(caller.window)?.send(message)
    },
  })
  const onClose = (window: Electron.BrowserWindow) => close(window.id)
  ctx.windows.on("close", onClose)
  ctx.onCleanup(() => { ctx.windows.list().forEach(window => close(window.id)); sockets.clear(); provider.dispose() })
}
