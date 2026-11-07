//! DispatchService: runs, their events and output, worktrees, quota, tiers,
//! agents, and stats.

use super::{
    Core, RpcError, RpcResult, convert,
    event_pages::{page_before, page_bytes},
    is_terminal, lock, now_rfc3339,
    workspace::{HOST_TIMEOUT, expand_path, run_area},
};
use crate::{
    Error,
    adapters::{
        Model, known,
        model::{base_model, provider_of},
    },
    proto as pb,
    reports::{self, stats::StatsOptions},
    runner::{self, AgentAdapter, Options, dispatch_run_task, empirical_quota, page, summary},
    tiers::{Entry, Table},
    types::{
        self, Agent, Caller, EVENT_FAILED, EVENT_MERGED, EVENT_OUTPUT, EVENT_STARTED, Filters,
        QuotaSnapshot, Run, RunEvent, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_FAILED,
        STATUS_ORPHANED, STATUS_PENDING, STATUS_RUNNING, STATUS_TIMED_OUT, Store as _,
        WORKTREE_ABANDONED, WORKTREE_ACTIVE, WORKTREE_READY, WorktreeInfo,
    },
    worktree,
};
use chrono::{DateTime, Utc};
use futures::Stream;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const WATCH_TICK: Duration = Duration::from_millis(150);

/// How long a probe of the agent CLIs' usage stays fresh. Each probe runs
/// every installed CLI's usage command, seconds in all, so Runs, Usage, and
/// Models share one.
const AGENT_PROBE_GAP: Duration = Duration::from_secs(60);

pub(crate) fn store_error(e: Error) -> RpcError {
    if types::is_unknown_run(&e) {
        RpcError::not_found(e.to_string())
    } else {
        RpcError::internal(e)
    }
}

fn required(id: &str, what: &str) -> RpcResult<String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(RpcError::invalid(format!("{what} is required")));
    }
    Ok(id.into())
}

pub(crate) fn installed(a: &dyn AgentAdapter) -> bool {
    which(&a.binary())
}

/// Whether `bin` resolves to an executable, like exec.LookPath.
pub(crate) fn which(bin: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if bin.contains('/') {
        return executable(Path::new(bin));
    }
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| executable(&Path::new(if d.is_empty() { "." } else { d }).join(bin)))
}

/// What the refusal of a second run editing a folder in place offers, word for word: the
/// app finds it to offer New worktree in one click.
/// How often a run waiting for a folder looks whether it is free.
const QUEUE_POLL: Duration = Duration::from_secs(1);

/// The note a run waiting in line carries, naming the run in front of it.
const WAITING_PREFIX: &str = "waiting for run ";

fn waiting_note(ahead: &str, dir: &str) -> String {
    format!(
        "{WAITING_PREFIX}{} to finish editing {dir} itself",
        short_run(ahead)
    )
}

