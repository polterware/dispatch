// Runs the OpenCode plugin with fake contexts and a fake dispatch binary:
// `node --test crates/dispatch-core/src/setup/opencode/`.
import assert from "node:assert/strict"
import fs from "node:fs"
import Module from "node:module"
import os from "node:os"
import path from "node:path"
import { afterEach, beforeEach, test } from "node:test"
import { fileURLToPath } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))
const require = Module.createRequire(import.meta.url)

const FAKE_DISPATCH = `#!/usr/bin/env node
const fs = require("node:fs")
const path = require("node:path")
const args = process.argv.slice(2)
const log = process.env.FAKE_DISPATCH_LOG
const input = args.includes("-") ? fs.readFileSync(0, "utf8") : ""
fs.appendFileSync(log, JSON.stringify({ args, cwd: process.cwd(), input, session: process.env.DISPATCH_CALLER_SESSION || "", agent: process.env.DISPATCH_CALLER_AGENT || "" }) + "\\n")
const out = (value) => process.stdout.write(JSON.stringify(value) + "\\n")
switch (args[0]) {
  case "help":
    process.exit(2)
  case "run":
    out({ id: "r1", status: "PENDING", agent: "codex", model: "gpt-6" })
    break
  case "wait": {
    const counter = path.join(path.dirname(log), "waits")
    const n = fs.existsSync(counter) ? Number(fs.readFileSync(counter, "utf8")) : 0
    fs.writeFileSync(counter, String(n + 1))
    if (n === 0) {
      out({ id: args[1], status: "RUNNING" })
      process.exit(3)
    }
    out({ id: args[1], status: "COMPLETED", worktree: "/wt/r1" })
    break
  }
  case "events":
    if (process.env.FAKE_DISPATCH_NO_EVENTS) {
      process.stderr.write('unknown command "events"\\nusage: dispatch ...\\n')
      process.exit(2)
    }
    // A run read from its end: its first event is cut at the read's offset.
    if (args[1] === "r2") {
      out({ status: "COMPLETED", more: true, events: [{ seq: 9, offset: 12, kind: "output", text: "cut head\\nwhole line\\n" }] })
      break
    }
    // A run that is not there yet: the command exists and says so.
    if (args[1] === "r3") {
      process.stderr.write("run not found\\n")
      process.exit(1)
    }
    // Live: started repeats the prompt, an agent's output comes as it is,
    // and any other kind is one line.
    out({
      status: "RUNNING",
      more: false,
      events: [
        { seq: 1, offset: 0, kind: "started", text: "Fix the parser" },
        { seq: 2, offset: 0, kind: "output", text: "\\u001b[32mline one\\u001b[0m\\n" },
        { seq: 3, offset: 10, kind: "merged", text: "three\\n" },
        { seq: 4, offset: 20, kind: "agent", text: "four" },
      ],
    })
    break
  case "result":
    if (!args.includes("--json")) {
      process.stdout.write("Done: changed two files.\\n")
      break
    }
    out({
      id: args[1],
      status: "RUNNING",
      section: "output",
      offset: 0,
      limit: 2000,
      total_lines: 2500,
      has_more: false,
      content: "\\u001b[32mline one\\u001b[0m\\nline two\\n[merged] three",
    })
    break
  case "runs":
    out({ runs: [{ ID: "r1", Status: "RUNNING", Agent: "codex", Model: "gpt-6", Prompt: "Fix the parser\\nmore", Usage: { inputTokens: 1000, cacheReadTokens: 500, outputTokens: 200 }, FilesChanged: 2, Insertions: 10, Deletions: 3 }] })
    break
  case "agents":
    out({ agents: [
      { Name: "codex", Bin: "codex", Provider: "openai", UsedPercent: 0, CooldownUntil: null, Enabled: true },
      { Name: "opencode", Bin: "opencode", Provider: "opencode", UsedPercent: null, CooldownUntil: null, Enabled: true },
      { Name: "gemini", Bin: "gemini", Provider: "google", UsedPercent: null, CooldownUntil: null, Enabled: false },
    ] })
    break
  case "models":
    out({ entries: [
      { agent: "opencode", model: "deepseek/deepseek-flash" },
      { agent: "codex", model: "gpt-6-astra" },
      { agent: "codex", model: "gpt-6-astra" },
      { agent: "codex", model: "gpt-6-luna" },
      { agent: "gemini", model: "gemini-3-flash" },
    ] })
    break
  case "merge":
    process.stdout.write("merged r1\\n")
    break
  case "cancel":
    process.stdout.write("cancelled\\n")
    break
  default:
    process.exit(2)
}
`

