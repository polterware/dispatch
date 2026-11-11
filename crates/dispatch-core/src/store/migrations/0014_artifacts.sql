-- The images runs made: one row per image, its copy living under the data directory's
-- artifacts/<run id>/. A run's artifacts go with it, and no delete touches run history.
CREATE TABLE dispatch_artifacts (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES dispatch_runs(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL DEFAULT '',
    conversation_id TEXT NOT NULL DEFAULT '',
    caller_agent TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    repo TEXT NOT NULL DEFAULT '',
    rel_path TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT '',
    bytes INTEGER NOT NULL DEFAULT 0,
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX dispatch_artifacts_created ON dispatch_artifacts (created_at DESC);
CREATE INDEX dispatch_artifacts_run ON dispatch_artifacts (run_id);
CREATE INDEX dispatch_artifacts_conversation ON dispatch_artifacts (conversation_id);
CREATE INDEX dispatch_artifacts_repo ON dispatch_artifacts (repo);
