#![forbid(unsafe_code)]

use dispatch_core::worktree::{
    self, NumStat, branch_name, commit, create, current_branch, dirty, exclude, has_commits, head,
    lock_merge, lock_timeout, merge, merge_tree, num_stat, parse_git_version, recover, rel, remove,
    root, set_lock_timeout, set_supports_merge_tree_override, supports_merge_tree,
    uncommitted_overlaps, version_supports_merge_tree, worktree_path,
};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicI32, Ordering},
    },
    thread,
    time::Duration,
};
use tempfile::TempDir;

static GLOBAL_TEST_MU: Mutex<()> = Mutex::new(());

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}\n{}{}",
        args,
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn must_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(output.status.success(), "git {:?} failed", args);
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write_file(dir: &Path, name: &str, body: &[u8]) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir parent");
    }
    fs::write(path, body).expect("write file");
}

struct LockTimeoutGuard(Duration);

impl Drop for LockTimeoutGuard {
    fn drop(&mut self) {
        set_lock_timeout(self.0);
    }
}

#[test]
fn test_lifecycle() {
    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q"]);
    write_file(repo, "a.txt", b"a");
    run_git(repo, &["add", "-A"]);
    run_git(repo, &["commit", "-q", "-m", "init"]);

    let resolved = fs::canonicalize(repo).expect("canonicalize");
    assert_eq!(root(repo), resolved.to_string_lossy());
    assert!(has_commits(repo));
    assert!(!dirty(repo));

    exclude(repo, ".dispatch/").expect("exclude first");
    exclude(repo, ".dispatch/").expect("exclude second");
    let excl = fs::read_to_string(repo.join(".git").join("info").join("exclude")).expect("exclude");
    assert_eq!(excl.matches(".dispatch/").count(), 1);

    let wt_parent = TempDir::new().expect("wt tempdir");
    let wt = wt_parent.path().join("wt");
    let branch = create(repo, &wt, "r1").expect("create worktree");
    assert_eq!(branch, "dispatch/r1");

    write_file(&wt, "b.txt", b"b");
    assert!(commit(&wt, "r1").expect("first commit"));
    assert!(!commit(&wt, "r1").expect("second commit"));

    merge(repo, &branch, "r1").expect("merge");
    assert!(repo.join("b.txt").exists(), "merge did not bring b.txt");
    remove(repo, &wt, &branch, false).expect("remove");
}

#[test]
fn test_num_stat() {
    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q", "-b", "main"]);
    write_file(repo, "a.txt", b"a\n");
    run_git(repo, &["add", "-A"]);
    run_git(repo, &["commit", "-qm", "base"]);

    let base = must_git(repo, &["rev-parse", "HEAD"]);
    let empty = num_stat(repo, &base).expect("empty numstat");
    assert_eq!(
        empty,
        NumStat {
            files: 0,
            insertions: 0,
            deletions: 0,
            lines_known: true,
        }
    );

    write_file(repo, "b.txt", b"b\n");
    run_git(repo, &["add", "b.txt"]);
    let added = num_stat(repo, &base).expect("added numstat");
    assert_eq!(
        added,
        NumStat {
            files: 1,
            insertions: 1,
            deletions: 0,
            lines_known: true,
        }
    );

    let err = num_stat(repo, "").expect_err("empty base should fail");
    assert_eq!(err.to_string(), "no base");

    write_file(repo, "bin.dat", &[0u8, 159, 146, 150, 0]);
    run_git(repo, &["add", "bin.dat"]);
    let with_bin = num_stat(repo, &base).expect("binary numstat");
    assert_eq!(with_bin.files, 2);
    assert!(!with_bin.lines_known);
}

