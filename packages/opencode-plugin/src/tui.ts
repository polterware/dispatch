import { Plugin } from "@opencode/plugin/tui"
import { ensureEngine, connect } from "@local-voice/client"
import type { VoiceClient } from "@local-voice/client"

export default Plugin.define({
  id: "local-voice",
  setup(ctx) {
    let client: VoiceClient | undefined
    let activeID: string | undefined
    let busy = false
    ctx.keymap.layer(() => ({ commands: [{
      id: "local-voice.talk",
      title: "Voice: talk",
      description: "Start or stop local push-to-talk capture",
      group: "Voice",
      bind: false,
      palette: true,
      slash: { name: "voice" },
      enabled: () => !!ctx.location && !busy,
      async run() {
        if (busy) return
        busy = true
        try {
          if (!client) {
            const info = await ensureEngine()
            client = connect(`ws://127.0.0.1:${info.port}/v1/ws`, info.token)
            client.on("transcript.final", message => {
              if (!message.text.trim()) return
              void (async () => {
                const route = ctx.ui.router.current()
                if (route.type !== "session") {
                  ctx.ui.toast.show({ message: `Transcript ready: ${message.text.slice(0, 160)}. Open a session to submit it.`, variant: "success" })
                  return
                }
                const model = ctx.ui.model.current()
                if (model) await ctx.client.session.switchModel({ sessionID: route.sessionID, model: { id: model.modelID, providerID: model.providerID, variant: model.variant } })
                await ctx.client.session.prompt({ sessionID: route.sessionID, text: message.text })
                ctx.ui.toast.show({ message: "Voice transcript submitted", variant: "success" })
              })().catch(error => ctx.ui.toast.show({ message: `Could not submit voice transcript: ${String(error)}`, variant: "error" }))
            })
          }
          if (activeID) {
            client.send({ type: "listen.stop", id: activeID })
            activeID = undefined
            ctx.ui.toast.show({ message: "Transcribing voice input", variant: "info" })
          } else {
            activeID = crypto.randomUUID()
            client.send({ type: "listen.start", id: activeID, mode: "push-to-talk", language: "auto" })
            ctx.ui.toast.show({ message: "Listening; run Voice: talk again to stop", variant: "info" })
          }
        } catch (error) {
          activeID = undefined
          ctx.ui.toast.show({ message: `Voice engine unavailable: ${String(error)}`, variant: "error" })
        } finally { busy = false }
      },
    }] }))
    return () => { client?.close(); client = undefined; activeID = undefined }
  },
})
