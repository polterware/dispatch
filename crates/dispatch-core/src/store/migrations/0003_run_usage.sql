-- What a run's agents reported spending: tokens and the API-equivalent cost,
-- as JSON (empty when no agent reported usage).
ALTER TABLE dispatch_runs ADD COLUMN usage_json TEXT NOT NULL DEFAULT '';