/// `note` without the waiting part, once the run leaves the line.
fn drop_waiting(note: &str) -> String {
    note.split("; ")
        .filter(|part| !part.starts_with(WAITING_PREFIX))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `StartRunRequest.worktree_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeMode {
    /// A worktree wherever the folder is a git checkout with commits: callers that
    /// predate the field.
    Auto,
    Worktree,
    InPlace,
}

pub(crate) fn worktree_mode(value: &str) -> RpcResult<WorktreeMode> {
    match value.trim() {
        "" | "auto" => Ok(WorktreeMode::Auto),
        "worktree" => Ok(WorktreeMode::Worktree),
        "in_place" | "in-place" => Ok(WorktreeMode::InPlace),
        other => Err(RpcError::invalid(format!(
            "worktree mode must be worktree, in-place, or auto, not {other:?}"
        ))),
    }
}

/// Whether a run in `dir` can get a worktree: the folder is in a git checkout with commits.
pub(crate) fn can_branch(dir: &str) -> bool {
    let root = worktree::root(dir);
    !root.is_empty() && worktree::has_commits(&root)
}

/// Whether a run edits `dir` itself rather than a worktree of its own: a writable run given a
/// folder to work in, or one in a folder that is not a git checkout with commits. Every other
/// run reads only, or gets its own worktree, so any number of them share a folder.
pub(crate) fn edits_in_place(
    read_only: bool,
    no_worktree: bool,
    worktree: &str,
    dir: &str,
) -> bool {
    if read_only {
        return false;
    }
    if no_worktree {
        return true;
    }
    if !worktree.is_empty() {
        return false;
    }
    let root = worktree::root(dir);
    root.is_empty() || !worktree::has_commits(&root)
}

pub(crate) fn same_dir(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    absolute(a) == absolute(b)
}

/// A run id as `dispatch runs` prints it.
fn short_run(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

pub(crate) fn absolute(p: &str) -> String {
    let joined = if p.starts_with('/') {
        p.to_owned()
    } else {
        std::env::current_dir()
            .map(|d| format!("{}/{p}", d.to_string_lossy()))
            .unwrap_or_else(|_| p.to_owned())
    };
    runner_clean(&joined)
}

fn runner_clean(p: &str) -> String {
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let j = parts.join("/");
    match (rooted, j.is_empty()) {
        (true, _) => format!("/{j}"),
        (false, true) => ".".into(),
        (false, false) => j,
    }
}

pub(crate) fn parse_quota_since(raw: &str, now: DateTime<Utc>) -> RpcResult<Option<DateTime<Utc>>> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(Some(t.with_timezone(&Utc)));
    }
    reports::stats::parse_since(s, now).map_err(|e| RpcError::invalid(e.to_string()))
}

fn since_text(t: Option<DateTime<Utc>>) -> String {
    t.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .unwrap_or_default()
}

/// Saves the run's worktree with the status its run's end gives it.
pub(crate) fn save_run_worktree(store: &dyn types::Store, run: &Run) -> Result<(), Error> {
    // A folder already gone keeps the status its removal wrote (an empty run's worktree
    // goes as the run ends).
    if run.worktree.is_empty() || !Path::new(&run.worktree).exists() {
        return Ok(());
    }
    let status = match run.status.as_str() {
        STATUS_COMPLETED => WORKTREE_READY,
        STATUS_FAILED | STATUS_CANCELLED | STATUS_TIMED_OUT | STATUS_ORPHANED => WORKTREE_ABANDONED,
        _ => WORKTREE_ACTIVE,
    };
    store.save_worktree(&WorktreeInfo {
        id: run.id.clone(),
        project_id: run.project_id.clone(),
        run_id: run.id.clone(),
        card_slug: run.card_slug.clone(),
        path: run.worktree.clone(),
        branch: run.branch.clone(),
        base_sha: run.base_sha.clone(),
        status: status.into(),
        owner: run.agent.clone(),
    })
}

fn event(run: &Run, kind: &str, text: &str) -> RunEvent {
    RunEvent {
        run_id: run.id.clone(),
        project_id: run.project_id.clone(),
        kind: kind.into(),
        agent: run.agent.clone(),
        tier: run.tier.clone(),
        text: text.into(),
        at: now_rfc3339(),
        ..RunEvent::default()
    }
}

/// Drops a trailing incomplete UTF-8 sequence so a chunk never splits a
/// character. Invalid but complete bytes are kept.
fn complete_char_prefix(b: &[u8]) -> &[u8] {
    match std::str::from_utf8(b) {
        Ok(_) => b,
        Err(e) if e.error_len().is_none() => &b[..e.valid_up_to()],
        Err(_) => {
            // An invalid sequence earlier on: only trim an incomplete tail.
            for n in 1..=3.min(b.len()) {
                let tail = &b[b.len() - n..];
                let lead = tail[0];
                let need = match lead {
                    0xC0..=0xDF => 2,
                    0xE0..=0xEF => 3,
                    0xF0..=0xF7 => 4,
                    _ => continue,
                };
                if need > n {
                    return &b[..b.len() - n];
                }
                break;
            }
            b
        }
    }
}

impl Core {
    /// The run `raw` names: its id, or the one run whose id starts with it (the CLI
    /// prints a run's first 12 characters). An unknown id is left for the lookup to refuse.
    pub(crate) fn full_run_id(&self, raw: &str) -> RpcResult<String> {
        let id = required(raw, "run id")?;
        if id.len() < 6 || id.len() >= 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(id);
        }
        let ids = self
            .store
            .run_ids_with_prefix(&id, 2)
            .map_err(RpcError::internal)?;
        match ids.as_slice() {
            [one] => Ok(one.clone()),
            [] => Ok(id),
            _ => Err(RpcError::invalid(format!(
                "run id {id} matches more than one run: give more of it"
            ))),
        }
    }

    pub(crate) async fn list_runs(
        &self,
        m: pb::ListRunsRequest,
    ) -> RpcResult<pb::ListRunsResponse> {
        let project = m.project_id.trim();
        let conversation = m.conversation_id.trim();
        // A conversation may start runs in several workspaces, so it lists across them.
        if project.is_empty() && conversation.is_empty() {
            return Err(RpcError::invalid("project id is required"));
        }
        let runs = self
            .store
            .list_runs(
                project,
                i64::from(m.limit),
                i64::from(m.offset),
                conversation,
            )
            .map_err(RpcError::internal)?;
        Ok(pb::ListRunsResponse {
            runs: runs.iter().map(convert::run).collect(),
        })
    }

    pub(crate) async fn get_run(&self, m: pb::GetRunRequest) -> RpcResult<pb::GetRunResponse> {
        let id = self.full_run_id(&m.id)?;
        let run = self.store.get_run(&id).map_err(store_error)?;
        Ok(pb::GetRunResponse {
            run: Some(convert::run(&run)),
        })
    }

    pub(crate) async fn start_run(
        self: &Arc<Self>,
        m: pb::StartRunRequest,
    ) -> RpcResult<pb::StartRunResponse> {
        if !m.resume.trim().is_empty() {
            return self.start_follow_up(m).await;
        }
        let project_id = required(&m.project_id, "project id")?;
        let tier = m.tier.trim().to_owned();
        if !tier.is_empty() && tier != runner::TIER_AUTO && !crate::adapters::valid_tier(&tier) {
            return Err(RpcError::invalid(
                "tier must be auto, easy, normal, or hard",
            ));
        }
        let prompt = m.prompt.trim().to_owned();
        if prompt.is_empty() {
            return Err(RpcError::invalid("prompt is required"));
        }
        let ws = self
            .workspace(&project_id)
            .await
            .map_err(|e| RpcError::invalid(e.to_string()))?;

        // The run directory: the request's dir, else (in a Polterware project)
        // its repository, else the card's component repository, else the
        // project's only repository; a standalone workspace runs in its folder.
        let mut dir = expand_path(&m.dir);
        let req_worktree = m.worktree.trim().to_owned();
        let mode = worktree_mode(&m.worktree_mode)?;
        if mode == WorktreeMode::Worktree && !req_worktree.is_empty() {
            return Err(RpcError::invalid(
                "--worktree runs in that folder itself: it cannot also get a new worktree",
            ));
        }
        let no_worktree = !req_worktree.is_empty() || mode == WorktreeMode::InPlace;
        if !req_worktree.is_empty() {
            dir = expand_path(&req_worktree);
        }
        let card_slug = m.card_slug.trim().to_owned();
        let hosted = self
            .host_run(&ws, &card_slug, &m.repo, &dir)
            .await
            .map_err(|e| RpcError::invalid(e.to_string()))?;
        let dir = absolute(&hosted.dir);
        if mode == WorktreeMode::Worktree && !m.read_only && !can_branch(&dir) {
            return Err(RpcError::invalid(format!(
                "{dir} is not a git checkout with commits, so a run there cannot get a worktree: run it in the folder itself (--worktree-mode in-place)"
            )));
        }
        let deps = self.deps_in(&dir).map_err(RpcError::internal)?;

        let start = self.start_mu.lock().await;
        let caller = m.caller.unwrap_or_default();
        let mut opts = Options {
            caller: Caller {
                agent: caller.agent,
                session_id: caller.session_id,
                conversation_id: caller.conversation_id,
                label: caller.label,
            },
            project_id: project_id.clone(),
            area: run_area(&hosted),
            card_slug: card_slug.clone(),
            card_text: hosted.card_text.clone(),
            tier,
            prompt: prompt.clone(),
            dir: dir.clone(),
            with: m.with.iter().map(|d| absolute(d.trim())).collect(),
            read_only: m.read_only,
            no_worktree,
            timeout: Duration::from_secs(u64::try_from(m.timeout_sec).unwrap_or(0)),
            filters: Filters {
                agent: m.agent.trim().into(),
                provider: m.provider.trim().into(),
                model: m.model.trim().into(),
                effort: m.effort.trim().into(),
                priority: m.priority.trim().into(),
                relax: m.relax,
            },
            ..Options::default()
        };
        let profile = m.profile.trim();
        if !profile.is_empty() {
            self.apply_profile(&mut opts, profile)
                .map_err(|e| RpcError::invalid(e.to_string()))?;
        }
        // Runs share a folder freely; only two editing the same folder itself would clash, so
        // a run that would edit a folder another run is editing waits in line for it.
        let busy = if edits_in_place(opts.read_only, no_worktree, "", &dir) {
            self.folder_writer(&dir, None).map_err(RpcError::internal)?
        } else {
            None
        };
        let begun = deps.begin(opts).await;
        drop(start);
        let run = begun.map_err(|e| RpcError::invalid(e.to_string()))?;
        match busy {
            Some(busy) => self.queue_run(ws, deps, run, prompt, req_worktree, dir, &busy),
            None => {
                self.launch_run(&ws, deps, run, &prompt, &req_worktree, &dir)
                    .await
            }
        }
    }

    /// The run editing `dir` itself, with no worktree of its own, that `run` waits behind:
    /// any such run still pending or working, in any workspace, started before `run` (every
    /// one, with no `run`).
    fn folder_writer(&self, dir: &str, run: Option<&Run>) -> Result<Option<Run>, Error> {
        Ok(self.store.list_runs("", 0, 0, "")?.into_iter().find(|r| {
            (r.status == STATUS_PENDING || r.status == STATUS_RUNNING)
                && edits_in_place(r.read_only, r.no_worktree, &r.worktree, &r.dir)
                && same_dir(&r.dir, dir)
                && run.is_none_or(|run| {
                    r.id != run.id
                        && (r.created_at.as_str(), r.id.as_str())
                            < (run.created_at.as_str(), run.id.as_str())
                })
        }))
    }

    /// Records `run` as waiting behind `busy` and starts it once no run started before it is
    /// editing `dir` itself. Cancel ends the wait; a core that goes away leaves it orphaned.
    #[allow(clippy::too_many_arguments)]
    fn queue_run(
        self: &Arc<Self>,
        ws: crate::store::Workspace,
        deps: runner::Deps,
        run: Run,
        prompt: String,
        req_worktree: String,
        dir: String,
        busy: &Run,
    ) -> RpcResult<pb::StartRunResponse> {
        let mut run = run;
        let waiting = waiting_note(&busy.id, &dir);
        run.note = runner::join_note(&run.note, &waiting);
        self.store.save_run(&run).map_err(RpcError::internal)?;
        let wait = CancellationToken::new();
        self.put_cancel(&run.id, wait.clone());
        let core = self.clone();
        let queued = run.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = wait.cancelled() => return,
                    () = tokio::time::sleep(QUEUE_POLL) => {}
                }
                let Ok(current) = core.store.get_run(&queued.id) else {
                    return;
                };
                if is_terminal(&current.status) {
                    return;
                }
                match core.folder_writer(&dir, Some(&current)) {
                    Ok(None) => break,
                    Ok(Some(ahead)) => {
                        // The run in front can change as the line moves; the note follows it.
                        let note = waiting_note(&ahead.id, &dir);
                        if !current.note.contains(&note) {
                            let mut current = current;
                            current.note = runner::join_note(&drop_waiting(&current.note), &note);
                            let _ = core.store.save_run(&current);
                        }
                    }
                    Err(_) => {}
                }
            }
            let Ok(mut current) = core.store.get_run(&queued.id) else {
                return;
            };
            if is_terminal(&current.status) || wait.is_cancelled() {
                return;
            }
            core.pop_cancel(&current.id);
            current.note = drop_waiting(&current.note);
            let _ = core.store.save_run(&current);
            let _ = core
                .launch_run(&ws, deps, current, &prompt, &req_worktree, &dir)
                .await;
        });
        Ok(pb::StartRunResponse {
            run: Some(convert::run(&run)),
        })
    }

    /// A follow-up: `m.prompt` goes to the session of the ended run `m.resume`, as a new
    /// run in that run's workspace, directory, and worktree, with its agent and model. The
    /// calling conversation is the request's, else the earlier run's.
    async fn start_follow_up(
        self: &Arc<Self>,
        m: pb::StartRunRequest,
    ) -> RpcResult<pb::StartRunResponse> {
        let prompt = m.prompt.trim().to_owned();
        if prompt.is_empty() {
            return Err(RpcError::invalid("prompt is required"));
        }
        let id = self.full_run_id(&m.resume)?;
        let prev = self.store.get_run(&id).map_err(store_error)?;
        if !is_terminal(&prev.status) {
            return Err(RpcError::invalid(format!(
                "run {} is still {}: wait for it to end before sending it a message",
                short_run(&prev.id),
                prev.status
            )));
        }
        if prev.session_id.is_empty() || prev.agent.is_empty() {
            return Err(RpcError::invalid(format!(
                "run {} has no agent session to send a message to",
                short_run(&prev.id)
            )));
        }
        let disabled = self.disabled_models();
        let reason =
            crate::config::disabled_reason(&disabled, &prev.provider, &prev.agent, &prev.model);
        if !reason.is_empty() {
            return Err(RpcError::invalid(reason));
        }
        let reason = crate::config::disabled_agent_reason(&self.disabled_agents(), &prev.agent);
        if !reason.is_empty() {
            return Err(RpcError::invalid(reason));
        }
        let project_id = if m.project_id.trim().is_empty() {
            prev.project_id.clone()
        } else {
            m.project_id.trim().to_owned()
        };
        let ws = self
            .workspace(&project_id)
            .await
            .map_err(|e| RpcError::invalid(e.to_string()))?;
        let dir = absolute(&prev.dir);
        let deps = self.deps_in(&dir).map_err(RpcError::internal)?;
        let start = self.start_mu.lock().await;
        let active = self
            .store
            .list_runs(&project_id, 0, 0, "")
            .map_err(RpcError::internal)?
            .into_iter()
            .find(|r| {
                (r.status == STATUS_PENDING || r.status == STATUS_RUNNING)
                    && (r.resumed_of == prev.id
                        || (!prev.worktree.is_empty() && same_dir(&r.worktree, &prev.worktree)))
            });
        if let Some(active) = active {
            let reason = if active.resumed_of == prev.id {
                "on a message to this run".to_owned()
            } else {
                format!("in {}", prev.worktree)
            };
            return Err(RpcError::invalid(format!(
                "run {} is still working {reason}: send the message once it ends",
                short_run(&active.id)
            )));
        }
        let caller = m.caller.unwrap_or_default();
        let (conversation_id, agent) = if caller.conversation_id.trim().is_empty() {
            (prev.conversation_id.clone(), prev.caller_agent.clone())
        } else {
            (caller.conversation_id, caller.agent)
        };
        let opts = Options {
            caller: Caller {
                agent,
                session_id: caller.session_id,
                conversation_id,
                label: caller.label,
            },
            project_id,
            card_slug: prev.card_slug.clone(),
            prompt: prompt.clone(),
            dir: dir.clone(),
            read_only: prev.read_only,
            no_worktree: prev.no_worktree,
            timeout: Duration::from_secs(u64::try_from(prev.timeout_sec).unwrap_or(0)),
            resume: prev.id.clone(),
            profile: prev.profile.clone(),
            ..Options::default()
        };
        let begun = deps.begin(opts).await;
        drop(start);
        let run = begun.map_err(|e| RpcError::invalid(e.to_string()))?;
        self.launch_run(&ws, deps, run, &prompt, "", &dir).await
    }

    /// Records a begun run's start and runs it in the background.
    async fn launch_run(
        self: &Arc<Self>,
        ws: &crate::store::Workspace,
        deps: runner::Deps,
        run: Run,
        prompt: &str,
        req_worktree: &str,
        dir: &str,
    ) -> RpcResult<pb::StartRunResponse> {
        let mut run = run;
        // A delegation is also an audited Run: the agent it launches writes
        // under this AgentRun, so Activity shows one history for both.
        if !ws.polterware_project_id.is_empty() {
            let task = dispatch_run_task(prompt);
            let started = tokio::time::timeout(
                HOST_TIMEOUT,
                self.host
                    .start_agent_run(&ws.polterware_project_id, &task, &run.card_slug),
            )
            .await;
            if let Ok(Ok(id)) = started
                && !id.is_empty()
            {
                run.agent_run_id = id;
                let _ = self.store.save_run(&run);
            }
        }

        if !req_worktree.is_empty() {
            dir.clone_into(&mut run.worktree);
            let root = worktree::root(dir);
            if !root.is_empty() {
                run.repo = root;
                if let Ok(br) = worktree::current_branch(dir)
                    && br != "HEAD"
                {
                    run.branch = br;
                }
                if let Ok(head) = worktree::head(dir) {
                    run.base_sha = head;
                }
            }
            self.store.save_run(&run).map_err(RpcError::internal)?;
            let _ = save_run_worktree(self.store.as_ref(), &run);
        }

        let started = event(&run, EVENT_STARTED, &run.prompt);
        self.store
            .append_event(&started)
            .map_err(RpcError::internal)?;

        let exec_cancel = CancellationToken::new();
        self.put_cancel(&run.id, exec_cancel.clone());
        let core = self.clone();
        let initial = run.clone();
        tokio::spawn(async move {
            let mu = core.run_mu(&initial.id);
            let guard = mu.lock().await;
            let current = core.store.get_run(&initial.id).unwrap_or(initial.clone());
            let final_run = if is_terminal(&current.status) {
                current
            } else {
                deps.execute(&exec_cancel, current.clone())
                    .await
                    .unwrap_or_else(|_| core.store.get_run(&initial.id).unwrap_or(current))
            };
            core.finalize(&final_run).await;
            drop(guard);
            core.pop_cancel(&initial.id);
            exec_cancel.cancel();
        });

        Ok(pb::StartRunResponse {
            run: Some(convert::run(&run)),
        })
    }

    /// Stops the agent tree and a runner that is not this process. The core's
    /// own pid is never signalled; in-process runs end through their token.
    async fn stop_foreign_procs(&self, run: &Run) -> Result<(), Error> {
        let mut owner = run.pid;
        if owner == i64::from(std::process::id()) {
            owner = 0;
        }
        if run.agent_pid <= 0 && owner <= 0 {
            return Ok(());
        }
        let mut tree =
            crate::process::Tree::capture(i32::try_from(run.agent_pid).unwrap_or(0)).await?;
        tree.include(i32::try_from(owner).unwrap_or(0), &run.proc_start)?;
        tree.stop(&CancellationToken::new(), Duration::from_secs(5))
            .await
    }

    fn cancelled(run: &mut Run) {
        run.status = STATUS_CANCELLED.into();
        if run.error.is_empty() {
            run.error = "cancelled".into();
        }
        run.finished_at = now_rfc3339();
    }

    pub(crate) async fn cancel_run(
        &self,
        m: pb::CancelRunRequest,
    ) -> RpcResult<pb::CancelRunResponse> {
        let id = self.full_run_id(&m.id)?;
        let had_cancel = match self.pop_cancel(&id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        };
        let mut run = self.store.get_run(&id).map_err(store_error)?;
        let self_owned = had_cancel || run.pid == i64::from(std::process::id());
        if !self_owned {
            let deps = self.deps().map_err(RpcError::internal)?;
            run = deps
                .cancel(&CancellationToken::new(), &id)
                .await
                .map_err(|e| {
                    if types::is_unknown_run(&e) {
                        RpcError::not_found(e.to_string())
                    } else {
                        RpcError::invalid(e.to_string())
                    }
                })?;
            if !is_terminal(&run.status) {
                Self::cancelled(&mut run);
                self.store.save_run(&run).map_err(RpcError::internal)?;
            }
            self.finalize(&run).await;
            return Ok(pb::CancelRunResponse {
                run: Some(convert::run(&run)),
            });
        }
        let already = is_terminal(&run.status);
        if already && !had_cancel {
            return Err(RpcError::invalid(format!(
                "run {} already {}",
                run.id, run.status
            )));
        }
        if !already {
            self.stop_foreign_procs(&run)
                .await
                .map_err(|e| RpcError::invalid(e.to_string()))?;
        }
        let mu = self.run_mu(&id);
        let _guard = mu.lock().await;
        if let Ok(latest) = self.store.get_run(&id) {
            run = latest;
        }
        if !is_terminal(&run.status) {
            Self::cancelled(&mut run);
            self.store.save_run(&run).map_err(RpcError::internal)?;
        } else if run.status != STATUS_CANCELLED {
            return Err(RpcError::invalid(format!(
                "run {} already {}",
                run.id, run.status
            )));
        }
        self.finalize(&run).await;
        Ok(pb::CancelRunResponse {
            run: Some(convert::run(&run)),
        })
    }

    pub(crate) async fn result(&self, m: pb::ResultRequest) -> RpcResult<pb::ResultResponse> {
        let id = self.full_run_id(&m.id)?;
        let run = self.store.get_run(&id).map_err(store_error)?;
        let section = match m.section.trim() {
            "" => "summary",
            s => s,
        };
        let raw = match section {
            "summary" => {
                let max = if m.limit <= 0 { 40 } else { m.limit as usize };
                summary(&run.output, max, 4096).0
            }
            "files" => serde_json::json!({
                "files_changed": run.files_changed,
                "insertions": run.insertions,
                "deletions": run.deletions,
            })
            .to_string(),
            "attempts" => serde_json::to_string(&run.attempts).map_err(RpcError::internal)?,
            "meta" => convert::run_meta(&run).to_string(),
            "output" | "agent" => run.output.clone(),
            other => return Err(RpcError::invalid(format!("unknown section {other}"))),
        };
        let (content, total, _) = page(&raw, i64::from(m.offset), i64::from(m.limit));
        Ok(pb::ResultResponse {
            content,
            total: i32::try_from(total).unwrap_or(i32::MAX),
        })
    }

    /// The page of the run's event text that ends at `before` (an event's seq and a byte
    /// offset into its text), for a client that opened the run at its end and scrolls back.
    pub(crate) async fn read_run_events(
        &self,
        m: pb::ReadRunEventsRequest,
    ) -> RpcResult<pb::ReadRunEventsResponse> {
        let id = self.full_run_id(&m.id)?;
        let run = self.store.get_run(&id).map_err(store_error)?;
        // A read at the run's end is a snapshot of its log so far, a live run included: what
        // its agent printed since the last reader becomes an output event first, as watching
        // it does, so a reader that never watches still sees the text.
        if m.before_seq <= 0 {
            let log = self
                .state_dir()
                .join("logs")
                .join(format!("{id}.agent.log"));
            self.sync_agent_log(&run, &log)
                .map_err(RpcError::internal)?;
        }
        let before =
            (m.before_seq > 0).then(|| (m.before_seq, u64::try_from(m.before_offset).unwrap_or(0)));
        let (events, more, _) = self.event_page(&id, before, page_bytes(m.max_bytes))?;
        Ok(pb::ReadRunEventsResponse { events, more })
    }

    /// The events of the page that ends at `before`, cut to it, whether text remains before
    /// it, and the run's newest seq.
    fn event_page(
        &self,
        id: &str,
        before: Option<(i64, u64)>,
        max_bytes: u64,
    ) -> RpcResult<(Vec<pb::WatchRunResponse>, bool, i64)> {
        let sizes = self.store.event_sizes(id).map_err(RpcError::internal)?;
        let newest = sizes.last().map_or(0, |size| size.seq);
        let (cuts, more) = page_before(&sizes, before, max_bytes);
        let mut events = Vec::with_capacity(cuts.len());
        for cut in cuts {
            if let Some((event, start)) = self
                .store
                .event_slice(id, cut.seq, cut.start, cut.end)
                .map_err(RpcError::internal)?
            {
                events.push(convert::watch_slice(&event, start));
            }
        }
        Ok((events, more, newest))
    }

    /// Streams the run's events from `after_seq`, turning the agent's live log
    /// into output events, until the run ends.
    pub(crate) fn watch_run(
        self: &Arc<Self>,
        m: pb::WatchRunRequest,
    ) -> RpcResult<impl Stream<Item = RpcResult<pb::WatchRunResponse>> + Send + 'static> {
        let id = self.full_run_id(&m.id)?;
        let run = self.store.get_run(&id).map_err(store_error)?;
        let core = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<RpcResult<pb::WatchRunResponse>>(64);
        tokio::spawn(async move {
            let _guard = core.stream_guard();
            let _ = core
                .watch_loop(run, m.after_seq, m.tail_bytes, &tx)
                .await
                .map_err(|e| {
                    let _ = tx.try_send(Err(e));
                });
        });
        Ok(tokio_stream_from(rx))
    }

    async fn watch_loop(
        &self,
        mut run: Run,
        after: i64,
        tail_bytes: i64,
        tx: &tokio::sync::mpsc::Sender<RpcResult<pb::WatchRunResponse>>,
    ) -> RpcResult<()> {
        let id = run.id.clone();
        let mut last = after;
        // Opened at the end: the last page first, then whatever comes after it.
        if after <= 0 && tail_bytes > 0 {
            let (page, _, newest) = self.event_page(&id, None, page_bytes(tail_bytes))?;
            for event in page {
                if tx.send(Ok(event)).await.is_err() {
                    return Ok(());
                }
            }
            last = newest;
        }
        let log = self
            .state_dir()
            .join("logs")
            .join(format!("{id}.agent.log"));
        let send_new = |last: &mut i64| -> RpcResult<Vec<RunEvent>> {
            let events = self
                .store
                .list_events(&id, *last, 0)
                .map_err(RpcError::internal)?;
            for e in &events {
                *last = (*last).max(e.seq);
            }
            Ok(events)
        };
        for e in send_new(&mut last)? {
            if tx.send(Ok(convert::watch_event(&e))).await.is_err() {
                return Ok(());
            }
        }
        let done = self.done();
        loop {
            self.sync_agent_log(&run, &log)
                .map_err(RpcError::internal)?;
            for e in send_new(&mut last)? {
                if tx.send(Ok(convert::watch_event(&e))).await.is_err() {
                    return Ok(());
                }
            }
            run = self.store.get_run(&id).map_err(RpcError::internal)?;
            if is_terminal(&run.status) {
                self.sync_agent_log(&run, &log)
                    .map_err(RpcError::internal)?;
                for e in send_new(&mut last)? {
                    if tx.send(Ok(convert::watch_event(&e))).await.is_err() {
                        return Ok(());
                    }
                }
                return Ok(());
            }
            tokio::select! {
                () = tx.closed() => return Ok(()),
                () = done.cancelled() => return Ok(()),
                () = tokio::time::sleep(WATCH_TICK) => {}
            }
        }
    }

    /// Appends what the agent's live log gained since the last read as an
    /// output event.
    fn sync_agent_log(&self, run: &Run, log: &Path) -> Result<(), Error> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(log) else {
            return Ok(());
        };
        let size = match f.metadata() {
            Ok(m) if m.len() > 0 => m.len(),
            _ => return Ok(()),
        };
        let mut w = lock(&self.watch);
        if !w.log_ready.contains(&run.id) {
            let seeded: u64 = self
                .store
                .list_events(&run.id, 0, 0)?
                .iter()
                .filter(|e| e.kind == EVENT_OUTPUT)
                .map(|e| e.text.len() as u64)
                .sum();
            w.log_off.insert(run.id.clone(), seeded);
            w.log_ready.insert(run.id.clone());
        }
        let off = w.log_off.get(&run.id).copied().unwrap_or(0).min(size);
        if off == size {
            w.log_off.insert(run.id.clone(), off);
            return Ok(());
        }
        f.seek(SeekFrom::Start(off))?;
        let mut buf = Vec::with_capacity(usize::try_from(size - off).unwrap_or(0));
        f.take(size - off).read_to_end(&mut buf)?;
        let emit = complete_char_prefix(&buf);
        if emit.is_empty() {
            return Ok(());
        }
        self.store
            .append_event(&event(run, EVENT_OUTPUT, &String::from_utf8_lossy(emit)))?;
        w.log_off.insert(run.id.clone(), off + emit.len() as u64);
        Ok(())
    }

    /// Records a run's end once: its worktree's status, its failure or merge
    /// event, and the AgentRun it linked.
    pub(crate) async fn finalize(&self, run: &Run) {
        if run.id.is_empty() {
            return;
        }
        if lock(&self.watch).finalized.contains(&run.id) {
            return;
        }
        let mut run = run.clone();
        if let Ok(latest) = self.store.get_run(&run.id)
            && is_terminal(&latest.status)
        {
            run = latest;
        }
        if !run.agent_run_id.is_empty() {
            let _ = tokio::time::timeout(
                HOST_TIMEOUT,
                self.host.complete_agent_run(&run.agent_run_id),
            )
            .await;
        }
        let mut w = lock(&self.watch);
        if !w.finalized.insert(run.id.clone()) {
            return;
        }
        w.log_off.remove(&run.id);
        w.log_ready.remove(&run.id);
        drop(w);
        let events = self.store.list_events(&run.id, 0, 0).unwrap_or_default();
        let _ = save_run_worktree(self.store.as_ref(), &run);
        let has = |kind: &str| events.iter().any(|e| e.kind == kind);
        if has(EVENT_FAILED) {
            return;
        }
        if matches!(
            run.status.as_str(),
            STATUS_FAILED | STATUS_TIMED_OUT | STATUS_CANCELLED
        ) {
            let text = if run.error.is_empty() {
                run.status.to_lowercase()
            } else {
                run.error.clone()
            };
            let _ = self.store.append_event(&event(&run, EVENT_FAILED, &text));
        }
        if run.merged_at.as_deref().is_some_and(|m| !m.is_empty()) && !has(EVENT_MERGED) {
            let _ = self
                .store
                .append_event(&event(&run, EVENT_MERGED, &run.merge_sha));
        }
    }

    pub(crate) async fn list_worktrees(
        &self,
        m: pb::ListWorktreesRequest,
    ) -> RpcResult<pb::ListWorktreesResponse> {
        let project = required(&m.project_id, "project id")?;
        let items = self
            .store
            .list_worktrees(&project)
            .map_err(RpcError::internal)?;
        Ok(pb::ListWorktreesResponse {
            worktrees: items.iter().map(convert::worktree).collect(),
        })
    }

    pub(crate) async fn list_quota(
        &self,
        m: pb::ListQuotaRequest,
    ) -> RpcResult<pb::ListQuotaResponse> {
        let now = Utc::now();
        let since = parse_quota_since(&m.since, now)?;
        let agent = m.agent.trim();
        self.probe_once(agent, now).await;
        let snaps = self
            .store
            .list_quota(agent, &since_text(since))
            .map_err(RpcError::internal)?;
        Ok(pb::ListQuotaResponse {
            snapshots: snaps.iter().map(convert::quota).collect(),
        })
    }

    pub(crate) fn tier_rows(&self) -> RpcResult<Vec<pb::DispatchTier>> {
        let rows = self.store.list_tiers().map_err(RpcError::internal)?;
        let disabled = self.disabled_models();
        Ok(rows
            .iter()
            .filter(|row| {
                !crate::config::is_model_disabled(&disabled, &row.provider, &row.agent, &row.model)
            })
            .map(convert::tier)
            .collect())
    }

    pub(crate) async fn list_tiers(
        &self,
        _: pb::ListTiersRequest,
    ) -> RpcResult<pb::ListTiersResponse> {
        if self
            .store
            .list_tiers()
            .map_err(RpcError::internal)?
            .is_empty()
        {
            let table = Table::embedded().map_err(RpcError::internal)?;
            self.store
                .replace_tiers(&table.rows())
                .map_err(RpcError::internal)?;
        }
        Ok(pb::ListTiersResponse {
            tiers: self.tier_rows()?,
        })
    }

    /// Stores a new tier table. Rows keep their request order within each tier.
    pub(crate) async fn replace_tiers(
        &self,
        m: pb::ReplaceTiersRequest,
    ) -> RpcResult<pb::ReplaceTiersResponse> {
        let mut table = Table::default();
        // The board lists only enabled models, so it never sends the disabled ones: they stay
        // in their tier, after the board's rows, until their model is turned back on.
        let disabled = self.disabled_models();
        let mut kept = self.store.list_tiers().map_err(RpcError::internal)?;
        kept.retain(|row| {
            crate::config::is_model_disabled(&disabled, &row.provider, &row.agent, &row.model)
        });
        kept.sort_by_key(|row| row.position);
        let kept = kept.into_iter().map(|row| pb::DispatchTier {
            name: row.name,
            provider: row.provider,
            agent: row.agent,
            model: row.model,
            ..pb::DispatchTier::default()
        });
        for row in m.tiers.into_iter().chain(kept) {
            let tier = row.name.trim().to_owned();
            if !crate::adapters::valid_tier(&tier) {
                return Err(RpcError::invalid(format!("unknown tier {:?}", row.name)));
            }
            table.tiers.entry(tier).or_default().push(Entry {
                provider: row.provider.trim().into(),
                agent: row.agent.trim().into(),
                model: row.model.trim().into(),
                ..Entry::default()
            });
        }
        let validated = table
            .validated()
            .map_err(|e| RpcError::invalid(e.to_string()))?;
        self.store
            .replace_tiers(&validated.rows())
            .map_err(RpcError::internal)?;
        Ok(pb::ReplaceTiersResponse {
            tiers: self.tier_rows()?,
        })
    }

    /// Asks every installed agent CLI for its models, in parallel and bounded
    /// by a timeout.
    pub(crate) async fn list_models(
        &self,
        _: pb::ListModelsRequest,
    ) -> RpcResult<pb::ListModelsResponse> {
        Ok(self.models_report().await)
    }

    pub(crate) async fn models_report(&self) -> pb::ListModelsResponse {
        let mut out = self.raw_models_report().await;
        let disabled = self.disabled_models();
        out.models.retain(|m| {
            !crate::config::is_model_disabled(&disabled, &m.provider, &m.agent, &m.model)
        });
        out
    }

    pub(crate) async fn raw_models_report(&self) -> pb::ListModelsResponse {
        let listed = futures::future::join_all(
            known::known()
                .into_iter()
                .filter(|a| installed(a.as_ref()))
                .map(|a| async move {
                    let r = tokio::time::timeout(Duration::from_secs(20), self.agent_models(&a))
                        .await
                        .unwrap_or_else(|_| {
                            Err(Error::Message("context deadline exceeded".into()))
                        });
                    (a, r)
                }),
        )
        .await;
        let mut out = pb::ListModelsResponse::default();
        for (a, r) in listed {
            match r {
                Ok(models) => {
                    // An agent that takes its effort as a flag lists it beside each model;
                    // one that names it in the model (antigravity) lists each effort as a model.
                    let caps = a.capabilities();
                    let efforts = if caps.effort_mode == "config" {
                        caps.efforts
                    } else {
                        Vec::new()
                    };
                    out.models
                        .extend(models.into_iter().map(|m| pb::DispatchModel {
                            agent: a.name(),
                            provider: provider_of(&a.name(), &m.id),
                            model: m.id,
                            tier: m.tier,
                            efforts: efforts.clone(),
                        }));
                }
                Err(e) => out.problems.push(format!("{}: {e}", a.name())),
            }
        }
        out
    }

    /// The installed agent CLIs' models as tier entries.
    pub(crate) async fn installed_entries(&self) -> (Vec<Entry>, Vec<String>) {
        let listed = self.models_report().await;
        let entries = listed
            .models
            .iter()
            .map(|m| Entry {
                agent: m.agent.clone(),
                model: m.model.clone(),
                provider: m.provider.clone(),
                ..Entry::default()
            })
            .collect();
        (entries, listed.problems)
    }

    /// The agents as last probed. A probe of their quota asks every installed CLI and can take
    /// seconds, so once there is an answer it runs behind this one, and only the very first
    /// list waits for it.
    pub(crate) async fn list_agents(
        self: &Arc<Self>,
        _: pb::ListAgentsRequest,
    ) -> RpcResult<pb::ListAgentsResponse> {
        let stored = self.store.list_agents().map_err(RpcError::internal)?;
        if stored.is_empty() {
            self.probe_once("", Utc::now()).await;
        } else {
            let core = self.clone();
            tokio::spawn(async move { core.probe_once("", Utc::now()).await });
        }
        let agents = self.store.list_agents().map_err(RpcError::internal)?;
        let installed: HashMap<String, bool> = known::known()
            .iter()
            .map(|a| (a.name(), installed(a.as_ref())))
            .collect();
        let disabled = self.disabled_agents();
        Ok(pb::ListAgentsResponse {
            agents: agents
                .iter()
                .map(|a| {
                    let mut row = convert::agent(a);
                    row.installed = installed.get(&a.name).copied().unwrap_or(false);
                    row.enabled = !crate::config::is_agent_disabled(&disabled, &a.name);
                    row
                })
                .collect(),
        })
    }

    pub(crate) async fn list_history(
        &self,
        m: pb::ListHistoryRequest,
    ) -> RpcResult<pb::ListHistoryResponse> {
        let events = self
            .store
            .list_history(
                m.project_id.trim(),
                m.agent.trim(),
                m.tier.trim(),
                i64::from(m.limit),
                i64::from(m.offset),
            )
            .map_err(RpcError::internal)?;
        Ok(pb::ListHistoryResponse {
            events: events.iter().map(convert::run_event).collect(),
        })
    }

    pub(crate) async fn check_tiers(
        &self,
        _: pb::CheckTiersRequest,
    ) -> RpcResult<pb::CheckTiersResponse> {
        let table =
            Table::load_from_store(Some(self.store.as_ref())).map_err(RpcError::internal)?;
        let adapters = known::known();
        let by_name: HashMap<String, Arc<dyn AgentAdapter>> =
            adapters.iter().map(|a| (a.name(), a.clone())).collect();
        let mut problems = table.capability_problems(&crate::tiers::known_adapters());

        // Ask the table's installed agents for their models at once, through the shared cache.
        let mut asked: Vec<String> = Vec::new();
        for tier in ["easy", "normal", "hard"] {
            for e in table.tiers.get(tier).into_iter().flatten() {
                if by_name.contains_key(&e.agent) && !asked.contains(&e.agent) {
                    asked.push(e.agent.clone());
                }
            }
        }
        let queried: HashMap<String, Result<Vec<Model>, Error>> = futures::future::join_all(
            asked
                .into_iter()
                .filter_map(|name| Some((by_name.get(&name)?.clone(), name)))
                .filter(|(a, _)| installed(a.as_ref()))
                .map(|(a, name)| async move {
                    let r = self.agent_models(&a).await;
                    (name, r)
                }),
        )
        .await
        .into_iter()
        .collect();
        for tier in ["easy", "normal", "hard"] {
            for e in table.tiers.get(tier).into_iter().flatten() {
                if !by_name.contains_key(&e.agent) {
                    problems.push(format!("{tier}: unknown agent {}", e.agent));
                    continue;
                }
                match queried.get(&e.agent) {
                    None => {}
                    Some(Err(err)) => {
                        problems.push(format!("{tier}: {}/{}: {err}", e.agent, e.model));
                    }
                    Some(Ok(models)) => {
                        let listed = models
                            .iter()
                            .any(|m| m.id == e.model || base_model(&m.id) == base_model(&e.model));
                        if !listed {
                            problems.push(format!(
                                "{tier}: {}/{}: model not listed by installed agent",
                                e.agent, e.model
                            ));
                        }
                    }
                }
            }
        }
        Ok(pb::CheckTiersResponse { problems })
    }

    pub(crate) async fn stats(&self, m: pb::StatsRequest) -> RpcResult<pb::StatsResponse> {
        let now = Utc::now();
        let since = reports::stats::parse_since(&m.since, now)
            .map_err(|e| RpcError::invalid(e.to_string()))?;
        let deps = self.deps().map_err(RpcError::internal)?;
        let (state_dir, catalog) = (deps.state_dir(), deps.catalog());
        let live = |run: &crate::types::Run| {
            runner::usage_with_log(run, &state_dir, &catalog).or_else(|| run.usage.clone())
        };
        let report = reports::stats::from_store_live(
            Some(self.store.as_ref()),
            StatsOptions {
                project_id: m.project_id.trim().into(),
                dir: m.dir.trim().into(),
                by: m.by.trim().into(),
                since,
                now: Some(now),
            },
            &live,
        )
        .map_err(|e| RpcError::invalid(e.to_string()))?;
        Ok(convert::stats(&report))
    }

    pub(crate) async fn capacity(&self, m: pb::CapacityRequest) -> RpcResult<pb::CapacityResponse> {
        let now = Utc::now();
        let since = parse_quota_since(&m.since, now)?;
        let report = reports::stats_capacity::capacity_from_store(
            Some(self.store.as_ref()),
            since,
            Some(now),
        )
        .map_err(RpcError::internal)?;
        Ok(pb::CapacityResponse {
            since: report.since.clone(),
            note: report.note.clone(),
            keys: report
                .keys
                .iter()
                .map(|k| pb::DispatchCapacityKey {
                    key: k.key.clone(),
                    windows: i32::try_from(k.windows).unwrap_or(i32::MAX),
                    hit_full: i32::try_from(k.hit_full).unwrap_or(i32::MAX),
                    leftover: k.leftover.clone(),
                    pace: k.pace.clone(),
                    consumed_pct: k.consumed_pct,
                    leftover_pct: k.leftover_pct,
                })
                .collect(),
        })
    }

    /// Probes at most once per [`AGENT_PROBE_GAP`] for the same filter. Calls
    /// that arrive while a probe runs wait for it and then read what it stored.
    async fn probe_once(&self, agent: &str, now: DateTime<Utc>) {
        let mut last = self.probes.lock().await;
        let fresh = |k: &str| last.get(k).is_some_and(|at| at.elapsed() < AGENT_PROBE_GAP);
        // A full probe covers every agent; one agent's probe covers only that agent.
        if fresh("") || (!agent.is_empty() && fresh(agent)) {
            return;
        }
        self.probe_agents(agent, now).await;
        last.insert(agent.into(), std::time::Instant::now());
    }

    /// Asks every agent's CLI for its quota at once: each answer can take seconds, and the
    /// first list of agents waits for the slowest of them, not for their sum.
    async fn probe_agents(&self, filter: &str, now: DateTime<Utc>) {
        futures::future::join_all(
            known::known()
                .into_iter()
                .filter(|a| filter.is_empty() || a.name() == filter)
                .map(|a| self.probe_agent(a, now)),
        )
        .await;
    }

    async fn probe_agent(&self, a: Arc<dyn AgentAdapter>, now: DateTime<Utc>) {
        let at = rfc3339(now);
        let time = |t: Option<DateTime<Utc>>| t.map(rfc3339).unwrap_or_default();
        let name = a.name();
        let provider = provider_of(&name, "");
        let emp = empirical_quota(Some(self.store.as_ref()), &name, Some(now));
        let mut row = Agent {
            name: name.clone(),
            bin: a.binary(),
            provider: provider.clone(),
            cooldown_until: emp.reset_at.map(|t| time(Some(t))),
            ..Agent::default()
        };
        // source and confidence describe the quota snapshots this probe saves.
        let mut source = emp.source.clone();
        let is_installed = installed(a.as_ref());
        if let Some(src) = a.usage_source() {
            source = src.into();
            if is_installed
                && let Ok(samples) = a.usage().await
                && !samples.is_empty()
            {
                for s in samples {
                    if row.used_percent.is_none() {
                        row.used_percent = s.used;
                    }
                    let _ = self.store.save_quota(&QuotaSnapshot {
                        agent: name.clone(),
                        provider: provider.clone(),
                        window: s.window.clone(),
                        source: s.source.clone(),
                        confidence: s.confidence.clone(),
                        used_percent: s.used,
                        window_minutes: i64::try_from(s.duration.as_secs() / 60).unwrap_or(0),
                        resets_at: time(s.reset_at),
                        sampled_at: s.sampled_at.map_or_else(|| at.clone(), |t| time(Some(t))),
                    });
                }
                let _ = self.store.upsert_agent(&row);
                return;
            }
        }
        if is_installed && let Ok(q) = a.quota().await {
            row.used_percent = Some(q.used_percent);
            let _ = self.store.save_quota(&QuotaSnapshot {
                agent: name.clone(),
                provider: provider.clone(),
                window: "default".into(),
                source: source.clone(),
                confidence: "official".into(),
                used_percent: Some(q.used_percent),
                window_minutes: q.window_minutes,
                resets_at: time(q.resets_at),
                sampled_at: at.clone(),
            });
            let _ = self.store.upsert_agent(&row);
            return;
        }
        let _ = self.store.save_quota(&QuotaSnapshot {
            agent: name.clone(),
            provider,
            window: emp.window.clone(),
            source,
            confidence: emp.confidence.clone(),
            used_percent: None,
            window_minutes: 0,
            resets_at: time(emp.reset_at),
            sampled_at: at,
        });
        let _ = self.store.upsert_agent(&row);
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

/// A stream over a channel's messages.
fn tokio_stream_from<T: Send + 'static>(
    mut rx: tokio::sync::mpsc::Receiver<T>,
) -> impl Stream<Item = T> + Send + 'static {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
