// Dispatch in OpenCode Desktop: the delegation switch (the runs panel's header,
// Settings › Dispatch, the palette, and /dispatch), and the conversation's runs
// in a side panel with each agent's bot, opened from the composer pill; a run
// opens in the Dispatch app.
// `pnpm --dir plugins/opencode-gui build` writes it to
// crates/dispatch-core/src/setup/opencode/gui-experimental.cjs, which the
// dispatch binary embeds and `dispatch setup opencode` installs.
import { Command, ComposerPill, MenuItem, Panel, Plugin, SettingsPage, Style } from "@opencode/plugin/gui";
import { For, Show, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { Dispatch } from "../../../crates/dispatch-core/src/setup/opencode/rpc.js";
import { AgentBot, EmptyBot } from "./avatars";
import { ACTIVE, FAILED, firstLine, formatDuration, formatTokens, runDetail, runLink, statusLabel, type RunView } from "./runs";

// The app's context; its types live in @opencode/plugin/gui.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Ctx = any;
type Session = { key: string; id: string; directory?: string; pending?: boolean };
type Status = { enabled: boolean; binary: string; available: boolean; loaded: boolean };

const POLL_ACTIVE_MS = 3_000;
const POLL_IDLE_MS = 15_000;

const CSS = `
.dispatch-panel { display: flex; flex-direction: column; gap: 4px; padding: 8px; overflow: auto; height: 100%; box-sizing: border-box; }
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
      "pill.on": "Dispatch",
      "pill.off": "Dispatch off",
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

    const refresh = async (session: Session | undefined) => {
      if (!session || session.pending) return;
      try {
        const answer = await rpc().runs({ sessionID: session.id, directory: session.directory || "" });
        // Reconciled by id: a poll that brings the same runs keeps their rows,
        // so each bot stays mounted instead of being made again every poll.
        setRuns(session.key, reconcile(answer.runs || [], { key: "id" }));
        setErrors(session.key, answer.error || "");
      } catch (error) {
        setErrors(session.key, String((error as Error)?.message || error));
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
    try {
      offChanged = rpc().events.on("changed", (event: { data: { enabled: boolean } }) =>
        setStatus((prev) => ({ ...prev, enabled: event.data.enabled })),
      );
      offStarted = rpc().events.on("started", () => void tick());
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

    const tab = {
      id: "runs",
      get title() {
        return ctx.t("panel.title");
      },
      closable: "hover",
    };

    const RunsView = (props: { session: Session }) => {
      const list = () => sessionRuns(props.session);
      const error = () => (props.session ? errors[props.session.key] : "");
      return (
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
                    <button class="dispatch-run" type="button" title={run.prompt} onClick={() => openRun(run)}>
                      <span class="dispatch-run-bot">
                        <AgentBot agent={run.agent} seed={run.id} status={run.status} />
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
    };
  },
});

export default definition;
export { Dispatch };
export const helpers = { firstLine, statusLabel, runDetail, runLink, formatTokens, formatDuration };
