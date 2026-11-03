use crate::Error;
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Resolves this binary; the core is its own `serve` subcommand.
pub fn executable() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Message(format!("resolve dispatch executable: {e}")))?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// Identifies a binary by modification time and size (`"<unix seconds>-<bytes>"`).
pub fn build_id(exe: impl AsRef<Path>) -> String {
    let Ok(info) = std::fs::metadata(exe.as_ref()) else {
        return String::new();
    };
    format!("{}-{}", info.mtime(), info.len())
}

/// Reports whether build `a` is a newer binary than build `b`.
pub fn newer_build(a: &str, b: &str) -> bool {
    build_time(a) > build_time(b)
}

fn build_time(build: &str) -> i64 {
    let head = build.split_once('-').map_or(build, |(h, _)| h);
    head.parse::<i64>().unwrap_or(0)
}