let tmp

beforeEach(() => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dispatch-opencode-"))
  fs.writeFileSync(path.join(tmp, "dispatch"), FAKE_DISPATCH, { mode: 0o755 })
  fs.mkdirSync(path.join(tmp, "repo"))
  process.env.FAKE_DISPATCH_LOG = path.join(tmp, "calls.log")
  process.env.DISPATCH_OPENCODE_WAIT_SLICE = "1"
})

afterEach(() => {
  fs.rmSync(tmp, { recursive: true, force: true })
})

function calls() {
  if (!fs.existsSync(process.env.FAKE_DISPATCH_LOG)) return []
  return fs.readFileSync(process.env.FAKE_DISPATCH_LOG, "utf8").trim().split("\n").map((line) => JSON.parse(line))
}

async function loadServer(options = {}) {
  const { default: plugin } = await import(path.join(here, "server.js"))
  const tools = {}
  const hooks = {}
  const emitted = []
  let impl
  const sessions = {
    child: { id: "child", parentID: "root", location: { directory: path.join(tmp, "repo") } },
    root: { id: "root", location: { directory: path.join(tmp, "repo") } },
  }
  // A second server part over the same storage reads what the first persisted.
  const stored = options.storage || new Map()
  const ctx = {
    options: { binary: path.join(tmp, "dispatch"), state: path.join(tmp, "state", "opencode-plugin.json") },
    storage: {
      get: async (key) => stored.get(key),
      set: async (key, value) => void stored.set(key, value),
    },
    location: { directory: tmp },
    session: {
      get: async ({ sessionID }) => {
        if (!sessions[sessionID]) throw new Error("not found")
        return sessions[sessionID]
      },
      hook: async (name, fn) => {
        hooks[name] = fn
      },
    },
    rpc: {
      register: async (definition, implementation) => {
        assert.equal(definition.id, "dispatch")
        impl = implementation
        return { events: { emit: async (name, data) => emitted.push({ name, data }) } }
      },
    },
    tool: {
      transform: async (fn) => {
        fn({ add: (tool) => (tools[tool.name] = tool) })
      },
    },
  }
  await plugin.setup(ctx)
  return { plugin, tools, hooks, emitted, impl: () => impl, ctx, stored }
}

function contextEvent() {
  return {
    sessionID: "child",
    system: [],
    // The tools a build agent has: reads, the editor, the shell, OpenCode's
    // subagent, and Dispatch's three.
    tools: {
      read: {},
      edit: {},
      write: {},
      bash: {},
      subagent: {},
      dispatch_run: {},
      dispatch_wait: {},
      dispatch_merge: {},
    },
  }
}

test("delegation on swaps the subagent tool for Dispatch's", async () => {
  const { plugin, tools, hooks } = await loadServer()
  assert.equal(plugin.id, "dispatch")
  assert.deepEqual(Object.keys(tools).sort(), ["dispatch_merge", "dispatch_run", "dispatch_wait"])
  const event = contextEvent()
  await hooks.context(event)
  assert.deepEqual(Object.keys(event.tools).sort(), ["bash", "dispatch_merge", "dispatch_run", "dispatch_wait", "edit", "read", "write"])
  assert.match(event.system[0].text, /dispatch_run/)
})

