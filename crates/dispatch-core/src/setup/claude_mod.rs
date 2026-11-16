//! The Claude Code mod `dispatch-delegate`: a folder marketplace in dispatch's
//! state folder, whose files this build carries and writes as it installs, and
//! which Claude Code reads straight from the folder, so rewriting them is the
//! update. Claude Code's own CLI adds the marketplace and installs the mod, so
//! it lists and enables it as it does any other.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::{Value, json};

use super::{CURRENT, OUTDATED, lossy};
use crate::Error;
use crate::hook::parent_dir;

/// The marketplace's name, and the mod's.
pub const MARKETPLACE: &str = "polterware-dispatch";
pub const PLUGIN: &str = "dispatch-delegate";

/// The mod as Claude Code names it once installed.
pub const PLUGIN_ID: &str = "dispatch-delegate@polterware-dispatch";

/// The wire values the report gives a mod that is not installed, and one
/// Claude Code cannot install because its CLI is not on this machine.
pub const NOT_INSTALLED: &str = "not_installed";
pub const NO_CLAUDE: &str = "no_claude";

/// What `claude` is missing when neither it nor DISPATCH_CLAUDE_BIN is found.
const NO_CLAUDE_MESSAGE: &str =
    "the claude command is not on the PATH, so Claude Code cannot be told about the mod";

/// The mod's plugin manifest, whose description the marketplace repeats.
const MANIFEST: &str = include_str!("claude_mod/.claude-plugin/plugin.json");

/// The mod's files, at their paths inside `plugins/dispatch-delegate` of the
/// marketplace folder.
const MOD: [(&str, &str); 6] = [
    (
        "plugins/dispatch-delegate/.claude-plugin/plugin.json",
        MANIFEST,
    ),
    (
        "plugins/dispatch-delegate/hooks/hooks.json",
        include_str!("claude_mod/hooks/hooks.json"),
    ),
    (
        "plugins/dispatch-delegate/hooks/register.tsx",
        include_str!("claude_mod/hooks/register.tsx"),
    ),
    (
        "plugins/dispatch-delegate/types/index.d.ts",
        include_str!("claude_mod/types/index.d.ts"),
    ),
    (
        "plugins/dispatch-delegate/README.md",
        include_str!("claude_mod/README.md"),
    ),
    (
        "plugins/dispatch-delegate/tests/delegate.test.ts",
        include_str!("claude_mod/tests/delegate.test.ts"),
    ),
];

/// The Claude Code mod in the setup report: Current (installed, every file as
/// this build carries it), Outdated (installed, a file differs or is missing),
/// Not installed, or No Claude Code (no `claude` command to install it with).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ClaudeMod {
    /// The marketplace folder.
    pub dir: String,
    pub state: String,
}

/// The marketplace folder in dispatch's state folder.
pub fn marketplace_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("claude-marketplace")
}

/// Reports the mod for the state folder. It asks Claude Code what it has
/// installed, and judges the folder itself when the CLI cannot answer.
pub fn read(state_dir: &Path) -> ClaudeMod {
    status(state_dir, claude_bin())
}

fn status(state_dir: &Path, claude: Option<PathBuf>) -> ClaudeMod {
    let dir = marketplace_dir(state_dir);
    let mut m = ClaudeMod {
        dir: lossy(&dir),
        state: NOT_INSTALLED.into(),
    };
    let Some(claude) = claude else {
        m.state = NO_CLAUDE.into();
        return m;
    };
    let installed = installed_ids(&claude)
        .map(|ids| ids.iter().any(|id| id == PLUGIN_ID))
        .unwrap_or_else(|| dir.is_dir());
    if installed {
        m.state = if current(&dir) { CURRENT } else { OUTDATED }.into();
    }
    m
}

/// Writes the marketplace folder, adds it to Claude Code when it does not know
/// it, and installs the mod when Claude Code does not list it. Claude Code reads
/// a folder marketplace from the folder, so the files written are the update.
pub fn install(state_dir: &Path) -> Result<PathBuf, Error> {
    install_with(state_dir, claude_bin())
}

fn install_with(state_dir: &Path, claude: Option<PathBuf>) -> Result<PathBuf, Error> {
    let claude = claude.ok_or_else(|| Error::Message(NO_CLAUDE_MESSAGE.into()))?;
    let dir = write(state_dir)?;
    let source = lossy(&dir);
    if !marketplace_added(&claude) {
        run(&claude, &["plugin", "marketplace", "add", &source])?;
    }
    let listed = installed_ids(&claude).is_some_and(|ids| ids.iter().any(|id| id == PLUGIN_ID));
    if !listed {
        run(
            &claude,
            &["plugin", "install", PLUGIN_ID, "--scope", "user"],
        )?;
    }
    Ok(dir)
}

