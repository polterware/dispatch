// A run's transcript, ported from app/src/models/transcript.rs: the panel draws
// the agent's JSON stream as the Dispatch app does. ACP session/update, Claude
// Code stream-json, Codex app-server and --json, Pi --mode json, and the core's
// `[kind] text` lines become steps, reasoning, replies, and events; usage lines
// are not drawn, and a log that starts mid-line drops that line.

/** One line of a run's transcript. */
export type Entry =
  | { kind: "step"; text: string; detail?: string; failed?: boolean; output?: string }
  | { kind: "thinking"; text: string }
  | { kind: "message"; text: string }
  | { kind: "output"; text: string }
  | { kind: "event"; event: string; text: string };

type Bag = Record<string, unknown>;
type BlockKind = "message" | "thinking";
type StepEntry = Extract<Entry, { kind: "step" }>;

type StepUpdate = {
  text: string;
  failed?: boolean;
  // null clears a detail or output an earlier update set.
  detail?: string | null;
  output?: string | null;
};

const EMPTY: Bag = {};

const obj = (value: unknown): Bag => (value && typeof value === "object" && !Array.isArray(value) ? (value as Bag) : EMPTY);
const arr = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
const str = (value: unknown): string => (typeof value === "string" ? value : "");
const num = (value: unknown): number | undefined => (typeof value === "number" && Number.isFinite(value) ? value : undefined);
const has = (entry: Bag, key: string): boolean => Object.prototype.hasOwnProperty.call(entry, key);

const truthy = (value: unknown): boolean => {
  if (value === undefined || value === null) return false;
  if (typeof value === "boolean") return value;
  if (typeof value === "number") return value !== 0;
  if (typeof value === "string") return value !== "";
  return true;
};

const blockIs = (entry: Entry | undefined, kind: BlockKind): boolean => entry?.kind === kind;

const blockEntry = (kind: BlockKind, text: string): Entry => ({ kind, text });

const stepEntry = (update: StepUpdate): StepEntry => {
  const entry: StepEntry = { kind: "step", text: update.text };
  if (typeof update.detail === "string") entry.detail = update.detail;
  if (update.failed !== undefined) entry.failed = update.failed;
  if (typeof update.output === "string") entry.output = update.output;
  return entry;
};

/** Drops terminal escapes and keeps what a carriage return left visible. */
export function stripAnsi(text: string): string {
  const cleaned = text
    .replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "")
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, "");
  return cleaned
    .split("\n")
    .map((line) => {
      const trimmed = line.endsWith("\r") ? line.slice(0, -1) : line;
      const at = trimmed.lastIndexOf("\r");
      return at >= 0 ? trimmed.slice(at + 1) : trimmed;
    })
    .join("\n");
}

