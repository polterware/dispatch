---
name: dispatch
description: Delegate a task to another coding agent (Codex, Claude Code, Cursor, Grok, Antigravity, OpenCode, Pi, Gemini, Qwen, Copilot, Kimi, or an ACP agent you added), routed by Auto (the cheapest model the benchmarks expect to do the task) or a difficulty tier (easy|normal|hard), and get a compact result back. Use it to save orchestrator tokens or to parallelize work: localized implementation, tests, review, code reading, investigation. Do not use it for trivial tasks you can finish in a few calls.
---

# dispatch

`dispatch run` routes a task to an installed agent CLI and returns a compact result.
With no `--tier` and a classifier on (`dispatch config set classifier jev|openai|cli`), **Auto** asks the classifier how hard the task is, then weighs each model's chance to do it (from public benchmarks: CursorBench, else the Epoch Capabilities Index), its cost, and the account's quota, and picks the cheapest model likely to succeed. If none meets the priority's bound, it picks the cheapest within 0.02 of the best bound. `--tier` routes that tier's table instead: its first model, or the next one when that account's quota is low (90% used, or running out before it resets) or rate limited. The core falls back when an agent fails. Routing reads only the model catalog's benchmarks (CursorBench, ECI, and models.dev prices), the classifier, and the CLIs' quota; a model no benchmark rates counts as mid-field.
You are the conductor: delegate, read short summaries, integrate.

## When to delegate

Delegate: localized implementation, tests, review, code reading, independent parallel work.
Do it yourself: a one-line change, work that needs this whole conversation, or a brief that costs more than the task.
Leave the route on Auto unless you need a tier; then pick the lowest one that can do the job.

| Tier | For |
|---|---|
| `easy` | clear, localized task; reading; simple tests; mechanical refactor |
| `normal` | feature across a few files; bug with an unknown cause |
| `hard` | architecture; sensitive code (auth, money, permissions, data); hard bugs |

Without a classifier there is no Auto: a run with no `--tier` takes `normal`, and `--tier auto` is refused. `dispatch classify [--card <slug>] "<prompt>"` asks the classifier and prints P(easy/normal/hard) and the tier, and `dispatch route --card <slug> "<prompt>"` shows how Auto prices each model for that card.

## Recipe

1. **Start the run.** It returns right away with an `id`; the core executes it in the background.

```bash
dispatch run --card <slug> - <<'BRIEF'
<goal; what it may and may not touch; the exact test command; final report in <= 10 lines>
BRIEF
```
   The brief carries **paths, never pasted code**.
2. **Wait:** `dispatch wait <id> --timeout 9m`. Exit 3 means still running: call `wait` again. Do not end the turn with runs pending.
3. **Read the summary** (compact JSON). The agent text is in `dispatch result <id> --section agent`. Check `git -C <worktree> diff --stat` and run the tests. Do not read the whole diff.
   **Follow up** on a small miss (a failing test, a missed file) with `dispatch send <id> - <<< "<what to fix>"`: it goes to the same agent's session, in the same worktree, as a new run to `wait` on, and the agent keeps what it already read. When the run's model was not up to the task, start a new `run` with a higher `--tier` and the findings instead.
4. **Integrate:** `dispatch merge <id>` (COMPLETED runs only) merges the run's branch and removes its worktree; you run the tests before it. A merge conflict is detected with `git merge-tree` before the working tree is touched, and the merge stops there. The result prints the `git revert` that undoes it. A failed run: `dispatch clean <id>`.

## Parallelism

Independent delegations go in the same response, each with its own `id`; then one `wait` per id. Write runs in a git repo get their own worktree and branch, so they do not conflict: any number run in one folder at once. That is the default; `--worktree-mode worktree` asks for it outright (refused outside git), and `--worktree-mode in-place` edits the folder itself (`--dir <repo>` says which folder). Only a second run that would edit the folder itself (in-place, outside git, or `--worktree <folder>`) is refused while the first works: start it with `--worktree-mode worktree` instead.

A run that changed nothing leaves no worktree or branch behind; one with changes keeps them for `merge` or `clean`.

## Result

`run` and `wait` print a summary: `status`, `agent`, `model`, `tier`, `run_ms`, files and +/-, `attempts`, `branch`, `worktree`, and pointers in `details`. Read more with `dispatch result <id> --section summary|agent|files|attempts|meta`.

- `attempts` > 1 means a **fallback** happened; a rate limit puts the agent on a 1h cooldown.
- Exit codes: 0 ok, 124 timeout, 1 other failures; `wait` returns 3 while the run is still going.

## Manage runs

