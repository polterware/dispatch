// Dispatch for OpenCode. While delegation is on (the default), a session
// delegates through Dispatch instead of OpenCode's own subagents: the
// subagent tool leaves the model's tools, and dispatch_run, dispatch_wait, and
// dispatch_merge take its place, each run recorded under the conversation
// `opencode:<root session id>`. The Desktop part reads the switch and the
// conversation's runs through the RPC in rpc.js. `dispatch setup opencode`
// installs this folder and overwrites it on update, so edits here do not last.
import { execFile } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { Dispatch } from "./rpc.js"

const NATIVE_SUBAGENT = "subagent"
const TOOLS = ["dispatch_run", "dispatch_wait", "dispatch_merge"]
// The tools that would let an agent do the work itself. Built-in OpenCode
// tools carry no namespace prefix; an MCP server's do, so these names are its
// editor and its shell alone.
const WORK_TOOLS = ["edit", "write", "patch", "apply_patch", "multiedit", "bash", "shell"]
const TIERS = ["auto", "easy", "normal", "hard"]
const TERMINAL = new Set(["COMPLETED", "FAILED", "CANCELLED", "TIMED_OUT", "ORPHANED"])
// One `dispatch wait` call; a longer run is waited on again.
const WAIT_SLICE = Number(process.env.DISPATCH_OPENCODE_WAIT_SLICE) || 300
const READ_TIMEOUT_MS = 20_000
const START_TIMEOUT_MS = 120_000
const RESULT_LINES = 200
const PROMPT_PREVIEW = 400
// The tail of a run's output the panel holds: enough to read, little to carry.
const LOG_LINES = 2000
const LOG_BYTES = 256 * 1024
// The app's own link, or a run in it.
const RUN_ID = /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/
const DISPATCH_LINK = /^dispatch:\/\/(run\/[A-Za-z0-9_-]{1,128})?$/
// An agent or model a forced run may name; it never starts with a dash, so it
// can never be read as a flag of its own.
const SAFE_TARGET = /^[A-Za-z0-9][A-Za-z0-9._:@+~/-]{0,127}$/
// The model list asks every installed CLI, which can take a while.
const TARGETS_TIMEOUT_MS = 30_000
const TARGETS_TTL_MS = 60_000

const INSTRUCTIONS = [
  "Delegation in this session goes through Dispatch; OpenCode's own subagents are off.",
  "To hand a task to another coding agent, call dispatch_run with a self-contained brief: the goal, what it may and may not touch, the exact test command, and a final report in at most 10 lines. Give paths, never pasted code.",
  "dispatch_run waits for the run and returns its summary and the agent's reply. Independent tasks go in the same response, one dispatch_run each. With wait false it returns the run id at once; call dispatch_wait on it before you end the turn.",
  "A COMPLETED write run leaves its work in a worktree: check it, run the tests, then call dispatch_merge. Do not delegate a one-line change or work that needs this whole conversation.",
].join("\n")

// FORCED is what a conversation the user ticked Delegate in gets on every
// model call, in front of the instructions above.
const FORCED = [
  "This conversation is forced to delegate: every task in this turn that reads code, edits files, runs tests, or reviews work MUST go through dispatch_run.",
  "Start independent tasks in the same response, one dispatch_run each, and wait for every run (wait true, or dispatch_wait) before you end the turn.",
  "Do not do that work yourself: you may talk to the user, plan, and call dispatch_run, dispatch_wait, and dispatch_merge.",
].join("\n")

function home() {
  return os.homedir()
}

function executable(file) {
  try {
    fs.accessSync(file, fs.constants.X_OK)
    return true
  } catch {
    return false
  }
}

// dispatchBinary is the configured binary while it is there, else the linked
// command, else whatever `dispatch` the PATH holds.
export function dispatchBinary(options = {}) {
  const linked = path.join(home(), ".local", "bin", "dispatch")
  for (const file of [options.binary, process.env.DISPATCH_BIN, linked]) {
    if (typeof file === "string" && file && executable(file)) return file
  }
  return "dispatch"
}

// switchPath is where the delegation switch lives, in Dispatch's state folder.
export function switchPath(options = {}) {
  if (typeof options.state === "string" && options.state) return options.state
  const data = process.env.PWDISPATCH_DATA_DIR || path.join(home(), "Library", "Application Support", "Polterware Dispatch")
  return path.join(data, "dispatch", "opencode-plugin.json")
}

