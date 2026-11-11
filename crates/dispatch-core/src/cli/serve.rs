//! `dispatch serve`, the core: the only process that opens the store. The
//! app, the CLI, and Polterware attach to it through core.json. `dispatch
//! stop` asks it to exit.

use super::{CliResult, Fail, Flags, flags::parse_duration};
use crate::{
    coreclient, host,
    runstate::{self, build_id, executable},
    server::{self, Core},
    store::Store,
};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const LOGIN_PATH_MARKER: &str = "__DISPATCH_PATH__";

/// Runs the core. Everything that touches the process environment happens
/// here, before the runtime starts its threads.
pub(super) fn run_serve(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("serve");
    f.string("listen", "127.0.0.1:0", "loopback listen address")
        .string(
            "token",
            &std::env::var("PWDISPATCH_SESSION_TOKEN").unwrap_or_default(),
            "session bearer token (default: minted)",
        )
        .duration(
            "idle",
            idle_from_env(),
            "exit after this long without clients (0, the default, keeps running)",
        );
    f.parse(args)?;

    let lock = match runstate::lock_core(db_path) {
        Ok(lock) => lock,
        Err(e) if runstate::is_core_locked(&e) => {
            match runstate::read_core(db_path) {
                Some(existing) => eprintln!(
                    "dispatch core already running for this database (pid {}, {})",
                    existing.pid, existing.endpoint
                ),
                None => eprintln!("dispatch core already running for this database"),
            }
            return Err(Fail::Exit(3));
        }
        Err(e) => return Err(super::usage_err(format!("core lock: {e}"))),
    };
    reset_inherited_state();
    adopt_login_path();

    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(serve(
        db_path,
        f.str("listen"),
        f.str("token"),
        f.dur("idle"),
    ));
    let _ = lock.close();
    result
}

async fn serve(db_path: &Path, listen: String, token: String, idle: Duration) -> CliResult {
    let store = Store::new(&db_path.to_string_lossy())
        .map_err(|e| super::usage_err(format!("db open: {e}")))?;
    let token = if token.is_empty() {
        server::mint_token()
    } else {
        token
    };
    let (listener, endpoint) = server::listen(&listen)
        .await
        .map_err(|e| super::usage_err(format!("listen: {e}")))?;
    eprintln!("dispatch core listening on {endpoint} with session-token authentication");

    let polterware_db = host::default_db();
    let build = executable().map(build_id).unwrap_or_default();
    let core = Core::new(
        Arc::new(store),
        token.clone(),
        Arc::new(host::Client::new(polterware_db.clone())),
        db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        build.clone(),
    );
    match core.seed_from_polterware(&polterware_db) {
        Err(e) => eprintln!("import from Polterware: {e}"),
        Ok(n) if n > 0 => eprintln!("imported {n} tier entries from Polterware"),
        Ok(_) => {}
    }
    // OpenCode reaches dispatch through the CLI and the skill; the plugin an
    // earlier version installed is gone.
    if let Some(home) = dirs::home_dir() {
        match crate::setup::remove_legacy_opencode_plugin(&home, &core.state_dir()) {
            Ok(true) => {
                eprintln!("removed the Dispatch plugin an earlier version installed in OpenCode")
            }
            Ok(false) => {}
            Err(e) => eprintln!("OpenCode plugin cleanup: {e}"),
        }
    }
    let server = tokio::spawn(server::serve(listener, core.clone()));
    // The prices and benchmarks age out by themselves, never in a request's way.
    core.spawn_catalog_refresh();

    let pid = i32::try_from(std::process::id()).unwrap_or(0);
    let state = runstate::Core {
        pid,
        endpoint,
        token,
        db_path: db_path.to_string_lossy().into_owned(),
        build,
        started_at: chrono::Utc::now(),
    };
    if let Err(e) = runstate::write_json(runstate::core_path(db_path), &state) {
        eprintln!("run state: {e}");
    }

    use tokio::signal::unix::{SignalKind, signal};
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sighup = signal(SignalKind::hangup())?;
    let mut ticker = tokio::time::interval(Duration::from_secs(30));
    ticker.tick().await;
    let done = core.done();
    loop {
        tokio::select! {
            _ = sigint.recv() => break,
            _ = sigterm.recv() => break,
            _ = sighup.recv() => break,
            () = done.cancelled() => break,
            _ = ticker.tick() => {
                if !idle.is_zero() && core.idle(idle) {
                    eprintln!("dispatch core idle for {}; exiting", super::flags::format_duration(idle));
                    break;
                }
            }
        }
    }
    core.stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    runstate::remove_core(db_path, pid);
    Ok(())
}

