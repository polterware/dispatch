use super::{Deps, fmt_time, msg};
use crate::{
    Error, sandbox,
    types::{
        EVENT_MERGED, Run, RunEvent, STATUS_COMPLETED, WORKTREE_ABANDONED, WORKTREE_MERGED,
        WORKTREE_REMOVED, WorktreeInfo,
    },
    worktree,
};
use serde::Serialize;

/// The JSON body of a merge. Status is "merged" or "merge_conflict"; ok is
/// true only for the first.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MergeResult {
    pub ok: bool,
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub merged: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub git: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cleaned: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revert: String,
}

pub(crate) fn redact_err(e: Error) -> Error {
    msg(sandbox::redact(&e.to_string()))
}

pub(super) fn worktree_info(rec: &Run, status: &str) -> WorktreeInfo {
    WorktreeInfo {
        id: rec.id.clone(),
        project_id: rec.project_id.clone(),
        run_id: rec.id.clone(),
        card_slug: rec.card_slug.clone(),
        path: rec.worktree.clone(),
        branch: rec.branch.clone(),
        base_sha: rec.base_sha.clone(),
        status: status.into(),
        owner: rec.agent.clone(),
    }
}

impl Deps {
    /// Integrates a COMPLETED run's worktree branch into the repository's
    /// current branch. It checks for conflicts before touching the working
    /// tree. A refusal (wrong status, uncommitted overlap) is an error; a
    /// conflict is a result with ok false.
    pub fn merge(&self, id: &str, keep: bool) -> Result<MergeResult, Error> {
        let st = self.store()?;
        let mut rec = st.get_run(id)?;
        let mut res = MergeResult {
            id: rec.id.clone(),
            ..MergeResult::default()
        };
        if rec.branch.is_empty() || rec.repo.is_empty() {
            return Err(msg(format!(
                "run {} has no worktree branch to merge",
                rec.id
            )));
        }
        if rec.status != STATUS_COMPLETED {
            return Err(msg(format!(
                "run {} is {}; only COMPLETED runs can be merged",
                rec.id, rec.status
            )));
        }
        // Every other repository the run spans merges with it, so each is checked first: one
        // that would conflict stops them all before any merges.
        let mut linked_locks = Vec::with_capacity(rec.linked.len());
        for l in rec.linked.iter().filter(|l| l.merge_sha.is_empty()) {
            let target = worktree::current_branch(&l.repo).unwrap_or_default();
            linked_locks.push(worktree::lock_merge(&l.repo, &target).map_err(redact_err)?);
            let overlapping =
                worktree::uncommitted_overlaps(&l.repo, &l.branch).map_err(redact_err)?;
            if !overlapping.is_empty() {
                return Err(msg(format!(
                    "{} has uncommitted changes in files touched by {}: {}; commit or stash them before merging",
                    l.repo,
                    l.branch,
                    overlapping.join(", ")
                )));
            }
            if worktree::supports_merge_tree(&l.repo) {
                let tree = worktree::merge_tree(&l.repo, "HEAD", &l.branch).map_err(redact_err)?;
                if !tree.clean {
                    rec.exit_class = "merge_conflict".into();
                    st.save_run(&rec)?;
                    res.status = "merge_conflict".into();
                    res.conflicts = tree
                        .conflicts
                        .iter()
                        .map(|c| format!("{}: {c}", l.repo))
                        .collect();
                    res.hint = format!(
                        "ask the agent to rebase branch {} in {}, or resolve the conflicts by hand",
                        l.branch, l.repo
                    );
                    return Ok(res);
                }
            }
        }
        let target = worktree::current_branch(&rec.repo).unwrap_or_default();
        let _lock = worktree::lock_merge(&rec.repo, &target).map_err(redact_err)?;
        let overlapping =
            worktree::uncommitted_overlaps(&rec.repo, &rec.branch).map_err(redact_err)?;
        if !overlapping.is_empty() {
            return Err(msg(format!(
                "working tree has uncommitted changes in files touched by {}: {}; commit or stash them before merging",
                rec.branch,
                overlapping.join(", ")
            )));
        }
        if worktree::supports_merge_tree(&rec.repo) {
            let tree = worktree::merge_tree(&rec.repo, "HEAD", &rec.branch).map_err(redact_err)?;
            if !tree.clean {
                rec.exit_class = "merge_conflict".into();
                st.save_run(&rec)?;
                res.status = "merge_conflict".into();
                res.conflicts = tree.conflicts;
                res.hint = format!(
                    "ask the agent to rebase branch {}, or resolve the conflicts by hand",
                    rec.branch
                );
                return Ok(res);
            }
        } else {
            res.note = "git does not support merge-tree --write-tree; skipped the pre-merge conflict check".into();
        }
        let out = worktree::merge(&rec.repo, &rec.branch, &rec.id).map_err(redact_err)?;
        let merge_sha = worktree::head(&rec.repo).unwrap_or_default();
        res.merged.clone_from(&rec.branch);
        res.git = out;
        if !keep {
            res.cleaned = worktree::remove(&rec.repo, &rec.worktree, &rec.branch, false).is_ok();
        }
        let mut reverts = vec![format!("git revert -m 1 {merge_sha}")];
        for l in rec.linked.iter_mut().filter(|l| l.merge_sha.is_empty()) {
            let out = worktree::merge(&l.repo, &l.branch, &rec.id).map_err(redact_err)?;
            l.merge_sha = worktree::head(&l.repo).unwrap_or_default();
            res.git = format!("{}\n{}: {out}", res.git, l.repo);
            reverts.push(format!("git -C {} revert -m 1 {}", l.repo, l.merge_sha));
            if !keep {
                let _ = worktree::remove(&l.repo, l.worktree_top(), &l.branch, false);
            }
        }
        drop(linked_locks);
        res.ok = true;
        res.status = "merged".into();
        res.revert = reverts.join(" && ");
        let now = fmt_time(self.now());
        rec.merged_at = Some(now.clone());
        rec.merge_sha = merge_sha;
        st.save_run(&rec)?;
        let _ = st.append_event(&RunEvent {
            run_id: rec.id.clone(),
            project_id: rec.project_id.clone(),
            kind: EVENT_MERGED.into(),
            agent: rec.agent.clone(),
            tier: rec.tier.clone(),
            text: rec.branch.clone(),
            at: now,
            ..RunEvent::default()
        });
        let status = if res.cleaned {
            WORKTREE_REMOVED
        } else {
            WORKTREE_MERGED
        };
        let _ = st.save_worktree(&worktree_info(&rec, status));
        Ok(res)
    }