// readEnabled reads the switch; a missing or unreadable file means on.
export function readEnabled(file) {
  try {
    const body = JSON.parse(fs.readFileSync(file, "utf8"))
    return body?.enabled !== false
  } catch {
    return true
  }
}

export function writeEnabled(file, enabled) {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const tmp = `${file}.${process.pid}.tmp`
  fs.writeFileSync(tmp, JSON.stringify({ enabled }, null, 2) + "\n")
  fs.renameSync(tmp, file)
}

// exec runs dispatch and resolves its exit code and output; it never rejects
// on a non-zero exit, because `dispatch wait` exits 3 while a run goes on.
function exec(binary, args, { cwd, env, input, timeout, signal }) {
  return new Promise((resolve) => {
    const options = { timeout, maxBuffer: 16 * 1024 * 1024, env: { ...process.env, ...env } }
    if (cwd && fs.existsSync(cwd)) options.cwd = cwd
    if (signal) options.signal = signal
    const child = execFile(binary, args, options, (error, stdout, stderr) => {
      const code = error ? (typeof error.code === "number" ? error.code : -1) : 0
      resolve({ code, stdout: String(stdout), stderr: String(stderr || (error && code === -1 ? error.message : "")).trim() })
    })
    // A binary that is missing or exits early closes stdin; that must not throw.
    child.stdin?.on("error", () => {})
    if (input !== undefined) child.stdin?.end(input)
  })
}

function findOnPath(name) {
  return (process.env.PATH || "").split(path.delimiter).some((dir) => dir && executable(path.join(dir, name)))
}

function parseJSON(text) {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}

function asString(value) {
  return typeof value === "string" ? value : ""
}

// compact drops the keys with no value: OpenCode's RPC refuses undefined.
function compact(view) {
  return Object.fromEntries(Object.entries(view).filter(([, value]) => value !== undefined))
}

// safeTarget reads the agent and model a forced run goes to; empty is Auto, and
// both have to match the safe pattern to be passed on to the CLI.
export function safeTarget(agent, model) {
  const a = asString(agent).trim()
  const m = asString(model).trim()
  if (!a || !SAFE_TARGET.test(a) || !SAFE_TARGET.test(m)) return { agent: "", model: "" }
  return { agent: a, model: m }
}

// forcedState reads the Delegate check as it was stored: an object with the
// check and its target, or the boolean an earlier plugin saved.
function forcedState(saved) {
  if (saved === true) return { force: true, agent: "", model: "" }
  if (saved && typeof saved === "object") {
    return { force: saved.force === true, agent: asString(saved.agent), model: asString(saved.model) }
  }
  return { force: false, agent: "", model: "" }
}

// forcedInstructions is what a forced conversation is told on every model call,
// naming the agent and model its runs go to when it has a target.
function forcedInstructions(forced) {
  const pick = safeTarget(forced.agent, forced.model)
  if (!pick.agent) return FORCED
  return [
    FORCED,
    `Every dispatch_run in this conversation goes to ${pick.agent} · ${pick.model}: Dispatch names that agent and model itself, whatever you pass.`,
  ].join("\n")
}

