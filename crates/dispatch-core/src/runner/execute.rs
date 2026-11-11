use super::{
    AgentCall, AttemptError, COOLDOWN_FOR, CREDIT_COOLDOWN_FOR, Candidate, DEPTH_ENV, Deps,
    ERR_RECURSIVE, NO_DELEGATE_ENV, Options, PARENT_RUN_ID_ENV, PRIORITY_BALANCED, RUN_ID_ENV,
    Started, TIER_AUTO, add_usage,
    classify::record_difficulty,
    classify_run, cooling, credit_key, fmt_time, go_duration, matches_model, msg, non_neg_ms,
    normalize_priority, parse_time, process_start,
    routing::{Cooldowns, candidate_provider, store_cooldowns},
    run_difficulty, store_disabled_agents,
    summary::class_of,
    tier_warning, validate_options,
};
use crate::{
    Error,
    adapters::{
        Request, RunResult,
        model::{effort_of, provider_of},
        probe::ProbeReport,
        valid_tier,
    },
    router, sandbox,
    types::{
        Attempt, Filters, Run, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_FAILED, STATUS_ORPHANED,
        STATUS_PENDING, STATUS_RUNNING, STATUS_TIMED_OUT, WORKTREE_ACTIVE, WorktreeInfo, new_id,
    },
    worktree,
};
use chrono::{DateTime, Utc};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub fn join_note(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.into();
    }
    if b.is_empty() || a.contains(b) {
        return a.into();
    }
    format!("{a}; {b}")
}

pub fn terminal_status(s: &str) -> bool {
    matches!(
        s,
        STATUS_ORPHANED | STATUS_COMPLETED | STATUS_FAILED | STATUS_CANCELLED | STATUS_TIMED_OUT
    )
}

/// Maps a terminal status.
pub fn exit_class(status: &str) -> &'static str {
    match status {
        STATUS_COMPLETED => "ok",
        STATUS_TIMED_OUT => "timeout",
        STATUS_CANCELLED => "cancelled",
        STATUS_FAILED => "failed",
        _ => "",
    }
}

/// Fills provider, effort, diff stats and exit class. Absent numbers stay
/// absent when the measurement is unknown.
pub fn complete_telemetry(rec: &mut Run, now: Option<DateTime<Utc>>) {
    if rec.run_ms.is_none()
        && terminal_status(&rec.status)
        && let (Some(start), Some(now)) = (parse_time(&rec.started_at), now)
    {
        rec.run_ms = Some(non_neg_ms(now - start));
    }
    if rec.provider.is_empty() {
        rec.provider = provider_of(&rec.agent, &rec.model);
    }
    if rec.effort.is_empty() {
        rec.effort = effort_of(&rec.model);
    }
    if rec.files_changed.is_none() && !rec.worktree.is_empty() && !rec.base_sha.is_empty() {
        fill_diff(rec);
    }
    if terminal_status(&rec.status) {
        let cls = classify_run(rec);
        if !cls.is_empty() {
            rec.exit_class = cls;
        }
    }
}

fn fill_diff(rec: &mut Run) {
    let Ok(stat) = worktree::num_stat(&rec.worktree, &rec.base_sha) else {
        return;
    };
    rec.files_changed = Some(i64::from(stat.files));
    if stat.lines_known {
        rec.insertions = Some(i64::from(stat.insertions));
        rec.deletions = Some(i64::from(stat.deletions));
    }
}

/// A run that cuts no worktree of its own cuts none for the other repositories either.
fn drop_linked(rec: &mut Run) {
    if rec.linked.is_empty() {
        return;
    }
    rec.linked.clear();
    rec.note = join_note(
        &rec.note,
        "the other repositories need a worktree of the run's own; they were left out",
    );
}

/// Where the live, redacted copy of an agent's output goes; empty without a
/// state directory.
pub fn agent_log_path(state_dir: &str, id: &str) -> String {
    if state_dir.is_empty() {
        return String::new();
    }
    std::path::Path::new(state_dir)
        .join("logs")
        .join(format!("{id}.agent.log"))
        .to_string_lossy()
        .into_owned()
}

/// The agent environment: recursion guards plus run ids.
pub fn child_environ(run_id: &str, parent: &str) -> Vec<String> {
    let mut extra = vec![
        format!("{DEPTH_ENV}=1"),
        format!("{RUN_ID_ENV}={run_id}"),
        format!("{NO_DELEGATE_ENV}=1"),
    ];
    if !parent.is_empty() {
        extra.push(format!("{PARENT_RUN_ID_ENV}={parent}"));
    }
    sandbox::environ(&extra)
}