test("delegation off keeps OpenCode's subagents and hides Dispatch's tools", async () => {
  const { hooks, impl, emitted } = await loadServer()
  const answer = await impl().setEnabled({ enabled: false })
  assert.equal(answer.enabled, false)
  assert.equal(answer.available, true)
  assert.deepEqual(emitted.at(-1), { name: "changed", data: { enabled: false } })
  const event = contextEvent()
  await hooks.context(event)
  assert.deepEqual(Object.keys(event.tools).sort(), ["bash", "edit", "read", "subagent", "write"])
  assert.equal(event.system.length, 0)
  assert.equal(JSON.parse(fs.readFileSync(path.join(tmp, "state", "opencode-plugin.json"), "utf8")).enabled, false)

  await impl().setEnabled({ enabled: true })
  const again = contextEvent()
  await hooks.context(again)
  assert.equal("subagent" in again.tools, false)
})

test("the Delegate check round-trips for the root conversation and persists", async () => {
  const { impl, emitted, stored } = await loadServer()
  assert.deepEqual(await impl().force({ sessionID: "child" }), { force: false, agent: "", model: "" })
  assert.deepEqual(await impl().setForce({ sessionID: "child", directory: "", force: true }), { force: true, agent: "", model: "" })
  // The conversation is the root session, so a child session's check is the chat's.
  assert.deepEqual(emitted.at(-1), { name: "forced", data: { sessionID: "root", force: true, agent: "", model: "" } })
  assert.deepEqual(await impl().force({ sessionID: "root" }), { force: true, agent: "", model: "" })
  assert.deepEqual(stored.get("force/root"), { force: true, agent: "", model: "" })

  // A server part started again over the same storage reads the flag back.
  const restarted = await loadServer({ storage: stored })
  assert.deepEqual(await restarted.impl().force({ sessionID: "child" }), { force: true, agent: "", model: "" })
  assert.deepEqual(await restarted.impl().setForce({ sessionID: "root", force: false }), { force: false, agent: "", model: "" })
  assert.deepEqual(stored.get("force/root"), { force: false, agent: "", model: "" })
})

test("a forced target is stored with the check, and a toggle keeps it", async () => {
  const { impl, emitted, stored } = await loadServer()
  const pick = { agent: "opencode", model: "deepseek/deepseek-flash" }
  assert.deepEqual(await impl().setForce({ sessionID: "child", force: true, ...pick }), { force: true, ...pick })
  assert.deepEqual(emitted.at(-1), { name: "forced", data: { sessionID: "root", force: true, ...pick } })
  assert.deepEqual(stored.get("force/root"), { force: true, ...pick })

  // Ticking or unticking the check leaves the target in place.
  assert.deepEqual(await impl().setForce({ sessionID: "root", force: false }), { force: false, ...pick })
  assert.deepEqual(await impl().setForce({ sessionID: "root", force: true }), { force: true, ...pick })

  // Auto passes empty strings and clears it.
  assert.deepEqual(await impl().setForce({ sessionID: "root", force: true, agent: "", model: "" }), { force: true, agent: "", model: "" })

  // A value that could read as a flag is dropped instead of stored.
  assert.deepEqual(await impl().setForce({ sessionID: "root", force: true, agent: "--force", model: "x" }), { force: true, agent: "", model: "" })

  // A server part started again over the same storage reads the target back.
  await impl().setForce({ sessionID: "root", force: true, ...pick })
  const restarted = await loadServer({ storage: stored })
  assert.deepEqual(await restarted.impl().force({ sessionID: "child" }), { force: true, ...pick })
})

test("targets lists each agent's models for the forced target pill", async () => {
  const { impl } = await loadServer()
  const answer = await impl().targets({})
  // A disabled agent is left out, as is one with no model; the duplicates are one row.
  assert.deepEqual(answer.agents, [
    { name: "codex", models: ["gpt-6-astra", "gpt-6-luna"] },
    { name: "opencode", models: ["deepseek/deepseek-flash"] },
  ])
  assert.equal(answer.error, undefined)
  assert.deepEqual(
    calls().map((call) => call.args.slice(0, 2).join(" ")).filter((args) => /^(agents|models) /.test(args)).sort(),
    ["agents --json", "models --json"],
  )
})

