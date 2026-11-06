//! The worktrees runs leave under their repositories' `.dispatch/worktrees`: an empty one
//! goes as its run ends, and `dispatch worktrees --prune` clears what is left.

use super::{Deps, join_note};
use crate::{
    Error,
    types::{Run, STATUS_PENDING, STATUS_RUNNING, WORKTREE_REMOVED},
    worktree,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// Where Dispatch cuts a repository's worktrees, under its root.
pub const WORKTREES_DIR: &str = ".dispatch/worktrees";

/// The branch prefix of Dispatch's worktrees: only these branches are ever deleted.
const BRANCH_PREFIX: &str = "dispatch/";

/// A folder under a repository's worktrees folder, as `dispatch worktrees` lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WorktreeFolder {
    pub path: String,
    pub repo: String,
    pub branch: String,
    /// The run the folder is for (its name).
    pub run_id: String,
    /// The status of the last run that worked in the folder; empty when none exists.
    pub run_status: String,
    /// Uncommitted files, or commits the repository's HEAD does not have.
    pub changes: bool,
    /// Its run ended with nothing to merge, or no longer exists.
    pub prunable: bool,
}

impl Deps {
    /// The folder Dispatch cuts worktrees in for the repository at `root`.
    pub(crate) fn worktrees_base(&self, root: &str) -> String {
        if self.worktree_dir.is_empty() {
            format!("{}/{WORKTREES_DIR}", root.trim_end_matches('/'))
        } else {
            self.worktree_dir.trim_end_matches('/').to_owned()
        }
    }

    /// Whether `path` is a worktree Dispatch cut for the repository at `root`: a folder right
    /// under its worktrees folder. A folder given with `--worktree` never is.
    pub(crate) fn owns_worktree(&self, root: &str, path: &str) -> bool {
        let base = self.worktrees_base(root);
        path.strip_prefix(&base)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|name| !name.is_empty() && !name.contains('/'))
    }

    /// Removes the worktree and branch of a run that ends with nothing to merge, so a run
    /// that changed nothing leaves nothing behind. The run keeps their names, for its page.
    pub(crate) fn drop_empty_worktree(&self, rec: &mut Run) {
        if rec.worktree.is_empty()
            || rec.no_worktree
            || rec.repo.is_empty()
            || !rec.branch.starts_with(BRANCH_PREFIX)
            || !self.owns_worktree(&rec.repo, &rec.worktree)
            || !Path::new(&rec.worktree).exists()
            || !worktree::nothing_to_merge(&rec.repo, &rec.worktree, &rec.base_sha)
        {
            return;
        }
        // Nothing to lose: no uncommitted files, and no commit the repository lacks.
        if worktree::remove(&rec.repo, &rec.worktree, &rec.branch, true).is_err() {
            return;
        }
        rec.note = join_note(
            &rec.note,
            "no changes to merge; its worktree and branch were removed",
        );
        if let Some(st) = self.store.as_deref() {
            let _ = st.save_worktree(&super::ops::worktree_info(rec, WORKTREE_REMOVED));
        }
    }

    /// Every folder under the worktrees folder of a repository runs have used, and of the one
    /// holding `dir`, with its run and whether it has anything to merge.
    pub fn worktree_folders(&self, dir: &str) -> Result<Vec<WorktreeFolder>, Error> {
        let runs = match self.store.as_deref() {
            Some(st) => st.list_runs("", 0, 0, "")?,
            None => Vec::new(),
        };
        // Newest first: a folder's status is that of the last run in it, a follow-up's
        // included.
        let mut by_id: HashMap<&str, &Run> = HashMap::new();
        let mut last_in: HashMap<&str, &Run> = HashMap::new();
        let mut bases: BTreeMap<String, String> = BTreeMap::new();
        for run in &runs {
            by_id.insert(run.id.as_str(), run);
            if run.worktree.is_empty() || run.no_worktree {
                continue;
            }
            last_in.entry(run.worktree.as_str()).or_insert(run);
            if !run.repo.is_empty() {
                bases
                    .entry(self.worktrees_base(&run.repo))
                    .or_insert_with(|| run.repo.clone());
            }
        }
        let root = if dir.trim().is_empty() {
            String::new()
        } else {
            worktree::root(dir)
        };
        if !root.is_empty() {
            bases
                .entry(self.worktrees_base(&root))
                .or_insert_with(|| root.clone());
        }
        let mut out = Vec::new();
        for (base, base_repo) in bases {
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in names {
                let path = format!("{base}/{name}");
                let owner = by_id.get(name.as_str()).copied();
                let last = last_in.get(path.as_str()).copied().or(owner);
                let repo = owner
                    .map(|run| run.repo.clone())
                    .filter(|repo| !repo.is_empty())
                    .unwrap_or_else(|| base_repo.clone());
                out.push(folder(&path, &repo, &name, owner, last));
            }
        }
        Ok(out)
    }

    /// Removes the prunable folders, and the Dispatch branches they were on. It returns what
    /// it removed and what it kept.
    pub fn prune_worktree_folders(
        &self,
        dir: &str,
    ) -> Result<(Vec<WorktreeFolder>, Vec<WorktreeFolder>), Error> {
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        let mut repos = Vec::new();
        for item in self.worktree_folders(dir)? {
            if !item.prunable {
                kept.push(item);
                continue;
            }
            let gone = if item.branch.starts_with(BRANCH_PREFIX) {
                worktree::remove(&item.repo, &item.path, &item.branch, true)
            } else {
                worktree::remove_folder(&item.repo, &item.path)
            };
            if gone.is_err() {
                kept.push(item);
                continue;
            }
            if let Some(st) = self.store.as_deref()
                && let Ok(run) = st.get_run(&item.run_id)
            {
                let _ = st.save_worktree(&super::ops::worktree_info(&run, WORKTREE_REMOVED));
            }
            if !repos.contains(&item.repo) {
                repos.push(item.repo.clone());
            }
            removed.push(item);
        }
        for repo in repos {
            let _ = worktree::git_exit(&repo, &["worktree", "prune"]);
        }
        Ok((removed, kept))
    }
}

