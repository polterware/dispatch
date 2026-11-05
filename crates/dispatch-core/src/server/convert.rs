//! The store's records as the wire carries them.

use super::wire_time;
use crate::{
    proto as pb,
    reports::{StatRank, stats::Report},
    store::Workspace,
    types::{Agent, Attempt, Filters, QuotaSnapshot, Run, RunEvent, Section, Tier, WorktreeInfo},
};
use serde_json::{Value, json};

fn i32_of(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

fn i32_of_usize(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

fn priority(run: &Run) -> String {
    if let Some(r) = run.resolved.as_ref().filter(|r| !r.priority.is_empty()) {
        return r.priority.clone();
    }
    run.requested
        .as_ref()
        .map(|r| r.priority.clone())
        .unwrap_or_default()
}

fn attempts(list: &[Attempt]) -> Vec<pb::DispatchAttempt> {
    list.iter()
        .map(|a| pb::DispatchAttempt {
            agent: a.agent.clone(),
            model: a.model.clone(),
            error: a.error.clone(),
            exit_class: a.exit_class.clone(),
            duration_ms: a.duration_ms,
        })
        .collect()
}

pub fn run(r: &Run) -> pb::DispatchRun {
    pb::DispatchRun {
        conversation_id: r.conversation_id.clone(),
        caller_agent: r.caller_agent.clone(),
        resumable: !r.session_id.is_empty() && !r.agent.is_empty(),
        id: r.id.clone(),
        project_id: r.project_id.clone(),
        status: r.status.clone(),
        tier: r.tier.clone(),
        agent: r.agent.clone(),
        model: r.model.clone(),
        provider: r.provider.clone(),
        effort: r.effort.clone(),
        prompt: r.prompt.clone(),
        card_slug: r.card_slug.clone(),
        agent_run_id: r.agent_run_id.clone(),
        dir: r.dir.clone(),
        repo: r.repo.clone(),
        worktree: r.worktree.clone(),
        branch: r.branch.clone(),
        base_sha: r.base_sha.clone(),
        read_only: r.read_only,
        exit_code: i32_of(r.exit_code),
        exit_class: r.exit_class.clone(),
        error: r.error.clone(),
        note: r.note.clone(),
        files_changed: r.files_changed.map(i32_of),
        insertions: r.insertions.map(i32_of),
        deletions: r.deletions.map(i32_of),
        created_at: wire_time(&r.created_at),
        started_at: wire_time(&r.started_at),
        finished_at: wire_time(&r.finished_at),
        merged_at: r.merged_at.as_deref().map(wire_time).unwrap_or_default(),
        merge_sha: r.merge_sha.clone(),
        attempts: attempts(&r.attempts),
        resumed_of: r.resumed_of.clone(),
        parent_run_id: r.parent_run_id.clone(),
        queued_ms: r.queued_ms,
        run_ms: r.run_ms,
        priority: priority(r),
        usage: r.usage.as_ref().map(|u| pb::DispatchUsage {
            input_tokens: u.input_tokens,
            cache_read_tokens: u.cache_read_tokens,
            cache_write_tokens: u.cache_write_tokens,
            output_tokens: u.output_tokens,
            reasoning_tokens: u.reasoning_tokens,
            cost_usd: u.cost_usd,
            cost_source: u.cost_source.clone(),
            context_tokens: u.context_tokens,
            context_window: u.context_window,
        }),
        worktree_status: r.worktree_status.clone(),
    }
}

pub fn run_event(e: &RunEvent) -> pb::DispatchRunEvent {
    pb::DispatchRunEvent {
        id: e.id.clone(),
        run_id: e.run_id.clone(),
        project_id: e.project_id.clone(),
        kind: e.kind.clone(),
        agent: e.agent.clone(),
        tier: e.tier.clone(),
        text: e.text.clone(),
        seq: e.seq,
        at: wire_time(&e.at),
    }
}

pub fn watch_event(e: &RunEvent) -> pb::WatchRunResponse {
    watch_slice(e, 0)
}

/// An event whose text is its whole text from byte `offset` on.
pub fn watch_slice(e: &RunEvent, offset: u64) -> pb::WatchRunResponse {
    pb::WatchRunResponse {
        seq: e.seq,
        kind: e.kind.clone(),
        text: e.text.clone(),
        at: wire_time(&e.at),
        text_offset: i64::try_from(offset).unwrap_or(i64::MAX),
    }
}

pub fn worktree(w: &WorktreeInfo) -> pb::DispatchWorktree {
    pb::DispatchWorktree {
        id: w.id.clone(),
        project_id: w.project_id.clone(),
        run_id: w.run_id.clone(),
        card_slug: w.card_slug.clone(),
        path: w.path.clone(),
        branch: w.branch.clone(),
        base_sha: w.base_sha.clone(),
        status: w.status.clone(),
        owner: w.owner.clone(),
    }
}

pub fn quota(s: &QuotaSnapshot) -> pb::DispatchQuotaSnapshot {
    pb::DispatchQuotaSnapshot {
        agent: s.agent.clone(),
        provider: s.provider.clone(),
        window: s.window.clone(),
        source: s.source.clone(),
        confidence: s.confidence.clone(),
        used_percent: s.used_percent,
        window_minutes: i32_of(s.window_minutes),
        resets_at: wire_time(&s.resets_at),
        sampled_at: wire_time(&s.sampled_at),
    }
}

pub fn tier(t: &Tier) -> pb::DispatchTier {
    pb::DispatchTier {
        name: t.name.clone(),
        provider: t.provider.clone(),
        agent: t.agent.clone(),
        model: t.model.clone(),
        position: i32_of(t.position),
    }
}

pub fn agent(a: &Agent) -> pb::DispatchAgent {
    pb::DispatchAgent {
        name: a.name.clone(),
        bin: a.bin.clone(),
        provider: a.provider.clone(),
        used_percent: a.used_percent,
        cooldown_until: a
            .cooldown_until
            .as_deref()
            .map(wire_time)
            .unwrap_or_default(),
        installed: false,
    }
}

pub fn section(s: &Section) -> pb::DispatchSection {
    pb::DispatchSection {
        id: s.id.clone(),
        name: s.name.clone(),
        position: i32_of(s.position),
        created_at: wire_time(&s.created_at),
        items: s
            .items
            .iter()
            .map(|i| pb::DispatchSectionItem {
                kind: i.kind.clone(),
                r#ref: i.r#ref.clone(),
                position: i32_of(i.position),
            })
            .collect(),
    }
}

pub fn workspace(w: &Workspace) -> pb::Workspace {
    pb::Workspace {
        id: w.id.clone(),
        name: w.name.clone(),
        path: w.path.clone(),
        polterware_project_id: w.polterware_project_id.clone(),
        created_at: wire_time(&w.created_at),
    }
}

fn stat_rank(r: &StatRank) -> pb::DispatchStatRank {
    pb::DispatchStatRank {
        key: r.key.clone(),
        runs: i32_of_usize(r.runs),
        samples: i32_of_usize(r.samples),
        reason: r.reason.clone(),
        p50_ms: r.p50_ms,
        success_pct: r.success_pct,
    }
}

pub fn stats(report: &Report) -> pb::StatsResponse {
    pb::StatsResponse {
        by: report.by.clone(),
        since: report.since.clone(),
        in_flight: i32_of_usize(report.in_flight),
        rows: report
            .rows
            .iter()
            .map(|r| pb::DispatchStatsRow {
                key: r.key.clone(),
                runs: i32_of_usize(r.runs),
                success_pct: r.success_pct,
                first_try_pct: r.first_try_pct,
                p50_ms: r.p50_ms,
                p90_ms: r.p90_ms,
                fallbacks: i32_of_usize(r.fallbacks),
                rate_limits: i32_of_usize(r.rate_limits),
                orphans: i32_of_usize(r.orphans),
                usage_runs: i32_of_usize(r.spend.usage_runs),
                input_tokens: r.spend.input_tokens,
                cache_read_tokens: r.spend.cache_read_tokens,
                cache_write_tokens: r.spend.cache_write_tokens,
                output_tokens: r.spend.output_tokens,
                reasoning_tokens: r.spend.reasoning_tokens,
                cost_usd: r.spend.cost_usd,
                cost_runs: i32_of_usize(r.spend.cost_runs),
            })
            .collect(),
        rankings: Some(pb::DispatchStatsRankings {
            most_used_model: Some(stat_rank(&report.rankings.most_used_model)),
            most_used_provider: Some(stat_rank(&report.rankings.most_used_provider)),
            slowest_model: Some(stat_rank(&report.rankings.slowest_model)),
            most_failed_model: Some(stat_rank(&report.rankings.most_failed_model)),
        }),
    }
}

/// A time as Go's encoding/json writes one: RFC 3339 with nanoseconds, the
/// zero time for unset.
fn go_time(s: &str) -> Value {
    match chrono::DateTime::parse_from_rfc3339(s.trim()) {
        Ok(t) => Value::String(
            t.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        ),
        Err(_) => Value::String("0001-01-01T00:00:00Z".into()),
    }
}

fn filters(f: Option<&Filters>) -> Value {
    f.map_or(Value::Null, |f| {
        json!({
            "Provider": f.provider, "Model": f.model, "Effort": f.effort,
            "Agent": f.agent, "Priority": f.priority, "Relax": f.relax,
        })
    })
}

/// The run's record without its output, keyed as the CLI's `--section meta`
/// has always printed it.
pub fn run_meta(r: &Run) -> Value {
    let nil_vec = |v: &[String]| {
        if v.is_empty() { Value::Null } else { json!(v) }
    };
    let pairs: Vec<(&str, Value)> = vec![
        ("ConversationID", json!(r.conversation_id)),
        ("CallerAgent", json!(r.caller_agent)),
        ("ID", json!(r.id)),
        ("ProjectID", json!(r.project_id)),
        ("Status", json!(r.status)),
        ("Tier", json!(r.tier)),
        ("RequestedTier", json!(r.requested_tier)),
        ("ExecutionTier", json!(r.execution_tier)),
        ("Agent", json!(r.agent)),
        ("Model", json!(r.model)),
        ("Provider", json!(r.provider)),
        ("Effort", json!(r.effort)),
        ("Prompt", json!(r.prompt)),
        ("Dir", json!(r.dir)),
        ("Repo", json!(r.repo)),
        ("Worktree", json!(r.worktree)),
        ("Branch", json!(r.branch)),
        ("BaseSHA", json!(r.base_sha)),
        ("ReadOnly", json!(r.read_only)),
        ("NoWorktree", json!(r.no_worktree)),
        ("AgentRunID", json!(r.agent_run_id)),
        ("CardSlug", json!(r.card_slug)),
        ("Area", json!(r.area)),
        ("Output", json!("")),
        ("Error", json!(r.error)),
        ("Note", json!(r.note)),
        ("ExitCode", json!(r.exit_code)),
        ("ExitClass", json!(r.exit_class)),
        ("TimeoutSec", json!(r.timeout_sec)),
        ("SessionID", json!(r.session_id)),
        ("ResumedOf", json!(r.resumed_of)),
        ("ParentRunID", json!(r.parent_run_id)),
        ("PID", json!(r.pid)),
        ("AgentPID", json!(r.agent_pid)),
        ("ProcStart", json!(r.proc_start)),
        ("Profile", json!(r.profile)),
        ("Prefer", json!(nil_vec(&r.prefer))),
        ("Avoid", json!(nil_vec(&r.avoid))),
        ("RolePick", json!(r.role_pick)),
        ("Requested", json!(filters(r.requested.as_ref()))),
        ("Resolved", json!(filters(r.resolved.as_ref()))),
        (
            "Attempts",
            json!(if r.attempts.is_empty() {
                Value::Null
            } else {
                json!(r.attempts)
            }),
        ),
        ("Classifier", json!(r.classifier)),
        (
            "DifficultyProbs",
            json!(if r.difficulty_probs.is_empty() {
                Value::Null
            } else {
                json!(r.difficulty_probs)
            }),
        ),
        ("Difficulty", json!(r.difficulty)),
        ("FilesChanged", json!(r.files_changed)),
        ("Insertions", json!(r.insertions)),
        ("Deletions", json!(r.deletions)),
        ("QueuedMs", json!(r.queued_ms)),
        ("RunMs", json!(r.run_ms)),
        ("Usage", json!(r.usage)),
        ("CreatedAt", json!(go_time(&r.created_at))),
        ("StartedAt", json!(go_time(&r.started_at))),
        ("FinishedAt", json!(go_time(&r.finished_at))),
        (
            "MergedAt",
            json!(r.merged_at.as_deref().map_or(Value::Null, go_time)),
        ),
        ("MergeSHA", json!(r.merge_sha)),
        ("WorktreeStatus", json!(r.worktree_status)),
    ];
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