#[test]
fn test_parse_git_version() {
    let cases = [
        ("git version 2.55.0", Some((2, 55)), true),
        ("git version 2.38.0", Some((2, 38)), true),
        ("git version 2.38.1.windows.1", Some((2, 38)), true),
        ("git version 2.37.1 (Apple Git-137.1)", Some((2, 37)), false),
        ("git version 1.9.5", Some((1, 9)), false),
        ("git version 3.0.0", Some((3, 0)), true),
        ("invalid", None, false),
    ];

    for (input, want, want_supported) in cases {
        let got = parse_git_version(input);
        assert_eq!(got, want, "parse_git_version({input:?})");
        let supp = got.is_some_and(|(maj, min)| version_supports_merge_tree(maj, min));
        assert_eq!(supp, want_supported, "supported({input:?})");
    }
}

#[test]
fn test_lock_merge_stale_detection() {
    let _guard = GLOBAL_TEST_MU
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q", "-b", "main"]);
    run_git(repo, &["commit", "--allow-empty", "-qm", "init"]);

    let lock1 = lock_merge(repo, "main").expect("first lock");
    let _restore = LockTimeoutGuard(lock_timeout());
    set_lock_timeout(Duration::from_millis(50));

    let err = lock_merge(repo, "main").expect_err("expected error acquiring held lock");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout waiting for lock"),
        "unexpected error message: {msg}"
    );
    assert!(
        msg.contains(&format!("pid {}", std::process::id())),
        "expected holder pid in message: {msg}"
    );

    lock1.unlock();

    let lock2 = lock_merge(repo, "main").expect("second lock after unlock");
    lock2.unlock();
}

#[test]
fn test_lock_merge_keeps_file_and_excludes() {
    let _guard = GLOBAL_TEST_MU
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q", "-b", "main"]);
    run_git(repo, &["commit", "--allow-empty", "-qm", "init"]);

    let _restore = LockTimeoutGuard(lock_timeout());
    set_lock_timeout(Duration::from_secs(20));

    let lock = lock_merge(repo, "main").expect("lock");
    lock.unlock();
    assert!(
        repo.join(".dispatch").join("merge-main.lock").exists(),
        "lock file must survive unlock"
    );
    let excl = fs::read_to_string(repo.join(".git").join("info").join("exclude")).expect("exclude");
    assert!(excl.contains(".dispatch/"));

    let inside = AtomicI32::new(0);
    let overlaps = AtomicI32::new(0);
    thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                let guard = lock_merge(repo, "main").expect("thread lock_merge");
                if inside.fetch_add(1, Ordering::SeqCst) + 1 > 1 {
                    overlaps.fetch_add(1, Ordering::SeqCst);
                }
                thread::sleep(Duration::from_millis(10));
                inside.fetch_sub(1, Ordering::SeqCst);
                guard.unlock();
            });
        }
    });
    assert_eq!(
        overlaps.load(Ordering::SeqCst),
        0,
        "lock holders overlapped"
    );
}

#[test]
fn test_uncommitted_overlaps_handles_odd_paths() {
    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q", "-b", "main"]);
    for name in ["a.txt", "with space.txt", "old.txt", "plain.txt"] {
        write_file(repo, name, b"base\n");
    }
    run_git(repo, &["add", "-A"]);
    run_git(repo, &["commit", "-q", "-m", "init"]);
    run_git(repo, &["checkout", "-q", "-b", "feat"]);
    for name in ["a.txt", "with space.txt", "old.txt", "plain.txt"] {
        write_file(repo, name, b"branch\n");
    }
    run_git(repo, &["commit", "-q", "-am", "feat"]);
    run_git(repo, &["checkout", "-q", "main"]);

    write_file(repo, "a.txt", b"dirty\n");
    write_file(repo, "with space.txt", b"dirty\n");
    run_git(repo, &["mv", "old.txt", "new.txt"]);

    let got = uncommitted_overlaps(repo, "feat").expect("uncommitted_overlaps");
    assert_eq!(got, vec!["a.txt", "old.txt", "with space.txt"]);
}

