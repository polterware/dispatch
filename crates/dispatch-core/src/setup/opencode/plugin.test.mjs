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
  assert.deepEqual(await impl().force({ sessionID: "child" }), { force: false })
  assert.deepEqual(await impl().setForce({ sessionID: "child", directory: "", force: true }), { force: true })
  // The conversation is the root session, so a child session's check is the chat's.
  assert.deepEqual(emitted.at(-1), { name: "forced", data: { sessionID: "root", force: true } })
  assert.deepEqual(await impl().force({ sessionID: "root" }), { force: true })
  assert.equal(stored.get("force/root"), true)

  // A server part started again over the same storage reads the flag back.
  const restarted = await loadServer({ storage: stored })
  assert.deepEqual(await restarted.impl().force({ sessionID: "child" }), { force: true })
  assert.deepEqual(await restarted.impl().setForce({ sessionID: "root", force: false }), { force: false })
  assert.equal(stored.get("force/root"), false)
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

test("log answers the end of a run's output without escapes, and its status", async () => {
  const { impl } = await loadServer()
  const answer = await impl().log({ runID: "r1" })
  const result = calls().filter((call) => call.args[0] === "result")
  // The first read ends short of the output; the second takes its tail.
  assert.deepEqual(result[0].args, ["result", "r1", "--section", "output", "--json", "--offset", "0", "--limit", "2000"])
  assert.deepEqual(result[1].args, ["result", "r1", "--section", "output", "--json", "--offset", "500", "--limit", "2000"])
  assert.equal(answer.status, "RUNNING")
  assert.equal(answer.text, "line one\nline two\n[merged] three")
  assert.equal(answer.lines, 3)
  assert.equal(Object.values(answer).some((value) => value === undefined), false)

  // A page from an offset asks once, where the reader left off.
  await impl().log({ runID: "r1", offset: 10 })
  const paged = calls().filter((call) => call.args[0] === "result")
  assert.deepEqual(paged.at(-1).args, ["result", "r1", "--section", "output", "--json", "--offset", "10", "--limit", "2000"])

  const refused = await impl().log({ runID: "--force" })
  assert.ok(refused.error)
  assert.equal(refused.text, "")
  assert.equal(calls().filter((call) => call.args[0] === "result").length, 3)
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
        force: async () => ({ force: false }),
        setForce: async ({ force }) => (rpcCalls.push(`force:${force}`), { force }),
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
    assert.deepEqual(pills.map((item) => item.id), ["runs", "force"])
    const force = pills.find((item) => item.id === "force")
    assert.equal(force.show({ session }), undefined)
    await toggle.run()
    const off = force.show({ session })
    assert.equal(off.label, "pill.force.off")
    assert.equal(off.icon, undefined)
    assert.equal(off.title, "pill.force.title")
    off.run()
    await new Promise((resolve) => setTimeout(resolve, 0))
    assert.ok(rpcCalls.includes("force:true"))
    const on = force.show({ session })
    assert.equal(on.label, "pill.force.on")
    assert.equal(on.icon, "circle-check")
    assert.equal(on.tone, "success")
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