/// Writes a separator into the live log when a run falls back, so a viewer
/// does not read the previous agent's error as the current one.
fn mark_attempt(path: &str, prev: &[Attempt], c: &Candidate) {
    let Some(last) = prev.last() else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let class = if last.exit_class.is_empty() {
        "failed"
    } else {
        &last.exit_class
    };
    let line = format!(
        "\n── dispatch: {}/{} failed ({class}); trying {}/{} ──\n",
        last.agent,
        last.model,
        c.agent(),
        c.model()
    );
    let path = std::path::Path::new(path);
    if let Some(dir) = path.parent() {
        use std::os::unix::fs::DirBuilderExt;
        if std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .is_err()
        {
            return;
        }
    }
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn last_attempt_log(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut start = 0;
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        let next = offset + line.len();
        let trimmed = line.trim();
        if trimmed.starts_with("── dispatch:") && trimmed.ends_with("──") {
            start = next;
        }
        offset = next;
    }
    raw[start..].to_owned()
}

fn missing_flags(r: &ProbeReport, capability: &str) -> Vec<String> {
    r.flags
        .iter()
        .filter(|f| !f.present && f.capability == capability)
        .map(|f| f.name.clone())
        .collect()
}

fn own_pid() -> i64 {
    i64::from(std::process::id())
}

async fn sample_candidate_quota(a: &dyn super::AgentAdapter, model: &str) -> Option<f64> {
    if let Some(fut) = a.model_quotas(vec![model.to_owned()]) {
        if let Ok(map) = fut.await
            && let Some(q) = map.get(model)
        {
            return Some(q.used_percent);
        }
        return None;
    }
    a.quota().await.ok().map(|q| q.used_percent)
}

impl Deps {
    /// Probes the CLI when it can. DEGRADED still runs and returns a note via
    /// the record. A missing flag for a capability this run requires is refused.
    async fn block(&self, rec: &mut Run, c: &Candidate, resume: bool) -> String {
        let Some(a) = &c.adapter else {
            return String::new();
        };
        let Some(probe) = a.probe() else {
            return String::new();
        };
        let report = probe.await;
        if report.status != "DEGRADED" {
            return String::new();
        }
        let name = a.name();
        if rec.read_only {
            let miss = missing_flags(&report, "readOnly");
            if !miss.is_empty() {
                return format!(
                    "{name} does not support readOnly (missing {})",
                    miss.join(", ")
                );
            }
        } else {
            let miss = missing_flags(&report, "fileEditing");
            if !miss.is_empty() {
                return format!(
                    "{name} does not support fileEditing (missing {})",
                    miss.join(", ")
                );
            }
        }
        if !c.route.model.is_empty()
            && let Some(f) = report
                .flags
                .iter()
                .find(|f| !f.present && (f.name == "--model" || f.name == "-m"))
        {
            return format!("model selection unavailable (missing {})", f.name);
        }
        if !c.route.effort.is_empty()
            && a.capabilities().effort_mode == "config"
            && report.flags.iter().any(|f| !f.present && f.name == "-c")
        {
            return "effort configuration unavailable (missing -c)".into();
        }
        if resume {
            let miss = missing_flags(&report, "sessionResume");
            if !miss.is_empty() {
                return format!(
                    "{name} does not support session resume (missing {})",
                    miss.join(", ")
                );
            }
        }
        let mut detail = report.notes.join("; ");
        if detail.is_empty() {
            detail = "capability flags missing".into();
        }
        rec.note = join_note(&rec.note, &format!("{name} probe DEGRADED: {detail}"));
        String::new()
    }

