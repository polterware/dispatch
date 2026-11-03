#[path = "runner_support.rs"]
mod support;

use dispatch_core::{
    process::Tree,
    runner::{pid_alive, process_start},
    types::{
        Run, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_ORPHANED, STATUS_PENDING, STATUS_RUNNING,
        Store,
    },
    worktree,
};
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::{Pid, getpgid},
};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use support::*;
use tokio_util::sync::CancellationToken;

fn own_pid() -> i64 {
    i64::from(std::process::id())
}

/// Kills the process group when the test ends.
struct Group(i32);

impl Drop for Group {
    fn drop(&mut self) {
        let _ = kill(Pid::from_raw(-self.0), Signal::SIGKILL);
    }
}

async fn require_process_tree() -> bool {
    match Tree::capture(std::process::id() as i32).await {
        Ok(_) => true,
        Err(e) => {
            let m = e.to_string();
            assert!(
                m.contains("Operation not permitted") || m.contains("Permission denied"),
                "{m}"
            );
            eprintln!("sandbox blocks process table: {m}");
            false
        }
    }
}

/// Reaps `child` in the background and reports when it exits.
fn reaper(mut child: Child) -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = child.wait();
        let _ = tx.send(());
    });
    rx
}

#[tokio::test]
async fn test_reconcile() {
    let t = test_deps(vec![]);
    let cancel = CancellationToken::new();
    let start = process_start(own_pid()).await;
    assert!(!start.is_empty(), "missing identity");
    let wt = temp_dir();
    let file = wt.path().join("work");
    std::fs::write(&file, "preserve").unwrap();
    for (id, pid, identity, want) in [
        ("dead", 99_999_999, "", STATUS_ORPHANED),
        ("reused", own_pid(), "different start", STATUS_ORPHANED),
        ("live", own_pid(), start.as_str(), STATUS_RUNNING),
        ("legacy", own_pid(), "", STATUS_PENDING),
        ("missing", 0, "", STATUS_ORPHANED),
    ] {
        let status = if id == "legacy" {
            STATUS_PENDING
        } else {
            STATUS_RUNNING
        };
        t.store
            .save_run(&Run {
                id: id.into(),
                status: status.into(),
                pid,
                proc_start: identity.into(),
                worktree: path_of(&wt),
                ..Run::default()
            })
            .unwrap();
        t.deps.reconcile(&cancel).await.unwrap();
        let got = t.get(id);
        assert_eq!(got.status, want, "{id}: {got:?}");
        if want == STATUS_ORPHANED {
            assert!(
                !got.note.is_empty() && !got.finished_at.is_empty(),
                "{got:?}"
            );
        }
    }
    let before = t.store.run_writes.load(Ordering::SeqCst);
    t.deps.reconcile(&cancel).await.unwrap();
    assert_eq!(
        before,
        t.store.run_writes.load(Ordering::SeqCst),
        "reconcile is not idempotent"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "preserve",
        "work lost"
    );
}

#[tokio::test]
async fn test_recover_and_clean() {
    let t = test_deps(vec![]);
    let dir = temp_dir();
    let root = path_of(&dir);
    git(&root, &["init", "-q"]);
    git(&root, &["commit", "--allow-empty", "-qm", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
    let wt_dir = temp_dir();
    let wt = format!("{}/worktree", path_of(&wt_dir));
    let branch = worktree::create(&root, &wt, "orphan").unwrap();
    let mut rec = Run {
        id: "orphan".into(),
        status: STATUS_ORPHANED.into(),
        repo: root.clone(),
        worktree: wt.clone(),
        branch: branch.clone(),
        base_sha: base,
        ..Run::default()
    };
    t.store.save_run(&rec).unwrap();
    let file = format!("{wt}/saved.txt");
    std::fs::write(&file, "saved\n").unwrap();
    let err = worktree::remove(&root, &wt, &branch, false).unwrap_err();
    assert!(
        err.to_string().contains("--force"),
        "clean accepted dirty orphan: {err}"
    );
    assert!(std::path::Path::new(&file).exists());
    let (out, stat) = t.deps.recover(&rec.id).await.unwrap();
    assert!(
        out.status == STATUS_COMPLETED
            && out.note == "recovered from orphan; unverified"
            && stat.contains("saved.txt"),
        "{out:?} {stat:?}"
    );
    assert_eq!(
        git(&wt, &["log", "-1", "--format=%s"]).trim(),
        "dispatch recover orphan"
    );
    assert!(!worktree::dirty(&wt), "worktree still dirty");
    worktree::merge(&root, &branch, &rec.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{root}/saved.txt")).unwrap(),
        "saved\n",
        "recovered work not mergeable"
    );
    let gone = temp_dir();
    rec.id = "gone".into();
    rec.status = STATUS_ORPHANED.into();
    rec.worktree = format!("{}/absent", path_of(&gone));
    t.store.save_run(&rec).unwrap();
    let before = t.store.run_writes.load(Ordering::SeqCst);
    assert!(
        t.deps.recover(&rec.id).await.is_err(),
        "recovered missing worktree"
    );
    assert_eq!(
        before,
        t.store.run_writes.load(Ordering::SeqCst),
        "missing worktree modified record"
    );
}

#[tokio::test]
async fn test_cancel_waits_for_processes() {
    if !require_process_tree().await {
        return;
    }
    let t = test_deps(vec![]);
    let owner = Command::new("sleep").arg("60").spawn().unwrap();
    let owner_pid = i64::from(owner.id());
    let _owner = Group(-(owner_pid as i32));
    let _owner_reaped = reaper(owner);
    let agent = Command::new("sh")
        .args(["-c", "trap '' TERM; sleep 60 & wait"])
        .process_group(0)
        .spawn()
        .unwrap();
    let agent_pid = agent.id() as i32;
    let _group = Group(agent_pid);
    let _reaped = reaper(agent);
    let rec = Run {
        id: "cancel".into(),
        status: STATUS_RUNNING.into(),
        pid: owner_pid,
        agent_pid: i64::from(agent_pid),
        proc_start: process_start(owner_pid).await,
        ..Run::default()
    };
    t.store.save_run(&rec).unwrap();
    let cancel = CancellationToken::new();
    let timer = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(12)).await;
        timer.cancel();
    });
    let out = t.deps.cancel(&cancel, &rec.id).await.unwrap();
    assert!(
        out.status == STATUS_CANCELLED && !pid_alive(-rec.agent_pid) && !pid_alive(rec.pid),
        "{out:?}"
    );
}

