// The RPC between the Dispatch plugin's server part and its OpenCode Desktop
// part: the delegation switch and the runs a conversation started.
// `dispatch setup opencode` installs this file and overwrites it on update;
// the GUI bundle builds it in, and the tests keep the two equal.

const state = {
  type: "object",
  properties: {
    enabled: { type: "boolean" },
    binary: { type: "string" },
    available: { type: "boolean" },
  },
  required: ["enabled", "binary", "available"],
  additionalProperties: false,
}

export const Dispatch = {
  id: "dispatch",
  methods: {
    // OpenCode reads every method's input, so one with none takes an empty object.
    state: { input: { type: "object", additionalProperties: false }, output: state },
    setEnabled: {
      input: {
        type: "object",
        properties: { enabled: { type: "boolean" } },
        required: ["enabled"],
        additionalProperties: false,
      },
      output: state,
    },
    // Opens a dispatch:// link in the Dispatch app; the Desktop's openExternal
    // opens only web links.
    open: {
      input: {
        type: "object",
        properties: { url: { type: "string" } },
        required: ["url"],
        additionalProperties: false,
      },
      output: {
        type: "object",
        properties: { opened: { type: "boolean" }, error: { type: "string" } },
        required: ["opened"],
        additionalProperties: false,
      },
    },
    runs: {
      input: {
        type: "object",
        properties: { sessionID: { type: "string" }, directory: { type: "string" } },
        required: ["sessionID"],
        additionalProperties: false,
      },
      output: {
        type: "object",
        properties: { runs: { type: "array", items: { type: "object" } }, error: { type: "string" } },
        required: ["runs"],
        additionalProperties: false,
      },
    },
    // The run's output so far, and the status the panel polls on: the Desktop
    // part shows it in place instead of only opening the run in the app.
    log: {
      input: {
        type: "object",
        properties: {
          runID: { type: "string" },
          directory: { type: "string" },
          offset: { type: "integer" },
        },
        required: ["runID"],
        additionalProperties: false,
      },
      output: {
        type: "object",
        properties: {
          status: { type: "string" },
          text: { type: "string" },
          lines: { type: "integer" },
          error: { type: "string" },
        },
        required: ["status", "text", "lines"],
        additionalProperties: false,
      },
    },
  },
  events: {
    changed: {
      schema: {
        type: "object",
        properties: { enabled: { type: "boolean" } },
        required: ["enabled"],
        additionalProperties: false,
      },
    },
    started: {
      schema: {
        type: "object",
        properties: { sessionID: { type: "string" }, runID: { type: "string" } },
        required: ["sessionID", "runID"],
        additionalProperties: false,
      },
    },
  },
}

export default Dispatch