    /// Validates the request and persists a PENDING record.
    pub async fn begin(&self, o: Options) -> Result<Run, Error> {
        let mut o = o;
        if self.recursive() {
            return Err(msg(ERR_RECURSIVE));
        }
        normalize_priority(&mut o)?;
        let reason = crate::config::disabled_agent_reason(
            &store_disabled_agents(self.store.as_deref()),
            &o.filters.agent,
        );
        if !reason.is_empty() {
            return Err(msg(reason));
        }
        let mut route_note = String::new();
        if o.resume.is_empty() {
            route_note = self.resolve_auto(&mut o).await?;
        }
        let pid = own_pid();
        let mut rec = Run {
            conversation_id: o.caller.conversation_id.clone(),
            caller_agent: o.caller.agent.clone(),
            pid,
            proc_start: process_start(pid).await,
            id: new_id(),
            project_id: o.project_id.clone(),
            agent_run_id: o.agent_run_id.clone(),
            card_slug: o.card_slug.clone(),
            area: o.area.clone(),
            status: STATUS_PENDING.into(),
            tier: o.tier.clone(),
            prompt: o.prompt.clone(),
            dir: o.dir.clone(),
            read_only: o.read_only,
            created_at: fmt_time(self.now()),
            timeout_sec: i64::try_from(o.timeout.as_secs()).unwrap_or(i64::MAX),
            no_worktree: o.no_worktree,
            profile: o.profile.clone(),
            prefer: o.prefer.clone(),
            avoid: o.avoid.clone(),
            role_pick: o.pick.clone(),
            requested: Some(o.filters.clone()),
            parent_run_id: self.env(RUN_ID_ENV),
            note: route_note,
            linked: o
                .with
                .iter()
                .map(|d| d.trim())
                .filter(|d| !d.is_empty())
                .map(|d| crate::types::LinkedRepo {
                    dir: d.to_owned(),
                    ..Default::default()
                })
                .collect(),
            ..Run::default()
        };
        if !o.resume.is_empty() {
            let prev = self
                .store()?
                .get_run(&o.resume)
                .map_err(|e| msg(format!("resume {}: {e}", o.resume)))?;
            if prev.session_id.is_empty() || prev.agent.is_empty() {
                return Err(msg(format!("run {} has no resumable session", prev.id)));
            }
            rec.resumed_of.clone_from(&prev.id);
            rec.tier.clone_from(&prev.tier);
            if rec.project_id.is_empty() {
                rec.project_id.clone_from(&prev.project_id);
            }
            rec.agent.clone_from(&prev.agent);
            rec.model.clone_from(&prev.model);
            rec.session_id.clone_from(&prev.session_id);
            rec.repo.clone_from(&prev.repo);
            rec.worktree.clone_from(&prev.worktree);
            rec.branch.clone_from(&prev.branch);
            rec.base_sha.clone_from(&prev.base_sha);
            rec.provider.clone_from(&prev.provider);
            rec.effort.clone_from(&prev.effort);
            rec.resolved.clone_from(&prev.resolved);
            rec.linked.clone_from(&prev.linked);
            let f = &o.filters;
            if (!f.agent.is_empty() && f.agent != prev.agent)
                || (!f.model.is_empty() && !matches_model(&prev.model, &f.model))
                || (!f.provider.is_empty() && f.provider != provider_of(&prev.agent, &prev.model))
                || (!f.effort.is_empty() && f.effort != prev.effort)
            {
                return Err(msg("resume filters conflict with existing session"));
            }
            if rec.dir.is_empty() {
                rec.dir.clone_from(&prev.dir);
            }
        } else if !valid_tier(&rec.tier)
            && rec.tier != TIER_AUTO
            && !(rec.tier.is_empty()
                && (!o.filters.model.is_empty() || !o.filters.provider.is_empty()))
        {
            return Err(msg(format!(
                "invalid tier {:?} (want auto|easy|normal|hard)",
                rec.tier
            )));
        }
        // An absolute dir lets the live viewer match runs to their repository.
        rec.dir = if rec.dir.is_empty() {
            self.cwd()
        } else {
            self.abs(&rec.dir)
        };
        if o.resume.is_empty() {
            record_difficulty(&mut rec, &o);
        }
        o.tier.clone_from(&rec.tier);
        validate_options(&o)?;
        if let Some(st) = self.store.as_deref() {
            st.save_run(&rec)?;
        }
        Ok(rec)
    }

    /// Begin + Execute.
    pub async fn run(&self, cancel: &CancellationToken, o: Options) -> Result<Run, Error> {
        self.reconcile(cancel).await?;
        let rec = self.begin(o).await?;
        self.execute(cancel, rec).await
    }

