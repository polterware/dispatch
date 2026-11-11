//! Agent setup: reports and changes how the agent CLIs on this machine reach
//! dispatch: the `dispatch` command on the PATH, the dispatch skill in the
//! skills folders they read, and each agent's SessionStart hook.

mod jsonc;
mod opencode;

pub use opencode::{opencode_config_path, opencode_plugin_dir, remove_legacy_opencode_plugin};

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, hook, skill};

/// The `dispatch` command agents run: Linked when it is this binary, Other
/// when another file holds its place (an older copy), Missing otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Cli {
    pub path: String,
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub binary: String,
}

/// The dispatch skill in one skills folder: Current, Outdated (another
/// version of it), or Missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Skill {
    /// The skills folder.
    pub dir: String,
    /// The skill's SKILL.md in it.
    pub path: String,
    /// The agents that read the folder.
    pub agents: Vec<String>,
    pub state: String,
    /// An agent's own folder the shared one already reaches, listed only
    /// while it holds a copy, so the copy can be removed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub extra: bool,
}

/// One agent's SessionStart hook: On when it is registered and the enabled
/// flag is set; command is what it runs, current when that is this binary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Hook {
    pub agent: String,
    pub on: bool,
    /// The file the hook lives in.
    pub settings: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    pub current: bool,
}

/// The whole setup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub cli: Cli,
    pub skills: Vec<Skill>,
    /// The hook of every agent on this machine that has one, and of any
    /// other agent whose hook is still registered.
    pub hooks: Vec<Hook>,
    /// The agents whose config folder is on this machine.
    pub agents: Vec<String>,
}

pub const LINKED: &str = "linked";
pub const OTHER: &str = "other";
pub const MISSING: &str = "missing";
pub const CURRENT: &str = "current";
pub const OUTDATED: &str = "outdated";