#[test]
fn test_merge_tree_reports_conflicted_paths_only() {
    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q", "-b", "main"]);
    write_file(repo, "my file.txt", b"base\n");
    write_file(repo, "ok.txt", b"base\n");
    run_git(repo, &["add", "-A"]);
    run_git(repo, &["commit", "-q", "-m", "init"]);

    run_git(repo, &["checkout", "-q", "-b", "feat"]);
    write_file(repo, "my file.txt", b"feat\n");
    write_file(repo, "ok.txt", b"feat\n");
    run_git(repo, &["commit", "-q", "-am", "feat"]);

    run_git(repo, &["checkout", "-q", "main"]);
    write_file(repo, "my file.txt", b"main\n");
    run_git(repo, &["commit", "-q", "-am", "main"]);

    let res = merge_tree(repo, "main", "feat").expect("merge_tree conflict");
    assert!(!res.clean);
    assert_eq!(res.conflicts, vec!["my file.txt"]);

    run_git(repo, &["checkout", "-q", "-b", "clean", "main~1"]);
    write_file(repo, "ok.txt", b"x\n");
    run_git(repo, &["commit", "-q", "-am", "c"]);

    let res = merge_tree(repo, "main", "clean").expect("merge_tree clean");
    assert!(res.clean);
    assert!(res.tree_oid.len() >= 40, "tree_oid={}", res.tree_oid);
}

#[test]
fn test_worktree_paths_dirty_remove_recover_and_branch_helpers() {
    let non_git = TempDir::new().expect("non-git tempdir");
    assert_eq!(root(non_git.path()), "");
    assert!(!has_commits(non_git.path()));

    let repo_dir = TempDir::new().expect("tempdir");
    let repo = repo_dir.path();
    run_git(repo, &["init", "-q", "-b", "main"]);
    assert!(!has_commits(repo));

    write_file(repo, "sub/a.txt", b"hello\n");
    run_git(repo, &["add", "-A"]);
    run_git(repo, &["commit", "-qm", "init"]);

    assert_eq!(branch_name("run42"), "dispatch/run42");
    let wt = worktree_path(repo, "run42");
    assert_eq!(wt, repo.join(".dispatch").join("worktrees").join("run42"));
    assert_eq!(current_branch(repo).expect("current_branch"), "main");

    assert_eq!(rel(repo, repo), ".");
    assert_eq!(rel(repo, repo.join("sub").join("inner")), "sub/inner");
    assert_eq!(rel(repo, non_git.path()), ".");

    let base_sha = head(repo).expect("head");
    let branch = create(repo, &wt, "run42").expect("create under .dispatch/worktrees");
    assert_eq!(branch, "dispatch/run42");

    write_file(&wt, "sub/new.txt", b"uncommitted\n");
    assert!(dirty(&wt));

    let err = remove(repo, &wt, &branch, false).expect_err("dirty remove without force");
    assert_eq!(
        err.to_string(),
        "worktree is dirty; use --force to discard changes"
    );

    let wrong = recover(&wt, "dispatch/other", &base_sha, "run42").expect_err("wrong branch");
    assert_eq!(
        wrong.to_string(),
        "worktree is not on run branch dispatch/other"
    );

    let stat = recover(&wt, &branch, "", "run42").expect("recover with reflog base");
    assert!(stat.contains("sub/new.txt"), "stat={stat}");
    assert!(!dirty(&wt));

    write_file(&wt, "sub/dirty_again.txt", b"discard me\n");
    remove(repo, &wt, &branch, true).expect("force remove dirty unmerged worktree");
    assert!(!wt.exists());

    // Detached HEAD reports "HEAD".
    run_git(repo, &["checkout", "-q", "--detach", &base_sha]);
    assert_eq!(current_branch(repo).expect("detached branch"), "HEAD");

    // Override hook for SupportsMergeTree.
    let _guard = GLOBAL_TEST_MU
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    set_supports_merge_tree_override(Some(false));
    assert!(!supports_merge_tree(repo));
    set_supports_merge_tree_override(Some(true));
    assert!(supports_merge_tree(repo));
    set_supports_merge_tree_override(None);
    let _ = worktree::git_exit(repo, &["status"]).expect("git_exit");
}
