-- What every run used and how it went, kept apart from the runs themselves: deleting a run, a
-- conversation, or a repository never takes its spend or stats with it. Triggers copy each
-- run as it is written, so every code path that saves a run keeps its history current; the
-- prompt, output, and transcript stay only with the run.
CREATE TABLE dispatch_run_history (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    tier TEXT NOT NULL DEFAULT '',
    requested_tier TEXT NOT NULL DEFAULT '',
    execution_tier TEXT NOT NULL DEFAULT '',
    agent TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    provider TEXT NOT NULL DEFAULT '',
    effort TEXT NOT NULL DEFAULT '',
    dir TEXT NOT NULL DEFAULT '',
    repo TEXT NOT NULL DEFAULT '',
    read_only INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT '',
    exit_code INTEGER NOT NULL DEFAULT 0,
    exit_class TEXT NOT NULL DEFAULT '',
    conversation_id TEXT NOT NULL DEFAULT '',
    profile TEXT NOT NULL DEFAULT '',
    requested_json TEXT,
    resolved_json TEXT,
    attempts_json TEXT,
    classifier TEXT NOT NULL DEFAULT '',
    difficulty REAL,
    queued_ms INTEGER,
    run_ms INTEGER,
    usage_json TEXT,
    created_at TEXT NOT NULL DEFAULT '',
    started_at TEXT,
    finished_at TEXT
);

CREATE INDEX dispatch_run_history_project ON dispatch_run_history (project_id, created_at);
CREATE INDEX dispatch_run_history_dir ON dispatch_run_history (dir, created_at);

INSERT INTO dispatch_run_history (
    id, project_id, status, tier, requested_tier, execution_tier, agent, model, provider, effort,
    dir, repo, read_only, error, exit_code, exit_class, conversation_id, profile,
    requested_json, resolved_json, attempts_json, classifier, difficulty, queued_ms, run_ms,
    usage_json, created_at, started_at, finished_at
)
SELECT
    id, project_id, status, tier, requested_tier, execution_tier, agent, model, provider, effort,
    dir, repo, read_only, error, exit_code, exit_class, conversation_id, profile,
    requested_json, resolved_json, attempts_json, classifier, difficulty, queued_ms, run_ms,
    usage_json, created_at, started_at, finished_at
FROM dispatch_runs;

CREATE TRIGGER dispatch_run_history_insert AFTER INSERT ON dispatch_runs
BEGIN
    -- A delete, then an insert: an upsert's conflict policy on the run's own write would
    -- override an OR REPLACE here.
    DELETE FROM dispatch_run_history WHERE id = NEW.id;
    INSERT INTO dispatch_run_history (
        id, project_id, status, tier, requested_tier, execution_tier, agent, model, provider,
        effort, dir, repo, read_only, error, exit_code, exit_class, conversation_id, profile,
        requested_json, resolved_json, attempts_json, classifier, difficulty, queued_ms, run_ms,
        usage_json, created_at, started_at, finished_at
    ) VALUES (
        NEW.id, NEW.project_id, NEW.status, NEW.tier, NEW.requested_tier, NEW.execution_tier,
        NEW.agent, NEW.model, NEW.provider, NEW.effort, NEW.dir, NEW.repo, NEW.read_only,
        NEW.error, NEW.exit_code, NEW.exit_class, NEW.conversation_id, NEW.profile,
        NEW.requested_json, NEW.resolved_json, NEW.attempts_json, NEW.classifier,
        NEW.difficulty, NEW.queued_ms, NEW.run_ms, NEW.usage_json, NEW.created_at,
        NEW.started_at, NEW.finished_at
    );
END;

CREATE TRIGGER dispatch_run_history_update AFTER UPDATE ON dispatch_runs
BEGIN
    -- A delete, then an insert: an upsert's conflict policy on the run's own write would
    -- override an OR REPLACE here.
    DELETE FROM dispatch_run_history WHERE id = NEW.id;
    INSERT INTO dispatch_run_history (
        id, project_id, status, tier, requested_tier, execution_tier, agent, model, provider,
        effort, dir, repo, read_only, error, exit_code, exit_class, conversation_id, profile,
        requested_json, resolved_json, attempts_json, classifier, difficulty, queued_ms, run_ms,
        usage_json, created_at, started_at, finished_at
    ) VALUES (
        NEW.id, NEW.project_id, NEW.status, NEW.tier, NEW.requested_tier, NEW.execution_tier,
        NEW.agent, NEW.model, NEW.provider, NEW.effort, NEW.dir, NEW.repo, NEW.read_only,
        NEW.error, NEW.exit_code, NEW.exit_class, NEW.conversation_id, NEW.profile,
        NEW.requested_json, NEW.resolved_json, NEW.attempts_json, NEW.classifier,
        NEW.difficulty, NEW.queued_ms, NEW.run_ms, NEW.usage_json, NEW.created_at,
        NEW.started_at, NEW.finished_at
    );
END;
