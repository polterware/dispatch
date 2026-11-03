-- Subagents are roles now: the classifier's pick of a run's role, as JSON.
ALTER TABLE dispatch_runs RENAME COLUMN subagent_json TO role_json;
