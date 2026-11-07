import { spawn } from "node:child_process"
import { access, readFile } from "node:fs/promises"
import { constants } from "node:fs"
import { homedir } from "node:os"
import { delimiter, join } from "node:path"
import type { EngineInfo, Health } from "./protocol.ts"

export function discoveryPath(env: NodeJS.ProcessEnv = process.env, platform = process.platform): string {
  if (env.VOICE_ENGINE_HOME) return join(env.VOICE_ENGINE_HOME, "engine.json")
  if (platform === "darwin") return join(homedir(), "Library", "Application Support", "voice-engine", "engine.json")
  if (env.XDG_RUNTIME_DIR) return join(env.XDG_RUNTIME_DIR, "voice-engine", "engine.json")
  return join(env.XDG_DATA_HOME || join(homedir(), ".local", "share"), "voice-engine", "engine.json")
}

export async function readEngineInfo(path = discoveryPath()): Promise<EngineInfo> {
  const value = JSON.parse(await readFile(path, "utf8")) as EngineInfo
  if (value.protocol !== 1 || !Number.isInteger(value.port) || typeof value.token !== "string") throw new Error("Invalid voice engine discovery file")
  return value
}

export async function healthCheck(info?: EngineInfo): Promise<Health | undefined> {
  try {
    const engine = info ?? await readEngineInfo()
    const response = await fetch(`http://127.0.0.1:${engine.port}/v1/health`, { signal: AbortSignal.timeout(1500) })
    if (!response.ok) return undefined
    const health = await response.json() as Health
    return health.ok && health.protocol === 1 ? health : undefined
  } catch { return undefined }
}

export async function resolveEngineBinary(binary?: string): Promise<string> {
  const candidates = [binary, process.env.VOICE_ENGINE_BIN, join(homedir(), ".local", "bin", "voice-engine"), "/opt/homebrew/bin/voice-engine", "/usr/local/bin/voice-engine"]
  for (const candidate of candidates) {
    if (!candidate) continue
    try { await access(candidate, constants.X_OK); return candidate } catch { /* try next candidate */ }
  }
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    const candidate = join(dir, "voice-engine")
    try { await access(candidate, constants.X_OK); return candidate } catch { /* try next candidate */ }
  }
  throw new Error("voice-engine was not found; install it or set VOICE_ENGINE_BIN")
}

export async function ensureEngine(options: { binary?: string; timeoutMs?: number } = {}): Promise<EngineInfo> {
  const current = await readEngineInfo().catch(() => undefined)
  if (current && await healthCheck(current)) return current
  const binary = await resolveEngineBinary(options.binary)
  const child = spawn(binary, ["serve", "--if-not-running"], { detached: true, stdio: "ignore", env: process.env })
  child.unref()
  const deadline = Date.now() + (options.timeoutMs ?? 15000)
  while (Date.now() < deadline) {
    const info = await readEngineInfo().catch(() => undefined)
    if (info && await healthCheck(info)) return info
    await new Promise(resolve => setTimeout(resolve, 150))
  }
  throw new Error(`voice-engine did not become healthy within ${options.timeoutMs ?? 15000} ms`)
}
