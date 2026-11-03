//! Agent setup: reports and changes how the agent CLIs on this machine reach
//! dispatch: the `dispatch` command on the PATH, the dispatch skill in each
//! agent's skills folder, the Claude Code SessionStart hook, and the Dispatch
//! plugin in OpenCode.

mod jsonc;
mod opencode;

pub use opencode::{
    OPENCODE_SWITCH, OpenCode, OpenCodeEntry, edit_opencode_config, install_opencode_plugin,
    opencode_config_path, opencode_plugin_dir, remove_opencode_plugin, set_opencode_delegation,
};

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

/// The dispatch skill for one agent: Current, Outdated (another version of
/// it), or Missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Skill {
    pub agent: String,
    pub path: String,
    pub state: String,
}

/// The Claude Code SessionStart hook: On when it is registered and the
/// enabled flag is set; command is what it runs, current when that is this
/// binary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Hook {
    pub on: bool,
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
    pub hook: Hook,
    pub opencode: OpenCode,
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
        opencode: opencode::read_opencode(home, &opencode_binary(home, binary), state_dir),
        ..Report::default()
    };
    for (agent, target) in skill::targets(home) {
        let path = target.dir.join(skill::NAME).join("SKILL.md");
        let state = match fs::read(&path) {
            Ok(b) if b == skill::CONTENT => CURRENT,
            Ok(_) => OUTDATED,
            Err(_) => MISSING,
        };
        r.skills.push(Skill {
            agent,
            path: lossy(&path),
            state: state.into(),
        });
    }
    let settings = hook::settings_path(home);
    let command = hook::registered_command(&settings);
    r.hook = Hook {
        on: !command.is_empty() && hook::is_enabled(state_dir),
        settings: lossy(&settings),
        current: !command.is_empty() && command == hook::hook_command(binary),
        command,
    };
    r
}

/// The dispatch the OpenCode plugin runs: the linked command when it is this
/// binary, so it follows the link, else binary itself.
pub fn opencode_binary(home: &Path, binary: &str) -> String {
    if read_cli(home, binary).state == LINKED {
        return lossy(&cli_path(home));
    }
    binary.to_string()
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

/// Writes the current skill for agent, or for every agent when it is empty,
/// over any older copy. It returns the paths written.
pub fn install_skill(home: &Path, agent: &str) -> Result<Vec<PathBuf>, Error> {
    let targets = skill::targets(home);
    let agent = agent.trim();
    if !agent.is_empty() && !targets.contains_key(agent) {
        return Err(Error::Message(format!("unknown agent {agent:?}")));
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (name, target) in &targets {
        if (!agent.is_empty() && name != agent) || seen.contains(&target.dir) {
            continue;
        }
        seen.insert(target.dir.clone());
        out.push(skill::install(&target.dir, true)?);
    }
    Ok(out)
}

/// Registers the SessionStart hook for binary and sets the enabled flag, or
/// removes both.
pub fn set_hook(home: &Path, binary: &str, state_dir: &Path, on: bool) -> Result<(), Error> {
    let settings = hook::settings_path(home);
    if !on {
        let errs = [
            hook::set_enabled(state_dir, false),
            hook::uninstall(&settings),
        ]
        .into_iter()
        .filter_map(Result::err)
        .collect();
        return join_errors(errs);
    }
    hook::set_enabled(state_dir, true)?;
    hook::install(&settings, binary)
}
