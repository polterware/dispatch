use crate::{
    Error,
    coreclient::{
        rpc::{CLIENT_HEADER, RpcTransport},
        services::Clients,
    },
    proto::{CheckRequest, ShutdownRequest},
    runstate::{self, Core, build_id, executable, newer_build},
};
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use std::{
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    process::Command,
    sync::oneshot,
    time::{Instant, sleep, sleep_until, timeout},
};

const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds Connect clients for `core`; `client` names the caller (`Pwdispatch-Client`).
pub fn new(core: &Core, client: &str) -> Clients {
    Clients::from_transport(RpcTransport::new(
        &core.endpoint,
        &core.token,
        CLIENT_HEADER,
        client,
        None,
    ))
}

/// Ensures a core for `db_path` and returns clients for it.
pub async fn connect(db_path: impl AsRef<Path>, client: &str) -> Result<Clients, Error> {
    let core = ensure(db_path).await?;
    Ok(new(&core, client))
}

/// Returns a live core for `db_path`, starting one if needed and replacing an older build.
pub async fn ensure(db_path: impl AsRef<Path>) -> Result<Core, Error> {
    let exe = executable()?;
    ensure_with_exe(exe, db_path).await
}

pub async fn ensure_with_exe(
    exe: impl AsRef<Path>,
    db_path: impl AsRef<Path>,
) -> Result<Core, Error> {
    let exe = exe.as_ref();
    let db_path = db_path.as_ref();
    let build = build_id(exe);

    for _attempt in 0..3 {
        let Some(core) = runstate::read_core(db_path) else {
            return start(exe, db_path).await;
        };
        let client = new(&core, "cli");
        let Ok(Ok(health)) = timeout(
            Duration::from_secs(2),
            client.health.check(&CheckRequest {}),
        )
        .await
        else {
            sleep(Duration::from_millis(200)).await;
            continue;
        };
        if newer_build(&build, &health.build) {
            stop(db_path)
                .await
                .map_err(|e| Error::Message(format!("replace older core: {e}")))?;
            continue;
        }
        return Ok(core);
    }

    Err(Error::Message(format!(
        "core for {} is not responding; see {}",
        db_path.display(),
        runstate::log_path(db_path).display()
    )))
}

fn format_exit(result: Result<std::io::Result<ExitStatus>, oneshot::error::RecvError>) -> String {
    match result {
        Ok(Ok(status)) => match status.code() {
            Some(code) => format!("exit status {code}"),
            None => status.to_string(),
        },
        Ok(Err(err)) => err.to_string(),
        Err(_) => "process channel closed".into(),
    }
}

/// Launches a detached core and waits for its `core.json`.
pub async fn start(exe: impl AsRef<Path>, db_path: impl AsRef<Path>) -> Result<Core, Error> {
    start_with_timeout(exe, db_path, DEFAULT_START_TIMEOUT).await
}

pub async fn start_with_timeout(
    exe: impl AsRef<Path>,
    db_path: impl AsRef<Path>,
    wait_timeout: Duration,
) -> Result<Core, Error> {
    let exe = exe.as_ref();
    let db_path = db_path.as_ref();
    if let Some(core) = runstate::read_core(db_path) {
        return Ok(core);
    }
    runstate::ensure_dir(db_path)?;
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(runstate::log_path(db_path))?;
    let log_err = log_file.try_clone()?;

    let mut child = Command::new(exe)
        .arg("-db")
        .arg(db_path)
        .args(["serve", "--listen", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
        .process_group(0)
        .spawn()?;

    let (tx, mut exited) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(child.wait().await);
    });

    let deadline = Instant::now() + wait_timeout;
    loop {
        if let Some(core) = runstate::read_core(db_path) {
            return Ok(core);
        }
        tokio::select! {
            res = &mut exited => {
                // Exit 3: another core took the lock first; attach to it.
                for _ in 0..20 {
                    if let Some(core) = runstate::read_core(db_path) {
                        return Ok(core);
                    }
                    sleep(Duration::from_millis(100)).await;
                }
                return Err(Error::Message(format!(
                    "core exited before listening ({}); see {}",
                    format_exit(res),
                    runstate::log_path(db_path).display()
                )));
            }
            () = sleep_until(deadline) => {
                return Err(Error::Message(format!(
                    "timed out waiting for core; see {}",
                    runstate::log_path(db_path).display()
                )));
            }
            () = sleep(Duration::from_millis(100)) => {}
        }
    }
}

/// Asks the running core to shut down and waits for it to exit.
pub async fn stop(db_path: impl AsRef<Path>) -> Result<(), Error> {
    let db_path = db_path.as_ref();
    let Some(core) = runstate::read_core(db_path) else {
        return Ok(());
    };
    let client = new(&core, "cli");
    let shutdown_res = timeout(
        Duration::from_secs(2),
        client.health.shutdown(&ShutdownRequest {
            reason: "stop".into(),
        }),
    )
    .await;
    if !matches!(shutdown_res, Ok(Ok(_)))
        && let Err(errno) = kill(Pid::from_raw(core.pid), Signal::SIGTERM)
        && errno != Errno::ESRCH
    {
        return Err(std::io::Error::from(errno).into());
    }
    for _ in 0..50 {
        if !runstate::pid_alive(core.pid) {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err(Error::Message(format!(
        "core pid {} did not exit",
        core.pid
    )))
}