test("targets reports a CLI that fails", async () => {
  const { impl, ctx } = await loadServer()
  fs.rmSync(ctx.options.binary)
  const savedPath = process.env.PATH
  const savedHome = process.env.HOME
  process.env.PATH = "/nonexistent"
  process.env.HOME = tmp
  try {
    const answer = await impl().targets({})
    assert.deepEqual(answer.agents, [])
    assert.ok(answer.error)
  } finally {
    process.env.PATH = savedPath
    process.env.HOME = savedHome
  }
})

test("a forced conversation is told to delegate and loses the work tools", async () => {
  const { hooks, impl } = await loadServer()
  const plain = contextEvent()
  await hooks.context(plain)
  assert.equal(plain.system.some((part) => /MUST go through dispatch_run/.test(part.text)), false)

  await impl().setForce({ sessionID: "child", force: true })
  const event = contextEvent()
  await hooks.context(event)
  assert.ok(event.system.some((part) => /forced to delegate/.test(part.text)))
  assert.ok(event.system.some((part) => /MUST go through dispatch_run/.test(part.text)))
  // The editor and the shell go; planning keeps its reads.
  for (const name of ["edit", "write", "bash"]) assert.equal(name in event.tools, false, name)
  assert.equal("read" in event.tools, true)
  assert.equal("dispatch_run" in event.tools, true)

  await impl().setForce({ sessionID: "root", force: false })
  const again = contextEvent()
  await hooks.context(again)
  assert.equal(again.system.length, 1)
  assert.ok("write" in again.tools)
})

test("a forced conversation is left alone while delegation is off", async () => {
  const { hooks, impl } = await loadServer()
  await impl().setForce({ sessionID: "root", force: true })
  await impl().setEnabled({ enabled: false })
  const event = contextEvent()
  await hooks.context(event)
  assert.equal(event.system.length, 0)
  assert.ok("bash" in event.tools)
  assert.ok("subagent" in event.tools)
})

test("a forced conversation with a target names it and sends every run there", async () => {
  const { hooks, impl, tools } = await loadServer()
  await impl().setForce({ sessionID: "root", force: true, agent: "opencode", model: "deepseek/deepseek-flash" })
  const event = contextEvent()
  await hooks.context(event)
  const forced = event.system.find((part) => /forced to delegate/.test(part.text))
  assert.ok(forced)
  assert.match(forced.text, /opencode · deepseek\/deepseek-flash/)

  await tools.dispatch_run.execute(
    { prompt: "Fix the parser", tier: "easy", agent: "codex", model: "gpt-6-astra" },
    { sessionID: "child", progress: async () => {} },
  )
  const [run] = calls().filter((call) => call.args[0] === "run")
  // The target wins over the call's own agent and model.
  assert.deepEqual(run.args, [
    "run",
    "--json",
    "--compact",
    "--dir",
    path.join(tmp, "repo"),
    "--agent",
    "opencode",
    "--model",
    "deepseek/deepseek-flash",
    "--tier",
    "easy",
    "-",
  ])
})

test("a forced conversation with no target keeps Auto", async () => {
  const { hooks, impl, tools } = await loadServer()
  await impl().setForce({ sessionID: "root", force: true })
  const event = contextEvent()
  await hooks.context(event)
  assert.equal(event.system.some((part) => /goes to/.test(part.text)), false)

  await tools.dispatch_run.execute({ prompt: "x", wait: false }, { sessionID: "child", progress: async () => {} })
  const [run] = calls().filter((call) => call.args[0] === "run")
  assert.equal(run.args.includes("--agent"), false)
  assert.equal(run.args.includes("--model"), false)
})

