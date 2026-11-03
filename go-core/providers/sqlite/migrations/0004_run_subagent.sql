-- How the classifier picked a run's subagent, as JSON: the role and its
-- probability per role (empty when the request named the subagent or none was picked).
ALTER TABLE dispatch_runs ADD COLUMN subagent_json TEXT NOT NULL DEFAULT '';
