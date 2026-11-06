use super::{Store, db, format_time, now, parse_time};
use crate::{
    Error,
    types::{
        self, Agent, Attempt, DecisionCandidate, DisabledModel, DispatchError, Filters,
        QuotaSnapshot, RolePick, RouteDecision, Run, RunEvent, Tier, Usage, WorktreeInfo, new_id,
    },
};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter, types::Value};
use serde::{Deserialize, Serialize};

const SELECT_RUN_COLUMNS: &str = "
	id, project_id, status, tier, requested_tier, execution_tier,
	agent, model, provider, effort, prompt, dir, repo, worktree, branch, base_sha,
	read_only, no_worktree, agent_run_id, card_slug,
	output, error, note, exit_code, exit_class,
	timeout_sec, session_id, conversation_id, caller_agent, resumed_of, parent_run_id, pid, agent_pid, proc_start, profile,
	prefer_json, avoid_json, requested_json, resolved_json, attempts_json,
	classifier, difficulty, difficulty_probs_json,
	files_changed, insertions, deletions, queued_ms, run_ms, usage_json, role_json,
	created_at, started_at, finished_at, merged_at, merge_sha,
	COALESCE((SELECT status FROM dispatch_worktrees WHERE run_id = dispatch_runs.id ORDER BY rowid DESC LIMIT 1), ''),
	linked_json";

/// [`SELECT_RUN_COLUMNS`]' shape over `dispatch_run_history`, which keeps no prompt, output,
/// or worktree: those read empty.
const SELECT_HISTORY_COLUMNS: &str = "
	id, project_id, status, tier, requested_tier, execution_tier,
	agent, model, provider, effort, '', dir, repo, '', '', '',
	read_only, 0, '', '',
	'', error, '', exit_code, exit_class,
	0, '', conversation_id, '', '', '', 0, 0, '', profile,
	'', '', COALESCE(requested_json, ''), COALESCE(resolved_json, ''), COALESCE(attempts_json, ''),
	classifier, difficulty, '',
	NULL, NULL, NULL, queued_ms, run_ms, COALESCE(usage_json, ''), '',
	created_at, COALESCE(started_at, ''), COALESCE(finished_at, ''), NULL, '',
	'', ''";

const ROUTE_DECISION_COLUMNS: &str =
    "run_id, project_id, policy_version, route, priority, classifier,
			probs_json, difficulty, candidates_json, chosen, created_at, area_json";

/// `Filters` as Go encodes it: no tags, so Go field names in declaration order.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct FiltersJson {
    #[serde(rename = "Provider")]
    provider: String,
    #[serde(rename = "Model")]
    model: String,
    #[serde(rename = "Effort")]
    effort: String,
    #[serde(rename = "Agent")]
    agent: String,
    #[serde(rename = "Priority")]
    priority: String,
    #[serde(rename = "Relax")]
    relax: bool,
}

impl From<&Filters> for FiltersJson {
    fn from(f: &Filters) -> Self {
        FiltersJson {
            provider: f.provider.clone(),
            model: f.model.clone(),
            effort: f.effort.clone(),
            agent: f.agent.clone(),
            priority: f.priority.clone(),
            relax: f.relax,
        }
    }
}

impl From<FiltersJson> for Filters {
    fn from(f: FiltersJson) -> Self {
        Filters {
            provider: f.provider,
            model: f.model,
            effort: f.effort,
            agent: f.agent,
            priority: f.priority,
            relax: f.relax,
        }
    }
}

/// `DecisionCandidate` read back leniently: Go omits empty fields when writing.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CandidateJson {
    agent: String,
    model: String,
    effort: String,
    p_pass: Option<f64>,
    p_pass_low: Option<f64>,
    price: Option<f64>,
    quota_multiplier: f64,
    effective_cost: Option<f64>,
    theta: Option<f64>,
    theta_source: String,
    qualified: bool,
    score: f64,
}