    /// Runs a record created by Begin and returns its final state.
    pub async fn execute(&self, cancel: &CancellationToken, rec: Run) -> Result<Run, Error> {
        let mut rec = rec;
        if self.recursive() {
            return Err(msg(ERR_RECURSIVE));
        }
        if terminal_status(&rec.status) {
            return Err(msg(format!("run {} already {}", rec.id, rec.status)));
        }
        let started = self.now();
        rec.pid = own_pid();
        rec.proc_start = process_start(rec.pid).await;
        rec.started_at = fmt_time(started);
        if let Some(created) = parse_time(&rec.created_at) {
            rec.queued_ms = Some(non_neg_ms(started - created));
        }
        if let Some(st) = self.store.as_deref() {
            st.save_run(&rec)?;
        }
        self.reconcile(cancel).await?;

        let mut cool = store_cooldowns(self.store.as_deref(), self.now());
        let (cands, resume_session) = match self.plan_candidates(&rec, &cool).await {
            Ok(v) => v,
            Err(fail) => return self.finish_run(rec, None, STATUS_FAILED, &fail),
        };
        let views: Vec<router::Candidate> = cands.iter().map(|c| c.route.clone()).collect();
        let note = router::auto_note(&views);
        if !note.is_empty() {
            rec.note = join_note(&rec.note, &note);
        }
        let mut priced = cands.clone();
        if cands[0].route.auto.is_none() && rec.resumed_of.is_empty() {
            priced = self.price_tier_route(&rec, priced);
            let note = tier_warning(&priced);
            if !note.is_empty() {
                rec.note = join_note(&rec.note, &note);
            }
        }
        self.record_route_decision(&rec, &priced);
        let dir = match self.prepare_worktree(&mut rec) {
            Ok(dir) => dir,
            Err(fail) => return self.finish_run(rec, None, STATUS_FAILED, &fail),
        };

        let timeout = Duration::from_secs(u64::try_from(rec.timeout_sec).unwrap_or(0));
        let child_env = child_environ(&rec.id, &self.env(RUN_ID_ENV));
        let log_path = agent_log_path(&self.state_dir(), &rec.id);
        // The start of the attempt that ends the run.
        let mut agent_start = None;
        let mut skipped = Vec::new();
        let mut last_attempt_output = String::new();

        for (idx, c) in cands.iter().enumerate() {
            if let Some(key) = cooling(&cool, c.agent(), &candidate_provider(c)) {
                skipped.push(format!("{key} in cooldown"));
                continue;
            }
            let blocked = self.block(&mut rec, c, !resume_session.is_empty()).await;
            if !blocked.is_empty() {
                skipped.push(blocked);
                continue;
            }
            let handoff = if let Some(prev) = rec.attempts.last() {
                if !prev.handoff.trim().is_empty() {
                    Some(prev.handoff.clone())
                } else if super::handoff::is_limit_class(&prev.exit_class) {
                    let log_slice = last_attempt_log(&log_path);
                    let built = super::handoff::build_handoff(
                        &rec,
                        prev,
                        &dir,
                        &log_slice,
                        &last_attempt_output,
                    );
                    super::handoff::write_worktree_handoff(&rec, &dir, &built);
                    if let Some(last_mut) = rec.attempts.last_mut() {
                        last_mut.handoff.clone_from(&built);
                    }
                    Some(built)
                } else {
                    None
                }
            } else {
                None
            };
            let has_next_candidate = cands[idx + 1..]
                .iter()
                .any(|next| cooling(&cool, next.agent(), &candidate_provider(next)).is_none());
            let attempt_start = self.now();
            agent_start = Some(attempt_start);
            let (res, run_err, near_limit_used) = self
                .run_attempt(
                    cancel,
                    &mut rec,
                    c,
                    &dir,
                    &resume_session,
                    timeout,
                    &child_env,
                    handoff.as_deref(),
                    has_next_candidate,
                )
                .await?;
            // The agent ended its turn with only the provider's limit notice: the account is
            // out, so this attempt failed and the next candidate takes the task.
            let run_err = match run_err {
                None => super::summary::limit_notice(&res.output).map(AttemptError::Failed),
                other => other,
            };
            match run_err {
                Some(AttemptError::ProcessesAlive) => {
                    let e = AttemptError::ProcessesAlive.to_string();
                    return self.finish_run(rec, agent_start, STATUS_FAILED, &e);
                }
                None => {
                    self.post_run(&mut rec, res.output, attempt_start);
                    return self.finish_run(rec, agent_start, STATUS_COMPLETED, "");
                }
                Some(_) if cancel.is_cancelled() => {
                    return self.finish_run(rec, agent_start, STATUS_CANCELLED, "context canceled");
                }
                Some(_) if near_limit_used.is_some() => {
                    self.handle_near_limit(
                        cancel,
                        &mut rec,
                        c,
                        &dir,
                        &child_env,
                        &log_path,
                        &res,
                        attempt_start,
                        near_limit_used.unwrap_or(super::handoff::HANDOFF_NEAR_LIMIT_PERCENT),
                    )
                    .await;
                }
                Some(AttemptError::DeadlineExceeded) => {
                    let e = format!("timeout after {}", go_duration(timeout));
                    return self.finish_run(rec, agent_start, STATUS_TIMED_OUT, &e);
                }
                Some(e) => {
                    last_attempt_output = res.output;
                    if self.record_attempt_failure(&mut rec, c, &e.to_string(), attempt_start) {
                        cool = store_cooldowns(self.store.as_deref(), self.now());
                    }
                }
            }
        }
        let Some(last) = rec.attempts.last().cloned() else {
            let mut m = skipped.join("; ");
            if m.is_empty() {
                m = "no candidate could run".into();
            }
            return self.finish_run(rec, agent_start, STATUS_FAILED, &m);
        };
        let m = format!(
            "all {} candidates failed; last: {}/{}: {}",
            rec.attempts.len(),
            last.agent,
            last.model,
            last.error
        );
        self.finish_run(rec, agent_start, STATUS_FAILED, &m)
    }

