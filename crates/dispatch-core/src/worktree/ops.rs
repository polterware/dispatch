use crate::{Error, worktree::git::git};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumStat {
    pub files: i32,
    pub insertions: i32,
    pub deletions: i32,
    pub lines_known: bool,
}

/// Returns the repo top-level containing `dir`, or `""` when `dir` is not in a git work tree.
pub fn root(dir: impl AsRef<Path>) -> String {
    git(dir.as_ref(), &["rev-parse", "--show-toplevel"]).unwrap_or_default()
}

/// Reports whether `HEAD` resolves (a worktree needs a commit).
pub fn has_commits(root: impl AsRef<Path>) -> bool {
    git(root.as_ref(), &["rev-parse", "--verify", "HEAD"]).is_ok()
}

/// Reports whether the checkout has uncommitted changes.
pub fn dirty(root: impl AsRef<Path>) -> bool {
    git(root.as_ref(), &["status", "--porcelain"]).is_ok_and(|out| !out.is_empty())
}

/// Adds `pattern` to the repo's local `.git/info/exclude` if it is not there yet.
pub fn exclude(root: impl AsRef<Path>, pattern: &str) -> Result<(), Error> {
    let root = root.as_ref();
    let raw = git(root, &["rev-parse", "--git-path", "info/exclude"])?;
    let p = Path::new(&raw);
    let exclude_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing.split('\n').any(|l| l.trim() == pattern) {
        return Ok(());
    }
    if let Some(parent) = exclude_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o644)
        .open(&exclude_path)?;
    let prefix = if !existing.is_empty() && !existing.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    file.write_all(format!("{prefix}{pattern}\n").as_bytes())?;
    Ok(())
}

/// Returns the canonical branch name `dispatch/<id>` for a run worktree.
pub fn branch_name(id: &str) -> String {
    format!("dispatch/{id}")
}

/// Returns the default worktree directory `<root>/.dispatch/worktrees/<id>`.
pub fn worktree_path(root: impl AsRef<Path>, id: &str) -> PathBuf {
    root.as_ref().join(".dispatch").join("worktrees").join(id)
}

/// Adds worktree `path` on new branch `dispatch/<id>` from `HEAD`.
pub fn create(root: impl AsRef<Path>, path: impl AsRef<Path>, id: &str) -> Result<String, Error> {
    let branch = branch_name(id);
    let path_str = path.as_ref().to_string_lossy();
    git(
        root.as_ref(),
        &["worktree", "add", "-b", &branch, &path_str, "HEAD"],
    )?;
    Ok(branch)
}

/// Stages and commits everything in the worktree. Reports whether a commit was made.
pub fn commit(path: impl AsRef<Path>, id: &str) -> Result<bool, Error> {
    commit_with_message(path.as_ref(), &format!("dispatch run {id}"))
}

pub(crate) fn commit_with_message(path: &Path, message: &str) -> Result<bool, Error> {
    let out = git(path, &["status", "--porcelain"])?;
    if out.is_empty() {
        return Ok(false);
    }
    git(path, &["add", "-A"])?;
    git(
        path,
        &[
            "-c",
            "user.name=dispatch",
            "-c",
            "user.email=dispatch@localhost",
            "commit",
            "-q",
            "-m",
            message,
        ],
    )?;
    Ok(true)
}

/// Merges `branch` into the repo's current branch with a merge commit.
pub fn merge(root: impl AsRef<Path>, branch: &str, id: &str) -> Result<String, Error> {
    let message = format!("Merge dispatch run {id}");
    git(root.as_ref(), &["merge", "--no-ff", "-m", &message, branch])
}

/// Deletes the worktree and its branch (`-d` unless `force` is set).
pub fn remove(
    root: impl AsRef<Path>,
    path: impl AsRef<Path>,
    branch: &str,
    force: bool,
) -> Result<(), Error> {
    let root = root.as_ref();
    let path = path.as_ref();
    if !force {
        let status = git(path, &["status", "--porcelain"])?;
        if !status.is_empty() {
            return Err(Error::Message(
                "worktree is dirty; use --force to discard changes".into(),
            ));
        }
    }
    let path_str = path.to_string_lossy();
    if force {
        git(root, &["worktree", "remove", &path_str, "--force"])?;
    } else {
        git(root, &["worktree", "remove", &path_str])?;
    }
    let del = if force { "-D" } else { "-d" };
    git(root, &["branch", del, branch])?;
    Ok(())
}

