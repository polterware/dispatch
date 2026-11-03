use crate::{
    Error,
    worktree::{
        git::git_raw,
        ops::{current_branch, exclude},
    },
};
use nix::fcntl::{Flock, FlockArg};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeTreeResult {
    pub clean: bool,
    pub conflicts: Vec<String>,
    pub tree_oid: String,
}

fn parse_conflicts(out: &str) -> Vec<String> {
    let mut fields = out.split('\0');
    let _ = fields.next();
    let mut seen = HashSet::new();
    let mut list = Vec::new();
    for f in fields {
        if f.is_empty() {
            break;
        }
        if seen.insert(f) {
            list.push(f.to_owned());
        }
    }
    list.sort();
    list
}

/// Runs `git merge-tree --write-tree` to check if `branch` can merge cleanly into `target`.
pub fn merge_tree(
    root: impl AsRef<Path>,
    target: &str,
    branch: &str,
) -> Result<MergeTreeResult, Error> {
    let (out, code) = git_raw(
        root.as_ref(),
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            target,
            branch,
        ],
    )?;
    match code {
        0 => Ok(MergeTreeResult {
            clean: true,
            conflicts: Vec::new(),
            tree_oid: out.trim_end_matches(['\0', '\n']).to_owned(),
        }),
        1 => Ok(MergeTreeResult {
            clean: false,
            conflicts: parse_conflicts(&out),
            tree_oid: String::new(),
        }),
        _ => Err(Error::Message(format!(
            "git merge-tree exited with code {code}: {}",
            out.trim()
        ))),
    }
}

pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
static LOCK_TIMEOUT_MS: AtomicU64 = AtomicU64::new(3_000);

pub fn lock_timeout() -> Duration {
    Duration::from_millis(LOCK_TIMEOUT_MS.load(Ordering::Relaxed))
}

pub fn set_lock_timeout(timeout: Duration) {
    let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    LOCK_TIMEOUT_MS.store(ms, Ordering::Relaxed);
}

fn sanitize_branch(branch: &str) -> String {
    branch
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | ' ' => '_',
            _ => c,
        })
        .collect()
}

/// Guard holding an exclusive merge lock. Releases on [`MergeLock::unlock`] or drop.
#[derive(Debug)]
pub struct MergeLock {
    flock: Option<Flock<File>>,
}

impl MergeLock {
    pub fn unlock(mut self) {
        self.release();
    }

    fn release(&mut self) {
        if let Some(flock) = self.flock.take() {
            let _ = flock.set_len(0);
            // Keep the lock file on disk so concurrent waiters lock the same inode.
            let _ = flock.unlock();
        }
    }
}

impl Drop for MergeLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Acquires an exclusive lock for merging into `target_branch` in `repo`.
pub fn lock_merge(repo: impl AsRef<Path>, target_branch: &str) -> Result<MergeLock, Error> {
    lock_merge_with_timeout(repo, target_branch, lock_timeout())
}

/// Acquires an exclusive merge lock waiting up to `timeout`.
pub fn lock_merge_with_timeout(
    repo: impl AsRef<Path>,
    target_branch: &str,
    timeout: Duration,
) -> Result<MergeLock, Error> {
    let repo = repo.as_ref();
    let target = if target_branch.is_empty() {
        current_branch(repo).unwrap_or_default()
    } else {
        target_branch.to_owned()
    };
    let dispatch_dir = repo.join(".dispatch");
    fs::create_dir_all(&dispatch_dir)?;
    let _ = exclude(repo, ".dispatch/");

    let lock_path = dispatch_dir.join(format!("merge-{}.lock", sanitize_branch(&target)));
    let deadline = Instant::now() + timeout;

    loop {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o644)
            .open(&lock_path)?;

        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(mut flock) => {
                let _ = flock.set_len(0);
                let _ = flock.seek(SeekFrom::Start(0));
                let _ = writeln!(&mut *flock, "{}", std::process::id());
                let _ = flock.sync_all();
                return Ok(MergeLock { flock: Some(flock) });
            }
            Err((mut file, _errno)) => {
                let mut raw_pid = String::new();
                let _ = file.read_to_string(&mut raw_pid);
                drop(file);
                let holder_pid = raw_pid.trim().parse::<i32>().unwrap_or(0);

                if Instant::now() > deadline {
                    if holder_pid > 0 {
                        return Err(Error::Message(format!(
                            "merge lock for branch {:?} in {} is held by pid {}; timeout waiting for lock",
                            target,
                            repo.display(),
                            holder_pid
                        )));
                    }
                    return Err(Error::Message(format!(
                        "merge lock for branch {:?} in {} is held by another process; timeout waiting for lock",
                        target,
                        repo.display()
                    )));
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Returns files in `root` that have uncommitted changes AND are touched by `branch` relative to `HEAD`.
pub fn uncommitted_overlaps(root: impl AsRef<Path>, branch: &str) -> Result<Vec<String>, Error> {
    let root = root.as_ref();
    let mut uncommitted = HashSet::new();
    if let Ok((out, 0)) = git_raw(root, &["diff", "--name-only", "--no-renames", "-z", "HEAD"]) {
        for f in out.split('\0') {
            if !f.is_empty() {
                uncommitted.insert(f.to_owned());
            }
        }
    }

    if let Ok((out, 0)) = git_raw(root, &["status", "--porcelain=v1", "-z"]) {
        let fields: Vec<&str> = out.split('\0').collect();
        let mut i = 0;
        while i < fields.len() {
            let e = fields[i];
            let b = e.as_bytes();
            if b.len() >= 4
                && let Some(path) = e.get(3..)
            {
                uncommitted.insert(path.to_owned());
                if b[0] == b'R' || b[0] == b'C' || b[1] == b'R' || b[1] == b'C' {
                    i += 1;
                    if i < fields.len() && !fields[i].is_empty() {
                        uncommitted.insert(fields[i].to_owned());
                    }
                }
            }
            i += 1;
        }
    }

    if uncommitted.is_empty() {
        return Ok(Vec::new());
    }

    let range = format!("HEAD...{branch}");
    let (out, code) = git_raw(root, &["diff", "--name-only", "--no-renames", "-z", &range])?;
    if code != 0 {
        return Err(Error::Message(format!(
            "git diff HEAD...{branch} exited with code {code}"
        )));
    }
    let mut seen = HashSet::new();
    let mut overlapping = Vec::new();
    for f in out.split('\0') {
        if !f.is_empty() && uncommitted.contains(f) && seen.insert(f) {
            overlapping.push(f.to_owned());
        }
    }
    overlapping.sort();
    Ok(overlapping)
}
