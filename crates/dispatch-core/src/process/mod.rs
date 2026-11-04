//! Child process tree capture and termination.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: i32,
    pub parent: i32,
    pub group: i32,
    pub start: String,
}

pub fn parse_table(output: &str) -> Result<HashMap<i32, ProcessInfo>, Error> {
    let mut table = HashMap::new();
    for line in output.trim().split('\n') {
        let fields: Vec<_> = line.split_whitespace().collect();
        let invalid = || Error::Message(format!("invalid ps process row {line:?}"));
        if fields.len() < 8 {
            return Err(invalid());
        }
        let pid: i32 = fields[0].parse().map_err(|_| invalid())?;
        let parent = fields[1].parse().map_err(|_| invalid())?;
        let group = fields[2].parse().map_err(|_| invalid())?;
        if pid < 0 {
            return Err(invalid());
        }
        table.insert(
            pid,
            ProcessInfo {
                pid,
                parent,
                group,
                start: fields[3..].join(" "),
            },
        );
    }
    Ok(table)
}

async fn read_table() -> Result<HashMap<i32, ProcessInfo>, Error> {
    let mut command = Command::new("ps");
    command
        .args(["-axo", "pid=,ppid=,pgid=,lstart="])
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .map_err(|_| Error::Message("read process tree: deadline has elapsed".into()))?
        .map_err(|e| Error::Message(format!("read process tree: {e}")))?;
    if !out.status.success() {
        return Err(Error::Message(format!("read process tree: {}", out.status)));
    }
    parse_table(&String::from_utf8_lossy(&out.stdout))
}

pub struct Tree {
    group: i32,
    members: Vec<ProcessInfo>,
    table: HashMap<i32, ProcessInfo>,
    #[cfg(test)]
    ops: Option<TestOps>,
}

impl std::fmt::Debug for Tree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tree")
            .field("group", &self.group)
            .field("members", &self.members)
            .field("table", &self.table)
            .finish()
    }
}

impl Tree {
    pub async fn capture(leader: i32) -> Result<Self, Error> {
        Ok(Self::from_table(read_table().await?, leader))
    }

    pub fn from_table(table: HashMap<i32, ProcessInfo>, leader: i32) -> Self {
        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        for p in table.values() {
            children.entry(p.parent).or_default().push(p.pid);
        }
        fn visit(
            pid: i32,
            table: &HashMap<i32, ProcessInfo>,
            children: &HashMap<i32, Vec<i32>>,
            seen: &mut HashSet<i32>,
            members: &mut Vec<ProcessInfo>,
        ) {
            if !seen.insert(pid) {
                return;
            }
            if let Some(kids) = children.get(&pid) {
                for child in kids {
                    visit(*child, table, children, seen, members);
                }
            }
            if let Some(p) = table.get(&pid) {
                members.push(p.clone());
            }
        }
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        if leader > 0 {
            visit(leader, &table, &children, &mut seen, &mut members);
            for p in table.values().filter(|p| p.group == leader) {
                visit(p.pid, &table, &children, &mut seen, &mut members);
            }
        }
        Self {
            group: leader,
            members,
            table,
            #[cfg(test)]
            ops: None,
        }
    }

    pub fn members(&self) -> &[ProcessInfo] {
        &self.members
    }

    pub fn include(&mut self, pid: i32, start: &str) -> Result<(), Error> {
        if pid <= 0 {
            return Ok(());
        }
        let Some(p) = self.table.get(&pid) else {
            return Ok(());
        };
        if !start.is_empty() && p.start != start.split_whitespace().collect::<Vec<_>>().join(" ") {
            return Err(Error::Message(format!("process {pid} identity changed")));
        }
        if !self.members.iter().any(|m| m.pid == pid) {
            self.members.push(p.clone());
        }
        Ok(())
    }

    fn same(p: &ProcessInfo, table: &HashMap<i32, ProcessInfo>) -> bool {
        table
            .get(&p.pid)
            .is_some_and(|current| current.start == p.start)
    }

    async fn snapshot(&self) -> Result<HashMap<i32, ProcessInfo>, Error> {
        #[cfg(test)]
        if let Some(ops) = &self.ops {
            return (ops.read)();
        }
        read_table().await
    }
    fn send(&self, pid: i32, signal: Signal) -> Result<(), Error> {
        #[cfg(test)]
        if let Some(ops) = &self.ops {
            return (ops.send)(pid, signal);
        }
        send(pid, signal)
    }
    pub async fn signal(&self, signal: Signal) -> Result<(), Error> {
        let mut table = self.snapshot().await?;
        let group_safe = self.group > 0
            && self.members.iter().any(|p| {
                Self::same(p, &table) && table.get(&p.pid).is_some_and(|p| p.group == self.group)
            });
        if group_safe {
            self.send(-self.group, signal)?;
            table = self.snapshot().await?;
        }
        for p in &self.members {
            if Self::same(p, &table)
                && !(group_safe && table.get(&p.pid).is_some_and(|p| p.group == self.group))
            {
                self.send(p.pid, signal)?;
            }
        }
        Ok(())
    }

    pub async fn wait(&self, cancel: &CancellationToken, grace: Duration) -> Result<(), Error> {
        for phase in 0..2 {
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                if cancel.is_cancelled() {
                    return Err(Error::Message("context canceled".into()));
                }
                let table = self.snapshot().await?;
                if !self.members.iter().any(|p| Self::same(p, &table)) {
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::select! { _ = cancel.cancelled() => return Err(Error::Message("context canceled".into())), _ = tokio::time::sleep(Duration::from_millis(50)) => {} }
            }
            if phase == 0 {
                self.signal(Signal::SIGKILL).await?;
            }
        }
        Err(Error::Message(
            "captured processes still exist after SIGKILL".into(),
        ))
    }