    /// The ordered candidates and the session to resume, or the failure
    /// message when nothing can run.
    async fn plan_candidates(
        &self,
        rec: &Run,
        cool: &Cooldowns,
    ) -> Result<(Vec<Candidate>, String), String> {
        if !rec.resumed_of.is_empty() {
            let a = self
                .adapter(&rec.agent)
                .filter(|a| self.is_installed(&a.binary()));
            let Some(a) = a else {
                return Err(format!("agent {} is not available to resume", rec.agent));
            };
            if !a.capabilities().session_resume {
                return Err(format!("{} does not support session resume", a.name()));
            }
            let c = Candidate {
                adapter: Some(a.clone()),
                route: router::Candidate {
                    agent: a.name(),
                    model: rec.model.clone(),
                    provider: rec.provider.clone(),
                    effort: rec.effort.clone(),
                    ..router::Candidate::default()
                },
                tier: String::new(),
                quota: None,
            };
            return Ok((vec![c], rec.session_id.clone()));
        }
        let mut o = Options {
            tier: rec.tier.clone(),
            read_only: rec.read_only,
            prefer: rec.prefer.clone(),
            avoid: rec.avoid.clone(),
            profile: rec.profile.clone(),
            area: rec.area.clone(),
            ..Options::default()
        };
        if !rec.profile.is_empty()
            && let Some(p) = self.profiles.get(&rec.profile)
        {
            o.role_board = p.board();
            o.role_columns = p.columns();
        }
        if let Some(f) = &rec.requested {
            o.filters = f.clone();
        }
        if rec.requested_tier == TIER_AUTO {
            o.tier = TIER_AUTO.into();
            o.prompt.clone_from(&rec.prompt);
            o.dir.clone_from(&rec.dir);
            o.difficulty = run_difficulty(rec);
        }
        if o.filters.priority.is_empty() {
            o.filters.priority = PRIORITY_BALANCED.into();
        }
        let planned = self.plan_with_cooldowns(&o, true, cool).await;
        if planned.ordered.is_empty() {
            if !planned.rejected.is_empty() {
                return Err(planned.rejected.join("; "));
            }
            return Err(format!("no installed agent serves tier {}", rec.tier));
        }
        Ok((planned.ordered, String::new()))
    }

    /// Resolves the agent directory and, for writable runs in a git checkout,
    /// cuts the run worktree. An error message fails the run.
    fn prepare_worktree(&self, rec: &mut Run) -> Result<String, String> {
        let mut dir = if rec.dir.is_empty() {
            self.cwd()
        } else {
            rec.dir.clone()
        };
        dir = self.abs(&dir);
        // Where to cut the worktree: the run's own folder, or the one a follow-up's run had.
        let mut path = String::new();
        if !rec.resumed_of.is_empty() {
            if rec.worktree.is_empty() {
                return Ok(dir);
            }
            if std::path::Path::new(&rec.worktree).exists() {
                if !worktree::root(&rec.worktree).is_empty() {
                    dir.clone_from(&rec.worktree);
                }
                return Ok(dir);
            }
            if rec.read_only || rec.no_worktree || !self.owns_worktree(&rec.repo, &rec.worktree) {
                rec.note = format!("previous worktree is gone; resumed in {dir}");
                return Ok(dir);
            }
            // The earlier run's worktree went, with nothing to merge: the follow-up cuts it
            // again at the same path, where the agent's session was, on a branch of its own.
            path = std::mem::take(&mut rec.worktree);
            rec.branch.clear();
            rec.base_sha.clear();
            rec.note = "the previous run's worktree had nothing to merge and was removed; this follow-up cut it again on a new branch".into();
        }
        if rec.read_only || rec.no_worktree {
            drop_linked(rec);
            return Ok(dir);
        }
        let root = worktree::root(&dir);
        if root.is_empty() || !worktree::has_commits(&root) {
            drop_linked(rec);
            return Ok(dir);
        }
        if self.worktree_dir.is_empty() {
            let _ = worktree::exclude(&root, ".dispatch/");
        }
        if path.is_empty() {
            path = format!("{}/{}", self.worktrees_base(&root), rec.id);
        } else {
            // Forget the old registration, should the folder have gone by hand.
            let _ = worktree::git_exit(&root, &["worktree", "prune"]);
        }
        let branch =
            worktree::create(&root, &path, &rec.id).map_err(|e| format!("worktree: {e}"))?;
        rec.repo.clone_from(&root);
        rec.worktree.clone_from(&path);
        rec.branch.clone_from(&branch);
        rec.base_sha = worktree::head(&path).map_err(|e| e.to_string())?;
        if let Some(st) = self.store.as_deref() {
            let _ = st.save_worktree(&WorktreeInfo {
                id: rec.id.clone(),
                project_id: rec.project_id.clone(),
                run_id: rec.id.clone(),
                card_slug: rec.card_slug.clone(),
                path: path.clone(),
                branch,
                base_sha: rec.base_sha.clone(),
                status: WORKTREE_ACTIVE.into(),
                owner: String::new(),
            });
        }
        let rel = worktree::rel(&root, &dir);
        let dir = super::clean_path(&format!("{path}/{rel}"));
        if worktree::dirty(&root) {
            rec.note = join_note(
                &rec.note,
                "checkout has uncommitted changes; the worktree was cut from HEAD and does not include them",
            );
        }
        if !rec.linked.is_empty() {
            self.cut_linked(rec)?;
            self.run_link_hooks(rec);
        }
        Ok(dir)
    }