fn clean_lexical(path: &Path) -> PathBuf {
    let mut root_prefix = PathBuf::new();
    let mut stack: Vec<OsString> = Vec::new();
    let is_abs = path.is_absolute();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => root_prefix.push(p.as_os_str()),
            Component::RootDir => root_prefix.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.last().is_some_and(|c| c != "..") {
                    stack.pop();
                } else if !is_abs {
                    stack.push("..".into());
                }
            }
            Component::Normal(c) => stack.push(c.to_os_string()),
        }
    }
    let mut out = root_prefix;
    for c in stack {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Returns `dir` relative to `root` (`"."` when equal or outside).
pub fn rel(root: impl AsRef<Path>, dir: impl AsRef<Path>) -> String {
    let root = root.as_ref();
    let dir = dir.as_ref();
    if root.is_absolute() != dir.is_absolute() {
        return ".".into();
    }
    let clean_root = clean_lexical(root);
    let clean_dir = clean_lexical(dir);
    if clean_root == clean_dir {
        return ".".into();
    }
    if clean_root == Path::new(".") {
        let s = clean_dir.to_string_lossy();
        return if s.is_empty() || s.starts_with("..") {
            ".".into()
        } else {
            s.into_owned()
        };
    }
    let Ok(sub) = clean_dir.strip_prefix(&clean_root) else {
        return ".".into();
    };
    let s = sub.to_string_lossy();
    if s.is_empty() || s.starts_with("..") {
        ".".into()
    } else {
        s.into_owned()
    }
}

/// Returns the commit used as the base of a new worktree.
pub fn head(path: impl AsRef<Path>) -> Result<String, Error> {
    git(path.as_ref(), &["rev-parse", "HEAD"])
}

/// Counts changes from `base` to the working tree (committed and not).
pub fn num_stat(dir: impl AsRef<Path>, base: &str) -> Result<NumStat, Error> {
    if base.is_empty() {
        return Err(Error::Message("no base".into()));
    }
    let out = git(dir.as_ref(), &["diff", "--numstat", base])?;
    let mut stat = NumStat {
        lines_known: true,
        ..Default::default()
    };
    if out.is_empty() {
        return Ok(stat);
    }
    for raw_line in out.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(a), Some(b), Some(_path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        stat.files += 1;
        if a == "-" || b == "-" {
            stat.lines_known = false;
            continue;
        }
        let (Ok(add), Ok(sub)) = (a.parse::<i32>(), b.parse::<i32>()) else {
            stat.lines_known = false;
            continue;
        };
        stat.insertions += add;
        stat.deletions += sub;
    }
    Ok(stat)
}

/// Checks the run branch before committing and reporting its diff.
pub fn recover(
    path: impl AsRef<Path>,
    branch: &str,
    base: &str,
    id: &str,
) -> Result<String, Error> {
    let path = path.as_ref();
    let current = git(path, &["symbolic-ref", "--short", "HEAD"])?;
    if branch.is_empty() || current != branch {
        return Err(Error::Message(format!(
            "worktree is not on run branch {branch}"
        )));
    }
    let mut resolved_base = base.to_owned();
    if resolved_base.is_empty() {
        resolved_base = git(path, &["rev-parse", &format!("{branch}@{{0}}")])?;
        let log = git(path, &["reflog", "show", "--format=%H", branch])?;
        if let Some(oldest) = log.split_whitespace().last() {
            resolved_base = oldest.to_owned();
        }
    }
    git(
        path,
        &[
            "rev-parse",
            "--verify",
            &format!("{resolved_base}^{{commit}}"),
        ],
    )?;
    commit_with_message(path, &format!("dispatch recover {id}"))?;
    git(path, &["diff", "--stat", &resolved_base, "HEAD", "--"])
}

/// Returns the currently checked out branch in `root`, or `"HEAD"` if detached.
pub fn current_branch(root: impl AsRef<Path>) -> Result<String, Error> {
    let root = root.as_ref();
    if let Ok(b) = git(root, &["symbolic-ref", "--short", "HEAD"])
        && !b.is_empty()
    {
        return Ok(b);
    }
    if let Ok(b) = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        && !b.is_empty()
    {
        return Ok(b);
    }
    Ok("HEAD".into())
}
