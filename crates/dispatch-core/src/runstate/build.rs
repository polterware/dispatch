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

/// The `dispatch` command agents run: this binary, or, inside Dispatch.app,
/// the `Contents/Resources/dispatch` link to it, the path the command, the
/// skill, and the hook have always pointed at.
pub fn command_binary() -> Result<PathBuf, Error> {
    let exe = executable()?;
    if let Some(contents) = exe
        .parent()
        .filter(|d| d.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
    {
        let link = contents.join("Resources").join("dispatch");
        if std::fs::canonicalize(&link).is_ok_and(|t| t == exe) {
            return Ok(link);
        }
    }
    Ok(exe)
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
