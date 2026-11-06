use super::{Deps, complete_telemetry, fmt_time, join_note, msg, terminal_status};
use crate::{
    Error,
    process::Tree,
    types::{
        Run, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_ORPHANED, STATUS_PENDING, STATUS_RUNNING,
    },
    worktree,
};
use nix::{errno::Errno, sys::signal::kill, unistd::Pid};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const STOP_GRACE: Duration = Duration::from_secs(5);

/// The start time `ps` reports for `pid`, empty when unknown.
pub async fn process_start(pid: i64) -> String {
    let out = tokio::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        _ => String::new(),
    }
}

/// Whether `pid` exists; a negative pid asks about its process group.
pub fn pid_alive(pid: i64) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    if raw == 0 {
        return false;
    }
    matches!(kill(Pid::from_raw(raw), None), Ok(()) | Err(Errno::EPERM))
}

fn to_pid(pid: i64) -> i32 {
    i32::try_from(pid).unwrap_or(0)
}

impl Deps {
    /// Preserves orphaned work so it can be inspected or recovered.
    pub async fn reconcile(&self, cancel: &CancellationToken) -> Result<(), Error> {
        let Some(st) = self.store.as_deref() else {
            return Ok(());
        };
        let recs = st.list_runs("", 0, 0, "")?;
        for rec in recs {
            if rec.status != STATUS_PENDING && rec.status != STATUS_RUNNING {
                continue;
            }
            if cancel.is_cancelled() {
                return Err(msg("context canceled"));
            }
            let mut note = "runner process exited unexpectedly";
            if rec.pid > 0 && pid_alive(rec.pid) {
                // Legacy records have no identity.
                if rec.proc_start.is_empty() {
                    continue;
                }
                let start = process_start(rec.pid).await;
                if cancel.is_cancelled() {
                    return Err(msg("context canceled"));
                }
                if start.is_empty() && pid_alive(rec.pid) {
                    return Err(msg(format!(
                        "cannot verify process identity for run {}",
                        rec.id
                    )));
                }
                if start == rec.proc_start {
                    continue;
                }
                note = "runner process identity changed (PID reused)";
            }
            // Avoid replacing a final record written while ps was running.
            let latest = st.get_run(&rec.id)?;
            if latest.status != rec.status
                || latest.pid != rec.pid
                || latest.proc_start != rec.proc_start
            {
                continue;
            }
            let mut rec = latest;
            if rec.agent_pid > 0 && (pid_alive(rec.agent_pid) || pid_alive(-rec.agent_pid)) {
                let tree = Tree::capture(to_pid(rec.agent_pid)).await?;
                tree.stop(cancel, STOP_GRACE).await?;
            }
            let now = self.now();
            // What the attempt used before the core went away is in its log; keep it.
            if let Some(usage) =
                super::usage::usage_with_log(&rec, &self.state_dir(), &self.catalog())
            {
                rec.usage = Some(usage);
            }
            rec.status = STATUS_ORPHANED.into();
            rec.note = join_note(&rec.note, note);
            rec.finished_at = fmt_time(now);
            rec.exit_code = 1;
            complete_telemetry(&mut rec, Some(now));
            st.save_run(&rec)?;
        }
        Ok(())
    }

    /// Waits for both the owner and the agent group before recording success.
    pub async fn cancel(&self, cancel: &CancellationToken, id: &str) -> Result<Run, Error> {
        self.reconcile(cancel).await?;
        let st = self.store()?;
        let rec = st.get_run(id)?;
        let orphan = rec.status == STATUS_ORPHANED;
        if terminal_status(&rec.status) && !orphan {
            return Err(msg(format!("run {} already {}", rec.id, rec.status)));
        }
        let owner = if orphan { 0 } else { rec.pid };
        if owner <= 0 && rec.agent_pid <= 0 {
            return Ok(rec);
        }
        let mut tree = Tree::capture(to_pid(rec.agent_pid)).await?;
        tree.include(to_pid(owner), &rec.proc_start)?;
        tree.stop(cancel, STOP_GRACE).await?;
        let mut rec = st.get_run(id)?;
        if orphan {
            return Ok(rec);
        }
        let now = self.now();
        rec.status = STATUS_CANCELLED.into();
        rec.error = "cancelled".into();
        rec.finished_at = fmt_time(now);
        complete_telemetry(&mut rec, Some(now));
        st.save_run(&rec)?;
        Ok(rec)
    }

    /// Commits preserved work, without claiming that it was verified.
    pub async fn recover(&self, id: &str) -> Result<(Run, String), Error> {
        let st = self.store()?;
        let mut rec = st.get_run(id)?;
        if rec.status != STATUS_ORPHANED {
            return Err(msg(format!(
                "run {} is {}; recovery requires ORPHANED",
                rec.id, rec.status
            )));
        }
        if rec.worktree.is_empty() {
            return Err(msg(format!("run {} has no worktree", rec.id)));
        }
        if !std::path::Path::new(&rec.worktree).is_dir() {
            return Err(msg(format!("worktree {} does not exist", rec.worktree)));
        }
        if rec.agent_pid > 0 && pid_alive(-rec.agent_pid) {
            return Err(msg(format!(
                "agent group still exists; cancel run {} before recovery",
                rec.id
            )));
        }
        let stat = worktree::recover(&rec.worktree, &rec.branch, &rec.base_sha, &rec.id)?;
        let now = self.now();
        rec.status = STATUS_COMPLETED.into();
        rec.note = "recovered from orphan; unverified".into();
        rec.finished_at = fmt_time(now);
        rec.error.clear();
        rec.exit_code = 0;
        complete_telemetry(&mut rec, Some(now));
        st.save_run(&rec)?;
        Ok((rec, stat))
    }
}