test("a call's own agent and model stand when the conversation is not forced", async () => {
  const { tools } = await loadServer()
  await tools.dispatch_run.execute({ prompt: "x", wait: false, agent: "codex", model: "gpt-6-astra" }, { sessionID: "root", progress: async () => {} })
  const [run] = calls().filter((call) => call.args[0] === "run")
  assert.deepEqual(run.args.slice(-5), ["--agent", "codex", "--model", "gpt-6-astra", "-"])
})

test("dispatch_run starts a run under the root conversation and waits for it", async () => {
  const { tools, emitted } = await loadServer()
  const progress = []
  const result = await tools.dispatch_run.execute(
    { prompt: "Fix the parser", tier: "easy", role: "implementer", readonly: false },
    { sessionID: "child", progress: async (update) => progress.push(update) },
  )
  const [run] = calls().filter((call) => call.args[0] === "run")
  assert.deepEqual(run.args, ["run", "--json", "--compact", "--dir", path.join(tmp, "repo"), "--tier", "easy", "--role", "implementer", "-"])
  assert.equal(run.input, "Fix the parser")
  assert.equal(run.session, "root")
  assert.equal(run.agent, "opencode")
  assert.equal(fs.realpathSync(run.cwd), fs.realpathSync(path.join(tmp, "repo")))
  assert.deepEqual(emitted.find((e) => e.name === "started").data, { sessionID: "root", runID: "r1" })
  assert.equal(calls().filter((call) => call.args[0] === "wait").length, 2)
  assert.match(result.content, /"status": "COMPLETED"/)
  assert.match(result.content, /Done: changed two files\./)
  assert.match(result.content, /dispatch_merge with id r1/)
  assert.ok(progress.some((update) => update.runID === "r1"))
})

test("dispatch_run with wait false answers the id at once", async () => {
  const { tools } = await loadServer()
  const result = await tools.dispatch_run.execute({ prompt: "x", wait: false }, { sessionID: "root", progress: async () => {} })
  assert.match(result.content, /dispatch_wait with id r1/)
  assert.equal(calls().filter((call) => call.args[0] === "wait").length, 0)
})

test("runs lists the root conversation's runs", async () => {
  const { impl } = await loadServer()
  const answer = await impl().runs({ sessionID: "child" })
  const call = calls().find((c) => c.args[0] === "runs")
  assert.deepEqual(call.args, ["runs", "--json", "--limit", "50", "--conversation", "opencode:root"])
  assert.equal(answer.runs.length, 1)
  assert.deepEqual(
    { id: answer.runs[0].id, status: answer.runs[0].status, tokens: answer.runs[0].tokens, files: answer.runs[0].filesChanged },
    { id: "r1", status: "RUNNING", tokens: 1700, files: 2 },
  )
})

test("log answers a run's events while it works, without escapes, and its status", async () => {
  const { impl } = await loadServer()
  const answer = await impl().log({ runID: "r1" })
  const [events] = calls().filter((call) => call.args[0] === "events")
  assert.deepEqual(events.args, ["events", "r1", "--json", "--tail-bytes", "262144"])
  assert.equal(answer.status, "RUNNING")
  // An agent's output as it came, other kinds as one `[kind]` line, and
  // `started`, which repeats the prompt, not at all.
  assert.equal(answer.text, "line one\n[merged] three\nfour")
  assert.equal(answer.lines, 3)
  assert.equal(Object.values(answer).some((value) => value === undefined), false)

  // A page read from the end starts at its first whole line.
  const tail = await impl().log({ runID: "r2" })
  assert.equal(tail.text, "whole line\n")
  assert.equal(tail.status, "COMPLETED")

  // A failure that is not the missing command is reported, not read as an
  // older binary.
  const missing = await impl().log({ runID: "r3" })
  assert.ok(missing.error)
  assert.equal(missing.text, "")
  assert.equal(calls().filter((call) => call.args[0] === "result").length, 0)

  const refused = await impl().log({ runID: "--force" })
  assert.ok(refused.error)
  assert.equal(refused.text, "")
  assert.equal(calls().filter((call) => call.args[0] === "events").length, 3)
})

