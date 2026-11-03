#[path = "runner_support.rs"]
mod support;

use dispatch_core::{
    types::{Run, STATUS_COMPLETED, Store, WORKTREE_ABANDONED, WORKTREE_MERGED, WORKTREE_REMOVED},
    worktree,
};
use support::*;

struct Completed {
    t: TestDeps,
    rec: Run,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

fn completed_worktree() -> Completed {
    let t = test_deps(vec![]);
    let root_dir = temp_dir();
    let root = path_of(&root_dir);
    git(&root, &["init", "-q"]);
    git(&root, &["commit", "--allow-empty", "-qm", "base"]);
    let wt_dir = temp_dir();
    let wt = format!("{}/worktree", path_of(&wt_dir));
    let branch = worktree::create(&root, &wt, "merge-test").unwrap();
    std::fs::write(format!("{wt}/result.txt"), "result\n").unwrap();
    git(&wt, &["add", "result.txt"]);
    git(&wt, &["commit", "-qm", "result"]);
    let rec = Run {
        id: "merge-test".into(),
        status: STATUS_COMPLETED.into(),
        repo: root,
        worktree: wt,
        branch,
        ..Run::default()
    };
    t.store.save_run(&rec).unwrap();
    Completed {
        t,
        rec,
        _dirs: (root_dir, wt_dir),
    }
}

fn assert_worktree_status(t: &TestDeps, id: &str, want: &str) {
    let trees = t.store.list_worktrees("").unwrap();
    let tree = trees
        .iter()
        .find(|w| w.run_id == id)
        .expect("missing worktree record");
    assert_eq!(tree.status, want, "worktree status");
}

#[test]
fn test_merge_worktree_lifecycle() {
    for keep in [false, true] {
        let c = completed_worktree();
        let (t, rec) = (&c.t, &c.rec);
        let res = t.deps.merge(&rec.id, keep).unwrap();
        assert!(res.ok && res.cleaned != keep, "merge = {res:?}");
        let saved = t.get(&rec.id);
        assert!(
            saved.merged_at.is_some() && !saved.merge_sha.is_empty(),
            "saved run = {saved:?}"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{}/result.txt", rec.repo)).unwrap(),
            "result\n"
        );
        if keep {
            assert_worktree_status(t, &rec.id, WORKTREE_MERGED);
            assert!(std::path::Path::new(&rec.worktree).exists());
            t.deps.clean(&rec.id, false).unwrap();
        }
        assert_worktree_status(t, &rec.id, WORKTREE_REMOVED);
        assert!(
            !std::path::Path::new(&rec.worktree).exists(),
            "worktree still exists"
        );
        git(
            &rec.repo,
            &["merge-base", "--is-ancestor", &saved.merge_sha, "HEAD"],
        );
    }
}

#[test]
fn test_clean_unmerged_worktree() {
    let c = completed_worktree();
    c.t.deps.clean(&c.rec.id, true).unwrap();
    assert_worktree_status(&c.t, &c.rec.id, WORKTREE_ABANDONED);
    assert!(
        !std::path::Path::new(&c.rec.worktree).exists(),
        "worktree still exists"
    );
}
