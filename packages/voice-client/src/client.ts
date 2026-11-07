import type { ClientMessage, EngineMessage } from "./protocol.ts"

type WebSocketLike = Pick<WebSocket, "send" | "close" | "addEventListener" | "removeEventListener"> & { readyState: number }
type Constructor = new (url: string) => WebSocketLike
export type EngineEventMap = { [K in EngineMessage["type"]]: Extract<EngineMessage, { type: K }> }
type Handler<K extends keyof EngineEventMap> = (message: EngineEventMap[K]) => void

export class VoiceClient {
  private socket?: WebSocketLike
  private handlers = new Map<string, Set<(message: EngineMessage) => void>>()
  private connectionHandlers = new Set<(connected: boolean) => void>()
  private closed = false
  private retry = 0
  private timer?: ReturnType<typeof setTimeout>
  private generation = 0
  private pending: ClientMessage[] = []
  private constructor(private url: string, private token: string, private WebSocketCtor: Constructor) {}

  static connect(url: string, token: string): VoiceClient {
    const ctor = globalThis.WebSocket as unknown as Constructor | undefined
    if (!ctor) throw new Error("This runtime does not provide WebSocket")
    const client = new VoiceClient(url, token, ctor)
    client.open()
    return client
  }

  on<K extends keyof EngineEventMap>(type: K, handler: Handler<K>): () => void {
    let set = this.handlers.get(type)
    if (!set) this.handlers.set(type, set = new Set())
    set.add(handler as (message: EngineMessage) => void)
    return () => set?.delete(handler as (message: EngineMessage) => void)
  }

  onConnectionChange(handler: (connected: boolean) => void): () => void {
    this.connectionHandlers.add(handler)
    return () => this.connectionHandlers.delete(handler)
  }

  send(message: ClientMessage): void {
    if (this.closed) throw new Error("Voice engine WebSocket is closed")
    if (this.socket?.readyState !== 1) { this.pending.push(message); return }
    this.socket.send(JSON.stringify(message))
  }

  close(): void {
    this.closed = true
    this.generation++
    if (this.timer) clearTimeout(this.timer)
    this.socket?.close()
    this.socket = undefined
  }

  private open(): void {
    if (this.closed) return
    const generation = ++this.generation
    const url = new URL(this.url)
    url.searchParams.set("token", this.token)
    const socket = new this.WebSocketCtor(url.toString())
    this.socket = socket
    socket.addEventListener("open", () => {
      if (generation !== this.generation) return
      this.retry = 0
      for (const handler of this.connectionHandlers) handler(true)
      for (const message of this.pending.splice(0)) socket.send(JSON.stringify(message))
    })
    socket.addEventListener("message", event => {
      if (generation !== this.generation) return
      try {
        const parsed = JSON.parse(String((event as MessageEvent).data)) as EngineMessage
        for (const handler of this.handlers.get(parsed.type) ?? []) handler(parsed)
      } catch { /* malformed frames are ignored */ }
    })
    socket.addEventListener("close", () => {
      if (generation !== this.generation) return
      for (const handler of this.connectionHandlers) handler(false)
      this.reconnect(generation)
    })
    socket.addEventListener("error", () => { /* close event schedules reconnect */ })
  }

  private reconnect(generation: number): void {
    if (this.closed || generation !== this.generation) return
    const delay = Math.min(10000, 250 * 2 ** this.retry++)
    this.timer = setTimeout(() => this.open(), delay)
  }
}

export function connect(url: string, token: string): VoiceClient { return VoiceClient.connect(url, token) }