pub(super) async fn run_stop(db_path: &Path) -> CliResult {
    if runstate::read_core(db_path).is_none() {
        println!("no dispatch core is running for this database");
        return Ok(());
    }
    coreclient::stop(db_path).await?;
    println!("dispatch core stopped");
    Ok(())
}

fn idle_from_env() -> Duration {
    std::env::var("PWDISPATCH_CORE_IDLE")
        .ok()
        .and_then(|v| parse_duration(&v))
        .unwrap_or(Duration::ZERO)
}

/// Drops what the core inherits from the process that started it and a terminal would not
/// give it: blocked signals, an ignored SIGCHLD, and macOS background mode. The app starts
/// the core from a background thread, and a core started so never heard its children
/// exit, so each run waited forever on the first command it ran. Called while the core has
/// one thread, before the runtime starts.
#[allow(unsafe_code)]
fn reset_inherited_state() {
    // SAFETY: these calls change only this thread's signal mask, one signal's disposition,
    // and this process's priority.
    unsafe {
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        #[cfg(target_os = "macos")]
        libc::setpriority(libc::PRIO_DARWIN_PROCESS, 0, 0);
    }
}

/// Gives the core the PATH a terminal has. An app opened from the Finder
/// starts the core with launchd's PATH (/usr/bin:/bin:…), where no agent CLI
/// lives, so every agent read as not installed and a run could not find node
/// or the CLI itself. The login shell's PATH comes first, then the one the
/// core was given, then the usual install folders that exist.
#[allow(unsafe_code)]
fn adopt_login_path() {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let login = login_shell_path(&shell, Duration::from_secs(5));
    let mut common = vec!["/opt/homebrew/bin".to_owned(), "/usr/local/bin".to_owned()];
    if let Some(home) = dirs::home_dir() {
        common.push(
            home.join(".local")
                .join("bin")
                .to_string_lossy()
                .into_owned(),
        );
    }
    let path = merge_path(&login, &std::env::var("PATH").unwrap_or_default(), &common);
    // SAFETY: called from run_serve before the runtime starts, while this
    // process has one thread.
    unsafe { std::env::set_var("PATH", path) };
}

/// Asks the user's interactive login shell for its PATH, empty when it
/// cannot say in time.
fn login_shell_path(shell: &str, timeout: Duration) -> String {
    let shell = if shell.is_empty() { "/bin/zsh" } else { shell };
    let script = format!("printf '\\n{LOGIN_PATH_MARKER}%s\\n' \"$PATH\"");
    let Ok(mut child) = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let mut stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut out);
        }
        let _ = tx.send(out);
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    // A child that keeps stdout open does not hold the core.
    let out = rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
    path_after_marker(&String::from_utf8_lossy(&out))
}

/// Reads the PATH line a shell printed among whatever its startup files print.
fn path_after_marker(out: &str) -> String {
    out.lines()
        .find_map(|line| line.strip_prefix(LOGIN_PATH_MARKER))
        .map(|rest| rest.trim().to_owned())
        .unwrap_or_default()
}

/// Joins the login PATH, the current one, and the common folders that exist,
/// each folder once, in that order.
fn merge_path(login: &str, current: &str, common: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut add = |dir: &str| {
        if !dir.is_empty() && seen.insert(dir.to_owned()) {
            dirs.push(dir.to_owned());
        }
    };
    for dir in login.split(':') {
        add(dir);
    }
    for dir in current.split(':') {
        add(dir);
    }
    for dir in common {
        if Path::new(dir).is_dir() {
            add(dir);
        }
    }
    dirs.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_marked_path_line() {
        let out = "motd\n\n__DISPATCH_PATH__/opt/a:/usr/bin \nbye\n";
        assert_eq!(path_after_marker(out), "/opt/a:/usr/bin");
        assert_eq!(path_after_marker("no marker"), "");
    }

    #[test]
    fn merges_each_folder_once() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().to_string_lossy().into_owned();
        let got = merge_path(
            "/a:/b",
            "/b:/c",
            &[existing.clone(), "/does/not/exist".into()],
        );
        assert_eq!(got, format!("/a:/b:/c:{existing}"));
    }
}