/** `bash -lc 'go test ./...'` reads as `go test ./...`. */
export function unwrapShell(command: string): string {
  const text = command.trim();
  const end = text.search(/\s/);
  const token = text.slice(0, end < 0 ? text.length : end);
  const starts = [...token.matchAll(/\//g)].map((match) => (match.index ?? 0) + 1);
  starts.push(0);
  starts.sort((left, right) => right - left);
  for (const start of starts) {
    const body = unwrapAt(text.slice(start));
    if (body !== null) return body;
  }
  return text;
}

function unwrapAt(command: string): string | null {
  let rest: string;
  if (command.startsWith("bash ")) rest = command.slice(5);
  else if (command.startsWith("zsh ")) rest = command.slice(4);
  else if (command.startsWith("sh ")) rest = command.slice(3);
  else return null;
  if (rest.startsWith("-lc ")) rest = rest.slice(4);
  else if (rest.startsWith("-c ")) rest = rest.slice(3);
  else return null;
  const quote = rest[0];
  if (quote !== "'" && quote !== '"') return null;
  const body = rest.slice(1);
  return body.endsWith(quote) ? body.slice(0, -1) : null;
}

/** The argument a step acts on, in the app's key order. */
function target(args: Bag): string {
  for (const key of ["command", "cmd", "path", "Path", "file_path", "target_file", "filePath", "pattern", "query", "url", "glob"]) {
    const value = args[key];
    if (value === undefined) continue;
    if (typeof value === "string" && value.trim()) return value.trim();
    if (Array.isArray(value) && value.length > 0 && value.every((part) => typeof part === "string")) {
      return value.join(" ");
    }
  }
  return "";
}

/** A tool call's line: the verb and its target, or the tool's own name. */
function stepFor(tool: string, args: Bag): { text: string; detail?: string } {
  const name = tool.endsWith("ToolCall") ? tool.slice(0, -8) : tool;
  const clean = name.replace(/[_-]/g, " ").trim();
  const lower = clean.toLowerCase().replace(/ /g, "");
  const on = target(args);
  const verb = containsAny(lower, ["shell", "bash", "terminal", "command", "exec"])
    ? "Running"
    : containsAny(lower, ["read", "view", "cat"])
      ? "Reading"
      : containsAny(lower, ["write", "create"])
        ? "Writing"
        : containsAny(lower, ["edit", "replace", "patch", "update", "delete"])
          ? "Editing"
          : containsAny(lower, ["grep", "glob", "search", "find", "ls", "list"])
            ? "Searching"
            : containsAny(lower, ["fetch", "web"])
              ? "Fetching"
              : "";
  if (verb) return { text: on ? `${verb} ${on}` : `${verb} (${clean})` };
  return on ? { text: clean || "Tool call", detail: on } : { text: clean || "Tool call" };
}

function containsAny(haystack: string, needles: string[]): boolean {
  return needles.some((needle) => haystack.includes(needle));
}

/** A tool result's text, as ACP, Claude, Pi, and Codex shape it. */
function contentText(value: unknown): string {
  if (typeof value === "string") return value;
  const parts = arr(value)
    .map(obj)
    .map((part) => {
      const text = str(part.text);
      if (text) return text;
      const inner = str(obj(part.content).text);
      if (inner) return inner;
      if (str(part.type) === "diff") return `Edited ${str(part.path)}`;
      return "";
    })
    .filter(Boolean);
  if (parts.length) return parts.join("\n");
  const raw = obj(value);
  for (const key of ["content", "output", "stdout", "result"]) {
    const text = str(raw[key]);
    if (text) return text;
  }
  return "";
}

function parseJson(line: string): Bag | null {
  try {
    const value = JSON.parse(line.trim()) as unknown;
    return value && typeof value === "object" && !Array.isArray(value) ? (value as Bag) : null;
  } catch {
    return null;
  }
}

function chunkHasJson(chunk: string): boolean {
  return chunk.split("\n").some((line) => line.trimStart().startsWith("{") && parseJson(line) !== null);
}

/** A cut JSON line, which a tail read can start or end on. */
function partialJson(line: string): boolean {
  const text = line.trim();
  if (!text) return false;
  const marked = text.startsWith("{") || text.endsWith("}") || text.endsWith("]");
  return marked && /["[\]{}]/.test(text);
}

function eventLine(line: string): { event: string; text: string } | null {
  if (!line.startsWith("[")) return null;
  const close = line.indexOf("]");
  if (close < 1) return null;
  const kind = line.slice(1, close);
  if (!/^[a-z][a-z_-]*$/.test(kind)) return null;
  if (line[close + 1] !== " ") return null;
  return { event: kind, text: line.slice(close + 2).trim() };
}

function trimFlush(text: string): string {
  return text.replace(/^\n+/, "").replace(/\s+$/, "");
}

class Builder {
  private entries: Entry[] = [];
  private items = new Map<string, number>();
  private untargeted = new Map<string, string>();
  private text: string[] = [];
  private fenced = false;
  private streamed = false;
  private delta: { kind: BlockKind; index: number } | null = null;
  private plain: number[] = [];

  feed(chunk: string): void {
    if (!chunk) return;
    if (!this.streamed && chunkHasJson(chunk)) {
      this.promotePlain();
      this.streamed = true;
    }
    const lines = chunk.split("\n");
    if (chunk.endsWith("\n") && lines[lines.length - 1] === "") lines.pop();
    const last = lines.length - 1;
    lines.forEach((line, index) => {
      if (!this.fenced && line.trimStart().startsWith("{")) {
        const event = parseJson(line);
        if (event) {
          this.flushText();
          this.readEvent(event);
          return;
        }
      }
      // A read tail can begin or end inside a JSON line: never draw its pieces.
      if ((index === 0 || index === last) && partialJson(line) && parseJson(line) === null) return;
      if (line.trimStart().startsWith("```") || line.trimStart().startsWith("~~~")) this.fenced = !this.fenced;
      const event = this.fenced ? null : eventLine(line);
      if (event) {
        this.flushText();
        // `started` repeats the prompt, which the panel already shows above.
        if (event.event !== "started") this.push({ kind: "event", event: event.event, text: event.text });
        return;
      }
      this.text.push(line);
    });
  }

  finish(): Entry[] {
    const result: Entry[] = [];
    for (const entry of this.entries) {
      if (entry.kind === "message" || entry.kind === "thinking") {
        const text = entry.text.trim();
        if (text) result.push({ kind: entry.kind, text });
      } else {
        result.push(entry);
      }
    }
    if (this.text.length) {
      const joined = trimFlush(this.text.join("\n")).trim();
      if (joined) result.push(this.streamed ? { kind: "output", text: joined } : { kind: "message", text: joined });
    }
    return result;
  }

  private promotePlain(): void {
    for (const index of this.plain) {
      const entry = this.entries[index];
      if (entry?.kind === "message") this.entries[index] = { kind: "output", text: entry.text };
    }
    this.plain = [];
  }

  private flushText(): void {
    if (!this.text.length) return;
    const joined = trimFlush(this.text.join("\n"));
    this.text = [];
    if (!joined.trim()) return;
    if (this.streamed) {
      this.push({ kind: "output", text: joined });
      return;
    }
    const index = this.push({ kind: "message", text: joined });
    this.plain.push(index);
  }

  private push(entry: Entry): number {
    this.delta = null;
    this.entries.push(entry);
    return this.entries.length - 1;
  }

  private appendText(index: number, text: string): void {
    const entry = this.entries[index];
    if (entry?.kind === "message" || entry?.kind === "thinking") {
      this.entries[index] = { kind: entry.kind, text: entry.text + text };
    }
  }

  private setBlockText(index: number, text: string): void {
    const entry = this.entries[index];
    if (entry?.kind === "message" || entry?.kind === "thinking") {
      this.entries[index] = { kind: entry.kind, text };
    }
  }

  private appendBlock(kind: BlockKind, chunk: string, key = ""): void {
    if (!chunk) return;
    if (key) {
      const index = this.items.get(key);
      if (index !== undefined && blockIs(this.entries[index], kind)) {
        this.appendText(index, chunk);
        return;
      }
    } else if (this.delta && this.delta.kind === kind && blockIs(this.entries[this.delta.index], kind)) {
      this.appendText(this.delta.index, chunk);
      return;
    }
    const index = this.push(blockEntry(kind, chunk));
    this.delta = { kind, index };
    if (key) this.items.set(key, index);
  }

  private completeBlock(kind: BlockKind, full: string): void {
    if (!full.trim()) return;
    if (this.delta && this.delta.kind === kind && blockIs(this.entries[this.delta.index], kind)) {
      this.setBlockText(this.delta.index, full);
      this.delta = null;
      return;
    }
    this.push(blockEntry(kind, full));
  }

  private streamItem(id: string, kind: BlockKind, chunk: string): void {
    if (!chunk) return;
    if (id) {
      const index = this.items.get(id);
      if (index !== undefined && blockIs(this.entries[index], kind)) {
        this.appendText(index, chunk);
        return;
      }
    }
    const index = this.push(blockEntry(kind, chunk));
    if (id) this.items.set(id, index);
  }

  private settleItem(id: string, kind: BlockKind, full: string): void {
    if (!full.trim()) return;
    if (id) {
      const index = this.items.get(id);
      if (index !== undefined && blockIs(this.entries[index], kind)) {
        this.setBlockText(index, full);
        return;
      }
    }
    this.streamItem(id, kind, full);
  }

  private applyStep(id: string, update: StepUpdate): void {
    if (id) {
      const index = this.items.get(id);
      if (index !== undefined) {
        this.mergeStep(index, update);
        return;
      }
    }
    const index = this.push(stepEntry(update));
    if (id) this.items.set(id, index);
  }

  private mergeStep(index: number, update: StepUpdate): void {
    const entry = this.entries[index];
    if (entry?.kind !== "step") {
      this.entries[index] = stepEntry(update);
      return;
    }
    const next: StepEntry = { ...entry, text: update.text };
    if (update.failed !== undefined) next.failed = update.failed;
    if (update.detail !== undefined) {
      if (update.detail === null) delete next.detail;
      else next.detail = update.detail;
    }
    if (update.output !== undefined) {
      if (update.output === null) delete next.output;
      else next.output = update.output;
    }
    this.entries[index] = next;
  }

  private patchStep(id: string, patch: { failed?: boolean; output?: string | null }): void {
    const index = this.items.get(id);
    if (index === undefined) return;
    const entry = this.entries[index];
    if (entry?.kind !== "step") return;
    const next: StepEntry = { ...entry };
    if (patch.failed !== undefined) next.failed = patch.failed;
    if (patch.output !== undefined) {
      if (patch.output === null) delete next.output;
      else next.output = patch.output;
    }
    this.entries[index] = next;
  }

  private appendOutput(id: string, chunk: string): void {
    if (!chunk) return;
    const index = this.items.get(id);
    if (index === undefined) return;
    const entry = this.entries[index];
    if (entry?.kind !== "step") return;
    this.entries[index] = { ...entry, output: (entry.output ?? "") + chunk };
  }

  private readEvent(ev: Bag): void {
    if (has(ev, "dispatch")) return;
    if (str(ev.method) === "session/update") {
      this.readAcp(obj(obj(ev.params).update));
      return;
    }
    if (str(ev.method)) {
      this.readNotification(str(ev.method), obj(ev.params));
      return;
    }
    const type = str(ev.type);
    if (type === "stream_event") {
      this.readClaude(obj(ev.event));
      return;
    }
    if (type.startsWith("message_") || type.startsWith("tool_execution_")) {
      this.readPi(type, ev);
      return;
    }
    if (type.startsWith("item.")) {
      this.readCodexJson(type, ev);
      return;
    }
    if (type === "turn.failed" || type === "error") {
      const message = str(obj(ev.error).message) || str(ev.message);
      if (message) this.push({ kind: "event", event: "failed", text: message });
      return;
    }
    if (type === "thinking") {
      this.appendBlock("thinking", str(ev.text));
      return;
    }
    if (type === "assistant" && has(ev, "message")) {
      this.readAssistant(ev);
      return;
    }
    if (type === "user" && has(ev, "message")) {
      this.readToolResults(ev);
      return;
    }
    if (type === "result") {
      if (ev.is_error === true && str(ev.result)) {
        this.push({ kind: "event", event: "failed", text: str(ev.result) });
      } else if (!this.entries.some((entry) => entry.kind === "message")) {
        this.completeBlock("message", str(ev.result));
      }
      return;
    }
    if (type === "text" && typeof ev.data === "string") {
      this.appendBlock("message", str(ev.data));
      return;
    }
    if ((type === "thought" || type === "reasoning") && typeof ev.data === "string") {
      this.appendBlock("thinking", str(ev.data));
      return;
    }
    if (type === "tool_call" && (has(ev, "toolName") || has(ev, "title"))) {
      const tool = str(ev.toolName) || str(ev.title);
      const built = stepFor(tool, obj(ev.rawInput));
      this.applyStep(str(ev.toolCallId), { text: built.text, detail: built.detail ?? null });
      return;
    }
    if (type === "tool_call_update") {
      if (str(ev.status) === "failed") this.patchStep(str(ev.toolCallId), { failed: true });
      return;
    }
    if (has(ev, "part")) {
      this.readOpencode(type, ev);
      return;
    }
    if (!type) {
      for (const key of ["response", "result", "output", "text"]) {
        if (typeof ev[key] === "string") {
          this.completeBlock("message", str(ev[key]));
          return;
        }
      }
    }
  }

  private readNotification(method: string, params: Bag): void {
    const id = str(params.itemId);
    if (method === "item/agentMessage/delta") {
      this.streamItem(id, "message", str(params.delta));
      return;
    }
    if (method === "item/reasoning/summaryTextDelta" || method === "item/reasoning/textDelta" || method === "item/plan/delta") {
      this.streamItem(id, "thinking", str(params.delta));
      return;
    }
    if (method === "item/commandExecution/outputDelta") {
      this.appendOutput(id, stripAnsi(str(params.delta)));
      return;
    }
    if (method === "error") {
      const message = str(obj(params.error).message);
      if (message && !truthy(params.willRetry)) this.push({ kind: "event", event: "failed", text: message });
      return;
    }
    if (method === "turn/completed") {
      const message = str(obj(obj(params.turn).error).message);
      if (message) this.push({ kind: "event", event: "failed", text: message });
      return;
    }
    if (method !== "item/started" && method !== "item/completed") return;
    const item = obj(params.item);
    const itemId = str(item.id);
    const done = method === "item/completed";
    switch (str(item.type)) {
      case "agentMessage":
        if (done) this.settleItem(itemId, "message", str(item.text));
        return;
      case "reasoning":
      case "plan":
        if (done) this.settleItem(itemId, "thinking", reasoningText(item));
        return;
      case "commandExecution": {
        const exit = num(item.exitCode);
        const update: StepUpdate = { text: `Running ${unwrapShell(str(item.command))}` };
        if (exit !== undefined) {
          update.failed = exit !== 0;
          update.detail = exit !== 0 ? `exit ${exit}` : null;
        }
        const output = trimEnd(stripAnsi(str(item.aggregatedOutput)));
        if (output) update.output = output;
        this.applyStep(itemId, update);
        return;
      }
      case "fileChange": {
        const paths = changePaths(item);
        this.applyStep(itemId, { text: paths ? `Edited ${paths}` : "Edited files", failed: str(item.status) === "failed" });
        return;
      }
      case "mcpToolCall": {
        const called = [str(item.server), str(item.tool)].filter(Boolean).join(".");
        this.applyStep(itemId, { text: `Called ${called}`, failed: str(item.status) === "failed" });
        return;
      }
      case "dynamicToolCall": {
        const built = stepFor(str(item.tool), obj(item.arguments));
        this.applyStep(itemId, { text: built.text, detail: built.detail ?? null, failed: str(item.status) === "failed" });
        return;
      }
      case "webSearch":
        this.applyStep(itemId, { text: `Searching the web for ${str(item.query)}` });
        return;
      default:
        return;
    }
  }

  private readAcp(update: Bag): void {
    const text = str(obj(update.content).text);
    const kind = str(update.sessionUpdate);
    if (kind === "agent_message_chunk") {
      this.appendBlock("message", text, str(update.messageId));
    } else if (kind === "agent_thought_chunk") {
      this.appendBlock("thinking", text, str(update.messageId));
    } else if (kind === "plan") {
      const plan = arr(update.entries)
        .map(obj)
        .map((entry) => `${str(entry.status) === "completed" ? "- [x]" : "- [ ]"} ${str(entry.content)}`)
        .join("\n");
      this.settleItem("acp-plan", "thinking", plan);
    } else if (kind === "tool_call" || kind === "tool_call_update") {
      this.readAcpTool(update);
    }
  }

  private readAcpTool(update: Bag): void {
    const id = str(update.toolCallId);
    const args: Bag = { ...obj(update.rawInput), ...obj(update.args) };
    const location = str(obj(arr(update.locations)[0]).path);
    // A tool's own argument, before its location stands in: OpenCode sends a
    // shell call's command only in a later update, with the folder as location.
    const own = target(args);
    if (!own && location) args.path = location;
    const tool = str(update.name) || str(update.kind) || str(update.title) || this.untargeted.get(id) || "";
    const status = str(update.status);
    const fromContent = contentText(update.content);
    const output = trimEnd(stripAnsi(fromContent || contentText(update.rawOutput)));
    const known = id ? this.items.get(id) : undefined;
    if (known !== undefined && this.entries[known]?.kind === "step") {
      if (this.untargeted.has(id) && own) {
        const built = stepFor(tool, args);
        this.mergeStep(known, { text: built.text, detail: built.detail ?? null });
        this.untargeted.delete(id);
      }
      this.patchStep(id, { failed: status === "failed" ? true : undefined, output: output ? output : undefined });
      return;
    }
    const built = stepFor(tool, args);
    const step: StepUpdate = { text: built.text, detail: built.detail ?? null };
    if (!own) this.untargeted.set(id, tool);
    if (!target(args)) {
      const title = str(update.title);
      if (title && title !== tool) step.text = title;
    }
    if (status === "failed") step.failed = true;
    if (output) step.output = output;
    this.applyStep(id, step);
  }

  private readClaude(event: Bag): void {
    if (str(event.type) !== "content_block_delta") return;
    const delta = obj(event.delta);
    if (str(delta.type) === "text_delta") this.appendBlock("message", str(delta.text));
    else if (str(delta.type) === "thinking_delta") this.appendBlock("thinking", str(delta.thinking));
  }

  private readPi(type: string, ev: Bag): void {
    if (type === "message_update") {
      const event = obj(ev.assistantMessageEvent);
      const kind = str(event.type);
      if (kind === "text_delta") this.appendBlock("message", str(event.delta));
      else if (kind === "thinking_delta") this.appendBlock("thinking", str(event.delta));
      else if (kind === "text_end") this.completeBlock("message", str(event.content));
      else if (kind === "thinking_end") this.completeBlock("thinking", str(event.content));
      return;
    }
    if (type === "message_end") {
      const message = obj(ev.message);
      if (str(message.stopReason) === "error") {
        const text = str(message.errorMessage);
        if (text) this.push({ kind: "event", event: "failed", text });
      }
      return;
    }
    const id = str(ev.toolCallId);
    if (type === "tool_execution_start") {
      const built = stepFor(str(ev.toolName), obj(ev.args));
      this.applyStep(id, { text: built.text, detail: built.detail ?? null });
      return;
    }
    if (type === "tool_execution_end") {
      const output = trimEnd(stripAnsi(contentText(obj(ev.result).content)));
      this.patchStep(id, { failed: ev.isError === true ? true : undefined, output: output ? output : undefined });
    }
  }

  private readCodexJson(type: string, ev: Bag): void {
    const item = obj(ev.item);
    const id = str(item.id);
    const itemType = str(item.type) || str(item.item_type);
    const done = type === "item.completed";
    if (itemType === "agent_message" || itemType === "assistant_message") {
      if (done) this.completeBlock("message", str(item.text));
      return;
    }
    if (itemType === "reasoning") {
      if (done) this.completeBlock("thinking", str(item.text));
      return;
    }
    if (itemType === "command_execution") {
      const exit = num(item.exit_code);
      const output = trimEnd(stripAnsi(str(item.aggregated_output)));
      const update: StepUpdate = { text: `Running ${unwrapShell(str(item.command))}` };
      if (exit !== undefined) {
        update.failed = exit !== 0;
        update.detail = exit !== 0 ? `exit ${exit}` : null;
      }
      if (output) update.output = output;
      this.applyStep(id, update);
      return;
    }
    if (itemType === "file_change") {
      const paths = changePaths(item);
      this.applyStep(id, { text: paths ? `Edited ${paths}` : "Edited files" });
      return;
    }
    if (itemType === "mcp_tool_call") {
      const called = [str(item.server), str(item.tool)].filter(Boolean).join(".");
      this.applyStep(id, { text: `Called ${called}`, failed: str(item.status) === "failed" });
      return;
    }
    if (itemType === "web_search") {
      this.applyStep(id, { text: `Searching the web for ${str(item.query)}` });
      return;
    }
    if (itemType === "error") {
      this.push({ kind: "event", event: "failed", text: str(item.message) });
    }
  }

  private readAssistant(ev: Bag): void {
    const parts = arr(obj(ev.message).content).map(obj);
    const chunk = parts
      .filter((part) => str(part.type) === "text")
      .map((part) => str(part.text))
      .join("");
    if (has(ev, "timestamp_ms")) this.appendBlock("message", chunk);
    else this.completeBlock("message", chunk);
    for (const part of parts) {
      if (str(part.type) === "thinking") this.completeBlock("thinking", str(part.thinking));
      if (str(part.type) === "tool_use") {
        const built = stepFor(str(part.name), obj(part.input));
        this.applyStep(str(part.id), { text: built.text, detail: built.detail ?? null });
      }
    }
  }

  private readToolResults(ev: Bag): void {
    for (const part of arr(obj(ev.message).content).map(obj)) {
      if (str(part.type) !== "tool_result") continue;
      const id = str(part.tool_use_id);
      if (!id) continue;
      const output = trimEnd(stripAnsi(contentText(part.content)));
      this.patchStep(id, {
        failed: part.is_error === true ? true : undefined,
        output: output ? output : undefined,
      });
    }
  }

  private readOpencode(type: string, ev: Bag): void {
    const part = obj(ev.part);
    if (type === "text") {
      this.appendBlock("message", str(part.text));
      return;
    }
    if (type === "reasoning") {
      this.appendBlock("thinking", str(part.text));
      return;
    }
    if (type !== "tool_use" && type !== "tool") return;
    const state = obj(part.state);
    const built = stepFor(str(part.tool), obj(state.input));
    const id = str(part.callID) || str(part.id);
    this.applyStep(id, {
      text: built.text,
      detail: built.detail ?? null,
      failed: str(state.status) === "error" ? true : undefined,
    });
  }
}

function reasoningText(item: Bag): string {
  const summary = arr(item.summary);
  const parts = summary.length ? summary : arr(item.content);
  const direct = str(item.text);
  if (direct) return direct;
  return parts
    .map(str)
    .filter(Boolean)
    .join("\n\n");
}

function changePaths(item: Bag): string {
  return arr(item.changes)
    .map(obj)
    .map((change) => str(change.path))
    .filter(Boolean)
    .join(", ");
}

function trimEnd(text: string): string {
  return text.replace(/\s+$/, "");
}

/** Reads a run's log the way the Dispatch app does. */
export function buildTranscript(log: string): Entry[] {
  const builder = new Builder();
  builder.feed(stripAnsi(log));
  return builder.finish();
}

/** The entries to draw, each with how many times its step ran in a row. */
export function foldRepeats(entries: readonly Entry[]): { index: number; repeats?: number }[] {
  const stepText = (index: number): string | undefined => {
    const entry = entries[index];
    return entry?.kind === "step" && entry.failed !== true ? entry.text : undefined;
  };
  const said = (index: number): boolean => {
    const entry = entries[index];
    return entry?.kind === "message" || entry?.kind === "thinking";
  };
  const shown: { index: number; repeats?: number }[] = [];
  let index = 0;
  while (index < entries.length) {
    const text = stepText(index);
    if (text === undefined) {
      shown.push({ index });
      index += 1;
      continue;
    }
    const unit = (at: number): number => (said(at + 1) ? 2 : 1);
    let last = index;
    let count = 1;
    let next = index + unit(index);
    while (stepText(next) === text) {
      last = next;
      count += 1;
      next += unit(next);
    }
    if (count === 1) {
      shown.push({ index });
      index += 1;
      continue;
    }
    shown.push({ index: last, repeats: count });
    if (said(last + 1)) shown.push({ index: last + 1 });
    index = next;
  }
  return shown;
}
