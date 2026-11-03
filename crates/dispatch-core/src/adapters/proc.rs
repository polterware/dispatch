use std::io::Write;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{
    failfast::FatalWatch,
    livelog::{LiveLog, StreamFilter},
};
use crate::{Error, process::Tree};

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub fatal: String,
}

async fn read_stream<R: AsyncRead + Unpin>(
    mut reader: R,
    mut log: LiveLog,
    mut watch: Option<FatalWatch>,
    filter_name: Option<String>,
) -> Vec<u8> {
    let mut output = Vec::new();
    let mut pending = Vec::new();
    let mut filter = filter_name.as_deref().map(StreamFilter::new);
    let mut buf = [0; 8192];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        output.extend_from_slice(&buf[..n]);
        if let Some(w) = watch.as_mut() {
            let _ = w.write_all(&buf[..n]);
        }
        if let Some(filter) = filter.as_mut() {
            pending.extend_from_slice(&buf[..n]);
            while let Some(i) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=i).collect();
                let value = String::from_utf8_lossy(&line);
                let _ = log.write_all(filter.line(value.trim_end_matches(['\r', '\n'])).as_bytes());
            }
        } else {
            let _ = log.write_all(&buf[..n]);
        }
    }
    if let Some(filter) = filter.as_mut()
        && !pending.is_empty()
    {
        let _ = log.write_all(filter.line(&String::from_utf8_lossy(&pending)).as_bytes());
    }
    output
}

pub async fn run_command(
    mut command: Command,
    input: Option<String>,
    cancel: CancellationToken,
    log: LiveLog,
    filter_name: Option<&str>,
) -> Result<CommandOutput, Error> {
    command
        .process_group(0)
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| Error::Message("agent process has no pid".into()))? as i32;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
        });
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Message("agent stdout unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Message("agent stderr unavailable".into()))?;
    let fatal_cancel = CancellationToken::new();
    let watch = FatalWatch::new(fatal_cancel.clone());
    let mut out_task = tokio::spawn(read_stream(
        stdout,
        log.clone(),
        None,
        filter_name.map(str::to_owned),
    ));
    let mut err_task = tokio::spawn(read_stream(stderr, log.clone(), Some(watch.clone()), None));
    let mut interrupted = false;
    let status = tokio::select! {
        status = child.wait() => status?,
        _ = cancel.cancelled() => { interrupted = true; stop_child(&mut child, pid).await? },
        _ = fatal_cancel.cancelled() => { interrupted = true; stop_child(&mut child, pid).await? },
    };
    let stdout = match tokio::time::timeout(Duration::from_secs(2), &mut out_task).await {
        Ok(Ok(v)) => v,
        _ => {
            out_task.abort();
            Vec::new()
        }
    };
    let stderr = match tokio::time::timeout(Duration::from_secs(2), &mut err_task).await {
        Ok(Ok(v)) => v,
        _ => {
            err_task.abort();
            Vec::new()
        }
    };
    log.close();
    if interrupted && cancel.is_cancelled() && watch.fatal().is_empty() {
        return Err(Error::Message("context canceled".into()));
    }
    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code: status.code().unwrap_or(-1),
        fatal: watch.fatal(),
    })
}

async fn stop_child(
    child: &mut tokio::process::Child,
    pid: i32,
) -> Result<std::process::ExitStatus, Error> {
    let tree = match Tree::capture(pid).await {
        Ok(t) => t,
        Err(err) => {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-pid),
                nix::sys::signal::Signal::SIGTERM,
            );
            let _ = child.kill().await;
            return Err(Error::Message(format!(
                "agent process cleanup incomplete: {err}"
            )));
        }
    };
    let signal_error = tree.signal(nix::sys::signal::Signal::SIGTERM).await.err();
    let cleanup_cancel = CancellationToken::new();
    let (status, cleanup) = tokio::join!(
        child.wait(),
        tree.wait(&cleanup_cancel, Duration::from_secs(5))
    );
    if let Some(err) = signal_error.or_else(|| cleanup.err()) {
        return Err(Error::Message(format!(
            "agent process cleanup incomplete: {err}"
        )));
    }
    Ok(status?)
}

pub(crate) struct RpcProcess {
    child: tokio::process::Child,
    pid: i32,
    stdin: Option<tokio::process::ChildStdin>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    stderr: tokio::task::JoinHandle<Vec<u8>>,
    watch: FatalWatch,
    fatal_cancel: CancellationToken,
    cancel: CancellationToken,
}

impl RpcProcess {
    pub(crate) fn start(
        mut command: Command,
        cancel: CancellationToken,
        log: LiveLog,
    ) -> Result<Self, Error> {
        use tokio::io::AsyncBufReadExt;
        command
            .process_group(0)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| Error::Message("agent process has no pid".into()))?
            as i32;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Message("agent stdout unavailable".into()))?;
        let err = child
            .stderr
            .take()
            .ok_or_else(|| Error::Message("agent stderr unavailable".into()))?;
        let fatal_cancel = CancellationToken::new();
        let watch = FatalWatch::new(fatal_cancel.clone());
        let stderr = tokio::spawn(read_stream(err, log, Some(watch.clone()), None));
        Ok(Self {
            child,
            pid,
            stdin,
            lines: tokio::io::BufReader::new(stdout).lines(),
            stderr,
            watch,
            fatal_cancel,
            cancel,
        })
    }
    pub(crate) async fn send(&mut self, message: serde_json::Value) -> Result<(), Error> {
        if let Some(input) = &mut self.stdin {
            let mut bytes = serde_json::to_vec(&message)?;
            bytes.push(b'\n');
            input.write_all(&bytes).await?;
        }
        Ok(())
    }
    pub(crate) async fn next(&mut self) -> Result<Option<String>, Error> {
        tokio::select! {
            line = self.lines.next_line() => Ok(line?),
            _ = self.cancel.cancelled() => Err(Error::Message("context canceled".into())),
            _ = self.fatal_cancel.cancelled() => Err(Error::Message(format!("stopped early: {}", self.watch.fatal()))),
        }
    }
    pub(crate) async fn finish(
        mut self,
        grace: Duration,
    ) -> Result<(i32, String, bool, String), Error> {
        self.stdin.take();
        let (status, forced) = tokio::select! {
            status = self.child.wait() => (status?, false),
            _ = tokio::time::sleep(grace) => (stop_child(&mut self.child, self.pid).await?, true),
            _ = self.cancel.cancelled() => { stop_child(&mut self.child, self.pid).await?; return Err(Error::Message("context canceled".into())); },
            _ = self.fatal_cancel.cancelled() => { stop_child(&mut self.child, self.pid).await?; return Err(Error::Message(format!("stopped early: {}",self.watch.fatal()))); },
        };
        let stderr = match tokio::time::timeout(Duration::from_secs(2), &mut self.stderr).await {
            Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            _ => {
                self.stderr.abort();
                String::new()
            }
        };
        Ok((
            status.code().unwrap_or(-1),
            stderr,
            forced,
            self.watch.fatal(),
        ))
    }
    pub(crate) async fn abort(mut self) -> Result<(), Error> {
        self.stdin.take();
        let result = stop_child(&mut self.child, self.pid).await.map(|_| ());
        self.stderr.abort();
        result
    }
}