/// Uninstalls the mod and removes the marketplace from Claude Code, then
/// deletes the folder. What is not installed, or no longer known, is skipped.
pub fn remove(state_dir: &Path) -> Result<PathBuf, Error> {
    remove_with(state_dir, claude_bin())
}

fn remove_with(state_dir: &Path, claude: Option<PathBuf>) -> Result<PathBuf, Error> {
    let dir = marketplace_dir(state_dir);
    if let Some(claude) = claude {
        for args in [
            ["plugin", "uninstall", PLUGIN_ID].as_slice(),
            ["plugin", "marketplace", "remove", MARKETPLACE].as_slice(),
        ] {
            match run(&claude, args) {
                Ok(_) => {}
                Err(e) if missing(&e) => {}
                Err(e) => return Err(e),
            }
        }
    }
    remove_all(&dir)?;
    Ok(dir)
}

/// Every file of the marketplace folder, at its path inside it: the manifest
/// listing the mod, and the mod's own files.
fn files() -> Vec<(&'static str, String)> {
    let marketplace = json!({
        "name": MARKETPLACE,
        "owner": { "name": "Polterware" },
        "plugins": [{
            "name": PLUGIN,
            "source": "./plugins/dispatch-delegate",
            "description": description(),
        }],
    });
    let mut out = vec![(
        ".claude-plugin/marketplace.json",
        format!(
            "{}\n",
            serde_json::to_string_pretty(&marketplace).unwrap_or_default()
        ),
    )];
    out.extend(MOD.iter().map(|(path, body)| (*path, (*body).to_string())));
    out
}

