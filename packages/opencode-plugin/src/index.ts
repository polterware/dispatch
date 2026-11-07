import { Plugin } from "@opencode/plugin"
import { healthCheck, ensureEngine, readEngineInfo } from "@local-voice/client"
import { LocalVoice } from "./rpc.ts"

export default Plugin.define({
  id: "local-voice",
  async setup(ctx) {
    return await ctx.rpc.register(LocalVoice, {
      engine: async () => {
        const info = await readEngineInfo().catch(() => undefined)
        const healthy = info && await healthCheck(info)
        return healthy ? { running: true, port: info.port, version: healthy.engine_version } : { running: false }
      },
      ensure: async () => {
        const info = await ensureEngine()
        return { running: true, port: info.port, version: info.engine_version }
      },
    })
  },
})
