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

Independent delegations go in the same response, each with its own `id`; then one `wait` per id. Write runs in a git repo get their own worktree and branch, so they do not conflict.

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
```

## Roles

A role is the kind of work a run does: instructions, `when` it fits, `readonly`, `timeout`, a lowest tier, and its own board of models per tier (primary first, then fallbacks; a tier with none uses the general tier table). Built in: `explorer`, `implementer`, `tester`, `reviewer`, `planner`; `roles.json` (the dispatch state dir) and `<repo>/.dispatch/roles.json` add or replace them (project wins).

Role and difficulty work together: the role picks the board, the task's difficulty picks the column. On Auto with a classifier, dispatch picks the role itself from each one's `when` (Jev and the CLI classifier; not OpenAI), at P ≥ 0.5, else `general`. A picked role never changes access, so read-only still comes only from `--readonly` or `--role`. Name one yourself when you know the kind of work:

```bash
dispatch roles                    # effective roles
dispatch run --role reviewer --dir <repo> - <<< "review branch X"
dispatch run --role none …        # no role, no pick
```

Explicit flags win over the role. `prefer` and `avoid` are weights in the ranking, not filters.

## Filters and routing (optional; Auto is the default with a classifier, else normal)

`run` accepts `--provider`, `--model`, `--effort low|medium|high|xhigh`, `--agent`, `--priority speed|balanced|quality`, `--relax`.
A filter narrows candidates and fallback; with none left the run fails (`--relax` returns to tier routing, with a note). `--model` or `--provider` replaces Auto. `--priority` sets how sure Auto must be: speed 0.70, balanced 0.80, quality 0.90.
`dispatch route [same filters] [--json] "<prompt>"` shows candidates, eliminations, and the winner without running anything; on Auto, with each one's pass probability and cost. `dispatch models` lists how Auto prices each model.
`dispatch config get|set|allow|deny` keeps the persistent policy, one for every repository (allow/deny of providers, models, agents; deny wins).
`dispatch agents` lists the installed agents; `dispatch agents add <name> -- <command>` adds any agent CLI that speaks ACP (the Agent Client Protocol), and `dispatch agents remove <name>` drops it.

## Rules

- **The verbose agent log is for the user** (Desktop Runs, TUI). Do not read it; use the summary.
- **No recursive delegation** from inside a dispatch run.
- **One writer per directory**, and no push, deploy, or secrets in the brief.
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