test("log falls back to the ended output when dispatch has no events command", async (t) => {
  process.env.FAKE_DISPATCH_NO_EVENTS = "1"
  t.after(() => delete process.env.FAKE_DISPATCH_NO_EVENTS)
  const { impl } = await loadServer()
  const answer = await impl().log({ runID: "r1" })
  assert.equal(answer.status, "RUNNING")
  assert.equal(answer.text, "line one\nline two\n[merged] three")
  assert.equal(answer.lines, 3)
  // The first read ends short of the output; the second takes its tail.
  const result = calls().filter((call) => call.args[0] === "result")
  assert.deepEqual(result[0].args, ["result", "r1", "--section", "output", "--json", "--offset", "0", "--limit", "2000"])
  assert.deepEqual(result[1].args, ["result", "r1", "--section", "output", "--json", "--offset", "500", "--limit", "2000"])

  // The answer is kept: the next read does not ask again.
  await impl().log({ runID: "r1" })
  assert.equal(calls().filter((call) => call.args[0] === "events").length, 1)
  assert.equal(calls().filter((call) => call.args[0] === "result").length, 4)
})

test("a run with no files, tokens, or cost answers no undefined values", async () => {
  const { runView } = await import(path.join(here, "server.js"))
  const view = runView({ ID: "r2", Status: "COMPLETED", Agent: "claude", FilesChanged: null, Usage: null })
  assert.equal(Object.values(view).some((value) => value === undefined), false)
  assert.equal("filesChanged" in view, false)
  assert.equal(view.id, "r2")
})

test("dispatch_wait and dispatch_merge refuse what is not a run id", async () => {
  const { tools } = await loadServer()
  for (const name of ["dispatch_wait", "dispatch_merge"]) {
    const result = await tools[name].execute({ id: "--force" }, { sessionID: "root", progress: async () => {} })
    assert.match(result.content, /Not a run id/)
  }
  assert.equal(calls().length, 0)
})

test("the panel shows the brief, not the role's instructions, and no routing note as an error", async () => {
  const { tools, impl } = await loadServer()
  await tools.dispatch_run.execute({ prompt: "Fix the parser\nin src/parse.ts", wait: false }, { sessionID: "child", progress: async () => {} })
  const [run] = (await impl().runs({ sessionID: "child" })).runs
  assert.equal(run.prompt, "Fix the parser\nin src/parse.ts")
  const { runView } = await import(path.join(here, "server.js"))
  assert.equal("error" in runView({ ID: "r3", Status: "COMPLETED", Note: "auto: d=-0.9 (hard)" }), false)
})

test("with no dispatch to run, delegation keeps OpenCode's subagents", async () => {
  const { hooks, ctx } = await loadServer()
  fs.rmSync(ctx.options.binary)
  const savedPath = process.env.PATH
  const savedHome = process.env.HOME
  process.env.PATH = "/nonexistent"
  process.env.HOME = tmp
  try {
    const event = contextEvent()
    await hooks.context(event)
    assert.equal("subagent" in event.tools, true)
    assert.equal(event.system.length, 0)
  } finally {
    process.env.PATH = savedPath
    process.env.HOME = savedHome
  }
})

test("a missing dispatch binary reads as unavailable", async (t) => {
  const { impl, ctx } = await loadServer()
  fs.rmSync(ctx.options.binary)
  const savedPath = process.env.PATH
  const savedHome = process.env.HOME
  process.env.PATH = "/nonexistent"
  process.env.HOME = tmp
  t.after(() => {
    process.env.PATH = savedPath
    process.env.HOME = savedHome
  })
  const answer = await impl().state({})
  assert.equal(answer.available, false)
  const runs = await impl().runs({ sessionID: "root" })
  assert.equal(runs.runs.length, 0)
  assert.ok(runs.error)
})

test("open takes only dispatch links", async () => {
  const { impl } = await loadServer()
  for (const url of ["https://example.com", "file:///etc/passwd", "dispatch://run/../x", "dispatch://delete/1"]) {
    assert.equal((await impl().open({ url })).opened, false, url)
  }
})