pub(crate) fn lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// join_errors is Go's errors.Join: nil for none, the error itself for one,
// the messages on separate lines for more.
pub(crate) fn join_errors(mut errs: Vec<Error>) -> Result<(), Error> {
    match errs.len() {
        0 => Ok(()),
        1 => Err(errs.remove(0)),
        _ => Err(Error::Message(
            errs.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// Where the `dispatch` command is linked: ~/.local/bin, the folder the agent
/// CLIs install themselves into.
pub fn cli_path(home: &Path) -> PathBuf {
    home.join(".local").join("bin").join("dispatch")
}

/// Reports the setup for binary, the dispatch executable agents should run.
pub fn read(home: &Path, binary: &str, state_dir: &Path) -> Report {
    let mut r = Report {
        cli: read_cli(home, binary),
        ..Report::default()
    };
    let mut seen = HashSet::new();
    for folder in skill::folders(home) {
        if !seen.insert(folder.dir.clone()) {
            continue;
        }
        let path = folder.skill_path();
        let state = match fs::read(&path) {
            Ok(b) if b == skill::CONTENT => CURRENT,
            Ok(_) => OUTDATED,
            Err(_) => MISSING,
        };
        if folder.extra && state == MISSING {
            continue;
        }
        r.skills.push(Skill {
            dir: lossy(&folder.dir),
            path: lossy(&path),
            agents: folder.agents,
            state: state.into(),
            extra: folder.extra,
        });
    }
    r.agents = present_agents(home);
    let enabled = hook::is_enabled(state_dir);
    for agent in hook::HOOK_AGENTS {
        let Some(h) = hook::agent_hook(home, agent) else {
            continue;
        };
        let command = h.registered();
        if command.is_empty() && !r.agents.iter().any(|a| a == agent) {
            continue;
        }
        r.hooks.push(Hook {
            agent: agent.into(),
            on: !command.is_empty() && enabled,
            settings: lossy(&h.path),
            current: !command.is_empty() && command == hook::agent_hook_command(binary, agent),
            command,
        });
    }
    r
}

/// The agents whose config folder is on this machine, the sign an agent CLI
/// was installed and run here.
pub fn present_agents(home: &Path) -> Vec<String> {
    let env = |key: &str| {
        std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|d| d.is_absolute())
    };
    let config = env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    let claude = hook::claude_config_dir().unwrap_or_else(|| home.join(".claude"));
    let codex = env("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
    let signs: [(&str, Vec<PathBuf>); 11] = [
        ("claude", vec![claude]),
        ("codex", vec![codex]),
        ("cursor", vec![home.join(".cursor")]),
        (
            "gemini",
            vec![
                home.join(".gemini/settings.json"),
                home.join(".gemini/oauth_creds.json"),
            ],
        ),
        ("antigravity", vec![home.join(".gemini/antigravity")]),
        (
            "opencode",
            vec![config.join("opencode"), home.join(".local/share/opencode")],
        ),
        ("grok", vec![home.join(".grok")]),
        ("pi", vec![home.join(".pi/agent")]),
        ("qwen", vec![home.join(".qwen")]),
        ("kimi", vec![home.join(".kimi-code"), home.join(".kimi")]),
        ("copilot", vec![home.join(".copilot")]),
    ];
    signs
        .into_iter()
        .filter(|(_, paths)| paths.iter().any(|p| fs::metadata(p).is_ok()))
        .map(|(agent, _)| agent.to_string())
        .collect()
}

fn read_cli(home: &Path, binary: &str) -> Cli {
    let path = cli_path(home);
    let mut c = Cli {
        path: lossy(&path),
        state: MISSING.into(),
        target: String::new(),
        binary: binary.to_string(),
    };
    let Ok(info) = fs::symlink_metadata(&path) else {
        return c;
    };
    c.state = OTHER.into();
    c.target = c.path.clone();
    if info.file_type().is_symlink()
        && let Ok(target) = fs::canonicalize(&path)
    {
        c.target = lossy(&target);
    }
    if same(Path::new(&c.target), Path::new(binary)) {
        c.state = LINKED.into();
    }
    c
}

fn same(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ra), Ok(rb)) => ra == rb,
        _ => false,
    }
}

/// Points the `dispatch` command at binary, replacing whatever held its
/// place: a link, or an older dispatch copy. It refuses a folder.
pub fn link_cli(home: &Path, binary: &str) -> Result<PathBuf, Error> {
    let path = cli_path(home);
    if let Ok(info) = fs::symlink_metadata(&path) {
        if info.is_dir() {
            return Err(Error::Message(format!("{} is a folder", path.display())));
        }
        fs::remove_file(&path)?;
    }
    fs::create_dir_all(hook::parent_dir(&path))?;
    std::os::unix::fs::symlink(binary, &path)?;
    Ok(path)
}

// skill_folder is the folder key names (a folder's path, or an agent).
fn skill_folder(home: &Path, key: &str) -> Result<skill::Folder, Error> {
    skill::resolve(home, key)
        .ok_or_else(|| Error::Message(format!("unknown agent or skills folder {:?}", key.trim())))
}

/// Writes the current skill into the folder key names (a folder's path, or
/// an agent, for the folder that agent reads), or into the three Dispatch
/// installs into when it is empty, over any older copy. It returns the paths
/// written.
pub fn install_skill(home: &Path, key: &str) -> Result<Vec<PathBuf>, Error> {
    if !key.trim().is_empty() {
        let folder = skill_folder(home, key)?;
        return Ok(vec![skill::install(&folder.dir, true)?]);
    }
    let mut out = Vec::new();
    for folder in skill::folders(home).into_iter().filter(|f| !f.extra) {
        out.push(skill::install(&folder.dir, true)?);
    }
    Ok(out)
}

/// Removes the skill from the folder key names (a folder's path, or an
/// agent), or from every folder when it is empty. It returns what it removed.
pub fn remove_skill(home: &Path, key: &str) -> Result<Vec<PathBuf>, Error> {
    let folders = if key.trim().is_empty() {
        skill::folders(home)
    } else {
        vec![skill_folder(home, key)?]
    };
    let mut out = Vec::new();
    for folder in folders {
        out.extend(skill::remove(&folder.dir)?);
    }
    Ok(out)
}

/// Registers agent's SessionStart hook for binary and sets the enabled flag,
/// or removes the hook, and the flag with the last one. An empty agent is
/// Claude Code.
pub fn set_hook(
    home: &Path,
    binary: &str,
    state_dir: &Path,
    agent: &str,
    on: bool,
) -> Result<(), Error> {
    let agent = match agent.trim() {
        "" => "claude",
        agent => agent,
    };
    let Some(h) = hook::agent_hook(home, agent) else {
        return Err(Error::Message(format!(
            "{agent:?} has no session hook: use one of {}",
            hook::HOOK_AGENTS.join(", ")
        )));
    };
    if on {
        hook::set_enabled(state_dir, true)?;
        return h.install(binary);
    }
    h.uninstall()?;
    let none_left = hook::HOOK_AGENTS
        .iter()
        .filter_map(|a| hook::agent_hook(home, a))
        .all(|h| h.registered().is_empty());
    if none_left {
        hook::set_enabled(state_dir, false)?;
    }
    Ok(())
}
