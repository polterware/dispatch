use crate::Error;
use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg},
};
use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

fn temp_candidate(path: &Path) -> (PathBuf, PathBuf) {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state".into());
    let suffix: u64 = rand::random();
    (
        dir.to_path_buf(),
        dir.join(format!(".{base}-{suffix:016x}")),
    )
}

pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut tmp_opt = None;
    for _ in 0..32 {
        let (_, candidate) = temp_candidate(path);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(f) => {
                tmp_opt = Some((candidate, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let Some((tmp_path, mut file)) = tmp_opt else {
        return Err(Error::Message("failed to create temporary file".into()));
    };
    let result = (|| -> Result<(), Error> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

pub async fn write_file_atomic_async(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut tmp_opt = None;
    for _ in 0..32 {
        let (_, candidate) = temp_candidate(path);
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(0o600);
        match opts.open(&candidate).await {
            Ok(f) => {
                tmp_opt = Some((candidate, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let Some((tmp_path, mut file)) = tmp_opt else {
        return Err(Error::Message("failed to create temporary file".into()));
    };
    let result = async {
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&tmp_path, path).await?;
        Ok::<(), Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
    result
}

fn lock_sidecar_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

fn acquire_sidecar_lock(path: &Path) -> Result<Flock<File>, Error> {
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        let mut builder = DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
    }
    let lock_path = lock_sidecar_path(path);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)?;
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(locked) => return Ok(locked),
            Err((returned, errno)) => {
                file = returned;
                let retryable = errno == Errno::EWOULDBLOCK || errno == Errno::EAGAIN;
                if !retryable || Instant::now() > deadline {
                    let base = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    return Err(Error::Message(format!(
                        "{base} lock: {}",
                        std::io::Error::from(errno)
                    )));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

pub fn with_file_lock<F>(path: &Path, f: F) -> Result<(), Error>
where
    F: FnOnce() -> Result<(), Error>,
{
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let flock = acquire_sidecar_lock(path)?;
    let res = f();
    let _ = flock.unlock();
    res
}