```bash
dispatch runs                  # history
dispatch cancel <id>         # cancel a running run
dispatch recover <id>        # ORPHANED: commit the preserved work
dispatch clean <id> [--force]  # discard the worktree
dispatch worktrees [--prune]   # list leftover worktrees; --prune removes the ones with nothing to merge
dispatch delete <id>... | --conversation <id>  # delete ended runs and the worktrees Dispatch cut for them
dispatch rename <conversation-id> <name>  # name a conversation as the app shows it
dispatch repo ls|add|rm        # the repositories Dispatch knows (same as dispatch workspace); rm deletes their runs
```

## Roles

A role is the kind of work a run does: instructions, `when` it fits, `readonly`, `timeout`, a lowest tier, and its own board of models per tier (primary first, then fallbacks; a tier with none uses the general tier table). Only `general` is fixed: `explorer`, `implementer`, `tester`, `reviewer`, and `planner` are templates (`dispatch roles templates`, `dispatch roles add <name> --from-template <template>`) that can be deleted (`dispatch roles remove <name>`, kept in `roles.json`), while `roles.json` (the dispatch state dir) and `<repo>/.dispatch/roles.json` add or replace roles (project wins). A role keeps one, two, or three tier columns (its `columns`, and `dispatch roles save <name> --columns easy,hard`; `general` keeps all three): a task in a column it dropped routes on the nearest kept column, the next harder first, else the next easier, with a note.

Role and difficulty work together: the role picks the board, the task's difficulty picks the column. On Auto with a classifier, dispatch picks the role itself from each one's `when` (Jev and the CLI classifier; not OpenAI), at P ≥ 0.5, else `general`. A picked role never changes access, so read-only still comes only from `--readonly` or `--role`. Name one yourself when you know the kind of work:

```bash
dispatch roles                    # effective roles
dispatch run --role reviewer --dir <repo> - <<< "review branch X"
dispatch run --role none …        # no role, no pick
```

Explicit flags win over the role. `prefer` and `avoid` are weights in the ranking, not filters.

## Filters and routing (optional; Auto is the default with a classifier, else normal)

`run --with <dir>,<dir>` spans other repositories: each gets a worktree on the run's branch, and `merge`/`clean` take them all as one (a repository's own `.dispatch/hooks/link` can point its build at the others; `DISPATCH_LINKED_WORKTREES` names them).
`run` accepts `--provider`, `--model`, `--effort low|medium|high|xhigh|max` (an agent that lacks one is passed over), `--agent`, `--priority speed|balanced|quality`, `--relax`.
A filter narrows candidates and fallback; with none left the run fails (`--relax` returns to tier routing, with a note). `--model` or `--provider` replaces Auto. A `--model` with no `--agent` runs on every agent that serves that model, the one with the most quota to spare first, and falls back among them on a limit; with `--agent` too it runs only there, and a limit ends the run. An `--agent` with no `--model` lets routing pick that agent's model. `--priority` sets how sure Auto must be: speed 0.70, balanced 0.80, quality 0.90.
`dispatch route [same filters] [--json] "<prompt>"` shows candidates, eliminations, and the winner without running anything; on Auto, with each one's pass probability and cost. `dispatch models` lists how Auto prices each model.
`dispatch config get|set` keeps the persistent routing policy, one for every repository. Settings > Models and `dispatch models enable|disable <model> --agent <name>` turn one agent's models off: each agent's card there lists the models its CLI reports flat, each with a switch of its own, and a model turned off leaves that agent's list and routing alone while another agent that offers it keeps it. `dispatch models` lists their enabled state. The older `dispatch models enable|disable <provider>[/<model>]` stays, one setting for every agent. Disabled entries never route, including explicit requests and fallback. Legacy deny providers and models migrate once; allow and agent rules are dropped.
`dispatch agents` lists the installed agents; `dispatch agents add <name> -- <command>` adds any agent CLI that speaks ACP (the Agent Client Protocol), and `dispatch agents remove <name>` drops it.

## Rules

- **The verbose agent log is for the user** (Desktop Runs, TUI). Do not read it; use the summary.
- **No recursive delegation** from inside a dispatch run.
- **One writer per folder**: a run that would edit a folder another run is editing in place waits in line (PENDING, its note names the run in front) and starts by itself once the folder is free. No push, deploy, or secrets in the brief.
- Claude Code's quota is the rate limits its last run printed, unknown before its first run and once they reset. Cursor, Grok, Pi, and the ACP agents have no quota source: their usage stays unknown until a rate limit puts them on cooldown.
- A `DEGRADED` CLI still runs, with a note. A required capability that is missing is refused with a clear error.
- `FAILED` with exhausted candidates means improve the brief or set a higher tier; do not resend it unchanged.

## Diagnostics

```bash
dispatch agents            # installed CLIs and their used quota
dispatch doctor [--json]   # version, flags, capabilities; never calls a model
dispatch tiers [--check]   # effective table, validated against installed agents
dispatch roles --check     # validate roles
dispatch usage --by model --since 7d
```