#[tokio::test]
async fn test_reconcile_stops_orphan_group() {
    if !require_process_tree().await {
        return;
    }
    let t = test_deps(vec![]);
    let child = Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let _group = Group(pid);
    let done = reaper(child);
    let wt = temp_dir();
    let rec = Run {
        id: "orphan-group".into(),
        status: STATUS_PENDING.into(),
        pid: 99_999_999,
        agent_pid: i64::from(pid),
        worktree: path_of(&wt),
        ..Run::default()
    };
    t.store.save_run(&rec).unwrap();
    t.deps.reconcile(&CancellationToken::new()).await.unwrap();
    assert!(
        done.recv_timeout(Duration::from_secs(2)).is_ok(),
        "orphan group did not receive TERM"
    );
    assert_eq!(t.get(&rec.id).status, STATUS_ORPHANED);
    assert!(wt.path().exists(), "worktree removed");
}

#[tokio::test]
async fn test_wait_timeout_preserves_record() {
    let t = test_deps(vec![]);
    t.store
        .save_run(&Run {
            id: "live-wait".into(),
            status: STATUS_RUNNING.into(),
            pid: own_pid(),
            ..Run::default()
        })
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (got, err) = t
        .deps
        .wait(&cancel, "live-wait", Duration::from_millis(1))
        .await;
    let err = err.unwrap_err();
    assert!(
        err.to_string() == "context canceled" && got.id == "live-wait",
        "{got:?} {err}"
    );
}

#[tokio::test]
async fn test_detached_descendants_stopped() {
    if !require_process_tree().await {
        return;
    }
    let Some(perl) = ["/usr/bin/perl", "/bin/perl"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
    else {
        eprintln!("perl unavailable for detached process fixture");
        return;
    };
    for orphan in [false, true] {
        let name = if orphan { "reconcile" } else { "cancel" };
        let t = test_deps(vec![]);
        let tmp = temp_dir();
        let path = format!("{}/child", path_of(&tmp));
        let program = r#"use POSIX qw(setsid); setsid() >= 0 or die $!; $SIG{TERM}="IGNORE"; open(my $f,">",$ARGV[0]) or die $!; print $f "$$\n"; close($f); exec "sleep","60";"#;
        let agent = Command::new("sh")
            .args([
                "-c",
                r#""$1" -e "$2" "$3" & trap 'exit' TERM; wait"#,
                "sh",
                perl,
                program,
                &path,
            ])
            .process_group(0)
            .spawn()
            .unwrap();
        let agent_pid = agent.id() as i32;
        let _group = Group(agent_pid);
        let done = reaper(agent);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut child = 0;
        while Instant::now() < deadline {
            child = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            if child > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child > 0, "{name}: detached process did not start");
        let _child_guard = Group(-child);
        let group = getpgid(Some(Pid::from_raw(child))).unwrap();
        assert_ne!(group.as_raw(), agent_pid, "{name}: process did not detach");
        let mut owner_pid = 99_999_999;
        let _owner = if orphan {
            None
        } else {
            let p = Command::new("sleep").arg("60").spawn().unwrap();
            owner_pid = i64::from(p.id());
            let guard = Group(-(owner_pid as i32));
            Some((guard, reaper(p)))
        };
        let rec = Run {
            id: name.into(),
            status: STATUS_RUNNING.into(),
            pid: owner_pid,
            agent_pid: i64::from(agent_pid),
            proc_start: process_start(owner_pid).await,
            ..Run::default()
        };
        t.store.save_run(&rec).unwrap();
        let cancel = CancellationToken::new();
        let want = if orphan {
            t.deps.reconcile(&cancel).await.unwrap();
            STATUS_ORPHANED
        } else {
            t.deps.cancel(&cancel, &rec.id).await.unwrap();
            STATUS_CANCELLED
        };
        assert_eq!(t.get(&rec.id).status, want, "{name}");
        assert_eq!(
            kill(Pid::from_raw(child), None),
            Err(Errno::ESRCH),
            "{name}: escaped descendant {child} survived"
        );
        assert!(
            done.recv_timeout(Duration::from_secs(1)).is_ok(),
            "{name}: agent survived"
        );
    }
}
