// Dispatch in OpenCode Desktop: the delegation switch (the runs panel's header,
// Settings › Dispatch, the palette, and /dispatch), and the conversation's runs
// in a side panel with each agent's bot, opened from the composer pill; a run
// opens in the panel as its log, and Open in Dispatch opens it in the app.
// `pnpm --dir plugins/opencode-gui build` writes it to
// crates/dispatch-core/src/setup/opencode/gui-experimental.cjs, which the
// dispatch binary embeds and `dispatch setup opencode` installs.
import { Command, ComposerPill, MenuItem, Panel, Plugin, SettingsPage, Style } from "@opencode/plugin/gui";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { Dispatch } from "../../../crates/dispatch-core/src/setup/opencode/rpc.js";
import { AgentBot, EmptyBot } from "./avatars";
import { expressionFor } from "./expressions";
import { ACTIVE, FAILED, filterTargets, firstLine, formatDuration, formatTokens, runDetail, runLink, statusLabel, type LogView, type RunView, type TargetAgent } from "./runs";

// The app's context; its types live in @opencode/plugin/gui.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Ctx = any;
type Session = { key: string; id: string; directory?: string; pending?: boolean };
type Status = { enabled: boolean; binary: string; available: boolean; loaded: boolean };
// The conversation's Delegate check and the target its runs go to, empty for Auto.
type ForcedView = { force: boolean; agent: string; model: string };
// One line of the Agent pill's popover: a selectable row, with the index the
// keyboard moves by, or the heading of the agent whose models follow it.
type TargetEntry =
  | { kind: "row"; index: number; agent: string; model: string; label: string }
  | { kind: "agent"; name: string };

const POLL_ACTIVE_MS = 3_000;
const POLL_IDLE_MS = 15_000;
// An open run's log: read again while it works, and followed until the reader
// scrolls away from the end, which is the app's transcript rule.
const LOG_POLL_MS = 2_000;
const FOLLOW_SLACK = 48;

const CSS = `
.dispatch-panel { display: flex; flex-direction: column; gap: 4px; padding: 8px; overflow: auto; height: 100%; box-sizing: border-box; }
.dispatch-panel-log { overflow: hidden; }
.dispatch-log-head { display: flex; align-items: center; gap: 8px; padding: 4px 4px 8px; }
.dispatch-log-head .dispatch-run-body { flex: 1; }
.dispatch-log-wrap { display: flex; flex-direction: column; flex: 1; min-height: 0; gap: 6px; }
.dispatch-log-body { flex: 1; min-height: 0; margin: 0; padding: 8px; overflow: auto; border-radius: 8px; background: var(--surface-base-hover); color: var(--text-base); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12px; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
.dispatch-log-empty { flex: 1; display: grid; place-items: center; margin: 0; color: var(--text-weak); font-size: 12px; }
.dispatch-log-foot { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 0 4px; }
.dispatch-log-follow { border: 0; border-radius: 999px; padding: 4px 10px; background: var(--surface-base-hover); color: var(--text-base); font: inherit; font-size: 12px; cursor: pointer; }
.dispatch-log-follow:hover, .dispatch-log-follow:focus-visible { background: var(--surface-base-hover); color: var(--text-strong); outline: none; }
.dispatch-panel-head { display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: 8px; padding: 4px 4px 8px 8px; }
.dispatch-panel-switch { display: flex; align-items: center; justify-self: start; }
.dispatch-panel-title { color: var(--text-strong); font-weight: 500; text-align: center; white-space: nowrap; }
.dispatch-panel-actions { display: flex; align-items: center; justify-self: end; }
.dispatch-runs { display: flex; flex-direction: column; gap: 2px; list-style: none; margin: 0; padding: 0; }
.dispatch-run { display: flex; gap: 10px; align-items: center; width: 100%; padding: 8px; border: 0; border-radius: 8px; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.dispatch-run:hover, .dispatch-run:focus-visible { background: var(--surface-base-hover); outline: none; }
.dispatch-run-bot { flex: none; display: grid; place-items: center; width: 28px; height: 28px; }
.dispatch-run-body { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.dispatch-run-title { overflow: hidden; color: var(--text-base); text-overflow: ellipsis; white-space: nowrap; }
.dispatch-muted { color: var(--text-weak); font-size: 12px; }
.dispatch-danger { color: var(--text-error); font-size: 12px; }
.dispatch-empty { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; padding: 24px; text-align: center; }
.dispatch-empty-title { margin-top: 8px; color: var(--text-strong); font-size: 15px; font-weight: 600; }
.dispatch-empty .dispatch-muted { max-width: 280px; line-height: 1.45; }
.dispatch-settings { display: flex; flex-direction: column; gap: 16px; }
.dispatch-target { display: flex; flex-direction: column; gap: 8px; width: 320px; box-sizing: border-box; }
.dispatch-target-search { width: 100%; }
.dispatch-target-list { display: flex; flex-direction: column; gap: 2px; min-height: 240px; max-height: 360px; overflow-y: auto; overscroll-behavior: contain; }
.dispatch-target-agent { margin: 6px 8px 2px; color: var(--text-weak); font-size: 11px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; }
.dispatch-target-row { display: flex; align-items: center; gap: 8px; width: 100%; padding: 6px 8px; border: 0; border-radius: 6px; background: transparent; color: var(--text-base); font: inherit; font-size: 13px; text-align: left; cursor: pointer; }
.dispatch-target-row:hover, .dispatch-target-row-active { background: var(--surface-base-hover); outline: none; }
.dispatch-target-row:focus-visible { background: var(--surface-base-hover); outline: none; }
.dispatch-target-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.dispatch-target-check { flex: none; display: grid; place-items: center; margin-left: auto; color: var(--text-weak); }
.dispatch-target-note { margin: 0; padding: 8px; color: var(--text-weak); font-size: 12px; }
`;

