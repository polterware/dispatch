//! Installs the dispatch skill into each agent's skills dir.
//!
//! Ported from `dispatch/backend/providers/skill`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::hook::{claude_config_dir, write_file};

/// The SKILL.md written into each skills directory.
pub const CONTENT: &[u8] = include_bytes!("SKILL.md");

pub const NAME: &str = "dispatch";

/// A skills directory read by one or more agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Absolute skills directory.
    pub dir: PathBuf,
    /// Agents that read it.
    pub agents: Vec<String>,
}

/// Maps agents to skills directories under home. Codex and Grok share
/// ~/.agents/skills (Grok verified via `grok inspect`; Codex and Antigravity
/// paths are not verified against their docs).
pub fn targets(home: &Path) -> BTreeMap<String, Target> {
    let claude_dir = match claude_config_dir() {
        Some(ccd) => ccd.join("skills"),
        None => home.join(".claude").join("skills"),
    };
    let shared = home.join(".agents").join("skills");
    let dirs = [
        ("claude", claude_dir),
        ("cursor", home.join(".cursor").join("skills")),
        ("codex", shared.clone()),
        ("grok", shared),
        ("antigravity", home.join(".gemini/config/skills")),
    ];
    dirs.into_iter()
        .map(|(agent, dir)| {
            let target = Target {
                dir,
                agents: Vec::new(),
            };
            (agent.to_string(), target)
        })
        .collect()
}

fn skill_path(dir: &Path) -> PathBuf {
    dir.join(NAME).join("SKILL.md")
}

/// Reports whether the dispatch skill exists in dir.
pub fn is_installed(dir: &Path) -> bool {
    fs::metadata(skill_path(dir)).is_ok()
}

/// Writes SKILL.md into dir/dispatch/. It refuses to overwrite an existing
/// skill unless force is set. It returns the written path.
pub fn install(dir: &Path, force: bool) -> Result<PathBuf, Error> {
    let path = skill_path(dir);
    if fs::metadata(&path).is_ok() && !force {
        return Err(Error::Message(format!(
            "{} already exists (use --force)",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_file(&path, CONTENT, 0o644)?;
    Ok(path)
}

/// Rewrites the skill only where dispatch already installed it, so an update
/// never creates new skill files. It returns the paths rewritten.
pub fn refresh(home: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for t in targets(home).into_values() {
        if !seen.insert(t.dir.clone()) {
            continue;
        }
        let path = skill_path(&t.dir);
        if fs::metadata(&path).is_err() {
            continue;
        }
        write_file(&path, CONTENT, 0o644)?;
        out.push(path);
    }
    Ok(out)
}
