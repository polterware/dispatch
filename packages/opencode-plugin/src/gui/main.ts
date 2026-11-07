import { Plugin } from "@opencode/plugin/gui"
import { ensureEngine } from "@local-voice/client"
import { connect, type VoiceClient } from "@local-voice/client"
import type { ClientMessage, EngineMessage } from "@local-voice/client"
import { EngineBridge } from "./contract.ts"

export const main: Plugin.MainSetup = ctx => {
  const sockets = new Map<number, VoiceClient>()
  const removers = new Map<number, Array<() => void>>()
  const close = (window: number) => {
    for (const remove of removers.get(window) ?? []) remove()
    removers.delete(window)
    sockets.get(window)?.close()
    sockets.delete(window)
  }
  const provider = ctx.provide(EngineBridge, {
    ensure: async (_input, _caller) => { await ensureEngine() },
    connect: async (_input, caller) => {
      close(caller.window)
      const engine = await ensureEngine()
      const client = connect(`ws://127.0.0.1:${engine.port}/v1/ws`, engine.token)
      sockets.set(caller.window, client)
      const unsubs = (['hello', 'state', 'level', 'vad', 'transcript.partial', 'transcript.final', 'speak.started', 'speak.ended', 'pong', 'error'] as const)
        .map(type => client.on(type, (message: EngineMessage) => provider.emit('message', JSON.stringify(message), caller.window)))
      unsubs.push(client.onConnectionChange(connected => provider.emit('message', JSON.stringify({ type: 'connection', connected }), caller.window)))
      removers.set(caller.window, unsubs)
    },
    disconnect: (_input, caller) => close(caller.window),
    send: (input, caller) => {
      const message = JSON.parse(input.json) as ClientMessage
      sockets.get(caller.window)?.send(message)
    },
  })
  ctx.windows.on('close', window => close(window.id))
  ctx.onCleanup(() => {
    for (const window of ctx.windows.list()) close(window.id)
    for (const window of [...sockets.keys()]) close(window)
    provider.dispose()
  })
}