/// The description this build's plugin manifest gives the mod.
fn description() -> String {
    serde_json::from_str::<Value>(MANIFEST)
        .ok()
        .and_then(|v| v.get("description")?.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Writes every file of the marketplace folder, over what was there.
fn write(state_dir: &Path) -> Result<PathBuf, Error> {
    let dir = marketplace_dir(state_dir);
    for (path, body) in files() {
        let file = dir.join(path);
        fs::create_dir_all(parent_dir(&file))?;
        fs::write(&file, body)?;
    }
    Ok(dir)
}

/// Whether the folder holds this build's files, all of them.
fn current(dir: &Path) -> bool {
    files()
        .iter()
        .all(|(path, want)| fs::read(dir.join(path)).is_ok_and(|have| have == want.as_bytes()))
}

fn remove_all(dir: &Path) -> Result<bool, Error> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// The `claude` command: DISPATCH_CLAUDE_BIN when it names one, else the first
/// one on the PATH.
fn claude_bin() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DISPATCH_CLAUDE_BIN").map(PathBuf::from)
        && !path.as_os_str().is_empty()
    {
        return Some(path);
    }
    let executable = |p: &Path| {
        fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| Path::new(if dir.is_empty() { "." } else { dir }).join("claude"))
        .find(|p| executable(p))
}

/// Runs the claude CLI and answers its stdout, or an Error carrying what it
/// wrote to stderr.
fn run(claude: &Path, args: &[&str]) -> Result<String, Error> {
    let line = format!("claude {}", args.join(" "));
    let out = Command::new(claude)
        .args(args)
        .output()
        .map_err(|e| Error::Message(format!("{line}: {e}")))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = match stderr.trim() {
        "" => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr => stderr.to_string(),
    };
    Err(Error::Message(format!("{line}: {detail}")))
}

/// Whether Claude Code already knows the marketplace.
fn marketplace_added(claude: &Path) -> bool {
    let Ok(out) = run(claude, &["plugin", "marketplace", "list", "--json"]) else {
        return false;
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(out.trim()) else {
        return false;
    };
    items
        .iter()
        .any(|item| item.get("name").and_then(Value::as_str) == Some(MARKETPLACE))
}

/// What Claude Code lists as installed, or None when it cannot say.
fn installed_ids(claude: &Path) -> Option<Vec<String>> {
    let out = run(claude, &["plugin", "list", "--json"]).ok()?;
    let Value::Array(items) = serde_json::from_str::<Value>(out.trim()).ok()? else {
        return None;
    };
    Some(
        items
            .iter()
            .filter_map(|item| Some(item.get("id")?.as_str()?.to_string()))
            .collect(),
    )
}

/// Whether a failure says there is nothing to undo: uninstalling what is not
/// installed, or removing a marketplace Claude Code does not know.
fn missing(e: &Error) -> bool {
    let text = e.to_string().to_lowercase();
    [
        "not installed",
        "not found",
        "no such",
        "does not exist",
        "unknown marketplace",
    ]
    .iter()
    .any(|say| text.contains(say))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A `claude` stand-in that logs its arguments and lists the mod when
    /// `installed`, printing nothing else.
    fn fake_claude(dir: &Path, installed: bool) -> PathBuf {
        let log = dir.join("claude.log");
        let script = dir.join("claude");
        let list = if installed {
            format!("[{{\"id\":\"{PLUGIN_ID}\",\"version\":\"0.1.0\"}}]")
        } else {
            "[]".to_string()
        };
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$1 $2\" in\n  'plugin list') echo '{list}' ;;\nesac\nexit 0\n",
            log = log.display(),
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn log(dir: &Path) -> Vec<String> {
        fs::read_to_string(dir.join("claude.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn writes_the_marketplace_folder() {
        let state = tempfile::tempdir().unwrap();
        let dir = write(state.path()).unwrap();
        let manifest = fs::read_to_string(dir.join(".claude-plugin/marketplace.json")).unwrap();
        let parsed: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(parsed["name"], MARKETPLACE);
        assert_eq!(parsed["owner"]["name"], "Polterware");
        assert_eq!(parsed["plugins"][0]["name"], PLUGIN);
        assert_eq!(
            parsed["plugins"][0]["source"],
            "./plugins/dispatch-delegate"
        );
        assert_eq!(
            parsed["plugins"][0]["description"],
            description().as_str(),
            "the manifest's description"
        );
        for (path, body) in MOD {
            let on_disk = fs::read_to_string(dir.join(path)).unwrap();
            assert_eq!(on_disk, body, "{path}");
        }
        assert!(current(&dir), "a folder just written is current");
    }

    #[test]
    fn status_reads_the_folder_and_claude() {
        let state = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let claude = fake_claude(bin.path(), true);
        assert_eq!(
            status(state.path(), Some(claude.clone())).state,
            OUTDATED,
            "Claude Code lists it but the folder is not there"
        );
        write(state.path()).unwrap();
        let m = status(state.path(), Some(claude.clone()));
        assert_eq!(m.state, CURRENT);
        assert_eq!(
            m.dir,
            lossy(&marketplace_dir(state.path())),
            "the folder is reported"
        );
        assert!(
            log(bin.path())
                .iter()
                .all(|line| line == "plugin list --json"),
            "reading status only asks Claude Code what it lists, never installs: {:?}",
            log(bin.path())
        );

        let edited =
            marketplace_dir(state.path()).join("plugins/dispatch-delegate/hooks/hooks.json");
        fs::write(&edited, "{}").unwrap();
        assert_eq!(status(state.path(), Some(claude)).state, OUTDATED);

        let other = tempfile::tempdir().unwrap();
        let none = fake_claude(other.path(), false);
        assert_eq!(
            status(state.path(), Some(none)).state,
            NOT_INSTALLED,
            "Claude Code lists no mod"
        );
    }

    #[test]
    fn install_adds_the_marketplace_and_installs_the_mod() {
        let state = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let claude = fake_claude(bin.path(), false);
        let dir = install_with(state.path(), Some(claude)).unwrap();
        assert_eq!(dir, marketplace_dir(state.path()));
        assert_eq!(
            log(bin.path()),
            [
                "plugin marketplace list --json".to_string(),
                format!("plugin marketplace add {}", lossy(&dir)),
                "plugin list --json".to_string(),
                format!("plugin install {PLUGIN_ID} --scope user"),
            ],
            "the marketplace is added once, then the mod installed"
        );
    }

    #[test]
    fn remove_uninstalls_and_deletes_the_folder() {
        let state = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let claude = fake_claude(bin.path(), true);
        let dir = write(state.path()).unwrap();
        remove_with(state.path(), Some(claude)).unwrap();
        assert_eq!(
            log(bin.path()),
            [
                format!("plugin uninstall {PLUGIN_ID}"),
                format!("plugin marketplace remove {MARKETPLACE}"),
            ]
        );
        assert!(!dir.exists(), "the folder goes with the mod");
    }

    #[test]
    fn without_claude_the_mod_is_reported_and_install_refuses() {
        let state = tempfile::tempdir().unwrap();
        assert_eq!(status(state.path(), None).state, NO_CLAUDE);
        assert!(
            install_with(state.path(), None).is_err(),
            "nothing can be installed without the CLI"
        );
        assert!(
            !marketplace_dir(state.path()).exists(),
            "and nothing is written"
        );
    }

    #[test]
    fn a_missing_plugin_is_not_an_error_to_remove() {
        let state = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let script = bin.path().join("claude");
        fs::write(
            &script,
            "#!/bin/sh\necho 'plugin is not installed' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        remove_with(state.path(), Some(script)).unwrap();
    }
}
