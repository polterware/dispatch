import { Schema } from "effect"
import { Ipc } from "@opencode/plugin/gui"

export const EngineBridge = Ipc.define({
  id: "local-voice.engine",
  methods: {
    ensure: { output: Schema.Void },
    connect: { output: Schema.Void },
    disconnect: { output: Schema.Void },
    send: { input: Schema.Struct({ json: Schema.String }), output: Schema.Void },
    /** An authenticated HTTP call to the engine's local API (path starts with /v1/). */
    request: {
      input: Schema.Struct({ method: Schema.String, path: Schema.String, body: Schema.optional(Schema.String) }),
      output: Schema.Struct({ status: Schema.Number, body: Schema.String }),
    },
  },
  events: { message: Schema.String },
})
