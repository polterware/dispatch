import { Schema } from "effect"
import { Ipc } from "@opencode/plugin/gui"

export const EngineBridge = Ipc.define({
  id: "local-voice.engine",
  methods: {
    ensure: { output: Schema.Void },
    connect: { output: Schema.Void },
    disconnect: { output: Schema.Void },
    send: { input: Schema.Struct({ json: Schema.String }), output: Schema.Void },
  },
  events: { message: Schema.String },
})
