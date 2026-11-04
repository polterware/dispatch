//! Installs and removes the dispatch skill in the skills folders the agent
//! CLIs read.
//!
//! Most agents read the shared Agent Skills folder, `~/.agents/skills`, so one
//! copy there reaches Codex, Cursor, Gemini CLI, OpenCode, Grok, Pi, Qwen Code,
//! Kimi Code, and Copilot. Claude Code reads only its own folder, and
//! Antigravity its own. The agents' own folders (`~/.cursor/skills`, …) are
//! read as well, so a copy in one of them next to the shared one is a second
//! copy: Dispatch never installs there and lists one only to remove it.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::hook::{claude_config_dir, write_file};

/// The SKILL.md written into each skills directory.
pub const CONTENT: &[u8] = include_bytes!("SKILL.md");

pub const NAME: &str = "dispatch";

/// The agents that read the shared folder, `~/.agents/skills`.
pub const SHARED_READERS: [&str; 9] = [
    "codex", "cursor", "gemini", "opencode", "grok", "pi", "qwen", "kimi", "copilot",
];

/// A skills folder and the agents that read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Absolute skills directory.
    pub dir: PathBuf,
    /// Agents that read it.
    pub agents: Vec<String>,
    /// An agent's own folder that the shared one already reaches.
    pub extra: bool,
}

impl Folder {
    fn new(dir: PathBuf, agents: &[&str], extra: bool) -> Self {
        Folder {
            dir,
            agents: agents.iter().map(|a| (*a).to_string()).collect(),
            extra,
        }
    }

    /// Where the skill's SKILL.md is in this folder.
    pub fn skill_path(&self) -> PathBuf {
        skill_path(&self.dir)
    }
}

/// Claude Code's skills folder: $CLAUDE_CONFIG_DIR/skills, else ~/.claude/skills.
pub fn claude_dir(home: &Path) -> PathBuf {
    match claude_config_dir() {
        Some(ccd) => ccd.join("skills"),
        None => home.join(".claude").join("skills"),
    }
}

// env_dir is an absolute directory an environment variable names.
fn env_dir(key: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(key)?);
    dir.is_absolute().then_some(dir)
}

/// The skills folders under home: the three Dispatch installs into (the
/// shared one, Claude Code's, Antigravity's), then the agents' own folders.
pub fn folders(home: &Path) -> Vec<Folder> {
    let codex = env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
    let config = env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    vec![
        Folder::new(home.join(".agents").join("skills"), &SHARED_READERS, false),
        Folder::new(claude_dir(home), &["claude"], false),
        Folder::new(home.join(".gemini/config/skills"), &["antigravity"], false),
        Folder::new(home.join(".cursor").join("skills"), &["cursor"], true),
        Folder::new(codex.join("skills"), &["codex"], true),
        Folder::new(home.join(".gemini").join("skills"), &["gemini"], true),
        Folder::new(config.join("opencode").join("skills"), &["opencode"], true),
        Folder::new(home.join(".grok").join("skills"), &["grok"], true),
        Folder::new(home.join(".pi/agent/skills"), &["pi"], true),
        Folder::new(home.join(".qwen").join("skills"), &["qwen"], true),
        Folder::new(home.join(".kimi-code").join("skills"), &["kimi"], true),
        Folder::new(home.join(".copilot").join("skills"), &["copilot"], true),
    ]
}

/// The folder `key` names: a folder's path, or an agent, which means the
/// folder Dispatch installs into for it (the shared one for most).
pub fn resolve(home: &Path, key: &str) -> Option<Folder> {
    let key = key.trim();
    let all = folders(home);
    if let Some(folder) = all.iter().find(|f| f.dir.as_os_str() == key) {
        return Some(folder.clone());
    }
    all.into_iter()
        .find(|f| !f.extra && f.agents.iter().any(|a| a == key))
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

/// Removes the dispatch skill from dir: its SKILL.md, then its folder when
/// nothing else is in it. A `dispatch` link is removed itself, never what it
/// points at. It returns what it removed, or None when there was no skill.
pub fn remove(dir: &Path) -> Result<Option<PathBuf>, Error> {
    let folder = dir.join(NAME);
    let info = match fs::symlink_metadata(&folder) {
        Ok(info) => info,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if info.file_type().is_symlink() {
        fs::remove_file(&folder)?;
        return Ok(Some(folder));
    }
    let path = skill_path(dir);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    // Only an empty folder goes: anything else in it is not Dispatch's.
    let _ = fs::remove_dir(&folder);
    Ok(Some(path))
}

/// Rewrites the skill only where dispatch already installed it, so an update
/// never creates new skill files. It returns the paths rewritten.
pub fn refresh(home: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for folder in folders(home) {
        if !seen.insert(folder.dir.clone()) {
            continue;
        }
        let path = folder.skill_path();
        if fs::metadata(&path).is_err() {
            continue;
        }
        write_file(&path, CONTENT, 0o644)?;
        out.push(path);
    }
    Ok(out)
}
