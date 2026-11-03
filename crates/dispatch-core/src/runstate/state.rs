use crate::Error;
use chrono::{DateTime, Utc};
use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg},
    sys::signal::kill,
    unistd::Pid,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const CORE_FILE: &str = "core.json";
pub const ERR_CORE_LOCKED: &str = "another dispatch core owns this database";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RunstateError {
    #[error("another dispatch core owns this database")]
    CoreLocked,
}

impl From<RunstateError> for Error {
    fn from(err: RunstateError) -> Self {
        Error::Message(err.to_string())
    }
}

pub fn is_core_locked(err: &Error) -> bool {
    matches!(err, Error::Message(msg) if msg == ERR_CORE_LOCKED)
}

/// Describes the running core (`dispatch serve`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Core {
    pub pid: i32,
    pub endpoint: String,
    pub token: String,
    pub db_path: String,
    pub build: String,
    pub started_at: DateTime<Utc>,
}

impl Default for Core {
    fn default() -> Self {
        Self {
            pid: 0,
            endpoint: String::new(),
            token: String::new(),
            db_path: String::new(),
            build: String::new(),
            started_at: DateTime::<Utc>::UNIX_EPOCH,
        }
    }
}

/// Exclusive per-database core lock held until closed or dropped.
#[derive(Debug)]
pub struct CoreLock(Option<Flock<File>>);

impl CoreLock {
    pub fn close(mut self) -> Result<(), Error> {
        if let Some(flock) = self.0.take() {
            flock
                .unlock()
                .map(drop)
                .map_err(|(_, errno)| std::io::Error::from(errno))?;
        }
        Ok(())
    }
}

/// Returns the run-state directory for a database path.
pub fn dir(db_path: impl AsRef<Path>) -> PathBuf {
    let p = db_path.as_ref();
    match p.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("run"),
        _ if p == Path::new("/") => PathBuf::from("/run"),
        _ => PathBuf::from("run"),
    }
}

pub fn core_path(db_path: impl AsRef<Path>) -> PathBuf {
    dir(db_path).join(CORE_FILE)
}

pub fn log_path(db_path: impl AsRef<Path>) -> PathBuf {
    dir(db_path).join("core.log")
}

pub fn lock_path(db_path: impl AsRef<Path>) -> PathBuf {
    dir(db_path).join("core.lock")
}

/// Creates the run-state directory with user-only permissions.
pub fn ensure_dir(db_path: impl AsRef<Path>) -> Result<(), Error> {
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir(db_path))?;
    Ok(())
}

/// Takes the exclusive per-database core lock (one core per database).
pub fn lock_core(db_path: impl AsRef<Path>) -> Result<CoreLock, Error> {
    let db_path = db_path.as_ref();
    ensure_dir(db_path)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(lock_path(db_path))?;
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(flock) => Ok(CoreLock(Some(flock))),
        Err((_file, errno)) => {
            if errno == Errno::EWOULDBLOCK || errno == Errno::EAGAIN {
                Err(RunstateError::CoreLocked.into())
            } else {
                Err(std::io::Error::from(errno).into())
            }
        }
    }
}

/// Writes `v` atomically with `0600` permissions.
pub fn write_json<T: Serialize>(path: impl AsRef<Path>, v: &T) -> Result<(), Error> {
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    let path = path.as_ref();
    let mut body = serde_json::to_vec_pretty(v)?;
    body.push(b'\n');

    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(parent)?;

    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut created = None;
    for attempt in 0..32u64 {
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".tmp-{pid}-{nanos:x}-{seq:x}-{attempt}"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => {
                created = Some((candidate, file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let Some((tmp_path, mut tmp_file)) = created else {
        return Err(Error::Message("create temporary file failed".into()));
    };

    let result = (|| -> Result<(), Error> {
        fs::set_permissions(&tmp_path, Permissions::from_mode(0o600))?;
        tmp_file.write_all(&body)?;
        tmp_file.sync_all()?;
        drop(tmp_file);
        fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Reports whether a process with `pid` exists.
pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match kill(Pid::from_raw(pid), None) {
        Ok(()) | Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// Returns the live core for `db_path`. A stale file is removed and reported as not found.
pub fn read_core(db_path: impl AsRef<Path>) -> Option<Core> {
    let path = core_path(db_path);
    let body = fs::read(&path).ok()?;
    let Ok(core) = serde_json::from_slice::<Core>(&body) else {
        let _ = fs::remove_file(&path);
        return None;
    };
    if !pid_alive(core.pid) {
        let _ = fs::remove_file(&path);
        return None;
    }
    Some(core)
}

/// Deletes `core.json` only when it still belongs to `pid`.
pub fn remove_core(db_path: impl AsRef<Path>, pid: i32) {
    let path = core_path(db_path);
    let Ok(body) = fs::read(&path) else {
        return;
    };
    if let Ok(core) = serde_json::from_slice::<Core>(&body)
        && core.pid != pid
    {
        return;
    }
    let _ = fs::remove_file(&path);
}