/// The folder at `path`, named for the run `owner` (if it still exists), whose last run is
/// `last`. A folder git does not take for a worktree is listed but never pruned.
fn folder(
    path: &str,
    repo: &str,
    name: &str,
    owner: Option<&Run>,
    last: Option<&Run>,
) -> WorktreeFolder {
    let is_worktree = is_worktree_root(path);
    let mut branch = if is_worktree {
        worktree::current_branch(path).unwrap_or_default()
    } else {
        String::new()
    };
    if (branch.is_empty() || branch == "HEAD")
        && let Some(run) = owner
    {
        branch.clone_from(&run.branch);
    }
    let base = owner.map_or("", |run| run.base_sha.as_str());
    let changes = is_worktree && !worktree::nothing_to_merge(repo, path, base);
    let run_status = last.map(|run| run.status.clone()).unwrap_or_default();
    let working = run_status == STATUS_PENDING || run_status == STATUS_RUNNING;
    WorktreeFolder {
        path: path.to_owned(),
        repo: repo.to_owned(),
        branch,
        run_id: name.to_owned(),
        prunable: is_worktree && !working && (owner.is_none() || !changes),
        run_status,
        changes,
    }
}

/// Whether git takes `path` itself for a worktree's top folder, rather than a plain folder
/// inside the repository that holds it.
fn is_worktree_root(path: &str) -> bool {
    let top = worktree::root(path);
    if top.is_empty() {
        return false;
    }
    match (std::fs::canonicalize(&top), std::fs::canonicalize(path)) {
        (Ok(top), Ok(path)) => top == path,
        _ => false,
    }
}