    /// Records the chosen candidate on `rec` and runs it. The outer error is
    /// a store failure that aborts Execute without finishing the run.
    #[allow(clippy::too_many_arguments)]
    async fn run_attempt(
        &self,
        cancel: &CancellationToken,
        rec: &mut Run,
        c: &Candidate,
        dir: &str,
        resume_session: &str,
        timeout: Duration,
        child_env: &[String],
        handoff: Option<&str>,
        has_next_candidate: bool,
    ) -> Result<(RunResult, Option<AttemptError>, Option<f64>), Error> {
        rec.agent_pid = 0;
        rec.status = STATUS_RUNNING.into();
        rec.agent = c.agent().into();
        rec.model = c.model().into();
        rec.execution_tier.clone_from(&c.tier);
        if rec.execution_tier.is_empty() && valid_tier(&rec.tier) {
            rec.execution_tier.clone_from(&rec.tier);
        }
        rec.provider.clone_from(&c.route.provider);
        if rec.provider.is_empty() {
            rec.provider = provider_of(&rec.agent, &rec.model);
        }
        rec.effort.clone_from(&c.route.effort);
        if rec.effort.is_empty() {
            rec.effort = effort_of(&rec.model);
        }
        let mut prio = c.route.priority.clone();
        if prio.is_empty()
            && let Some(f) = &rec.requested
        {
            prio.clone_from(&f.priority);
        }
        if prio.is_empty() {
            prio = PRIORITY_BALANCED.into();
        }
        rec.resolved = Some(Filters {
            provider: rec.provider.clone(),
            model: rec.model.clone(),
            agent: rec.agent.clone(),
            effort: rec.effort.clone(),
            priority: prio,
            relax: false,
        });
        rec.note = join_note(&rec.note, &c.route.note);
        if let Some(st) = self.store.as_deref() {
            st.save_run(rec)?;
        }
        let shared = Arc::new(Mutex::new(rec.clone()));
        let store = self.store.clone();
        let cell = shared.clone();
        let started: Started = Arc::new(move |pid: i64| {
            let mut r = cell.lock().unwrap_or_else(|e| e.into_inner());
            r.agent_pid = pid;
            match &store {
                Some(st) => st.save_run(&r),
                None => Ok(()),
            }
        });
        let mut env = child_env.to_vec();
        if !rec.linked.is_empty() {
            env.push(format!(
                "{}={}",
                super::linked::LINKED_ENV,
                super::linked::linked_env(rec)
            ));
        }
        // Every adapter gets the run's folder for the images it makes.
        let artifacts = self.artifacts_for(&rec.id);
        if let Some(dir) = &artifacts {
            env.push(format!(
                "{}={}",
                crate::artifacts::ARTIFACTS_ENV,
                dir.display()
            ));
        }
        let base_prompt = match handoff {
            Some(h) if !h.is_empty() => super::handoff::prompt_with_handoff(h),
            _ => rec.prompt.clone(),
        };
        // The image mode of a later phase asks for images; until then only the folder is passed.
        let wants_images = false;
        let artifacts_note = artifacts
            .as_deref()
            .map(|dir| crate::artifacts::brief_note(wants_images, dir))
            .unwrap_or_default();
        let request = Request {
            effort: rec.effort.clone(),
            prompt: format!(
                "{base_prompt}{}{artifacts_note}",
                super::linked::prompt_note(rec)
            ),
            tier: rec.tier.clone(),
            dir: dir.into(),
            read_only: rec.read_only,
            timeout,
            resume_session: resume_session.into(),
            env,
            log_path: agent_log_path(&self.state_dir(), &rec.id),
            started: None,
        };
        mark_attempt(&request.log_path, &rec.attempts, c);
        let attempt_cancel = cancel.child_token();
        let near_limit = Arc::new(Mutex::new(None::<f64>));
        let watch_task = if has_next_candidate && let Some(adapter) = c.adapter.clone() {
            let watch_cancel = attempt_cancel.clone();
            let near_limit_cell = near_limit.clone();
            let model = c.model().to_owned();
            let poll = self.quota_poll();
            let is_claude_stream =
                adapter.usage_source() == Some(crate::adapters::quota::SOURCE_CLAUDE_STREAM);
            Some(tokio::spawn(async move {
                if is_claude_stream {
                    let mut rx = crate::adapters::claude_limits::subscribe();
                    loop {
                        tokio::select! {
                            () = watch_cancel.cancelled() => break,
                            res = rx.changed() => {
                                if res.is_err() {
                                    break;
                                }
                            }
                        }
                        let Some(used) = sample_candidate_quota(adapter.as_ref(), &model).await
                        else {
                            continue;
                        };
                        if used >= super::handoff::HANDOFF_NEAR_LIMIT_PERCENT {
                            *near_limit_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(used);
                            watch_cancel.cancel();
                            break;
                        }
                    }
                } else {
                    loop {
                        tokio::select! {
                            () = watch_cancel.cancelled() => break,
                            () = tokio::time::sleep(poll) => {}
                        }
                        let Some(used) = sample_candidate_quota(adapter.as_ref(), &model).await
                        else {
                            continue;
                        };
                        if used >= super::handoff::HANDOFF_NEAR_LIMIT_PERCENT {
                            *near_limit_cell.lock().unwrap_or_else(|e| e.into_inner()) = Some(used);
                            watch_cancel.cancel();
                            break;
                        }
                    }
                }
            }))
        } else {
            None
        };
        let call = AgentCall {
            request,
            model: c.model().into(),
            cancel: attempt_cancel,
            started,
        };
        let out = match &c.adapter {
            Some(a) if !resume_session.is_empty() => a.resume(call).await,
            Some(a) => a.run(call).await,
            None => super::AttemptResult {
                result: RunResult::default(),
                error: Some(AttemptError::Failed(format!(
                    "agent {} is not available",
                    c.agent()
                ))),
            },
        };
        if let Some(task) = watch_task {
            task.abort();
        }
        rec.agent_pid = shared.lock().unwrap_or_else(|e| e.into_inner()).agent_pid;
        rec.exit_code = i64::from(out.result.exit_code);
        rec.session_id.clone_from(&out.result.session_id);
        add_usage(rec, c.model(), out.result.usage.as_ref(), &self.catalog());
        let near_limit_used = if cancel.is_cancelled() {
            None
        } else {
            *near_limit.lock().unwrap_or_else(|e| e.into_inner())
        };
        Ok((out.result, out.error, near_limit_used))
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_near_limit(
        &self,
        cancel: &CancellationToken,
        rec: &mut Run,
        c: &Candidate,
        dir: &str,
        child_env: &[String],
        log_path: &str,
        res: &RunResult,
        attempt_start: DateTime<Utc>,
        used_pct: f64,
    ) {
        let log_slice = last_attempt_log(log_path);
        let limit_err = format!("{}% of usage limit", used_pct.round() as i64);
        let model_reply = self
            .resume_for_handoff(cancel, rec, c, dir, child_env, &res.session_id)
            .await;
        let mut attempt = Attempt {
            agent: c.agent().into(),
            model: c.model().into(),
            error: limit_err,
            exit_class: super::handoff::EXIT_NEAR_LIMIT.into(),
            duration_ms: Some(non_neg_ms(self.now() - attempt_start)),
            handoff: String::new(),
        };
        let handoff = model_reply
            .as_deref()
            .and_then(super::handoff::model_handoff)
            .unwrap_or_else(|| {
                super::handoff::build_handoff(rec, &attempt, dir, &log_slice, &res.output)
            });
        super::handoff::write_worktree_handoff(rec, dir, &handoff);
        attempt.handoff = handoff;
        rec.attempts.push(attempt);
        if let Some(st) = self.store.as_deref() {
            let _ = st.save_run(rec);
        }
    }

    async fn resume_for_handoff(
        &self,
        cancel: &CancellationToken,
        rec: &mut Run,
        c: &Candidate,
        dir: &str,
        child_env: &[String],
        session_id: &str,
    ) -> Option<String> {
        let a = c.adapter.as_ref()?;
        if session_id.trim().is_empty() || !a.capabilities().session_resume {
            return None;
        }
        if !self.block(rec, c, true).await.is_empty() {
            return None;
        }
        let mut env = child_env.to_vec();
        if !rec.linked.is_empty() {
            env.push(format!(
                "{}={}",
                super::linked::LINKED_ENV,
                super::linked::linked_env(rec)
            ));
        }
        let run_timeout = Duration::from_secs(u64::try_from(rec.timeout_sec).unwrap_or(0));
        let timeout = if run_timeout.is_zero() {
            super::handoff::HANDOFF_RESUME_TIMEOUT
        } else {
            run_timeout.min(super::handoff::HANDOFF_RESUME_TIMEOUT)
        };
        let request = Request {
            effort: rec.effort.clone(),
            prompt: super::handoff::HANDOFF_MODEL_PROMPT.into(),
            tier: rec.tier.clone(),
            dir: dir.into(),
            read_only: rec.read_only,
            timeout,
            resume_session: session_id.trim().into(),
            env,
            log_path: String::new(),
            started: None,
        };
        let resume_cancel = cancel.child_token();
        let started: Started = Arc::new(|_| Ok(()));
        let call = AgentCall {
            request,
            model: c.model().into(),
            cancel: resume_cancel.clone(),
            started,
        };
        let out = match tokio::time::timeout(timeout, a.resume(call)).await {
            Ok(out) => out,
            Err(_) => {
                resume_cancel.cancel();
                return None;
            }
        };
        add_usage(rec, c.model(), out.result.usage.as_ref(), &self.catalog());
        if out.error.is_some() {
            return None;
        }
        Some(out.result.output)
    }

    /// Stores a successful attempt's output and commits the worktree.
    fn post_run(&self, rec: &mut Run, output: String, agent_start: DateTime<Utc>) {
        rec.output = output;
        rec.run_ms = Some(non_neg_ms(self.now() - agent_start));
        if rec.worktree.is_empty() || rec.read_only {
            return;
        }
        let note = match worktree::commit(&rec.worktree, &rec.id) {
            Err(e) => format!("commit failed: {e}"),
            Ok(true) => format!("changes committed on {}", rec.branch),
            Ok(false) => "no file changes".into(),
        };
        rec.note = join_note(&rec.note, &note);
        self.commit_linked(rec);
        self.note_overlaps(rec);
    }

    /// Warns, before any merge, of files this run changed that another run working, or done
    /// and not merged yet, changed too in the same repository.
    fn note_overlaps(&self, rec: &mut Run) {
        let Some(st) = self.store.as_deref() else {
            return;
        };
        let mine = super::linked::branches(rec);
        if mine.is_empty() {
            return;
        }
        let Ok(others) = st.list_runs("", 0, 0, "") else {
            return;
        };
        for other in others {
            let open = matches!(other.status.as_str(), STATUS_PENDING | STATUS_RUNNING)
                || (other.status == STATUS_COMPLETED && other.merged_at.is_none());
            if other.id == rec.id || !open {
                continue;
            }
            let shared = super::linked::overlap(rec, &other);
            if shared.is_empty() {
                continue;
            }
            let shown: Vec<&str> = shared.iter().take(5).map(String::as_str).collect();
            let more = shared.len().saturating_sub(shown.len());
            let more = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            rec.note = join_note(
                &rec.note,
                &format!(
                    "run {} changed the same files: {}{more}; expect a conflict when both merge",
                    &other.id[..other.id.len().min(8)],
                    shown.join(", ")
                ),
            );
        }
    }

    /// Sets cooldowns for quota errors and appends the failed attempt. It
    /// reports whether a cooldown was written.
    fn record_attempt_failure(
        &self,
        rec: &mut Run,
        c: &Candidate,
        err: &str,
        attempt_start: DateTime<Utc>,
    ) -> bool {
        let now = self.now();
        let cls = class_of(err);
        let mut cooled = false;
        if let Some(st) = self.store.as_deref() {
            match cls.as_str() {
                "credits" => {
                    let key = credit_key(c.agent(), &candidate_provider(c));
                    let _ = st.set_cooldown(&key, &fmt_time(now + CREDIT_COOLDOWN_FOR));
                    cooled = true;
                }
                "rate_limit" => {
                    // Until the reset the notice names, else an hour. Antigravity keeps a
                    // quota per family of models, so only this provider inside it waits.
                    let wait = super::summary::resets_in(err)
                        .and_then(|d| chrono::TimeDelta::from_std(d).ok())
                        .filter(|d| *d <= super::MAX_COOLDOWN_FOR)
                        .unwrap_or(COOLDOWN_FOR);
                    let key = if c.agent() == "antigravity" {
                        credit_key(c.agent(), &candidate_provider(c))
                    } else {
                        c.agent().to_owned()
                    };
                    let _ = st.set_cooldown(&key, &fmt_time(now + wait));
                    cooled = true;
                }
                _ => {}
            }
        }
        rec.attempts.push(Attempt {
            agent: c.agent().into(),
            model: c.model().into(),
            error: err.into(),
            exit_class: cls,
            duration_ms: Some(non_neg_ms(now - attempt_start)),
            handoff: String::new(),
        });
        cooled
    }

    /// Redacts and persists a terminal run.
    fn finish_run(
        &self,
        rec: Run,
        agent_start: Option<DateTime<Utc>>,
        status: &str,
        m: &str,
    ) -> Result<Run, Error> {
        let mut rec = rec;
        let now = self.now();
        rec.status = status.into();
        rec.error = m.into();
        rec.finished_at = fmt_time(now);
        if rec.run_ms.is_none()
            && let Some(start) = agent_start
        {
            rec.run_ms = Some(non_neg_ms(now - start));
        }
        complete_telemetry(&mut rec, Some(now));
        self.drop_empty_worktree(&mut rec);
        rec.output = sandbox::redact(&rec.output);
        rec.error = sandbox::redact(&rec.error);
        rec.note = sandbox::redact(&rec.note);
        for a in &mut rec.attempts {
            a.error = sandbox::redact(&a.error);
            a.handoff = sandbox::redact(&a.handoff);
        }
        if let Some(st) = self.store.as_deref() {
            st.save_run(&rec)?;
        }
        Ok(rec)
    }

    /// Polls until the run reaches a terminal status or `cancel` fires. The
    /// run is the last one read, also on error.
    pub async fn wait(
        &self,
        cancel: &CancellationToken,
        id: &str,
        poll: Duration,
    ) -> (Run, Result<(), Error>) {
        let Ok(st) = self.store() else {
            return (Run::default(), Err(msg("no dispatch store")));
        };
        loop {
            if let Err(e) = self.reconcile(cancel).await {
                return (st.get_run(id).unwrap_or_default(), Err(e));
            }
            let rec = match st.get_run(id) {
                Ok(r) => r,
                Err(e) => return (Run::default(), Err(e)),
            };
            if terminal_status(&rec.status) {
                return (rec, Ok(()));
            }
            tokio::select! {
                () = cancel.cancelled() => return (rec, Err(msg("context canceled"))),
                () = tokio::time::sleep(poll) => {}
            }
        }
    }
}