    pub async fn stop(&self, cancel: &CancellationToken, grace: Duration) -> Result<(), Error> {
        if cancel.is_cancelled() {
            return Err(Error::Message("context canceled".into()));
        }
        self.signal(Signal::SIGTERM).await?;
        self.wait(&CancellationToken::new(), grace).await
    }
}

fn send(pid: i32, signal: Signal) -> Result<(), Error> {
    match kill(Pid::from_raw(pid), signal) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        // Darwin refuses to signal a zombie, a process that has exited but is not reaped
        // yet, and a group whose only members are zombies, with EPERM: it has stopped.
        Err(Errno::EPERM) if only_zombies(pid) => Ok(()),
        Err(e) => Err(Error::Message(e.to_string())),
    }
}

/// Whether every process `pid` names (a process, or a group when negative) has exited,
/// gone or a zombie, as `ps` says.
fn only_zombies(pid: i32) -> bool {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,pgid=,stat="])
        .output()
    else {
        return false;
    };
    out.status.success() && only_zombies_in(&String::from_utf8_lossy(&out.stdout), pid)
}

fn only_zombies_in(table: &str, pid: i32) -> bool {
    table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let row: i32 = fields.next()?.parse().ok()?;
            let group: i32 = fields.next()?.parse().ok()?;
            let named = if pid < 0 { group == -pid } else { row == pid };
            named.then(|| fields.next().unwrap_or(""))
        })
        .all(|state| state.starts_with('Z'))
}

#[cfg(test)]
struct TestOps {
    read: Box<dyn Fn() -> Result<HashMap<i32, ProcessInfo>, Error> + Send + Sync>,
    send: Box<dyn Fn(i32, Signal) -> Result<(), Error> + Send + Sync>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_of_zombies_has_stopped() {
        let table = "  10  10 Ss\n  11  10 Z\n  12  12 Z+\n  13  13 R\n";
        assert!(
            !only_zombies_in(table, -10),
            "the group's leader still runs"
        );
        assert!(only_zombies_in(table, 11));
        assert!(only_zombies_in(table, -12));
        assert!(only_zombies_in(table, 99), "gone");
        assert!(!only_zombies_in(table, 13));
    }
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn capture_and_reparented_escalation() {
        let table = parse_table("10 1 10 Thu Oct 15 10:00:00 2026\n20 10 20 Thu Oct 15 10:00:01 2026\n30 20 30 Thu Oct 15 10:00:02 2026\n40 1 40 Thu Oct 15 10:00:03 2026").unwrap();
        let mut tree = Tree::from_table(table.clone(), 10);
        assert_eq!(tree.members.len(), 3);
        assert_eq!(tree.members[0].pid, 30);
        let shared = Arc::new(Mutex::new(table));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let read = shared.clone();
        let killed = shared.clone();
        let signals = calls.clone();
        tree.ops = Some(TestOps {
            read: Box::new(move || Ok(read.lock().unwrap().clone())),
            send: Box::new(move |pid, signal| {
                signals.lock().unwrap().push((pid, signal));
                if signal == Signal::SIGKILL {
                    killed.lock().unwrap().remove(&pid);
                }
                Ok(())
            }),
        });
        tree.signal(Signal::SIGTERM).await.unwrap();
        for pid in [20, 30, -10] {
            assert!(calls.lock().unwrap().contains(&(pid, Signal::SIGTERM)));
        }
        {
            let mut table = shared.lock().unwrap();
            table.remove(&10);
            table.get_mut(&30).unwrap().parent = 1;
            let reused = table.get_mut(&20).unwrap();
            reused.parent = 1;
            reused.start = "new process".into();
            table.insert(
                10,
                ProcessInfo {
                    pid: 10,
                    parent: 1,
                    group: 10,
                    start: "reused group leader".into(),
                },
            );
        }
        tree.wait(&CancellationToken::new(), Duration::from_millis(1))
            .await
            .unwrap();
        let calls = calls.lock().unwrap();
        assert!(calls.contains(&(30, Signal::SIGKILL)));
        for pid in [10, 20, 40, -10] {
            assert!(!calls.contains(&(pid, Signal::SIGKILL)));
        }
        assert!(!calls.contains(&(40, Signal::SIGTERM)));
    }

    #[tokio::test]
    async fn stop_finishes_after_caller_cancellation() {
        let table = HashMap::from([(
            10,
            ProcessInfo {
                pid: 10,
                parent: 1,
                group: 10,
                start: "original".into(),
            },
        )]);
        let mut tree = Tree::from_table(table.clone(), 10);
        let shared = Arc::new(Mutex::new(table));
        let read = shared.clone();
        let killed = shared.clone();
        let cancel = CancellationToken::new();
        let signal_cancel = cancel.clone();
        tree.ops = Some(TestOps {
            read: Box::new(move || Ok(read.lock().unwrap().clone())),
            send: Box::new(move |_, signal| {
                if signal == Signal::SIGTERM {
                    signal_cancel.cancel();
                }
                if signal == Signal::SIGKILL {
                    killed.lock().unwrap().remove(&10);
                }
                Ok(())
            }),
        });
        tree.stop(&cancel, Duration::from_millis(1)).await.unwrap();
        assert!(shared.lock().unwrap().is_empty());
    }
}