test("the GUI's RPC copy matches rpc.js", async () => {
  const { Dispatch } = await import(path.join(here, "rpc.js"))
  const gui = loadGui()
  assert.deepEqual(gui.Dispatch, Dispatch)
})

function stubModule(exports) {
  return new Proxy(exports, { get: (target, key) => (key in target ? target[key] : () => () => {}) })
}

// loadGui evaluates the GUI bundle with the app's modules stubbed out.
function loadGui() {
  const registry = (id) => ({ id })
  const stubs = {
    "@opencode/plugin/gui": {
      Plugin: { define: (plugin) => plugin },
      Command: registry("command"),
      Panel: registry("panel"),
      SettingsPage: registry("settings-page"),
      ComposerPill: registry("composer-pill"),
      MenuItem: registry("menu-item"),
      Style: registry("style"),
    },
    // Any other export is a no-op: setup only registers, and nothing renders.
    "solid-js": stubModule({
      createSignal: (value) => {
        let current = value
        return [() => current, (next) => (current = typeof next === "function" ? next(current) : next)]
      },
    }),
    "solid-js/store": stubModule({
      createStore: (value) => [value, (key, next) => (value[key] = next)],
    }),
    "solid-js/web": stubModule({}),
  }
  const file = path.join(here, "gui-experimental.cjs")
  const original = Module._load
  Module._load = function (request, parent, isMain) {
    if (request in stubs) return stubs[request]
    return original.call(this, request, parent, isMain)
  }
  try {
    delete require.cache[file]
    return require(file)
  } finally {
    Module._load = original
  }
}

