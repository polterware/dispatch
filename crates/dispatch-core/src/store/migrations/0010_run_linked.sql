-- The other repositories a run spans, each with a worktree of its own (JSON list).
ALTER TABLE dispatch_runs ADD COLUMN linked_json TEXT NOT NULL DEFAULT '';