impl From<CandidateJson> for DecisionCandidate {
    fn from(c: CandidateJson) -> Self {
        DecisionCandidate {
            agent: c.agent,
            model: c.model,
            effort: c.effort,
            p_pass: c.p_pass,
            p_pass_low: c.p_pass_low,
            price: c.price,
            quota_multiplier: c.quota_multiplier,
            effective_cost: c.effective_cost,
            theta: c.theta,
            theta_source: c.theta_source,
            qualified: c.qualified,
            score: c.score,
        }
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn slice_json<T: Serialize>(items: &[T]) -> Result<String, Error> {
    if items.is_empty() {
        return Ok("[]".into());
    }
    Ok(serde_json::to_string(items)?)
}

fn optional_json<T: Serialize>(v: Option<&T>) -> Result<String, Error> {
    match v {
        None => Ok(String::new()),
        Some(v) => Ok(serde_json::to_string(v)?),
    }
}

fn filters_json(f: &Option<Filters>) -> Result<String, Error> {
    optional_json(f.as_ref().map(FiltersJson::from).as_ref())
}

fn scan_run(r: &Row<'_>) -> rusqlite::Result<Run> {
    let mut i = 0;
    let mut next = || {
        i += 1;
        i - 1
    };
    let mut run = Run {
        id: r.get(next())?,
        project_id: r.get(next())?,
        status: r.get(next())?,
        tier: r.get(next())?,
        requested_tier: r.get(next())?,
        execution_tier: r.get(next())?,
        agent: r.get(next())?,
        model: r.get(next())?,
        provider: r.get(next())?,
        effort: r.get(next())?,
        prompt: r.get(next())?,
        dir: r.get(next())?,
        repo: r.get(next())?,
        worktree: r.get(next())?,
        branch: r.get(next())?,
        base_sha: r.get(next())?,
        read_only: r.get::<_, i64>(next())? != 0,
        no_worktree: r.get::<_, i64>(next())? != 0,
        agent_run_id: r.get(next())?,
        card_slug: r.get(next())?,
        output: r.get(next())?,
        error: r.get(next())?,
        note: r.get(next())?,
        exit_code: r.get(next())?,
        exit_class: r.get(next())?,
        timeout_sec: r.get(next())?,
        session_id: r.get(next())?,
        conversation_id: r.get(next())?,
        caller_agent: r.get(next())?,
        resumed_of: r.get(next())?,
        parent_run_id: r.get(next())?,
        pid: r.get(next())?,
        agent_pid: r.get(next())?,
        proc_start: r.get(next())?,
        profile: r.get(next())?,
        ..Default::default()
    };
    let prefer: String = r.get(next())?;
    let avoid: String = r.get(next())?;
    let requested: String = r.get(next())?;
    let resolved: String = r.get(next())?;
    let attempts: String = r.get(next())?;
    run.classifier = r.get(next())?;
    run.difficulty = r.get(next())?;
    let probs: String = r.get(next())?;
    run.files_changed = r.get(next())?;
    run.insertions = r.get(next())?;
    run.deletions = r.get(next())?;
    run.queued_ms = r.get(next())?;
    run.run_ms = r.get(next())?;
    let usage: String = r.get(next())?;
    let role: String = r.get(next())?;
    let created: String = r.get(next())?;
    let started: String = r.get(next())?;
    let finished: String = r.get(next())?;
    let merged: Option<String> = r.get(next())?;
    run.merge_sha = r.get(next())?;
    run.worktree_status = r.get(next())?;
    let linked: String = r.get(next())?;
    if !linked.is_empty() {
        run.linked = serde_json::from_str(&linked).unwrap_or_default();
    }

    if !prefer.is_empty() {
        run.prefer = serde_json::from_str(&prefer).unwrap_or_default();
    }
    if !avoid.is_empty() {
        run.avoid = serde_json::from_str(&avoid).unwrap_or_default();
    }
    let filters = |s: &str| {
        if s.is_empty() || s == "null" {
            return None;
        }
        serde_json::from_str::<FiltersJson>(s)
            .ok()
            .map(Filters::from)
    };
    run.requested = filters(&requested);
    run.resolved = filters(&resolved);
    if !attempts.is_empty() {
        run.attempts = serde_json::from_str::<Vec<Attempt>>(&attempts).unwrap_or_default();
    }
    if !probs.is_empty() {
        run.difficulty_probs = serde_json::from_str(&probs).unwrap_or_default();
    }
    if !usage.is_empty() {
        run.usage = serde_json::from_str::<Usage>(&usage).ok();
    }
    if !role.is_empty() {
        run.role_pick = serde_json::from_str::<RolePick>(&role).ok();
    }
    run.created_at = parse_time(&created);
    run.started_at = parse_time(&started);
    run.finished_at = parse_time(&finished);
    run.merged_at = merged
        .filter(|m| !m.is_empty())
        .map(|m| parse_time(&m))
        .filter(|m| !m.is_empty());
    Ok(run)
}

fn scan_event(r: &Row<'_>) -> rusqlite::Result<RunEvent> {
    Ok(RunEvent {
        id: r.get(0)?,
        run_id: r.get(1)?,
        project_id: r.get(2)?,
        kind: r.get(3)?,
        agent: r.get(4)?,
        tier: r.get(5)?,
        text: r.get(6)?,
        seq: r.get(7)?,
        at: parse_time(&r.get::<_, String>(8)?),
    })
}

fn scan_decision(r: &Row<'_>) -> Result<RouteDecision, Error> {
    let probs: String = r.get(6).map_err(db)?;
    let candidates: String = r.get(8).map_err(db)?;
    let area: String = r.get(11).map_err(db)?;
    let candidates: Vec<CandidateJson> = serde_json::from_str(&candidates)?;
    Ok(RouteDecision {
        run_id: r.get(0).map_err(db)?,
        project_id: r.get(1).map_err(db)?,
        policy_version: r.get(2).map_err(db)?,
        route: r.get(3).map_err(db)?,
        priority: r.get(4).map_err(db)?,
        classifier: r.get(5).map_err(db)?,
        probs: serde_json::from_str(&probs)?,
        difficulty: r.get(7).map_err(db)?,
        candidates: candidates.into_iter().map(Into::into).collect(),
        chosen: r.get(9).map_err(db)?,
        created_at: parse_time(&r.get::<_, String>(10).map_err(db)?),
        area: if area.is_empty() {
            Default::default()
        } else {
            serde_json::from_str(&area)?
        },
    })
}

pub(super) fn list_tiers_on(conn: &Connection) -> Result<Vec<Tier>, Error> {
    let mut stmt = conn
        .prepare(
            "SELECT name, provider, agent, model, position
			 FROM dispatch_tiers
			 ORDER BY position ASC, id ASC",
        )
        .map_err(db)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Tier {
                name: r.get(0)?,
                provider: r.get(1)?,
                agent: r.get(2)?,
                model: r.get(3)?,
                position: r.get(4)?,
            })
        })
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

