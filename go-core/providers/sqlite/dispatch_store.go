package sqlite

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

var _ dispatch.Store = (*Store)(nil)

const selectDispatchRunColumns = `
	id, project_id, status, tier, requested_tier, execution_tier,
	agent, model, provider, effort, prompt, dir, repo, worktree, branch, base_sha,
	read_only, no_worktree, agent_run_id, card_slug,
	output, error, note, exit_code, exit_class,
	timeout_sec, session_id, conversation_id, caller_agent, resumed_of, parent_run_id, pid, agent_pid, proc_start, profile,
	prefer_json, avoid_json, requested_json, resolved_json, attempts_json,
	classifier, difficulty, difficulty_probs_json,
	files_changed, insertions, deletions, queued_ms, run_ms, usage_json, role_json,
	created_at, started_at, finished_at, merged_at, merge_sha,
	COALESCE((SELECT status FROM dispatch_worktrees WHERE run_id = dispatch_runs.id ORDER BY rowid DESC LIMIT 1), '')`

func (s *Store) SaveRun(ctx context.Context, run dispatch.Run) error {
	if run.ID == "" {
		id, err := dispatch.NewID()
		if err != nil {
			return err
		}
		run.ID = id
	}
	if run.Status == "" {
		run.Status = dispatch.StatusPending
	}
	if run.CreatedAt.IsZero() {
		run.CreatedAt = time.Now().UTC()
	}

	preferJSON, err := marshalSliceJSON(run.Prefer)
	if err != nil {
		return err
	}
	avoidJSON, err := marshalSliceJSON(run.Avoid)
	if err != nil {
		return err
	}
	requestedJSON, err := marshalOptionalJSON(run.Requested)
	if err != nil {
		return err
	}
	resolvedJSON, err := marshalOptionalJSON(run.Resolved)
	if err != nil {
		return err
	}
	attemptsJSON, err := marshalSliceJSON(run.Attempts)
	if err != nil {
		return err
	}
	difficultyProbsJSON, err := marshalSliceJSON(run.DifficultyProbs)
	if err != nil {
		return err
	}
	usageJSON := ""
	if run.Usage != nil {
		b, err := json.Marshal(run.Usage)
		if err != nil {
			return err
		}
		usageJSON = string(b)
	}
	roleJSON := ""
	if run.RolePick != nil {
		b, err := json.Marshal(run.RolePick)
		if err != nil {
			return err
		}
		roleJSON = string(b)
	}

	var mergedAt any
	if run.MergedAt != nil && !run.MergedAt.IsZero() {
		mergedAt = formatDispatchTime(*run.MergedAt)
	}

	_, err = s.DB.ExecContext(ctx, `
		INSERT INTO dispatch_runs (
			id, project_id, status, tier, requested_tier, execution_tier,
			agent, model, provider, effort, prompt, dir, repo, worktree, branch, base_sha,
			read_only, no_worktree, agent_run_id, card_slug,
			output, error, note, exit_code, exit_class,
			timeout_sec, session_id, conversation_id, caller_agent, resumed_of, parent_run_id, pid, agent_pid, proc_start, profile,
			prefer_json, avoid_json, requested_json, resolved_json, attempts_json,
			classifier, difficulty, difficulty_probs_json,
			files_changed, insertions, deletions, queued_ms, run_ms, usage_json, role_json,
			created_at, started_at, finished_at, merged_at, merge_sha
		) VALUES (
			?, ?, ?, ?, ?, ?,
			?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
			?, ?, ?, ?,
			?, ?, ?, ?, ?,
			?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
			?, ?, ?, ?, ?,
			?, ?, ?,
			?, ?, ?, ?, ?, ?, ?,
			?, ?, ?, ?, ?
		)
		ON CONFLICT(id) DO UPDATE SET
			project_id = excluded.project_id,
			status = excluded.status,
			tier = excluded.tier,
			requested_tier = excluded.requested_tier,
			execution_tier = excluded.execution_tier,
			agent = excluded.agent,
			model = excluded.model,
			provider = excluded.provider,
			effort = excluded.effort,
			prompt = excluded.prompt,
			dir = excluded.dir,
			repo = excluded.repo,
			worktree = excluded.worktree,
			branch = excluded.branch,
			base_sha = excluded.base_sha,
			read_only = excluded.read_only,
			no_worktree = excluded.no_worktree,
			agent_run_id = excluded.agent_run_id,
			card_slug = excluded.card_slug,
			output = excluded.output,
			error = excluded.error,
			note = excluded.note,
			exit_code = excluded.exit_code,
			exit_class = excluded.exit_class,
			timeout_sec = excluded.timeout_sec,
			session_id = excluded.session_id,
            conversation_id = excluded.conversation_id,
            caller_agent = excluded.caller_agent,
			resumed_of = excluded.resumed_of,
			parent_run_id = excluded.parent_run_id,
			pid = excluded.pid,
			agent_pid = excluded.agent_pid,
			proc_start = excluded.proc_start,
			profile = excluded.profile,
			prefer_json = excluded.prefer_json,
			avoid_json = excluded.avoid_json,
			requested_json = excluded.requested_json,
			resolved_json = excluded.resolved_json,
			attempts_json = excluded.attempts_json,
			classifier = excluded.classifier,
			difficulty = excluded.difficulty,
			difficulty_probs_json = excluded.difficulty_probs_json,
			files_changed = excluded.files_changed,
			insertions = excluded.insertions,
			deletions = excluded.deletions,
			queued_ms = excluded.queued_ms,
			run_ms = excluded.run_ms,
			usage_json = excluded.usage_json,
			role_json = excluded.role_json,
			created_at = excluded.created_at,
			started_at = excluded.started_at,
			finished_at = excluded.finished_at,
			merged_at = excluded.merged_at,
			merge_sha = excluded.merge_sha`,
		run.ID, run.ProjectID, run.Status, run.Tier, run.RequestedTier, run.ExecutionTier,
		run.Agent, run.Model, run.Provider, run.Effort, run.Prompt, run.Dir, run.Repo, run.Worktree, run.Branch, run.BaseSHA,
		boolToInt(run.ReadOnly), boolToInt(run.NoWorktree), run.AgentRunID, run.CardSlug,
		run.Output, run.Error, run.Note, run.ExitCode, run.ExitClass,
		run.TimeoutSec, run.SessionID, run.ConversationID, run.CallerAgent, run.ResumedOf, run.ParentRunID, run.PID, run.AgentPID, run.ProcStart, run.Profile,
		preferJSON, avoidJSON, requestedJSON, resolvedJSON, attemptsJSON,
		run.Classifier, nullableFloat64(run.Difficulty), difficultyProbsJSON,
		nullableInt(run.FilesChanged), nullableInt(run.Insertions), nullableInt(run.Deletions),
		nullableInt64(run.QueuedMs), nullableInt64(run.RunMs), usageJSON, roleJSON,
		formatDispatchTime(run.CreatedAt), formatDispatchTime(run.StartedAt), formatDispatchTime(run.FinishedAt),
		mergedAt, run.MergeSHA,
	)
	return err
}

