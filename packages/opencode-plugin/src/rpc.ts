import { Rpc } from "@opencode/plugin/rpc"
const engineInfo = { type: "object", properties: { running: { type: "boolean" }, port: { type: "number" }, version: { type: "string" } }, required: ["running"], additionalProperties: false } as const
export const LocalVoice = Rpc.define({
  id: "local-voice",
  methods: {
    engine: { input: { type: "object", additionalProperties: false } as const, output: engineInfo },
    ensure: { input: { type: "object", additionalProperties: false } as const, output: engineInfo },
  },
  events: {},
})
