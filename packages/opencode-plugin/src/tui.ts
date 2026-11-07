import { Command, Plugin } from "@opencode/plugin/tui"
import { ensureEngine, connect } from "@local-voice/client"
import type { VoiceClient } from "@local-voice/client"

export default Plugin.define({
  id: "local-voice",
  setup(ctx) {
    let client: VoiceClient | undefined
    let activeID: string | undefined
    ctx.add(Command, () => ({
      id: "local-voice.talk",
      title: "Voice: talk",
      async run() {
        try {
          if (!client) {
            const info = await ensureEngine()
            client = connect(`ws://127.0.0.1:${info.port}/v1/ws`, info.token)
            client.on("transcript.final", message => {
              if (!message.text.trim()) return
              ctx.ui.toast.show({ message: `Voice transcript ready: ${message.text.slice(0, 80)}`, variant: "success" })
              // The published TUI API exposes no composer insertion or prompt submission method.
            })
          }
          if (activeID) { client.send({ type: "listen.stop", id: activeID }); activeID = undefined; ctx.ui.toast.show({ message: "Transcribing voice input", variant: "info" }); return }
          activeID = crypto.randomUUID()
          client.send({ type: "listen.start", id: activeID, mode: "push-to-talk", language: "auto" })
          ctx.ui.toast.show({ message: "Listening; run Voice: talk again to stop", variant: "info" })
        } catch (error) { ctx.ui.toast.show({ message: `Voice engine unavailable: ${String(error)}`, variant: "error" }) }
      },
    }))
    return () => client?.close()
  },
})