func (s *Store) GetRun(ctx context.Context, id string) (dispatch.Run, error) {
	row := s.DB.QueryRowContext(ctx, `SELECT `+selectDispatchRunColumns+` FROM dispatch_runs WHERE id = ?`, id)
	run, err := scanDispatchRun(row.Scan)
	if errors.Is(err, sql.ErrNoRows) {
		return dispatch.Run{}, dispatch.ErrUnknownRun
	}
	if err != nil {
		return dispatch.Run{}, err
	}
	return run, nil
}

// ListRuns lists a workspace's runs, newest first; an empty projectID lists
// every workspace's, as the domain's lifecycle and quota reads expect. The optional
// conversationID filters before pagination.
func (s *Store) ListRuns(ctx context.Context, projectID string, limit, offset int, conversationID ...string) ([]dispatch.Run, error) {
	if offset < 0 {
		offset = 0
	}
	// limit 0 means every row. Callers that want a page pass a positive limit.
	query := `SELECT ` + selectDispatchRunColumns + `
		 FROM dispatch_runs
		 WHERE (? = '' OR project_id = ?) AND (? = '' OR conversation_id = ?)
		 ORDER BY created_at DESC, rowid DESC`
	filter := ""
	if len(conversationID) > 0 {
		filter = conversationID[0]
	}
	args := []any{projectID, projectID, filter, filter}
	if limit > 0 {
		query += ` LIMIT ? OFFSET ?`
		args = append(args, limit, offset)
	}
	rows, err := s.DB.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []dispatch.Run
	for rows.Next() {
		run, err := scanDispatchRun(rows.Scan)
		if err != nil {
			return nil, err
		}
		out = append(out, run)
	}
	return out, rows.Err()
}