const definition = Plugin.define({
  i18n: {
    en: {
      title: "Dispatch",
      "switch.label": "Delegate through Dispatch",
      "switch.description":
        "Replaces OpenCode's subagents with Dispatch runs: each task goes to the agent and model Dispatch routes it to, in its own worktree, and lists under this conversation in the Dispatch app.",
      "switch.on": "Dispatch delegation is on",
      "switch.off": "Dispatch delegation is off",
      "binary.missing": "The dispatch command was not found at {{binary}}. Install Dispatch, then run dispatch setup opencode.",
      "binary.found": "Runs with {{binary}}",
      "command.toggle.on": "Turn Dispatch delegation off",
      "command.toggle.off": "Turn Dispatch delegation on",
      "command.panel": "Show Dispatch runs",
      "command.app": "Open Dispatch",
      "panel.title": "Dispatch",
      "panel.runs": "Runs",
      "panel.empty.title": "No runs yet",
      "panel.empty.on": "When the agent hands work to another agent, the run shows up here.",
      "panel.empty.off.title": "Delegation is off",
      "panel.empty.off": "This conversation uses OpenCode's own subagents. Turn Dispatch on with the switch above.",
      "panel.unreachable": "Dispatch did not answer: {{error}}",
      "panel.back": "Back to runs",
      "log.empty": "Waiting for output…",
      "log.none": "This run left no output.",
      "log.failed": "Could not read the run: {{error}}",
      "log.following": "Following the end",
      "log.paused": "Not following new lines",
      "log.follow": "Follow",
      "log.open": "Open in Dispatch",
      "pill.on": "Dispatch",
      "pill.off": "Dispatch off",
      // The check beside the runs pill: the label stays Delegate, and the check
      // and the success tone say whether this conversation must delegate.
      "pill.force.on": "Delegate",
      "pill.force.off": "Delegate",
      "pill.force.title": "Force delegation: this conversation must hand its work to Dispatch",
      // The pill beside it, while the check is on: the agent and model each
      // forced run goes to, or Auto.
      "pill.target.auto": "Auto",
      "pill.target.auto.row": "Auto (Dispatch decides)",
      "pill.target.title": "Agent and model every forced run goes to",
      "pill.target.search": "Search agents and models",
      "pill.target.loading": "Loading agents…",
      "pill.target.empty": "No agents to pick",
      "pill.target.none": "No models match",
      "pill.target.failed": "Could not read the agents: {{error}}",
      "runs.one": "{{count}} run",
      "runs.other": "{{count}} runs",
      "runs.running": "{{count}} running",
      "settings.open": "Open Dispatch",
      "open.failed": "Could not open Dispatch",
    },
  },
  setup(ctx: Ctx) {
    const rpc = () => ctx.client.rpc(Dispatch);
    const [status, setStatus] = createSignal<Status>({ enabled: true, binary: "", available: true, loaded: false });
    // Runs by session key, and the last error reading them.
    const [runs, setRuns] = createStore<Record<string, RunView[]>>({});
    const [errors, setErrors] = createStore<Record<string, string>>({});
    // The conversation's Delegate check, as the server part holds it, and the
    // agent and model its forced runs go to.
    const [forced, setForced] = createStore<Record<string, ForcedView>>({});
    // The agents and models a target can pick, read once from the CLIs when a
    // conversation is forced; undefined until then.
    const [targets, setTargets] = createSignal<TargetAgent[] | undefined>(undefined);
    const [targetsError, setTargetsError] = createSignal("");
    let targetsLoading = false;
    let targetsTried = 0;

    const loadState = async () => {
      try {
        setStatus({ ...(await rpc().state({})), loaded: true });
      } catch {
        setStatus((prev) => ({ ...prev, loaded: true }));
      }
    };

    const setEnabled = async (enabled: boolean) => {
      try {
        setStatus({ ...(await rpc().setEnabled({ enabled })), loaded: true });
        ctx.toast.show({ variant: "success", title: ctx.t(enabled ? "switch.on" : "switch.off") });
      } catch (error) {
        ctx.toast.show({ variant: "error", title: ctx.t("title"), description: String((error as Error)?.message || error) });
      }
    };
    const toggle = () => setEnabled(!status().enabled);

    // forcedView reads the check and its target from an RPC answer, empty strings
    // for Auto, so a server part from before the target only says force.
    const forcedView = (answer: { force?: boolean; agent?: string; model?: string }): ForcedView => ({
      force: answer.force === true,
      agent: answer.agent || "",
      model: answer.model || "",
    });

    // ensureTargets reads the agents and models a target can pick, once: a
    // conversation that is forced gets the list behind its Agent pill, and a
    // failure is tried again after a pause rather than on every render.
    const ensureTargets = async () => {
      if (targets() !== undefined || targetsLoading) return;
      if (Date.now() - targetsTried < 15_000) return;
      targetsTried = Date.now();
      targetsLoading = true;
      try {
        const answer = await rpc().targets({});
        setTargets(Array.isArray(answer.agents) ? answer.agents : []);
        setTargetsError(answer.error || "");
      } catch (error) {
        setTargetsError(String((error as Error)?.message || error));
      } finally {
        targetsLoading = false;
      }
    };

    const refresh = async (session: Session | undefined) => {
      if (!session || session.pending) return;
      try {
        // The runs and the Delegate check at once; a server part that does not
        // know force yet leaves the check alone rather than the runs missing.
        const [answer, flag] = await Promise.all([
          rpc().runs({ sessionID: session.id, directory: session.directory || "" }),
          rpc().force({ sessionID: session.id, directory: session.directory || "" }).catch(() => undefined),
        ]);
        // Reconciled by id: a poll that brings the same runs keeps their rows,
        // so each bot stays mounted instead of being made again every poll.
        setRuns(session.key, reconcile(answer.runs || [], { key: "id" }));
        setErrors(session.key, answer.error || "");
        if (flag) {
          const view = forcedView(flag);
          setForced(session.key, view);
          if (view.force) void ensureTargets();
        }
      } catch (error) {
        setErrors(session.key, String((error as Error)?.message || error));
      }
    };

    // toggleForce ticks or unticks the Delegate check: ticked, every model call
    // of the conversation must hand its work to Dispatch.
    const toggleForce = async (session: Session) => {
      const force = forced[session.key]?.force !== true;
      try {
        const answer = await rpc().setForce({ sessionID: session.id, directory: session.directory || "", force });
        setForced(session.key, forcedView(answer));
        if (answer.force === true) void ensureTargets();
      } catch (error) {
        ctx.toast.show({ variant: "error", title: ctx.t("title"), description: String((error as Error)?.message || error) });
      }
    };

    // pickTarget sends the conversation's forced runs to an agent and model,
    // empty strings for Auto; it is stored with the check, per conversation.
    const pickTarget = async (session: Session, agent: string, model: string) => {
      const force = forced[session.key]?.force === true;
      try {
        const answer = await rpc().setForce({ sessionID: session.id, directory: session.directory || "", force, agent, model });
        setForced(session.key, forcedView(answer));
      } catch (error) {
        ctx.toast.show({ variant: "error", title: ctx.t("title"), description: String((error as Error)?.message || error) });
      }
    };

    const sessionRuns = (session: Session | undefined): RunView[] => (session ? runs[session.key] || [] : []);
    // The server part opens dispatch:// links: the Desktop opens only web links.
    const openLink = async (url: string) => {
      try {
        const answer = await rpc().open({ url });
        if (!answer.opened) throw new Error(answer.error || "Dispatch did not open");
      } catch (error) {
        ctx.toast.show({ variant: "error", title: ctx.t("open.failed"), description: String((error as Error)?.message || error) });
      }
    };
    const openRun = (run: RunView) => void openLink(runLink(run.id));
    const openApp = () => void openLink("dispatch://");

    void loadState();

    // Poll the routed session: often while a run is going, else now and then.
    // One poll at a time; the switch is read again too, since the CLI can
    // turn it on or off with no event.
    let timer: ReturnType<typeof setTimeout> | undefined;
    let polling = false;
    let disposed = false;
    const tick = async () => {
      clearTimeout(timer);
      timer = undefined;
      if (polling || disposed) return;
      polling = true;
      try {
        const session = ctx.sessions.current();
        if (document.visibilityState === "visible") {
          await loadState();
          if (session) await refresh(session);
        }
      } finally {
        polling = false;
      }
      if (disposed) return;
      const active = sessionRuns(ctx.sessions.current()).some((run) => ACTIVE.has(run.status));
      timer = setTimeout(tick, active ? POLL_ACTIVE_MS : POLL_IDLE_MS);
    };
    timer = setTimeout(tick, 500);

    let offChanged = () => {};
    let offStarted = () => {};
    let offForced = () => {};
    try {
      offChanged = rpc().events.on("changed", (event: { data: { enabled: boolean } }) =>
        setStatus((prev) => ({ ...prev, enabled: event.data.enabled })),
      );
      offStarted = rpc().events.on("started", () => void tick());
      // The check changed in another window, or from the CLI: read it again.
      offForced = rpc().events.on("forced", () => void tick());
    } catch {
      // The server part is not up yet; polling still finds the runs.
    }

    ctx.add(Style, CSS);

    ctx.add(SettingsPage, {
      id: "dispatch",
      icon: "fork",
      get title() {
        return ctx.t("title");
      },
      get entries() {
        return [{ id: "delegation", title: ctx.t("switch.label"), keywords: "dispatch subagent delegate route" }];
      },
      render: () => (
        <div class="dispatch-settings">
          <ctx.ui.Switch
            checked={status().enabled}
            onChange={(checked: boolean) => void setEnabled(checked)}
            label={ctx.t("switch.label")}
            description={ctx.t("switch.description")}
          />
          <p class="dispatch-muted">
            {status().available
              ? ctx.t("binary.found", { binary: status().binary })
              : ctx.t("binary.missing", { binary: status().binary || "dispatch" })}
          </p>
          <div>
            <ctx.ui.Button variant="neutral" icon="square-arrow-top-right" onClick={openApp}>
              {ctx.t("settings.open")}
            </ctx.ui.Button>
          </div>
        </div>
      ),
    });

    ctx.add(Command, () => ({
      id: "toggle",
      title: ctx.t(status().enabled ? "command.toggle.on" : "command.toggle.off"),
      keywords: "dispatch subagent delegate",
      slash: { name: "dispatch" },
      run: () => toggle(),
    }));

    ctx.add(Command, () => ({
      id: "panel",
      title: ctx.t("command.panel"),
      keywords: "dispatch runs subagents",
      enabled: !!ctx.sessions.current(),
      run: () => {
        const session = ctx.sessions.current();
        if (session) ctx.layout.open(`${ctx.id}:runs`, session);
      },
    }));

    ctx.add(Command, () => ({
      id: "app",
      title: ctx.t("command.app"),
      keywords: "dispatch",
      run: openApp,
    }));

    ctx.add(MenuItem, () => {
      const session = ctx.sessions.current();
      const count = sessionRuns(session).length;
      return {
        menu: "session.panel",
        id: "runs",
        title: ctx.t("panel.title"),
        icon: "fork",
        keybind: `${ctx.id}.panel`,
        description: count ? ctx.plural("runs", count) : undefined,
        opened: !!session && ctx.layout.state(`${ctx.id}:runs`, session) !== "closed",
        run: () => {
          if (session) ctx.layout.open(`${ctx.id}:runs`, session);
        },
      };
    });

    const openPanel = (session: Session | undefined) => {
      if (session) ctx.layout.open(`${ctx.id}:runs`, session);
    };

    ctx.add(ComposerPill, {
      id: "runs",
      // Always there: a click opens the runs panel, whose header holds the switch.
      show: (input: { session: Session }) => {
        const list = sessionRuns(input.session);
        const on = status().enabled;
        const running = list.filter((run) => ACTIVE.has(run.status)).length;
        let label = ctx.t(on ? "pill.on" : "pill.off");
        if (running) label = `${label} · ${ctx.t("runs.running", { count: running })}`;
        else if (list.length) label = `${label} · ${ctx.plural("runs", list.length)}`;
        return {
          icon: running ? "status-active" : on ? "fork" : "circle-ban-sign",
          label,
          tone: on ? "success" : undefined,
          title: ctx.t("command.panel"),
          run: () => openPanel(input.session),
        };
      },
    });

    ctx.add(ComposerPill, {
      id: "force",
      // The Delegate check beside it: ticked, the conversation must hand its
      // work to Dispatch. It goes with delegation, so it is hidden while no
      // dispatch_run can take the work.
      show: (input: { session: Session }) => {
        if (!status().enabled) return undefined;
        const on = forced[input.session.key]?.force === true;
        return {
          icon: on ? "circle-check" : undefined,
          label: ctx.t(on ? "pill.force.on" : "pill.force.off"),
          tone: on ? "success" : undefined,
          title: ctx.t("pill.force.title"),
          run: () => void toggleForce(input.session),
        };
      },
    });

    // TargetMenu is the Agent pill's popover: a search field, then Auto and the
    // models the installed CLIs list, grouped by agent. The host owns the
    // capsule, the placement, the focus and the dismissal; its list scrolls in
    // a fixed box, so a long list never outgrows the window or jumps while the
    // search narrows it.
    const TargetMenu = (props: { session: Session; close: () => void }) => {
      const [query, setQuery] = createSignal("");
      const [active, setActive] = createSignal(0);
      const elements: (HTMLButtonElement | undefined)[] = [];
      void ensureTargets();
      const pick = () => forced[props.session.key];
      const chosen = (agent: string, model: string) => pick()?.agent === agent && (pick()?.model || "") === model;
      // Auto first, then each agent the search keeps, with one row per model.
      const entries = () => {
        const list: TargetEntry[] = [];
        let index = 0;
        list.push({ kind: "row", index: index++, agent: "", model: "", label: ctx.t("pill.target.auto.row") });
        for (const agent of filterTargets(targets() || [], query())) {
          list.push({ kind: "agent", name: agent.name });
          for (const model of agent.models) list.push({ kind: "row", index: index++, agent: agent.name, model, label: model });
        }
        return list;
      };
      const rows = () => entries().flatMap((entry) => (entry.kind === "row" ? [entry] : []));
      const note = () => {
        const list = targets();
        if (!list) {
          return targetsError() ? ctx.t("pill.target.failed", { error: targetsError() }) : ctx.t("pill.target.loading");
        }
        if (!list.length) return ctx.t("pill.target.empty");
        if (query().trim() && rows().length <= 1) return ctx.t("pill.target.none");
        return "";
      };
      const choose = (agent: string, model: string) => {
        void pickTarget(props.session, agent, model);
        props.close();
      };
      const move = (delta: number) => {
        const next = Math.max(0, Math.min(rows().length - 1, active() + delta));
        setActive(next);
        elements[next]?.scrollIntoView({ block: "nearest" });
      };
      const onKeyDown = (event: KeyboardEvent) => {
        if (event.key === "ArrowDown") {
          event.preventDefault();
          move(1);
        } else if (event.key === "ArrowUp") {
          event.preventDefault();
          move(-1);
        } else if (event.key === "Enter") {
          const row = rows()[active()];
          if (!row) return;
          event.preventDefault();
          choose(row.agent, row.model);
        }
      };
      return (
        <div class="dispatch-target" onKeyDown={onKeyDown}>
          <ctx.ui.TextInput
            class="dispatch-target-search"
            value={query()}
            placeholder={ctx.t("pill.target.search")}
            label={ctx.t("pill.target.search")}
            autofocus
            onInput={(value: string) => {
              setQuery(value);
              setActive(0);
            }}
          />
          <div class="dispatch-target-list" role="listbox" aria-label={ctx.t("pill.target.title")}>
            <For each={entries()}>
              {(entry) =>
                entry.kind === "agent" ? (
                  <p class="dispatch-target-agent">{entry.name}</p>
                ) : (
                  <button
                    ref={(el) => (elements[entry.index] = el)}
                    type="button"
                    role="option"
                    aria-selected={chosen(entry.agent, entry.model)}
                    class="dispatch-target-row"
                    classList={{ "dispatch-target-row-active": active() === entry.index }}
                    onClick={() => choose(entry.agent, entry.model)}
                  >
                    <span class="dispatch-target-name" title={entry.label}>
                      {entry.label}
                    </span>
                    <Show when={chosen(entry.agent, entry.model)}>
                      <span class="dispatch-target-check">
                        <ctx.ui.Icon name="circle-check" size="small" />
                      </span>
                    </Show>
                  </button>
                )
              }
            </For>
            <Show when={note()}>
              <p class="dispatch-target-note">{note()}</p>
            </Show>
          </div>
        </div>
      );
    };

    ctx.add(ComposerPill, {
      id: "target",
      // The agent and model every forced run goes to, beside the Delegate
      // check while it is on: Auto, or a model of one of the agents here. The
      // capsule names the pick, and the popover searches the models the CLIs
      // list, read the first time a conversation needs them.
      show: (input: { session: Session }) => {
        if (!status().enabled || forced[input.session.key]?.force !== true) return undefined;
        void ensureTargets();
        const pick = forced[input.session.key];
        return {
          icon: "models",
          label: pick?.agent ? (pick.model ? `${pick.agent} · ${pick.model}` : pick.agent) : ctx.t("pill.target.auto"),
          title: ctx.t("pill.target.title"),
        };
      },
      menu: (input: { session: Session; close: () => void }) => (
        <TargetMenu session={input.session} close={input.close} />
      ),
    });

    const tab = {
      id: "runs",
      get title() {
        return ctx.t("panel.title");
      },
      closable: "hover",
    };

    // RunLog is a run's page in the panel: its status, followed by the end of
    // its output while the reader stays there, and asked again while it works.
    const RunLog = (props: { run: RunView; session: Session; onBack: () => void }) => {
      const [log, setLog] = createSignal<LogView | undefined>(undefined);
      const [failure, setFailure] = createSignal("");
      const [following, setFollowing] = createSignal(true);
      let area: HTMLPreElement | undefined;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let stopped = false;

      const status = () => log()?.status || props.run.status;
      const working = () => ACTIVE.has(status());
      const atEnd = () => {
        const el = area;
        return !el || el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_SLACK;
      };
      const toEnd = () => {
        const el = area;
        if (el) el.scrollTop = el.scrollHeight;
        setFollowing(true);
      };

      const load = async () => {
        try {
          const answer = (await rpc().log({ runID: props.run.id, directory: props.session.directory || "" })) as LogView;
          if (stopped) return false;
          setLog(answer);
          setFailure(answer.error || "");
          if (following()) toEnd();
          return !answer.error && ACTIVE.has(answer.status);
        } catch (error) {
          if (stopped) return false;
          setFailure(String((error as Error)?.message || error));
          return false;
        }
      };
      const tick = async () => {
        timer = undefined;
        if (stopped || !(await load())) return;
        timer = setTimeout(tick, LOG_POLL_MS);
      };

      onMount(() => void tick());
      onCleanup(() => {
        stopped = true;
        clearTimeout(timer);
      });

      return (
        <div class="dispatch-panel dispatch-panel-log">
          <div class="dispatch-log-head">
            <ctx.ui.Tooltip value={ctx.t("panel.back")}>
              <ctx.ui.IconButton icon="arrow-left" variant="ghost" size="small" label={ctx.t("panel.back")} onClick={props.onBack} />
            </ctx.ui.Tooltip>
            <span class="dispatch-run-bot">
              <AgentBot agent={props.run.agent} seed={props.run.id} status={status()} mergedAt={props.run.mergedAt} />
            </span>
            <span class="dispatch-run-body">
              <span class="dispatch-run-title">{firstLine(props.run.prompt, 120) || props.run.id}</span>
              <span class={FAILED.has(status()) ? "dispatch-danger" : "dispatch-muted"}>{runDetail({ ...props.run, status: status() })}</span>
            </span>
            <ctx.ui.Tooltip value={ctx.t("log.open")}>
              <ctx.ui.IconButton icon="square-arrow-top-right" variant="ghost" size="small" label={ctx.t("log.open")} onClick={() => openRun(props.run)} />
            </ctx.ui.Tooltip>
          </div>
          <Show when={props.run.error}>
            <p class="dispatch-danger">{props.run.error}</p>
          </Show>
          <Show when={failure()}>
            <p class="dispatch-danger">{ctx.t("log.failed", { error: failure() })}</p>
          </Show>
          <div class="dispatch-log-wrap">
            <Show
              when={log()?.text}
              fallback={<p class="dispatch-log-empty">{ctx.t(working() ? "log.empty" : "log.none")}</p>}
            >
              <pre class="dispatch-log-body" ref={(el) => (area = el)} onScroll={() => setFollowing(atEnd())}>
                {log()?.text}
              </pre>
            </Show>
            <Show when={working() || !following()}>
              <div class="dispatch-log-foot">
                <span class="dispatch-muted">{ctx.t(following() ? "log.following" : "log.paused")}</span>
                <Show when={!following()}>
                  <button class="dispatch-log-follow" type="button" onClick={toEnd}>
                    {ctx.t("log.follow")}
                  </button>
                </Show>
              </div>
            </Show>
          </div>
        </div>
      );
    };

    const RunsView = (props: { session: Session }) => {
      // The run whose log this panel shows, if any.
      const [selected, setSelected] = createSignal<RunView | undefined>(undefined);
      const list = () => sessionRuns(props.session);
      const error = () => (props.session ? errors[props.session.key] : "");
      // The row the panel polls keeps the open run's status and detail fresh.
      const selectedRun = () => {
        const open = selected();
        if (!open) return undefined;
        return list().find((run) => run.id === open.id) || open;
      };
      return (
        <Show
          when={selectedRun()}
          fallback={
            <div class="dispatch-panel">
              <div class="dispatch-panel-head">
                <span class="dispatch-panel-switch">
                  <ctx.ui.Tooltip value={ctx.t(status().enabled ? "command.toggle.on" : "command.toggle.off")}>
                    <ctx.ui.Switch checked={status().enabled} onChange={(checked: boolean) => void setEnabled(checked)} aria-label={ctx.t(status().enabled ? "command.toggle.on" : "command.toggle.off")} />
                  </ctx.ui.Tooltip>
                </span>
                <span class="dispatch-panel-title">
                  {ctx.t("panel.runs")}
                  <Show when={list().length > 0}>
                    <span class="dispatch-muted"> · {list().length}</span>
                  </Show>
                </span>
                <span class="dispatch-panel-actions">
                  <ctx.ui.Tooltip value={ctx.t("command.app")}>
                    <ctx.ui.IconButton icon="square-arrow-top-right" variant="ghost" size="small" label={ctx.t("command.app")} onClick={openApp} />
                  </ctx.ui.Tooltip>
                </span>
              </div>
              <Show when={error()}>
                <p class="dispatch-danger">{ctx.t("panel.unreachable", { error: error() })}</p>
              </Show>
              <Show
                when={list().length > 0}
                fallback={
                  <div class="dispatch-empty">
                    <EmptyBot mood={status().enabled ? "sleeping" : "lost"} />
                    <p class="dispatch-empty-title">{ctx.t(status().enabled ? "panel.empty.title" : "panel.empty.off.title")}</p>
                    <p class="dispatch-muted">{ctx.t(status().enabled ? "panel.empty.on" : "panel.empty.off")}</p>
                  </div>
                }
              >
                <ul class="dispatch-runs">
                  <For each={list()}>
                    {(run) => (
                      <li>
                        <button class="dispatch-run" type="button" title={run.prompt} onClick={() => setSelected(run)}>
                          <span class="dispatch-run-bot">
                            <AgentBot agent={run.agent} seed={run.id} status={run.status} mergedAt={run.mergedAt} />
                          </span>
                          <span class="dispatch-run-body">
                            <span class="dispatch-run-title">{firstLine(run.prompt, 120) || run.id}</span>
                            <span class={FAILED.has(run.status) ? "dispatch-danger" : "dispatch-muted"}>{runDetail(run)}</span>
                          </span>
                        </button>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          }
        >
          {(run) => <RunLog run={run()} session={props.session} onBack={() => setSelected(undefined)} />}
        </Show>
      );
    };

    ctx.add(Panel, {
      id: "runs",
      region: "side",
      mobile: {
        get title() {
          return ctx.t("panel.title");
        },
        order: 60,
        kind: "menu",
        icon: "fork",
      },
      list: (input: { open: string[] }) => (input.open.includes("runs") ? [tab] : []),
      focus: (input: { session: Session }) => void refresh(input.session),
      render: (props: { session: Session }) => <RunsView session={props.session} />,
    });

    return () => {
      disposed = true;
      clearTimeout(timer);
      offChanged();
      offStarted();
      offForced();
    };
  },
});

export default definition;
export { Dispatch };
export const helpers = { firstLine, statusLabel, runDetail, runLink, formatTokens, formatDuration, filterTargets, expressionFor };
