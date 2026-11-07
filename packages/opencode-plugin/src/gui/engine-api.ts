import type { Catalog, EngineMessage, EngineSettings, SettingsPatch } from "@local-voice/client"

/** The window's side of the main-process bridge (EngineBridge in contract.ts). */
export interface Bridge {
  ensure(): Promise<void>
  connect(): Promise<void>
  disconnect(): Promise<void>
  send(input: { json: string }): Promise<void>
  request(input: { method: string; path: string; body?: string }): Promise<{ status: number; body: string }>
  on(event: "message", listener: (json: string) => void): () => void
}

export class EngineError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

/**
 * One link to the engine per window: a single socket shared by voice mode and the settings page,
 * with messages fanned out to every subscriber.
 */
export function createEngineLink(bridge: Bridge) {
  const listeners = new Set<(message: EngineMessage) => void>()
  let users = 0
  let removeListener: (() => void) | undefined

  const call = async <T>(method: string, path: string, body?: unknown): Promise<T> => {
    const response = await bridge.request({ method, path, body: body === undefined ? undefined : JSON.stringify(body) })
    if (response.status >= 400) throw new EngineError(response.status, response.body || `HTTP ${response.status}`)
    return (response.body ? JSON.parse(response.body) : undefined) as T
  }

  return {
    /** Starts the engine if needed and keeps the socket open until every `acquire` is released. */
    async acquire(): Promise<() => void> {
      users += 1
      await bridge.ensure()
      if (!removeListener) {
        removeListener = bridge.on("message", json => {
          let message: EngineMessage
          try {
            message = JSON.parse(json) as EngineMessage
          } catch {
            return
          }
          for (const listener of listeners) listener(message)
        })
      }
      await bridge.connect()
      let released = false
      return () => {
        if (released) return
        released = true
        users -= 1
        if (users <= 0) {
          users = 0
          removeListener?.()
          removeListener = undefined
          void bridge.disconnect().catch(() => {})
        }
      }
    },
    subscribe(listener: (message: EngineMessage) => void) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    send(message: Record<string, unknown>) {
      return bridge.send({ json: JSON.stringify(message) })
    },
    catalog: () => call<Catalog>("GET", "/v1/catalog"),
    settings: () => call<EngineSettings>("GET", "/v1/settings"),
    patchSettings: (patch: SettingsPatch) => call<EngineSettings>("PATCH", "/v1/settings", patch),
    download: (id: string) => call<void>("POST", `/v1/models/${encodeURIComponent(id)}/download`),
    remove: (id: string) => call<void>("DELETE", `/v1/models/${encodeURIComponent(id)}`),
  }
}

export type EngineLink = ReturnType<typeof createEngineLink>

export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`
  if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`
  return `${Math.max(1, Math.round(bytes / 1e3))} KB`
}