func (s *Store) AppendEvent(ctx context.Context, event dispatch.RunEvent) error {
	if event.ID == "" {
		id, err := dispatch.NewID()
		if err != nil {
			return err
		}
		event.ID = id
	}
	if event.At.IsZero() {
		event.At = time.Now().UTC()
	}

	tx, err := s.DB.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()

	if event.ProjectID == "" || event.Agent == "" || event.Tier == "" {
		var projectID, agent, tier string
		if err := tx.QueryRowContext(
			ctx,
			`SELECT project_id, agent, tier FROM dispatch_runs WHERE id = ?`,
			event.RunID,
		).Scan(&projectID, &agent, &tier); err != nil {
			if errors.Is(err, sql.ErrNoRows) {
				return dispatch.ErrUnknownRun
			}
			return err
		}
		if event.ProjectID == "" {
			event.ProjectID = projectID
		}
		if event.Agent == "" {
			event.Agent = agent
		}
		if event.Tier == "" {
			event.Tier = tier
		}
	}

	if event.Seq <= 0 {
		if err := tx.QueryRowContext(
			ctx,
			`SELECT COALESCE(MAX(seq), 0) + 1 FROM dispatch_run_events WHERE run_id = ?`,
			event.RunID,
		).Scan(&event.Seq); err != nil {
			return err
		}
	}

	if _, err := tx.ExecContext(
		ctx,
		`INSERT INTO dispatch_run_events (id, run_id, project_id, kind, agent, tier, text, seq, at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		event.ID, event.RunID, event.ProjectID, event.Kind, event.Agent, event.Tier, event.Text,
		event.Seq, formatDispatchTime(event.At),
	); err != nil {
		return err
	}
	return tx.Commit()
}

func (s *Store) ListEvents(ctx context.Context, runID string, afterSeq int64, limit int) ([]dispatch.RunEvent, error) {
	query := `
		SELECT id, run_id, project_id, kind, agent, tier, text, seq, at
		FROM dispatch_run_events
		WHERE run_id = ? AND seq > ?
		ORDER BY seq ASC`
	args := []any{runID, afterSeq}
	if limit > 0 {
		query += ` LIMIT ?`
		args = append(args, limit)
	}
	rows, err := s.DB.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanDispatchRunEvents(rows)
}

func (s *Store) ListHistory(ctx context.Context, projectID, agent, tier string, limit, offset int) ([]dispatch.RunEvent, error) {
	if limit <= 0 {
		limit = 50
	}
	if offset < 0 {
		offset = 0
	}
	query := `
		SELECT id, run_id, project_id, kind, agent, tier, text, seq, at
		FROM dispatch_run_events
		WHERE kind IN (?, ?, ?)`
	args := []any{
		dispatch.EventStarted,
		dispatch.EventMerged,
		dispatch.EventFailed,
	}
	if projectID != "" {
		query += ` AND project_id = ?`
		args = append(args, projectID)
	}
	if agent != "" {
		query += ` AND agent = ?`
		args = append(args, agent)
	}
	if tier != "" {
		query += ` AND tier = ?`
		args = append(args, tier)
	}
	query += ` ORDER BY at DESC, seq DESC, rowid DESC LIMIT ? OFFSET ?`
	args = append(args, limit, offset)

	rows, err := s.DB.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanDispatchRunEvents(rows)
}

func (s *Store) SaveWorktree(ctx context.Context, info dispatch.WorktreeInfo) error {
	if info.ID == "" {
		id, err := dispatch.NewID()
		if err != nil {
			return err
		}
		info.ID = id
	}
	if info.Status == "" {
		info.Status = dispatch.WorktreeActive
	}
	_, err := s.DB.ExecContext(
		ctx,
		`INSERT INTO dispatch_worktrees (id, project_id, run_id, card_slug, path, branch, base_sha, status, owner)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT(id) DO UPDATE SET
			project_id = excluded.project_id,
			run_id = excluded.run_id,
			card_slug = excluded.card_slug,
			path = excluded.path,
			branch = excluded.branch,
			base_sha = excluded.base_sha,
			status = excluded.status,
			owner = excluded.owner`,
		info.ID, info.ProjectID, info.RunID, info.CardSlug, info.Path, info.Branch, info.BaseSHA, info.Status, info.Owner,
	)
	return err
}

func (s *Store) ListWorktrees(ctx context.Context, projectID string) ([]dispatch.WorktreeInfo, error) {
	rows, err := s.DB.QueryContext(
		ctx,
		`SELECT id, project_id, run_id, card_slug, path, branch, base_sha, status, owner
		 FROM dispatch_worktrees
		 WHERE project_id = ?
		 ORDER BY rowid DESC`,
		projectID,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []dispatch.WorktreeInfo
	for rows.Next() {
		var item dispatch.WorktreeInfo
		if err := rows.Scan(
			&item.ID, &item.ProjectID, &item.RunID, &item.CardSlug,
			&item.Path, &item.Branch, &item.BaseSHA, &item.Status, &item.Owner,
		); err != nil {
			return nil, err
		}
		out = append(out, item)
	}
	return out, rows.Err()
}

func (s *Store) SaveQuota(ctx context.Context, snap dispatch.QuotaSnapshot) error {
	if snap.SampledAt.IsZero() {
		snap.SampledAt = time.Now().UTC()
	}
	_, err := s.DB.ExecContext(
		ctx,
		`INSERT INTO dispatch_quota_snapshots (
			agent, provider, window, source, confidence, used_percent, window_minutes, resets_at, sampled_at
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		snap.Agent, snap.Provider, snap.Window, snap.Source, snap.Confidence,
		nullableFloat64(snap.UsedPercent), snap.WindowMinutes,
		formatDispatchTime(snap.ResetsAt), formatDispatchTime(snap.SampledAt),
	)
	return err
}

func (s *Store) ListQuota(ctx context.Context, agent string, since time.Time) ([]dispatch.QuotaSnapshot, error) {
	query := `
		SELECT agent, provider, window, source, confidence, used_percent, window_minutes, resets_at, sampled_at
		FROM dispatch_quota_snapshots
		WHERE 1 = 1`
	var args []any
	if agent != "" {
		query += ` AND agent = ?`
		args = append(args, agent)
	}
	if !since.IsZero() {
		query += ` AND sampled_at >= ?`
		args = append(args, formatDispatchTime(since))
	}
	query += ` ORDER BY sampled_at DESC, id DESC`

	rows, err := s.DB.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []dispatch.QuotaSnapshot
	for rows.Next() {
		var snap dispatch.QuotaSnapshot
		var used sql.NullFloat64
		var resetsAt, sampledAt string
		if err := rows.Scan(
			&snap.Agent, &snap.Provider, &snap.Window, &snap.Source, &snap.Confidence,
			&used, &snap.WindowMinutes, &resetsAt, &sampledAt,
		); err != nil {
			return nil, err
		}
		if used.Valid {
			v := used.Float64
			snap.UsedPercent = &v
		}
		snap.ResetsAt = parseDispatchTime(resetsAt)
		snap.SampledAt = parseDispatchTime(sampledAt)
		out = append(out, snap)
	}
	return out, rows.Err()
}

func (s *Store) ListTiers(ctx context.Context) ([]dispatch.Tier, error) {
	rows, err := s.DB.QueryContext(
		ctx,
		`SELECT name, provider, agent, model, speed, strength, position
		 FROM dispatch_tiers
		 ORDER BY position ASC, id ASC`,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []dispatch.Tier
	for rows.Next() {
		var tier dispatch.Tier
		if err := rows.Scan(
			&tier.Name, &tier.Provider, &tier.Agent, &tier.Model,
			&tier.Speed, &tier.Strength, &tier.Position,
		); err != nil {
			return nil, err
		}
		out = append(out, tier)
	}
	return out, rows.Err()
}

func (s *Store) ReplaceTiers(ctx context.Context, tiers []dispatch.Tier) error {
	tx, err := s.DB.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()

	if _, err := tx.ExecContext(ctx, `DELETE FROM dispatch_tiers`); err != nil {
		return err
	}
	for _, tier := range tiers {
		if _, err := tx.ExecContext(
			ctx,
			`INSERT INTO dispatch_tiers (name, provider, agent, model, speed, strength, position)
			 VALUES (?, ?, ?, ?, ?, ?, ?)`,
			tier.Name, tier.Provider, tier.Agent, tier.Model, tier.Speed, tier.Strength, tier.Position,
		); err != nil {
			return err
		}
	}
	return tx.Commit()
}

func (s *Store) UpsertAgent(ctx context.Context, agent dispatch.Agent) error {
	var cooldown any
	if agent.CooldownUntil != nil && !agent.CooldownUntil.IsZero() {
		cooldown = formatDispatchTime(*agent.CooldownUntil)
	}
	_, err := s.DB.ExecContext(
		ctx,
		`INSERT INTO dispatch_agents (
			name, bin, provider, used_percent, cooldown_until
		) VALUES (?, ?, ?, ?, ?)
		ON CONFLICT(name) DO UPDATE SET
			bin = excluded.bin,
			provider = excluded.provider,
			used_percent = excluded.used_percent,
			cooldown_until = COALESCE(excluded.cooldown_until, dispatch_agents.cooldown_until)`,
		agent.Name, agent.Bin, agent.Provider, nullableFloat64(agent.UsedPercent), cooldown,
	)
	return err
}

func (s *Store) ListAgents(ctx context.Context) ([]dispatch.Agent, error) {
	rows, err := s.DB.QueryContext(
		ctx,
		`SELECT name, bin, provider, used_percent, cooldown_until
		 FROM dispatch_agents
		 ORDER BY name ASC`,
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var out []dispatch.Agent
	for rows.Next() {
		var agent dispatch.Agent
		var used sql.NullFloat64
		var cooldown sql.NullString
		if err := rows.Scan(
			&agent.Name, &agent.Bin, &agent.Provider, &used, &cooldown,
		); err != nil {
			return nil, err
		}
		if used.Valid {
			v := used.Float64
			agent.UsedPercent = &v
		}
		if cooldown.Valid && cooldown.String != "" {
			t := parseDispatchTime(cooldown.String)
			if !t.IsZero() {
				agent.CooldownUntil = &t
			}
		}
		out = append(out, agent)
	}
	return out, rows.Err()
}

func (s *Store) SetCooldown(ctx context.Context, agent string, until time.Time) error {
	_, err := s.DB.ExecContext(
		ctx,
		`INSERT INTO dispatch_agents (name, cooldown_until)
		 VALUES (?, ?)
		 ON CONFLICT(name) DO UPDATE SET cooldown_until = excluded.cooldown_until`,
		agent, formatDispatchTime(until),
	)
	return err
}

func (s *Store) ClearCooldown(ctx context.Context, agent string) error {
	_, err := s.DB.ExecContext(
		ctx,
		`UPDATE dispatch_agents SET cooldown_until = NULL WHERE name = ?`,
		agent,
	)
	return err
}

func scanDispatchRun(scan func(dest ...any) error) (dispatch.Run, error) {
	var run dispatch.Run
	var readOnly, noWorktree int
	var preferJSON, avoidJSON, requestedJSON, resolvedJSON, attemptsJSON, difficultyProbsJSON, usageJSON, roleJSON string
	var difficulty sql.NullFloat64
	var filesChanged, insertions, deletions, queuedMs, runMs sql.NullInt64
	var createdAt, startedAt, finishedAt string
	var mergedAt sql.NullString

	if err := scan(
		&run.ID, &run.ProjectID, &run.Status, &run.Tier, &run.RequestedTier, &run.ExecutionTier,
		&run.Agent, &run.Model, &run.Provider, &run.Effort, &run.Prompt,
		&run.Dir, &run.Repo, &run.Worktree, &run.Branch, &run.BaseSHA,
		&readOnly, &noWorktree, &run.AgentRunID, &run.CardSlug,
		&run.Output, &run.Error, &run.Note, &run.ExitCode, &run.ExitClass,
		&run.TimeoutSec, &run.SessionID, &run.ConversationID, &run.CallerAgent, &run.ResumedOf, &run.ParentRunID,
		&run.PID, &run.AgentPID, &run.ProcStart, &run.Profile,
		&preferJSON, &avoidJSON, &requestedJSON, &resolvedJSON, &attemptsJSON,
		&run.Classifier, &difficulty, &difficultyProbsJSON,
		&filesChanged, &insertions, &deletions, &queuedMs, &runMs, &usageJSON, &roleJSON,
		&createdAt, &startedAt, &finishedAt, &mergedAt, &run.MergeSHA, &run.WorktreeStatus,
	); err != nil {
		return dispatch.Run{}, err
	}

	run.ReadOnly = readOnly != 0
	run.NoWorktree = noWorktree != 0

	if preferJSON != "" {
		_ = json.Unmarshal([]byte(preferJSON), &run.Prefer)
	}
	if avoidJSON != "" {
		_ = json.Unmarshal([]byte(avoidJSON), &run.Avoid)
	}
	if requestedJSON != "" && requestedJSON != "null" {
		var f dispatch.Filters
		if err := json.Unmarshal([]byte(requestedJSON), &f); err == nil {
			run.Requested = &f
		}
	}
	if resolvedJSON != "" && resolvedJSON != "null" {
		var f dispatch.Filters
		if err := json.Unmarshal([]byte(resolvedJSON), &f); err == nil {
			run.Resolved = &f
		}
	}
	if attemptsJSON != "" {
		_ = json.Unmarshal([]byte(attemptsJSON), &run.Attempts)
	}
	if difficultyProbsJSON != "" {
		_ = json.Unmarshal([]byte(difficultyProbsJSON), &run.DifficultyProbs)
	}
	if difficulty.Valid {
		v := difficulty.Float64
		run.Difficulty = &v
	}
	if usageJSON != "" {
		var u dispatch.Usage
		if err := json.Unmarshal([]byte(usageJSON), &u); err == nil {
			run.Usage = &u
		}
	}
	if roleJSON != "" {
		var pick dispatch.RolePick
		if err := json.Unmarshal([]byte(roleJSON), &pick); err == nil {
			run.RolePick = &pick
		}
	}

	if filesChanged.Valid {
		v := int(filesChanged.Int64)
		run.FilesChanged = &v
	}
	if insertions.Valid {
		v := int(insertions.Int64)
		run.Insertions = &v
	}
	if deletions.Valid {
		v := int(deletions.Int64)
		run.Deletions = &v
	}
	if queuedMs.Valid {
		v := queuedMs.Int64
		run.QueuedMs = &v
	}
	if runMs.Valid {
		v := runMs.Int64
		run.RunMs = &v
	}

	run.CreatedAt = parseDispatchTime(createdAt)
	run.StartedAt = parseDispatchTime(startedAt)
	run.FinishedAt = parseDispatchTime(finishedAt)
	if mergedAt.Valid && mergedAt.String != "" {
		t := parseDispatchTime(mergedAt.String)
		if !t.IsZero() {
			run.MergedAt = &t
		}
	}
	return run, nil
}

func scanDispatchRunEvents(rows *sql.Rows) ([]dispatch.RunEvent, error) {
	var out []dispatch.RunEvent
	for rows.Next() {
		var ev dispatch.RunEvent
		var at string
		if err := rows.Scan(
			&ev.ID, &ev.RunID, &ev.ProjectID, &ev.Kind,
			&ev.Agent, &ev.Tier, &ev.Text, &ev.Seq, &at,
		); err != nil {
			return nil, err
		}
		ev.At = parseDispatchTime(at)
		out = append(out, ev)
	}
	return out, rows.Err()
}

func formatDispatchTime(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return t.UTC().Format(time.RFC3339)
}

func parseDispatchTime(value string) time.Time {
	if value == "" {
		return time.Time{}
	}
	if t, err := time.Parse(time.RFC3339, value); err == nil {
		return t.UTC()
	}
	t, _ := time.Parse(time.RFC3339Nano, value)
	return t.UTC()
}

func boolToInt(v bool) int {
	if v {
		return 1
	}
	return 0
}

func nullableInt(v *int) any {
	if v == nil {
		return nil
	}
	return *v
}

func nullableInt64(v *int64) any {
	if v == nil {
		return nil
	}
	return *v
}

func nullableFloat64(v *float64) any {
	if v == nil {
		return nil
	}
	return *v
}

func marshalSliceJSON[T any](items []T) (string, error) {
	if len(items) == 0 {
		return "[]", nil
	}
	b, err := json.Marshal(items)
	if err != nil {
		return "", err
	}
	return string(b), nil
}

func marshalOptionalJSON[T any](v *T) (string, error) {
	if v == nil {
		return "", nil
	}
	b, err := json.Marshal(v)
	if err != nil {
		return "", err
	}
	return string(b), nil
}

func (s *Store) SaveRouteDecision(ctx context.Context, dec dispatch.RouteDecision) error {
	if dec.CreatedAt.IsZero() {
		dec.CreatedAt = time.Now().UTC()
	}
	probs, err := marshalSliceJSON(dec.Probs)
	if err != nil {
		return err
	}
	candidates, err := marshalSliceJSON(dec.Candidates)
	if err != nil {
		return err
	}
	area, err := json.Marshal(dec.Area)
	if err != nil {
		return err
	}
	_, err = s.DB.ExecContext(
		ctx,
		`INSERT INTO dispatch_route_decisions (
			run_id, project_id, policy_version, route, priority, classifier,
			probs_json, difficulty, candidates_json, chosen, created_at, area_json
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
		ON CONFLICT(run_id) DO UPDATE SET
			policy_version = excluded.policy_version,
			route = excluded.route,
			priority = excluded.priority,
			classifier = excluded.classifier,
			probs_json = excluded.probs_json,
			difficulty = excluded.difficulty,
			candidates_json = excluded.candidates_json,
			chosen = excluded.chosen,
			area_json = excluded.area_json`,
		dec.RunID, dec.ProjectID, dec.PolicyVersion, dec.Route, dec.Priority, dec.Classifier,
		probs, nullableFloat64(dec.Difficulty), candidates, dec.Chosen,
		formatDispatchTime(dec.CreatedAt), string(area),
	)
	return err
}

const routeDecisionColumns = `run_id, project_id, policy_version, route, priority, classifier,
			probs_json, difficulty, candidates_json, chosen, created_at, area_json`

func scanRouteDecision(scan func(...any) error) (dispatch.RouteDecision, error) {
	var dec dispatch.RouteDecision
	var probs, candidates, createdAt, area string
	var difficulty sql.NullFloat64
	if err := scan(
		&dec.RunID, &dec.ProjectID, &dec.PolicyVersion, &dec.Route, &dec.Priority, &dec.Classifier,
		&probs, &difficulty, &candidates, &dec.Chosen, &createdAt, &area,
	); err != nil {
		return dec, err
	}
	if err := json.Unmarshal([]byte(probs), &dec.Probs); err != nil {
		return dec, err
	}
	if err := json.Unmarshal([]byte(candidates), &dec.Candidates); err != nil {
		return dec, err
	}
	if area != "" {
		if err := json.Unmarshal([]byte(area), &dec.Area); err != nil {
			return dec, err
		}
	}
	if difficulty.Valid {
		v := difficulty.Float64
		dec.Difficulty = &v
	}
	dec.CreatedAt = parseDispatchTime(createdAt)
	return dec, nil
}

func (s *Store) GetRouteDecision(ctx context.Context, runID string) (dispatch.RouteDecision, bool, error) {
	row := s.DB.QueryRowContext(ctx, `SELECT `+routeDecisionColumns+` FROM dispatch_route_decisions WHERE run_id = ?`, runID)
	dec, err := scanRouteDecision(row.Scan)
	if errors.Is(err, sql.ErrNoRows) {
		return dispatch.RouteDecision{}, false, nil
	}
	if err != nil {
		return dispatch.RouteDecision{}, false, err
	}
	return dec, true, nil
}

func (s *Store) ListRouteDecisions(ctx context.Context, projectID string) ([]dispatch.RouteDecision, error) {
	query := `SELECT ` + routeDecisionColumns + ` FROM dispatch_route_decisions`
	var args []any
	if projectID != "" {
		query += ` WHERE project_id = ?`
		args = append(args, projectID)
	}
	query += ` ORDER BY created_at DESC, run_id DESC`
	rows, err := s.DB.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []dispatch.RouteDecision
	for rows.Next() {
		dec, err := scanRouteDecision(rows.Scan)
		if err != nil {
			return nil, err
		}
		out = append(out, dec)
	}
	return out, rows.Err()
}
