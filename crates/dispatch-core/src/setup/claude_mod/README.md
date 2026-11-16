# dispatch-delegate

Claude Code mod that makes delegation to Dispatch the default, or mandatory.

- `/delegate off | default | mandatory`: no argument opens the pane.
- `/delegate-model <model> [agent] | auto`: pins a model; no argument opens the pane.
- `/delegate-set <key> <value>` or `/delegate-set reset`: keys tier, agent, model, provider, role, effort, priority, worktree, relax, readonly.
- Default: a system-prompt section tells Claude to delegate non-trivial work with `dispatch run`.
- Mandatory: Edit/Write/NotebookEdit on project files are blocked in the main session (memory, plans and /tmp stay writable).
- Every option chosen becomes a `dispatch run` flag, added to each run that does not pass that flag itself.

Every change is checked with `dispatch route` (on the normal table when the tier is Auto, so the classifier is not called): the pane shows the model it would run on, or why no model fits. Effort lists only what the chosen agent accepts (from `dispatch doctor`). A run that would fail on the pane's flags is stopped with the reason; passing a flag on the run overrides the pane's. Only a run where a command starts is rewritten, never text that mentions it.

Settings persist across sessions. Requires the `dispatch` CLI on PATH.