// ANSI is what an agent's output colors, not text the panel can draw.
const ANSI_CSI = /\u001b\[[0-?]*[ -/]*[@-~]/g
const ANSI_OSC = /\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g

function stripAnsi(text) {
  return String(text).replace(ANSI_OSC, "").replace(ANSI_CSI, "").replace(/\r\n/g, "\n")
}

// logText keeps the end of the output: its last lines inside the byte cap.
function logText(raw) {
  let text = stripAnsi(raw)
  if (text.length > LOG_BYTES) {
    text = text.slice(text.length - LOG_BYTES)
    const cut = text.indexOf("\n")
    if (cut >= 0) text = text.slice(cut + 1)
  }
  const rows = text.split("\n")
  return rows.length > LOG_LINES ? rows.slice(-LOG_LINES).join("\n") : text
}

// runView is what the Desktop part shows of one run.
export function runView(raw) {
  const usage = raw.Usage && typeof raw.Usage === "object" ? raw.Usage : undefined
  return compact({
    id: asString(raw.ID),
    status: asString(raw.Status),
    tier: asString(raw.Tier),
    agent: asString(raw.Agent),
    model: asString(raw.Model),
    provider: asString(raw.Provider),
    role: asString(raw.Profile),
    prompt: asString(raw.Prompt).slice(0, PROMPT_PREVIEW),
    worktree: asString(raw.Worktree),
    createdAt: asString(raw.CreatedAt),
    finishedAt: asString(raw.FinishedAt),
    mergedAt: asString(raw.MergedAt),
    runMs: typeof raw.RunMs === "number" ? raw.RunMs : 0,
    filesChanged: typeof raw.FilesChanged === "number" ? raw.FilesChanged : undefined,
    insertions: typeof raw.Insertions === "number" ? raw.Insertions : undefined,
    deletions: typeof raw.Deletions === "number" ? raw.Deletions : undefined,
    tokens: usage
      ? (usage.inputTokens || 0) + (usage.cacheReadTokens || 0) + (usage.cacheWriteTokens || 0) + (usage.outputTokens || 0)
      : undefined,
    costUsd: usage && typeof usage.costUsd === "number" ? usage.costUsd : undefined,
    error: asString(raw.Error) || undefined,
  })
}

export default {
  id: "dispatch",
  async setup(ctx) {
    const options = ctx.options || {}
    // Read again on each use, so a Dispatch installed or moved later is found.
    const binary = () => dispatchBinary(options)
    const file = switchPath(options)
    // The brief each run was started with, by run id: Dispatch stores the
    // prompt with its role's instructions in front of it.
    const briefKey = (id) => `brief/${id}`
    let cached = { at: -1, enabled: true }
    const enabled = () => {
      let at = 0
      try {
        at = fs.statSync(file).mtimeMs
      } catch {
        at = 0
      }
      if (at !== cached.at) cached = { at, enabled: readEnabled(file) }
      return cached.enabled
    }

    // The conversation is the root session, so a run a child session starts
    // still lists under the chat the user sees.
    const conversation = async (sessionID) => {
      let id = sessionID
      let directory = ""
      for (let depth = 0; depth < 16; depth++) {
        let info
        try {
          info = await ctx.session.get({ sessionID: id })
        } catch {
          break
        }
        directory ||= asString(info?.location?.directory)
        if (!info?.parentID) break
        id = info.parentID
      }
      return { id, directory: directory || ctx.location.directory }
    }

    const callerEnv = (root) => ({ DISPATCH_CALLER_SESSION: root, DISPATCH_CALLER_AGENT: "opencode" })

    // Any exit code means the binary ran; -1 is a spawn failure.
    const state = async () => {
      const probe = await exec(binary(), ["help"], { timeout: READ_TIMEOUT_MS })
      return { enabled: enabled(), binary: binary(), available: probe.code !== -1 }
    }
    // Delegation takes the subagent tool away only while dispatch can run.
    let reachable = { at: 0, ok: true }
    const usable = () => {
      const now = Date.now()
      if (now - reachable.at > 30_000) reachable = { at: now, ok: binary() !== "dispatch" || findOnPath("dispatch") }
      return reachable.ok
    }

    // The Delegate check, per conversation (the root session): kept in memory
    // for the model calls and in storage, so it survives a restart. A forced
    // conversation also keeps the agent and model its runs go to, empty for
    // Auto, under the same key.
    const forced = new Map()
    const forceKey = (id) => `force/${id}`
    const forceOf = async (id) => {
      if (forced.has(id)) return forced.get(id)
      const saved = await ctx.storage.get(forceKey(id)).catch(() => undefined)
      // An earlier Dispatch stored the check alone, as the boolean itself.
      const state = forcedState(saved)
      const value = { force: state.force, ...safeTarget(state.agent, state.model) }
      forced.set(id, value)
      return value
    }

    // The agents and models a target can pick, from the CLIs' own lists, kept
    // a minute so opening the pill again does not ask every agent.
    let targetsCache
    const targets = async () => {
      const now = Date.now()
      if (targetsCache && now - targetsCache.at < TARGETS_TTL_MS) return targetsCache.value
      const [listed, catalog] = await Promise.all([
        exec(binary(), ["agents", "--json"], { cwd: ctx.location.directory, timeout: READ_TIMEOUT_MS }),
        exec(binary(), ["models", "--json"], { cwd: ctx.location.directory, timeout: TARGETS_TIMEOUT_MS }),
      ])
      const agents = parseJSON(listed.stdout)
      const models = parseJSON(catalog.stdout)
      let value
      if (listed.code !== 0 || !agents) {
        value = { agents: [], error: listed.stderr || `dispatch agents exited ${listed.code}` }
      } else if (catalog.code !== 0 || !models) {
        value = { agents: [], error: catalog.stderr || `dispatch models exited ${catalog.code}` }
      } else {
        // The catalog names each model as the agent's CLI does (an ACP agent's
        // provider prefix included), which is what `dispatch run --model` takes.
        const byAgent = new Map()
        for (const entry of models.entries || []) {
          const agent = asString(entry?.agent)
          const model = asString(entry?.model)
          if (!agent || !model) continue
          if (!byAgent.has(agent)) byAgent.set(agent, new Set())
          byAgent.get(agent).add(model)
        }
        const names = []
        const disabled = new Set()
        for (const raw of agents.agents || []) {
          const name = asString(raw?.Name)
          if (!name) continue
          if (raw?.Enabled === false) disabled.add(name)
          else if (!names.includes(name)) names.push(name)
        }
        // An agent the catalog knows and `dispatch agents` did not list still has models.
        for (const name of byAgent.keys()) {
          if (!names.includes(name) && !disabled.has(name)) names.push(name)
        }
        value = {
          agents: names
            .map((name) => ({ name, models: [...(byAgent.get(name) || [])].sort() }))
            .filter((agent) => agent.models.length > 0),
        }
      }
      targetsCache = { at: now, value }
      return value
    }

    const registration = await ctx.rpc.register(Dispatch, {
      state: async () => state(),
      setEnabled: async (input) => {
        writeEnabled(file, input.enabled === true)
        await registration.events.emit("changed", { enabled: enabled() })
        return state()
      },
      open: async (input) => {
        const url = asString(input.url)
        if (!DISPATCH_LINK.test(url)) return { opened: false, error: "only dispatch:// links open" }
        const opener = process.platform === "darwin" ? "open" : process.platform === "win32" ? "explorer" : "xdg-open"
        const out = await exec(opener, [url], { timeout: READ_TIMEOUT_MS })
        return out.code === 0 ? { opened: true } : { opened: false, error: out.stderr || `${opener} exited ${out.code}` }
      },
      runs: async (input) => {
        const root = await conversation(asString(input.sessionID))
        const cwd = asString(input.directory) || root.directory
        const out = await exec(binary(), ["runs", "--json", "--limit", "50", "--conversation", `opencode:${root.id}`], {
          cwd,
          timeout: READ_TIMEOUT_MS,
        })
        const body = parseJSON(out.stdout)
        if (out.code !== 0 || !body) return { runs: [], error: out.stderr || "dispatch runs failed" }
        const runs = []
        for (const raw of body.runs || []) {
          const view = runView(raw)
          const brief = view.id ? await ctx.storage.get(briefKey(view.id)).catch(() => undefined) : undefined
          if (typeof brief === "string" && brief) view.prompt = brief.slice(0, PROMPT_PREVIEW)
          runs.push(view)
        }
        return { runs }
      },
      // The conversation's Delegate check and its target: true means every
      // model call must hand its work to Dispatch, and an agent with a model
      // sends every run there instead of Auto.
      force: async (input) => {
        const root = await conversation(asString(input.sessionID))
        return { ...(await forceOf(root.id)) }
      },
      setForce: async (input) => {
        const root = await conversation(asString(input.sessionID))
        const before = await forceOf(root.id)
        // A pick passes the target, empty strings for Auto; a toggle passes
        // neither and leaves the target as it was.
        const pick =
          typeof input.agent === "string" || typeof input.model === "string"
            ? safeTarget(input.agent, input.model)
            : { agent: before.agent, model: before.model }
        const value = { force: input.force === true, ...pick }
        forced.set(root.id, value)
        await ctx.storage.set(forceKey(root.id), value).catch(() => {})
        await registration.events.emit("forced", { sessionID: root.id, ...value })
        return { ...value }
      },
      // The agents and models a forced conversation can send its runs to.
      targets: async () => targets(),
      // log answers the end of a run's output for the panel's log view, with
      // the run's status so the view knows when to stop asking.
      log: async (input) => {
        const id = asString(input.runID)
        if (!RUN_ID.test(id)) return { status: "", text: "", lines: 0, error: "not a run id" }
        const cwd = asString(input.directory)
        const read = async (offset, limit) => {
          const out = await exec(
            binary(),
            ["result", id, "--section", "output", "--json", "--offset", String(offset), "--limit", String(limit)],
            { cwd, timeout: READ_TIMEOUT_MS },
          )
          const body = parseJSON(out.stdout)
          if (out.code !== 0 || !body) return { error: out.stderr || `dispatch result exited ${out.code}` }
          return { body }
        }
        const offset = Number.isInteger(input.offset) && input.offset > 0 ? input.offset : 0
        const first = await read(offset, LOG_LINES)
        if (first.error) return { status: "", text: "", lines: 0, error: first.error }
        const total = typeof first.body.total_lines === "number" ? first.body.total_lines : 0
        // The end of a longer output, which the first read ends short of.
        let page = first
        if (!offset && total > LOG_LINES) {
          page = await read(total - LOG_LINES, LOG_LINES)
          if (page.error) return { status: "", text: "", lines: 0, error: page.error }
        }
        const text = logText(asString(page.body.content))
        const rows = text ? text.split("\n") : []
        if (rows.length && !rows[rows.length - 1]) rows.pop()
        return { status: asString(page.body.status), text, lines: rows.length }
      },
    })

    // cancelStarted cancels the conversation's runs that began after since and
    // still run, when the call that started one stopped before it answered.
    const cancelStarted = async (root, since) => {
      const out = await exec(binary(), ["runs", "--json", "--limit", "10", "--conversation", `opencode:${root.id}`], {
        cwd: root.directory,
        timeout: READ_TIMEOUT_MS,
      })
      for (const raw of parseJSON(out.stdout)?.runs || []) {
        const created = Date.parse(asString(raw.CreatedAt))
        if (!RUN_ID.test(asString(raw.ID)) || TERMINAL.has(asString(raw.Status)) || !(created >= since - 5_000)) continue
        await exec(binary(), ["cancel", raw.ID], { cwd: root.directory, timeout: READ_TIMEOUT_MS })
      }
    }

    // waitFor waits on a run until it ends or the tool call stops, and
    // answers its summary and the agent's reply.
    const waitFor = async (id, cwd, signal, progress) => {
      let summary
      for (;;) {
        if (signal?.aborted) {
          await exec(binary(), ["cancel", id], { cwd, timeout: READ_TIMEOUT_MS })
          return `Run ${id} was cancelled.`
        }
        const out = await exec(binary(), ["wait", id, "--timeout", `${WAIT_SLICE}s`], { cwd, signal })
        summary = parseJSON(out.stdout)
        if (signal?.aborted) continue
        if (out.code === 3) {
          await progress?.({ status: "running", runID: id })
          continue
        }
        if (!summary) return `dispatch wait ${id} failed: ${out.stderr || out.stdout}`
        break
      }
      const reply = await exec(binary(), ["result", id, "--section", "agent", "--limit", String(RESULT_LINES)], {
        cwd,
        timeout: READ_TIMEOUT_MS,
      })
      const lines = [JSON.stringify(summary, null, 2)]
      if (reply.code === 0 && reply.stdout.trim()) lines.push("", "Agent reply:", reply.stdout.trim())
      if (summary.status === "COMPLETED" && summary.worktree) {
        lines.push("", `The work is in ${summary.worktree}. Check it and run the tests, then call dispatch_merge with id ${id}.`)
      }
      return lines.join("\n")
    }

    await ctx.tool.transform((editor) => {
      editor.add({
        name: "dispatch_run",
        description:
          "Delegate a task to another coding agent through Dispatch, which routes it to the model that fits its difficulty, role, and quota, and runs write tasks in their own git worktree. Use it instead of subagents for localized implementation, tests, review, code reading, and independent parallel work.",
        input: {
          type: "object",
          properties: {
            prompt: { type: "string", description: "A self-contained brief: goal, allowed scope, test command, and a short final report." },
            tier: { type: "string", enum: TIERS, description: "Difficulty tier. Leave it out for Dispatch's default (Auto with a classifier, else normal)." },
            role: { type: "string", description: "Optional role, such as explorer, implementer, tester, reviewer, or planner." },
            card: { type: "string", description: "Optional Polterware Work card slug the run works on." },
            readonly: { type: "boolean", description: "Run read-only: the agent reads and reports but writes nothing." },
            agent: { type: "string", description: "Optional agent to run on. A conversation forced to a target overrides it." },
            model: { type: "string", description: "Optional model to run on. A conversation forced to a target overrides it." },
            wait: { type: "boolean", description: "Wait for the run to end (default true). With false, answers the run id at once." },
          },
          required: ["prompt"],
          additionalProperties: false,
        },
        options: { codemode: false },
        execute: async (input, context) => {
          const root = await conversation(context.sessionID)
          // A forced conversation with a target sends every run to its agent
          // and model, whatever the call asks for; without one, or not forced,
          // the call's own agent and model stand, and Dispatch routes.
          const flag = await forceOf(root.id)
          const target = flag.force ? safeTarget(flag.agent, flag.model) : { agent: "", model: "" }
          const args = ["run", "--json", "--compact", "--dir", root.directory]
          if (target.agent) {
            args.push("--agent", target.agent, "--model", target.model)
          } else {
            const agent = asString(input.agent).trim()
            const model = asString(input.model).trim()
            if (SAFE_TARGET.test(agent)) args.push("--agent", agent)
            if (SAFE_TARGET.test(model)) args.push("--model", model)
          }
          if (TIERS.includes(input.tier)) args.push("--tier", input.tier)
          if (asString(input.role).trim()) args.push("--role", input.role.trim())
          if (asString(input.card).trim()) args.push("--card", input.card.trim())
          if (input.readonly === true) args.push("--readonly")
          args.push("-")
          const startedAt = Date.now()
          const started = await exec(binary(), args, {
            cwd: root.directory,
            env: callerEnv(root.id),
            input: String(input.prompt ?? ""),
            timeout: START_TIMEOUT_MS,
            signal: context.signal,
          })
          const run = parseJSON(started.stdout)
          if (started.code !== 0 || !run?.id) {
            // Stopped or timed out while starting: the core may hold the run already.
            if (context.signal?.aborted || started.code === -1) await cancelStarted(root, startedAt)
            return { content: `dispatch run failed: ${started.stderr || started.stdout}` }
          }
          await ctx.storage.set(briefKey(run.id), String(input.prompt ?? "").slice(0, PROMPT_PREVIEW)).catch(() => {})
          await registration.events.emit("started", { sessionID: root.id, runID: run.id })
          await context.progress({ status: "running", runID: run.id, agent: asString(run.agent), model: asString(run.model) })
          if (input.wait === false || TERMINAL.has(run.status)) {
            return { content: JSON.stringify(run, null, 2) + `\n\nCall dispatch_wait with id ${run.id} before you end the turn.` }
          }
          return { content: await waitFor(run.id, root.directory, context.signal, context.progress) }
        },
      })
      editor.add({
        name: "dispatch_wait",
        description: "Wait for a Dispatch run to end and answer its summary and the agent's reply.",
        input: {
          type: "object",
          properties: { id: { type: "string", description: "The run id dispatch_run answered." } },
          required: ["id"],
          additionalProperties: false,
        },
        options: { codemode: false },
        execute: async (input, context) => {
          if (!RUN_ID.test(String(input.id))) return { content: `Not a run id: ${input.id}` }
          const root = await conversation(context.sessionID)
          return { content: await waitFor(String(input.id), root.directory, context.signal, context.progress) }
        },
      })
      editor.add({
        name: "dispatch_merge",
        description: "Merge a COMPLETED Dispatch run's branch into the checkout and remove its worktree. Run the tests in the worktree first.",
        input: {
          type: "object",
          properties: { id: { type: "string", description: "The run id." } },
          required: ["id"],
          additionalProperties: false,
        },
        options: { codemode: false },
        execute: async (input, context) => {
          if (!RUN_ID.test(String(input.id))) return { content: `Not a run id: ${input.id}` }
          const root = await conversation(context.sessionID)
          const out = await exec(binary(), ["merge", String(input.id)], { cwd: root.directory, timeout: START_TIMEOUT_MS })
          return { content: (out.stdout + (out.stderr ? `\n${out.stderr}` : "")).trim() || `dispatch merge exited ${out.code}` }
        },
      })
    })

    await ctx.session.hook("context", async (event) => {
      if (enabled() && usable()) {
        delete event.tools[NATIVE_SUBAGENT]
        event.system.push({ type: "text", text: INSTRUCTIONS })
        // A forced conversation is told to delegate, and loses the tools that
        // would let it do the work itself; with a target, it names the agent
        // and model every run goes to.
        const root = await conversation(event.sessionID)
        const flag = await forceOf(root.id)
        if (flag.force) {
          for (const name of WORK_TOOLS) delete event.tools[name]
          event.system.push({ type: "text", text: forcedInstructions(flag) })
        }
        return
      }
      for (const name of TOOLS) delete event.tools[name]
    })
  },
}
