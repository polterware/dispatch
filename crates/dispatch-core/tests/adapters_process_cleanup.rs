use dispatch_core::{
    adapters::{livelog::LiveLog, proc::run_command},
    process::Tree,
};
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::{Pid, getpgid},
};
use std::{
    fs,
    time::{Duration, Instant},
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

async fn process_table_readable() -> bool {
    if Tree::capture(std::process::id() as i32).await.is_err() {
        eprintln!("skipped: process table unreadable");
        return false;
    }
    true
}
struct Cleanup(i32);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = kill(Pid::from_raw(self.0), Signal::SIGKILL);
    }
}
async fn read_pid(path: &std::path::Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(content) = fs::read_to_string(path)
            && let Ok(pid) = content.trim().parse::<i32>()
            && pid > 0
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "grandchild did not start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while kill(Pid::from_raw(pid), None).is_ok() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        kill(Pid::from_raw(pid), None),
        Err(Errno::ESRCH),
        "grandchild survived"
    );
}
#[tokio::test]
async fn agent_cancellation_kills_grandchild() {
    if !process_table_readable().await {
        return;
    }
    for stubborn in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid");
        let script = if stubborn {
            r#"trap '' TERM; sh -c 'sleep 60 & echo $! > "$1"; wait' sh "$1" & wait"#
        } else {
            r#"sh -c 'trap "wait; exit" TERM; sleep 60 & echo $! > "$1"; wait' sh "$1" & trap 'wait; exit' TERM; wait"#
        };
        let mut command = Command::new("sh");
        command.args(["-c", script, "sh", path.to_str().unwrap()]);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            run_command(command, None, task_cancel, LiveLog::default(), None, None).await
        });
        let pid = read_pid(&path).await;
        let _cleanup = Cleanup(pid);
        assert!(kill(Pid::from_raw(pid), None).is_ok());
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(12), task)
            .await
            .expect("cancellation hung")
            .unwrap();
        gone(pid).await;
    }
}
#[tokio::test]
async fn agent_cancellation_kills_detached_grandchild() {
    if !process_table_readable().await {
        return;
    }
    if !std::path::Path::new("/usr/bin/perl").exists() {
        eprintln!("skipped: perl unavailable for detached process fixture");
        return;
    }
    for timeout in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid");
        let perl = r#"use POSIX qw(setsid); setsid() >= 0 or die $!; $SIG{TERM}="IGNORE"; open(my $f,">",$ARGV[0]) or die $!; print $f "$$\n"; close($f); exec "sleep","60";"#;
        let mut command = Command::new("sh");
        command.args([
            "-c",
            r#""$1" -e "$2" "$3" & trap 'exit' TERM; wait"#,
            "sh",
            "/usr/bin/perl",
            perl,
            path.to_str().unwrap(),
        ]);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            run_command(command, None, task_cancel, LiveLog::default(), None, None).await
        });
        let pid = read_pid(&path).await;
        let _cleanup = Cleanup(pid);
        assert_eq!(getpgid(Some(Pid::from_raw(pid))).unwrap().as_raw(), pid);
        if timeout {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(13), task)
            .await
            .expect("cancellation hung")
            .unwrap();
        if let Err(err) = result {
            assert!(!err.to_string().contains("cleanup incomplete"), "{err}");
        }
        gone(pid).await;
    }
}