test("the GUI adds the switch, the runs pill, and the panel", async () => {
  const gui = loadGui()
  const definition = gui.default
  assert.equal(typeof definition.setup, "function")
  assert.equal(definition.i18n.en["runs.other"], "{{count}} runs")

  const added = []
  const opened = []
  const panels = []
  const rpcCalls = []
  globalThis.document = { visibilityState: "hidden" }
  const ctx = {
    id: "dispatch",
    t: (key, params) => (params ? `${key}:${JSON.stringify(params)}` : key),
    plural: (key, count) => `${count} ${key}`,
    add: (registry, item) => {
      added.push({ registry: registry.id, item })
      return () => {}
    },
    client: {
      rpc: () => ({
        state: async () => (rpcCalls.push("state"), { enabled: true, binary: "/bin/dispatch", available: true }),
        setEnabled: async ({ enabled }) => (rpcCalls.push(`set:${enabled}`), { enabled, binary: "/bin/dispatch", available: true }),
        runs: async () => ({ runs: [] }),
        force: async () => ({ force: false, agent: "", model: "" }),
        setForce: async ({ force, agent, model }) => (
          rpcCalls.push(`force:${force}`),
          { force, agent: agent || "", model: model || "" }
        ),
        targets: async () => {
          await new Promise((resolve) => setTimeout(resolve, 0))
          return { agents: [{ name: "opencode", models: ["deepseek/deepseek-flash"] }] }
        },
        open: async ({ url }) => (opened.push(url), { opened: true }),
        events: { on: () => () => {} },
      }),
    },
    sessions: { current: () => undefined },
    layout: { open: (id, session) => panels.push([id, session]), state: () => "closed" },
    system: { openExternal: () => assert.fail("dispatch:// links open through the server part") },
    toast: { show: () => ({ dismiss() {} }) },
    ui: {},
  }
  const cleanup = definition.setup(ctx)
  try {
    const kinds = added.map((entry) => entry.registry)
    for (const kind of ["settings-page", "command", "menu-item", "composer-pill", "panel", "style"]) {
      assert.ok(kinds.includes(kind), `missing ${kind}`)
    }
    const commands = added.filter((entry) => entry.registry === "command").map((entry) => entry.item())
    const toggle = commands.find((command) => command.id === "toggle")
    assert.deepEqual(toggle.slash, { name: "dispatch" })
    await toggle.run()
    assert.ok(rpcCalls.includes("set:false"))

    const pill = added.find((entry) => entry.registry === "composer-pill").item
    const session = { key: "s\nroot", id: "root" }
    const view = pill.show({ session })
    assert.equal(view.label, "pill.off")
    assert.equal(view.rows, undefined)
    view.run()
    await new Promise((resolve) => setTimeout(resolve, 0))
    // The pill opens the runs panel; the switch lives in the panel's header.
    assert.deepEqual(panels, [["dispatch:runs", session]])
    assert.equal(rpcCalls.filter((call) => call === "set:true").length, 0)
    assert.equal(added.some((entry) => entry.registry === "slot"), false)

    // The Delegate check beside it goes with delegation: hidden while off.
    const pills = added.filter((entry) => entry.registry === "composer-pill").map((entry) => entry.item)
    assert.deepEqual(pills.map((item) => item.id), ["runs", "force", "target"])
    const force = pills.find((item) => item.id === "force")
    const target = pills.find((item) => item.id === "target")
    assert.equal(force.show({ session }), undefined)
    assert.equal(target.show({ session }), undefined)
    await toggle.run()
    const off = force.show({ session })
    assert.equal(off.label, "pill.force.off")
    assert.equal(off.icon, undefined)
    assert.equal(off.title, "pill.force.title")
    assert.equal(target.show({ session }), undefined)
    off.run()
    await new Promise((resolve) => setTimeout(resolve, 0))
    assert.ok(rpcCalls.includes("force:true"))
    const on = force.show({ session })
    assert.equal(on.label, "pill.force.on")
    assert.equal(on.icon, "circle-check")
    assert.equal(on.tone, "success")

    // The target pill is beside it while the check is on: Auto until a model
    // is picked, then that agent and model.
    const auto = target.show({ session })
    assert.equal(auto.label, "pill.target.auto")
    assert.equal(auto.title, "pill.target.title")
    await new Promise((resolve) => setTimeout(resolve, 0))
    const ready = target.show({ session })
    assert.deepEqual(ready.rows.map((row) => row.label), ["pill.target.auto.row", "opencode · deepseek/deepseek-flash"])
    assert.equal(ready.rows[0].icon, "circle-check")
    ready.rows[1].run()
    await new Promise((resolve) => setTimeout(resolve, 0))
    const picked = target.show({ session })
    assert.equal(picked.label, "opencode · deepseek/deepseek-flash")
    assert.equal(picked.rows[1].icon, "circle-check")
    picked.rows[0].run()
    await new Promise((resolve) => setTimeout(resolve, 0))
    assert.equal(target.show({ session }).label, "pill.target.auto")
    assert.equal(added.some((entry) => entry.registry === "slot"), false)

    commands.find((command) => command.id === "app").run()
    await new Promise((resolve) => setTimeout(resolve, 0))
    assert.deepEqual(opened, ["dispatch://"])
  } finally {
    cleanup()
    delete globalThis.document
  }
})

test("the GUI's run rows read like the app", () => {
  const { helpers } = loadGui()
  assert.equal(helpers.runLink("a b"), "dispatch://run/a%20b")
  assert.equal(helpers.firstLine("\n  Fix the parser\nmore", 40), "  Fix the parser")
  assert.equal(helpers.firstLine("x".repeat(10), 5), "xxxx…")
  assert.equal(helpers.formatTokens(1700), "1.7k tokens")
  assert.equal(
    helpers.runDetail({ status: "COMPLETED", agent: "codex", model: "gpt-6", filesChanged: 2, insertions: 10, deletions: 3, tokens: 500, runMs: 75_000 }),
    "Done · codex · gpt-6 · 2 files +10 −3 · 500 tokens · 1m 15s",
  )
  assert.equal(helpers.statusLabel({ status: "COMPLETED", mergedAt: "2026-11-02T00:00:00Z" }), "Merged")
})
