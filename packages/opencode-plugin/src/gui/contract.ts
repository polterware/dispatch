import { Schema } from "effect"
import { Ipc } from "@opencode/plugin/gui"

// The exact Ipc.define, caller-window, provider.emit, and cleanup shapes are documented by the GUI plugin API.
export const EngineBridge = Ipc.define({
  id: "local-voice.engine",
  methods: {
    ensure: { input: Schema.Struct({}), output: Schema.String },
    connect: { input: Schema.Struct({}), output: Schema.String },
    disconnect: { input: Schema.Struct({}), output: Schema.Void },
    send: { input: Schema.Struct({ json: Schema.String }), output: Schema.Void },
  },
  events: { message: Schema.String },
})
