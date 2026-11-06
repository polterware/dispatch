//! Runs that span more than one repository. Besides its own, a run names the other folders the
//! task touches (`--with`); each gets a worktree of its own on the run's branch name, cut with
//! the run's, committed with it, merged with it as one (every branch is checked for conflicts
//! before any merges), and cleaned with it.
//!
//! How code in one worktree finds another is each ecosystem's business (a Cargo `[patch]`, an
//! npm link, an editable install, a Go `replace`), so Dispatch neither guesses nor edits build
//! files. It tells the agent where the other worktrees are (the prompt and
//! `DISPATCH_LINKED_WORKTREES`), and runs a repository's own link hook,
//! `.dispatch/hooks/link`, in each worktree once all are cut, with the same variables, so a
//! repository that knows how to point its build at its siblings does it there.

use std::path::Path;
use std::process::Command;

use super::{Deps, join_note};
use crate::types::{LinkedRepo, Run};
use crate::worktree;

/// The variable that names every worktree of the run: `name=path` pairs, `:`-separated.
pub const LINKED_ENV: &str = "DISPATCH_LINKED_WORKTREES";
/// A repository's own hook, run in each of the run's worktrees once all are cut.
pub const LINK_HOOK: &str = ".dispatch/hooks/link";

/// A repository's short name: its top folder's.
pub fn repo_name(repo: &str) -> &str {
    repo.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(repo)
}

/// `name=path` for the run's own worktree and each linked one.
pub fn linked_env(rec: &Run) -> String {
    let mut pairs = Vec::with_capacity(rec.linked.len() + 1);
    if !rec.worktree.is_empty() {
        pairs.push(format!("{}={}", repo_name(&rec.repo), rec.worktree));
    }
    for l in rec.linked.iter().filter(|l| !l.worktree.is_empty()) {
        pairs.push(format!("{}={}", repo_name(&l.repo), l.worktree));
    }
    pairs.join(":")
}

/// What the agent is told about the other repositories, after the task.
pub fn prompt_note(rec: &Run) -> String {
    let cut: Vec<&LinkedRepo> = rec
        .linked
        .iter()
        .filter(|l| !l.worktree.is_empty())
        .collect();
    if cut.is_empty() {
        return String::new();
    }
    let mut note = String::from(
        "\n\nThis task also spans other repositories. Each has a worktree of its own for this run; edit them there, never in their own checkouts:\n",
    );
    for l in cut {
        note.push_str(&format!(
            "- {} ({}): {}\n",
            repo_name(&l.repo),
            l.repo,
            l.worktree
        ));
    }
    note
}

impl Deps {
    /// Cuts a worktree in each linked repository on the run's branch name. A folder that is
    /// not a git checkout with commits is left out, with a note.
    pub(crate) fn cut_linked(&self, rec: &mut Run) -> Result<(), String> {
        let id = rec.id.clone();
        let mut notes = Vec::new();
        for l in &mut rec.linked {
            let dir = self.abs(&l.dir);
            let root = worktree::root(&dir);
            if root.is_empty() || !worktree::has_commits(&root) {
                notes.push(format!(
                    "{dir} is not a git checkout with commits; left out"
                ));
                continue;
            }
            if root == rec.repo {
                notes.push(format!("{dir} is in the run's own repository; left out"));
                continue;
            }
            if self.worktree_dir.is_empty() {
                let _ = worktree::exclude(&root, ".dispatch/");
            }
            let path = format!("{}/{id}", self.worktrees_base(&root));
            let branch = worktree::create(&root, &path, &id)
                .map_err(|e| format!("worktree in {root}: {e}"))?;
            l.base_sha = worktree::head(&path).map_err(|e| e.to_string())?;
            l.repo = root.clone();
            l.branch = branch;
            let rel = worktree::rel(&root, &dir);
            l.worktree = super::clean_path(&format!("{path}/{rel}"));
        }
        rec.linked.retain(|l| !l.worktree.is_empty());
        for n in notes {
            rec.note = join_note(&rec.note, &n);
        }
        Ok(())
    }

