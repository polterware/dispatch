use crate::Error;
use std::{
    path::Path,
    process::{Command, ExitStatus},
    sync::atomic::{AtomicU8, Ordering},
};

fn format_status(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => status.to_string(),
    }
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Message(format!("git {}: {e}: ", args.join(" "))))?;

    let mut combined = output.stdout;
    if !output.stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with(b"\n") {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&output.stderr);
    }
    let s = String::from_utf8_lossy(&combined).trim().to_owned();
    if !output.status.success() {
        return Err(Error::Message(format!(
            "git {}: {}: {s}",
            args.join(" "),
            format_status(&output.status)
        )));
    }
    Ok(s)
}

/// Returns stdout untouched (no trimming, no stderr mixed in) and the exit code.
pub(crate) fn git_raw(dir: &Path, args: &[&str]) -> Result<(String, i32), Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Message(format!("git {}: {e}: ", args.join(" "))))?;

    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok((out, 0));
    }
    let code = output.status.code().unwrap_or(-1);
    Ok((out, code))
}

pub fn git_exit(dir: impl AsRef<Path>, args: &[&str]) -> Result<(String, i32), Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir.as_ref())
        .args(args)
        .output()?;
    let mut combined = output.stdout;
    if !output.stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with(b"\n") {
            combined.push(b'\n');
        }
        combined.extend_from_slice(&output.stderr);
    }
    let s = String::from_utf8_lossy(&combined).trim().to_owned();
    if output.status.success() {
        return Ok((s, 0));
    }
    Ok((s, output.status.code().unwrap_or(-1)))
}

pub fn parse_git_version(mut s: &str) -> Option<(i32, i32)> {
    if let Some(idx) = s.find("git version ") {
        s = &s[idx + "git version ".len()..];
    }
    let first = s.trim().split(' ').next()?;
    if first.is_empty() {
        return None;
    }
    let mut nums = first.split('.');
    let maj = nums.next()?.parse::<i32>().ok()?;
    let min = nums.next()?.parse::<i32>().ok()?;
    Some((maj, min))
}

pub fn version_supports_merge_tree(major: i32, minor: i32) -> bool {
    major > 2 || (major == 2 && minor >= 38)
}

static MERGE_TREE_OVERRIDE: AtomicU8 = AtomicU8::new(0);

pub fn set_supports_merge_tree_override(override_val: Option<bool>) {
    let encoded = match override_val {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    MERGE_TREE_OVERRIDE.store(encoded, Ordering::Relaxed);
}

/// Reports whether git supports `merge-tree --write-tree` (git >= 2.38).
pub fn supports_merge_tree(root: impl AsRef<Path>) -> bool {
    match MERGE_TREE_OVERRIDE.load(Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    let Ok(out) = git(root.as_ref(), &["version"]) else {
        return false;
    };
    let Some((maj, min)) = parse_git_version(&out) else {
        return false;
    };
    version_supports_merge_tree(maj, min)
}
