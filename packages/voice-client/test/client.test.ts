import { describe, expect, test } from "bun:test"
import { connect } from "../src/client.ts"
import { discoveryPath } from "../src/discovery.ts"

let loopbackAvailable = true
try {
  const probe = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response("ok") })
  probe.stop(true)
} catch (error) {
  loopbackAvailable = false
  console.warn(`Skipping fake WebSocket server test; loopback bind is unavailable: ${String(error)}`)
}

describe("voice client", () => {
  test.skipIf(!loopbackAvailable)("uses token query and dispatches typed messages from a fake server", async () => {
    let seenUrl = ""
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request, server) {
      if (new URL(request.url).pathname === "/v1/ws") {
        seenUrl = request.url
        if (server.upgrade(request)) return
      }
      return new Response("ok")
    }, websocket: { message(ws) { ws.send(JSON.stringify({ type: "pong", nonce: 8 })) } } })
    try {
      const client = connect(`ws://127.0.0.1:${server.port}/v1/ws`, "unit-token")
      const pong = new Promise<number>(resolve => client.on("pong", message => resolve(message.nonce)))
      client.send({ type: "ping", nonce: 8 })
      expect(await pong).toBe(8)
      expect(seenUrl).toContain("token=unit-token")
      client.close()
    } finally { server.stop(true) }
  })

  test("resolves the engine discovery override", () => {
    expect(discoveryPath({ VOICE_ENGINE_HOME: "/tmp/custom" }, "linux")).toBe("/tmp/custom/engine.json")
  })
})