    /// Discards a run's worktree and branch. Without force it refuses a
    /// branch that is not merged.
    pub fn clean(&self, id: &str, force: bool) -> Result<Run, Error> {
        let st = self.store()?;
        let rec = st.get_run(id)?;
        if rec.worktree.is_empty() {
            return Err(msg(format!("run {} has no worktree", rec.id)));
        }
        if std::path::Path::new(&rec.worktree).exists() {
            worktree::remove(&rec.repo, &rec.worktree, &rec.branch, force).map_err(redact_err)?;
        } else {
            // `dispatch merge` removes the worktree it merges, so its folder may be gone.
            worktree::forget(&rec.repo, &rec.branch, force).map_err(redact_err)?;
        }
        for l in &rec.linked {
            let top = l.worktree_top();
            let gone = if std::path::Path::new(&top).exists() {
                worktree::remove(&l.repo, &top, &l.branch, force)
            } else {
                worktree::forget(&l.repo, &l.branch, force)
            };
            gone.map_err(|e| msg(format!("{}: {}", l.repo, redact_err(e))))?;
        }
        let status = if rec.merged_at.is_some() {
            WORKTREE_REMOVED
        } else {
            WORKTREE_ABANDONED
        };
        let _ = st.save_worktree(&worktree_info(&rec, status));
        Ok(rec)
    }
}