    /// Runs each worktree's link hook, when its repository has one, with the run's worktrees
    /// in [`LINKED_ENV`]. A hook that fails leaves a note; the run goes on.
    pub(crate) fn run_link_hooks(&self, rec: &mut Run) {
        if rec.linked.is_empty() {
            return;
        }
        let env = linked_env(rec);
        let mut places = vec![(rec.repo.clone(), rec.worktree.clone())];
        places.extend(
            rec.linked
                .iter()
                .map(|l| (l.repo.clone(), l.worktree.clone())),
        );
        for (repo, tree) in places {
            let top = worktree::root(&tree);
            let hook = Path::new(&top).join(LINK_HOOK);
            if !hook.is_file() {
                continue;
            }
            let ran = Command::new(&hook)
                .current_dir(&top)
                .env(LINKED_ENV, &env)
                .env("DISPATCH_RUN_ID", &rec.id)
                .output();
            let note = match ran {
                Ok(out) if out.status.success() => continue,
                Ok(out) => format!(
                    "{} link hook failed: {}",
                    repo_name(&repo),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                Err(e) => format!("{} link hook could not run: {e}", repo_name(&repo)),
            };
            rec.note = join_note(&rec.note, &note);
        }
    }

    /// Commits each linked worktree, as the run's own is committed.
    pub(crate) fn commit_linked(&self, rec: &mut Run) {
        for l in &rec.linked {
            let note = match worktree::commit(&l.worktree, &rec.id) {
                Err(e) => format!("{} commit failed: {e}", repo_name(&l.repo)),
                Ok(true) => format!("{} changes committed on {}", repo_name(&l.repo), l.branch),
                Ok(false) => continue,
            };
            rec.note = join_note(&rec.note, &note);
        }
    }
}

/// The files `branch` changed from `base` in `repo`.
pub fn changed_files(repo: &str, base: &str, branch: &str) -> Vec<String> {
    if repo.is_empty() || base.is_empty() || branch.is_empty() {
        return Vec::new();
    }
    match worktree::git_exit(repo, &["diff", "--name-only", &format!("{base}..{branch}")]) {
        Ok((out, 0)) => out
            .lines()
            .map(str::to_owned)
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Every repository and branch a run changed: its own and each linked one's.
pub fn branches(rec: &Run) -> Vec<(String, String, String)> {
    let mut all = Vec::new();
    if !rec.branch.is_empty() {
        all.push((rec.repo.clone(), rec.base_sha.clone(), rec.branch.clone()));
    }
    for l in &rec.linked {
        all.push((l.repo.clone(), l.base_sha.clone(), l.branch.clone()));
    }
    all
}

/// The files `rec` changed that `other` changed too, by repository, for the note that warns
/// before a merge.
pub fn overlap(rec: &Run, other: &Run) -> Vec<String> {
    let mut shared = Vec::new();
    for (repo, base, branch) in branches(rec) {
        for (o_repo, o_base, o_branch) in branches(other) {
            if o_repo != repo {
                continue;
            }
            let mine = changed_files(&repo, &base, &branch);
            let theirs = changed_files(&o_repo, &o_base, &o_branch);
            for f in mine.into_iter().filter(|f| theirs.contains(f)) {
                shared.push(format!("{}/{f}", repo_name(&repo)));
            }
        }
    }
    shared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_env_and_prompt_name_every_worktree() {
        let rec = Run {
            repo: "/code/app".into(),
            worktree: "/code/app/.dispatch/worktrees/r1".into(),
            linked: vec![LinkedRepo {
                repo: "/code/lib".into(),
                worktree: "/code/lib/.dispatch/worktrees/r1".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            linked_env(&rec),
            "app=/code/app/.dispatch/worktrees/r1:lib=/code/lib/.dispatch/worktrees/r1"
        );
        let note = prompt_note(&rec);
        assert!(note.contains("- lib (/code/lib): /code/lib/.dispatch/worktrees/r1"));
        assert!(prompt_note(&Run::default()).is_empty());
    }
}
