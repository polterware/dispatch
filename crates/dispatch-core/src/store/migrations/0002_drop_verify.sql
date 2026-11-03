-- Verify and the learned router are gone: a run merges its worktree directly,
-- and Auto prices models from the model catalog alone.
ALTER TABLE dispatch_runs DROP COLUMN no_verify;
ALTER TABLE dispatch_runs DROP COLUMN verification;
ALTER TABLE dispatch_runs DROP COLUMN verify_json;
ALTER TABLE dispatch_runs DROP COLUMN verify_ms;
ALTER TABLE dispatch_runs DROP COLUMN merge_check;
UPDATE dispatch_runs SET exit_class = 'ok' WHERE exit_class = 'verify_fail';
DELETE FROM dispatch_run_events WHERE kind = 'verified';
UPDATE dispatch_worktrees SET status = 'active' WHERE status = 'verifying';

ALTER TABLE dispatch_route_decisions DROP COLUMN propensity;

-- Routing reads only an agent's cooldown and its quota snapshots.
ALTER TABLE dispatch_agents DROP COLUMN quota_source;
ALTER TABLE dispatch_agents DROP COLUMN confidence;
ALTER TABLE dispatch_agents DROP COLUMN risk;