impl Store {
    /// The ids of up to `limit` runs whose id starts with `prefix`.
    pub fn run_ids_with_prefix(&self, prefix: &str, limit: i64) -> Result<Vec<String>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT id FROM dispatch_runs WHERE substr(id, 1, length(?1)) = ?1 LIMIT ?2")
            .map_err(db)?;
        let rows = stmt
            .query_map(rusqlite::params![prefix, limit], |row| {
                row.get::<_, String>(0)
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    /// Drops an agent from the agents list, for an added agent the user removed.
    pub fn delete_agent(&self, name: &str) -> Result<(), Error> {
        self.conn()
            .execute("DELETE FROM dispatch_agents WHERE name = ?1", [name])
            .map_err(db)?;
        Ok(())
    }
}

impl types::Store for Store {
    fn save_run(&self, run: &Run) -> Result<(), Error> {
        let id = if run.id.is_empty() {
            new_id()
        } else {
            run.id.clone()
        };
        let status = if run.status.is_empty() {
            types::STATUS_PENDING
        } else {
            &run.status
        };
        let created = if run.created_at.is_empty() {
            now()
        } else {
            run.created_at.clone()
        };
        let prefer = slice_json(&run.prefer)?;
        let avoid = slice_json(&run.avoid)?;
        let requested = filters_json(&run.requested)?;
        let resolved = filters_json(&run.resolved)?;
        let attempts = slice_json(&run.attempts)?;
        let probs = slice_json(&run.difficulty_probs)?;
        let usage = optional_json(run.usage.as_ref())?;
        let role = optional_json(run.role_pick.as_ref())?;
        let linked = if run.linked.is_empty() {
            String::new()
        } else {
            slice_json(&run.linked)?
        };
        let merged = run
            .merged_at
            .as_deref()
            .map(format_time)
            .filter(|m| !m.is_empty());

        self.conn()
            .execute(
                "
		INSERT INTO dispatch_runs (
			id, project_id, status, tier, requested_tier, execution_tier,
			agent, model, provider, effort, prompt, dir, repo, worktree, branch, base_sha,
			read_only, no_worktree, agent_run_id, card_slug,
			output, error, note, exit_code, exit_class,
			timeout_sec, session_id, conversation_id, caller_agent, resumed_of, parent_run_id, pid, agent_pid, proc_start, profile,
			prefer_json, avoid_json, requested_json, resolved_json, attempts_json,
			classifier, difficulty, difficulty_probs_json,
			files_changed, insertions, deletions, queued_ms, run_ms, usage_json, role_json,
			created_at, started_at, finished_at, merged_at, merge_sha, linked_json
		) VALUES (
			?, ?, ?, ?, ?, ?,
			?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
			?, ?, ?, ?,
			?, ?, ?, ?, ?,
			?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
			?, ?, ?, ?, ?,
			?, ?, ?,
			?, ?, ?, ?, ?, ?, ?,
			?, ?, ?, ?, ?, ?
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
			merge_sha = excluded.merge_sha,
			linked_json = excluded.linked_json",
                params![
                    id,
                    run.project_id,
                    status,
                    run.tier,
                    run.requested_tier,
                    run.execution_tier,
                    run.agent,
                    run.model,
                    run.provider,
                    run.effort,
                    run.prompt,
                    run.dir,
                    run.repo,
                    run.worktree,
                    run.branch,
                    run.base_sha,
                    run.read_only as i64,
                    run.no_worktree as i64,
                    run.agent_run_id,
                    run.card_slug,
                    run.output,
                    run.error,
                    run.note,
                    run.exit_code,
                    run.exit_class,
                    run.timeout_sec,
                    run.session_id,
                    run.conversation_id,
                    run.caller_agent,
                    run.resumed_of,
                    run.parent_run_id,
                    run.pid,
                    run.agent_pid,
                    run.proc_start,
                    run.profile,
                    prefer,
                    avoid,
                    requested,
                    resolved,
                    attempts,
                    run.classifier,
                    run.difficulty,
                    probs,
                    run.files_changed,
                    run.insertions,
                    run.deletions,
                    run.queued_ms,
                    run.run_ms,
                    usage,
                    role,
                    format_time(&created),
                    format_time(&run.started_at),
                    format_time(&run.finished_at),
                    merged,
                    run.merge_sha,
                    linked,
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn get_run(&self, id: &str) -> Result<Run, Error> {
        let conn = self.conn();
        let run = conn
            .query_row(
                &format!("SELECT {SELECT_RUN_COLUMNS} FROM dispatch_runs WHERE id = ?"),
                [id],
                scan_run,
            )
            .optional()
            .map_err(db)?;
        run.ok_or_else(|| DispatchError::UnknownRun.into())
    }

    fn list_runs(
        &self,
        project_id: &str,
        limit: i64,
        offset: i64,
        conversation_id: &str,
    ) -> Result<Vec<Run>, Error> {
        let offset = offset.max(0);
        let mut query = format!(
            "SELECT {SELECT_RUN_COLUMNS}
		 FROM dispatch_runs
		 WHERE (? = '' OR project_id = ?) AND (? = '' OR conversation_id = ?)
		 ORDER BY created_at DESC, rowid DESC"
        );
        let mut args: Vec<Value> = vec![
            text(project_id),
            text(project_id),
            text(conversation_id),
            text(conversation_id),
        ];
        if limit > 0 {
            query.push_str(" LIMIT ? OFFSET ?");
            args.push(limit.into());
            args.push(offset.into());
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(&query).map_err(db)?;
        let rows = stmt
            .query_map(params_from_iter(args), scan_run)
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn list_run_history(&self, project_id: &str, dir: &str) -> Result<Vec<Run>, Error> {
        let dir = dir.trim_end_matches('/');
        let query = format!(
            "SELECT {SELECT_HISTORY_COLUMNS}
		 FROM dispatch_run_history
		 WHERE (?1 = '' OR project_id = ?1)
		   AND (?2 = '' OR dir = ?2 OR dir LIKE ?2 || '/%' OR repo = ?2)
		 ORDER BY created_at DESC, rowid DESC"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&query).map_err(db)?;
        let rows = stmt
            .query_map(params![project_id, dir], scan_run)
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn append_event(&self, event: &RunEvent) -> Result<(), Error> {
        let mut event = event.clone();
        if event.id.is_empty() {
            event.id = new_id();
        }
        if event.at.is_empty() {
            event.at = now();
        }
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(db)?;
        if event.project_id.is_empty() || event.agent.is_empty() || event.tier.is_empty() {
            let (project_id, agent, tier): (String, String, String) = tx
                .query_row(
                    "SELECT project_id, agent, tier FROM dispatch_runs WHERE id = ?",
                    [&event.run_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(db)?
                .ok_or(DispatchError::UnknownRun)?;
            if event.project_id.is_empty() {
                event.project_id = project_id;
            }
            if event.agent.is_empty() {
                event.agent = agent;
            }
            if event.tier.is_empty() {
                event.tier = tier;
            }
        }
        if event.seq <= 0 {
            event.seq = tx
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM dispatch_run_events WHERE run_id = ?",
                    [&event.run_id],
                    |r| r.get(0),
                )
                .map_err(db)?;
        }
        tx.execute(
            "INSERT INTO dispatch_run_events (id, run_id, project_id, kind, agent, tier, text, seq, at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                event.id,
                event.run_id,
                event.project_id,
                event.kind,
                event.agent,
                event.tier,
                event.text,
                event.seq,
                format_time(&event.at),
            ],
        )
        .map_err(db)?;
        tx.commit().map_err(db)
    }

    fn list_events(
        &self,
        run_id: &str,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<RunEvent>, Error> {
        let mut query = String::from(
            "
		SELECT id, run_id, project_id, kind, agent, tier, text, seq, at
		FROM dispatch_run_events
		WHERE run_id = ? AND seq > ?
		ORDER BY seq ASC",
        );
        let mut args: Vec<Value> = vec![text(run_id), after_seq.into()];
        if limit > 0 {
            query.push_str(" LIMIT ?");
            args.push(limit.into());
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(&query).map_err(db)?;
        let rows = stmt
            .query_map(params_from_iter(args), scan_event)
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn event_sizes(&self, run_id: &str) -> Result<Vec<types::EventSize>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "
		SELECT seq, kind, length(CAST(text AS BLOB))
		FROM dispatch_run_events
		WHERE run_id = ?
		ORDER BY seq ASC",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([run_id], |r| {
                Ok(types::EventSize {
                    seq: r.get(0)?,
                    kind: r.get(1)?,
                    bytes: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    /// Reads only the slice's bytes, and one past it, so a page of a huge event never loads
    /// the event whole.
    fn event_slice(
        &self,
        run_id: &str,
        seq: i64,
        start: u64,
        end: u64,
    ) -> Result<Option<(RunEvent, u64)>, Error> {
        let from = i64::try_from(start).unwrap_or(i64::MAX);
        let wanted = i64::try_from(end.saturating_sub(start)).unwrap_or(i64::MAX);
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "
		SELECT id, run_id, project_id, kind, agent, tier,
		       substr(CAST(text AS BLOB), ? + 1, ? + 1), seq, at
		FROM dispatch_run_events
		WHERE run_id = ? AND seq = ?",
            )
            .map_err(db)?;
        let mut rows = stmt
            .query_map(params![from, wanted, run_id, seq], |r| {
                let window: Vec<u8> = r.get(6)?;
                Ok((
                    RunEvent {
                        id: r.get(0)?,
                        run_id: r.get(1)?,
                        project_id: r.get(2)?,
                        kind: r.get(3)?,
                        agent: r.get(4)?,
                        tier: r.get(5)?,
                        text: String::new(),
                        seq: r.get(7)?,
                        at: parse_time(&r.get::<_, String>(8)?),
                    },
                    window,
                ))
            })
            .map_err(db)?;
        let Some(row) = rows.next() else {
            return Ok(None);
        };
        let (mut event, window) = row.map_err(db)?;
        let (text, moved) = types::cut_utf8(&window, usize::try_from(wanted).unwrap_or(usize::MAX));
        event.text = text;
        Ok(Some((event, start + moved as u64)))
    }

    fn list_history(
        &self,
        project_id: &str,
        agent: &str,
        tier: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<RunEvent>, Error> {
        let limit = if limit <= 0 { 50 } else { limit };
        let offset = offset.max(0);
        let mut query = String::from(
            "
		SELECT id, run_id, project_id, kind, agent, tier, text, seq, at
		FROM dispatch_run_events
		WHERE kind IN (?, ?, ?)",
        );
        let mut args: Vec<Value> = vec![
            text(types::EVENT_STARTED),
            text(types::EVENT_MERGED),
            text(types::EVENT_FAILED),
        ];
        for (column, value) in [("project_id", project_id), ("agent", agent), ("tier", tier)] {
            if !value.is_empty() {
                query.push_str(&format!(" AND {column} = ?"));
                args.push(text(value));
            }
        }
        query.push_str(" ORDER BY at DESC, seq DESC, rowid DESC LIMIT ? OFFSET ?");
        args.push(limit.into());
        args.push(offset.into());
        let conn = self.conn();
        let mut stmt = conn.prepare(&query).map_err(db)?;
        let rows = stmt
            .query_map(params_from_iter(args), scan_event)
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn save_worktree(&self, info: &WorktreeInfo) -> Result<(), Error> {
        let id = if info.id.is_empty() {
            new_id()
        } else {
            info.id.clone()
        };
        let status = if info.status.is_empty() {
            types::WORKTREE_ACTIVE
        } else {
            &info.status
        };
        self.conn()
            .execute(
                "INSERT INTO dispatch_worktrees (id, project_id, run_id, card_slug, path, branch, base_sha, status, owner)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
		 ON CONFLICT(id) DO UPDATE SET
			project_id = excluded.project_id,
			run_id = excluded.run_id,
			card_slug = excluded.card_slug,
			path = excluded.path,
			branch = excluded.branch,
			base_sha = excluded.base_sha,
			status = excluded.status,
			owner = excluded.owner",
                params![
                    id,
                    info.project_id,
                    info.run_id,
                    info.card_slug,
                    info.path,
                    info.branch,
                    info.base_sha,
                    status,
                    info.owner
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn list_worktrees(&self, project_id: &str) -> Result<Vec<WorktreeInfo>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, run_id, card_slug, path, branch, base_sha, status, owner
		 FROM dispatch_worktrees
		 WHERE project_id = ?
		 ORDER BY rowid DESC",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([project_id], |r| {
                Ok(WorktreeInfo {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    run_id: r.get(2)?,
                    card_slug: r.get(3)?,
                    path: r.get(4)?,
                    branch: r.get(5)?,
                    base_sha: r.get(6)?,
                    status: r.get(7)?,
                    owner: r.get(8)?,
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn save_quota(&self, snap: &QuotaSnapshot) -> Result<(), Error> {
        let sampled = if snap.sampled_at.is_empty() {
            now()
        } else {
            snap.sampled_at.clone()
        };
        self.conn()
            .execute(
                "INSERT INTO dispatch_quota_snapshots (
			agent, provider, window, source, confidence, used_percent, window_minutes, resets_at, sampled_at
		) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    snap.agent,
                    snap.provider,
                    snap.window,
                    snap.source,
                    snap.confidence,
                    snap.used_percent,
                    snap.window_minutes,
                    format_time(&snap.resets_at),
                    format_time(&sampled),
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn list_quota(&self, agent: &str, since: &str) -> Result<Vec<QuotaSnapshot>, Error> {
        let mut query = String::from(
            "
		SELECT agent, provider, window, source, confidence, used_percent, window_minutes, resets_at, sampled_at
		FROM dispatch_quota_snapshots
		WHERE 1 = 1",
        );
        let mut args: Vec<Value> = Vec::new();
        if !agent.is_empty() {
            query.push_str(" AND agent = ?");
            args.push(text(agent));
        }
        if !since.is_empty() {
            query.push_str(" AND sampled_at >= ?");
            args.push(Value::Text(format_time(since)));
        }
        query.push_str(" ORDER BY sampled_at DESC, id DESC");
        let conn = self.conn();
        let mut stmt = conn.prepare(&query).map_err(db)?;
        let rows = stmt
            .query_map(params_from_iter(args), |r| {
                Ok(QuotaSnapshot {
                    agent: r.get(0)?,
                    provider: r.get(1)?,
                    window: r.get(2)?,
                    source: r.get(3)?,
                    confidence: r.get(4)?,
                    used_percent: r.get(5)?,
                    window_minutes: r.get(6)?,
                    resets_at: parse_time(&r.get::<_, String>(7)?),
                    sampled_at: parse_time(&r.get::<_, String>(8)?),
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn list_tiers(&self) -> Result<Vec<Tier>, Error> {
        list_tiers_on(&self.conn())
    }

    fn replace_tiers(&self, tiers: &[Tier]) -> Result<(), Error> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(db)?;
        tx.execute("DELETE FROM dispatch_tiers", []).map_err(db)?;
        for t in tiers {
            tx.execute(
                "INSERT INTO dispatch_tiers (name, provider, agent, model, position)
		 VALUES (?, ?, ?, ?, ?)",
                params![t.name, t.provider, t.agent, t.model, t.position],
            )
            .map_err(db)?;
        }
        tx.commit().map_err(db)
    }

    fn upsert_agent(&self, agent: &Agent) -> Result<(), Error> {
        let cooldown = agent
            .cooldown_until
            .as_deref()
            .map(format_time)
            .filter(|c| !c.is_empty());
        self.conn()
            .execute(
                "INSERT INTO dispatch_agents (
			name, bin, provider, used_percent, cooldown_until
		) VALUES (?, ?, ?, ?, ?)
		ON CONFLICT(name) DO UPDATE SET
			bin = excluded.bin,
			provider = excluded.provider,
			used_percent = excluded.used_percent,
			cooldown_until = COALESCE(excluded.cooldown_until, dispatch_agents.cooldown_until)",
                params![
                    agent.name,
                    agent.bin,
                    agent.provider,
                    agent.used_percent,
                    cooldown
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn list_agents(&self) -> Result<Vec<Agent>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT name, bin, provider, used_percent, cooldown_until
		 FROM dispatch_agents
		 ORDER BY name ASC",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([], |r| {
                let cooldown: Option<String> = r.get(4)?;
                Ok(Agent {
                    name: r.get(0)?,
                    bin: r.get(1)?,
                    provider: r.get(2)?,
                    used_percent: r.get(3)?,
                    cooldown_until: cooldown
                        .filter(|c| !c.is_empty())
                        .map(|c| parse_time(&c))
                        .filter(|c| !c.is_empty()),
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn set_cooldown(&self, agent: &str, until: &str) -> Result<(), Error> {
        self.conn()
            .execute(
                "INSERT INTO dispatch_agents (name, cooldown_until)
		 VALUES (?, ?)
		 ON CONFLICT(name) DO UPDATE SET cooldown_until = excluded.cooldown_until",
                params![agent, format_time(until)],
            )
            .map_err(db)?;
        Ok(())
    }

    fn clear_cooldown(&self, agent: &str) -> Result<(), Error> {
        self.conn()
            .execute(
                "UPDATE dispatch_agents SET cooldown_until = NULL WHERE name = ?",
                [agent],
            )
            .map_err(db)?;
        Ok(())
    }

    fn save_route_decision(&self, dec: &RouteDecision) -> Result<(), Error> {
        let created = if dec.created_at.is_empty() {
            now()
        } else {
            dec.created_at.clone()
        };
        let probs = slice_json(&dec.probs)?;
        let candidates = slice_json(&dec.candidates)?;
        let area = serde_json::to_string(&dec.area)?;
        self.conn()
            .execute(
                "INSERT INTO dispatch_route_decisions (
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
			area_json = excluded.area_json",
                params![
                    dec.run_id,
                    dec.project_id,
                    dec.policy_version,
                    dec.route,
                    dec.priority,
                    dec.classifier,
                    probs,
                    dec.difficulty,
                    candidates,
                    dec.chosen,
                    format_time(&created),
                    area,
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn get_route_decision(&self, run_id: &str) -> Result<Option<RouteDecision>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ROUTE_DECISION_COLUMNS} FROM dispatch_route_decisions WHERE run_id = ?"
            ))
            .map_err(db)?;
        let mut rows = stmt.query([run_id]).map_err(db)?;
        match rows.next().map_err(db)? {
            Some(r) => Ok(Some(scan_decision(r)?)),
            None => Ok(None),
        }
    }

    fn list_route_decisions(&self, project_id: &str) -> Result<Vec<RouteDecision>, Error> {
        let mut query = format!("SELECT {ROUTE_DECISION_COLUMNS} FROM dispatch_route_decisions");
        let mut args: Vec<Value> = Vec::new();
        if !project_id.is_empty() {
            query.push_str(" WHERE project_id = ?");
            args.push(text(project_id));
        }
        query.push_str(" ORDER BY created_at DESC, run_id DESC");
        let conn = self.conn();
        let mut stmt = conn.prepare(&query).map_err(db)?;
        let mut rows = stmt.query(params_from_iter(args)).map_err(db)?;
        let mut out = Vec::new();
        while let Some(r) = rows.next().map_err(db)? {
            out.push(scan_decision(r)?);
        }
        Ok(out)
    }

    fn list_disabled_models(&self) -> Result<Vec<DisabledModel>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT provider, model FROM dispatch_disabled_models ORDER BY provider ASC, model ASC",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(DisabledModel {
                    provider: r.get(0)?,
                    model: r.get(1)?,
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn set_model_enabled(&self, provider: &str, model: &str, enabled: bool) -> Result<(), Error> {
        let provider = provider.trim().to_ascii_lowercase();
        let model = model.trim();
        let model = if model.is_empty() {
            String::new()
        } else {
            let base = crate::catalog::base_model(model);
            match base.split_once('/') {
                Some((head, rest))
                    if !rest.is_empty()
                        && (provider.is_empty() || head.eq_ignore_ascii_case(&provider)) =>
                {
                    rest.to_owned()
                }
                _ => base.to_owned(),
            }
        };
        if provider.is_empty() && model.is_empty() {
            return Err(Error::Message("provider is required".into()));
        }
        let conn = self.conn();
        if enabled {
            if model.is_empty() {
                conn.execute(
                    "DELETE FROM dispatch_disabled_models WHERE lower(provider) = lower(?1) AND model = ''",
                    params![provider],
                )
                .map_err(db)?;
            } else {
                conn.execute(
                    "DELETE FROM dispatch_disabled_models WHERE (lower(provider) = lower(?1) OR provider = '') AND lower(model) = lower(?2)",
                    params![provider, model],
                )
                .map_err(db)?;
            }
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO dispatch_disabled_models (provider, model) VALUES (?1, ?2)",
                params![provider, model],
            )
            .map_err(db)?;
        }
        Ok(())
    }
}
