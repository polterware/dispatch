//! The Claude Code SessionStart hook that points agents at the dispatch skill,
//! and the enabled flag.

pub(crate) mod gojson;

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::Error;

/// HOOK_SUFFIX follows the binary in the hook command (`dispatch hook`).
pub const HOOK_SUFFIX: &str = " hook";

// LEGACY_SUFFIX is the hook Polterware registered before Dispatch became its
// own app (`polterware dispatch hook`); it is still ours, so install
// replaces it and uninstall removes it.
const LEGACY_SUFFIX: &str = " dispatch hook";

/// SESSION_START is the Claude Code hook event name.
pub const SESSION_START: &str = "SessionStart";

/// DEFAULT_TIMEOUT is the timeout in seconds for the session start hook.
pub const DEFAULT_TIMEOUT: u64 = 10;

/// CONTEXT_MESSAGE is emitted to Claude Code session context on
/// startup/resume/clear/compact when dispatch is enabled. It is factual and
/// concise (<= 400 chars).
pub const CONTEXT_MESSAGE: &str = "Dispatch is active in this environment. The `dispatch` skill explains how to delegate tasks to other coding agents (Auto picks the model; a tier easy|normal|hard pins the table); read it before delegating. Keep doing trivial tasks directly.\n";

/// The path to the enabled flag file.
pub fn enabled_path(state_dir: &Path) -> PathBuf {
    state_dir.join("enabled")
}

/// Reports whether the dispatch enabled flag exists.
pub fn is_enabled(state_dir: &Path) -> bool {
    fs::metadata(enabled_path(state_dir)).is_ok()
}

/// Creates or removes the enabled flag file.
pub fn set_enabled(state_dir: &Path, enabled: bool) -> Result<(), Error> {
    let p = enabled_path(state_dir);
    if enabled {
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        return write_file(&p, b"", 0o644);
    }
    match fs::remove_file(&p) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// The text injected into the session start hook.
pub fn context_block() -> &'static str {
    CONTEXT_MESSAGE
}

/// The Claude settings path: $CLAUDE_CONFIG_DIR/settings.json if set, else
/// <user_home>/.claude/settings.json. An empty user_home means the user's home.
pub fn settings_path(user_home: &Path) -> PathBuf {
    if let Some(ccd) = claude_config_dir() {
        return ccd.join("settings.json");
    }
    let mut home = user_home.to_path_buf();
    if home.as_os_str().is_empty()
        && let Some(u) = dirs::home_dir()
    {
        home = u;
    }
    home.join(".claude").join("settings.json")
}

/// $CLAUDE_CONFIG_DIR when it is set and not empty.
pub(crate) fn claude_config_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")?;
    (!dir.is_empty()).then(|| PathBuf::from(dir))
}

/// The full command string for binary_path.
pub fn hook_command(binary_path: &str) -> String {
    let binary_path = binary_path.trim();
    if binary_path.ends_with(HOOK_SUFFIX) {
        return binary_path.to_string();
    }
    shell_quote(binary_path) + HOOK_SUFFIX
}

// shell_quote single-quotes p only when it has characters a shell would split
// or expand, so ordinary paths stay readable and old hook entries still match.
fn shell_quote(p: &str) -> String {
    const UNSAFE: &str = " \t\n\"'`$&;|<>()*?[]{}!\\#~";
    if !p.is_empty() && !p.chars().any(|c| UNSAFE.contains(c)) {
        return p.to_string();
    }
    format!("'{}'", p.replace('\'', r"'\''"))
}

/// Reports whether cmd is recognized as our session hook command: a dispatch
/// binary followed by HOOK_SUFFIX, or the legacy Polterware one.
pub fn is_hook_command(cmd: &str) -> bool {
    let cmd = cmd.trim().trim_matches('\'');
    if cmd.ends_with(LEGACY_SUFFIX) {
        return true;
    }
    let Some(bin) = cmd.strip_suffix(HOOK_SUFFIX) else {
        return false;
    };
    go_base(bin.trim_end_matches('\'')) == "dispatch"
}

// go_base is Go's filepath.Base.
fn go_base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return "/";
    }
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

// hook_commands yields the command of every hook action under hooks.SessionStart.
fn hook_commands(root: &Map<String, Value>) -> impl Iterator<Item = &str> {
    root.get("hooks")
        .and_then(Value::as_object)
        .and_then(|h| h.get(SESSION_START))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.as_object()?.get("hooks")?.as_array())
        .flatten()
        .filter_map(Value::as_object)
        .map(|a| a.get("command").and_then(Value::as_str).unwrap_or(""))
}

/// Reports whether the SessionStart hook is registered in settings_path.
pub fn is_installed(settings_path: &Path) -> Result<bool, Error> {
    let root = read_settings(settings_path)?;
    Ok(hook_commands(&root).any(is_hook_command))
}

/// Adds or updates our SessionStart hook in settings_path atomically and
/// idempotently. It preserves unknown keys and other hooks.
pub fn install(settings_path: &Path, binary_path: &str) -> Result<(), Error> {
    let mut root = read_settings(settings_path)?;
    let target_cmd = hook_command(binary_path);

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(Error::Message(format!(
            "malformed settings {}: 'hooks' is not an object",
            settings_path.display()
        )));
    };
    let session_start = hooks
        .entry(SESSION_START)
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(session_start) = session_start.as_array_mut() else {
        return Err(Error::Message(format!(
            "malformed settings {}: 'hooks.{SESSION_START}' is not an array",
            settings_path.display()
        )));
    };

    // Check if already installed.
    let mut found = false;
    'groups: for g in session_start.iter_mut() {
        let Some(inner) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        for h in inner.iter_mut() {
            let Some(action) = h.as_object_mut() else {
                continue;
            };
            let cmd = action.get("command").and_then(Value::as_str).unwrap_or("");
            if is_hook_command(cmd) {
                let timeout = action.get("timeout").and_then(Value::as_u64);
                if cmd == target_cmd && timeout == Some(DEFAULT_TIMEOUT) {
                    // Already installed and matches exactly.
                    return Ok(());
                }
                // Update command and timeout in-place.
                action.insert("command".into(), Value::String(target_cmd.clone()));
                action.insert("timeout".into(), Value::from(DEFAULT_TIMEOUT));
                found = true;
                break 'groups;
            }
        }
    }

    if !found {
        // Not found: add a new hook group to SessionStart.
        let mut action = Map::new();
        action.insert("type".into(), Value::String("command".into()));
        action.insert("command".into(), Value::String(target_cmd));
        action.insert("timeout".into(), Value::from(DEFAULT_TIMEOUT));
        let mut group = Map::new();
        group.insert("hooks".into(), Value::Array(vec![Value::Object(action)]));
        session_start.push(Value::Object(group));
    }

    write_settings_atomic(settings_path, &root)
}

/// Removes our SessionStart hook from settings_path atomically. It preserves
/// all other hooks, other hook events, and unknown keys.
pub fn uninstall(settings_path: &Path) -> Result<(), Error> {
    let mut root = read_settings(settings_path)?;
    if root.is_empty() {
        return Ok(());
    }
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let Some(session_start) = hooks.get_mut(SESSION_START).and_then(Value::as_array_mut) else {
        return Ok(());
    };

    let mut modified = false;
    let mut new_session_start = Vec::new();
    for mut g in std::mem::take(session_start) {
        let Some(inner) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            new_session_start.push(g);
            continue;
        };
        let mut new_inner = Vec::new();
        for h in std::mem::take(inner) {
            if let Some(action) = h.as_object() {
                let cmd = action.get("command").and_then(Value::as_str).unwrap_or("");
                if is_hook_command(cmd) {
                    modified = true;
                    continue;
                }
            }
            new_inner.push(h);
        }
        if new_inner.is_empty() {
            // Dropped our group.
            modified = true;
        } else {
            *inner = new_inner;
            new_session_start.push(g);
        }
    }

    if !modified {
        return Ok(());
    }

    if new_session_start.is_empty() {
        hooks.remove(SESSION_START);
        if hooks.is_empty() {
            root.remove("hooks");
        }
    } else {
        *session_start = new_session_start;
    }

    write_settings_atomic(settings_path, &root)
}

fn read_settings(path: &Path) -> Result<Map<String, Value>, Error> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e.into()),
    };
    if data.trim_ascii().is_empty() {
        return Ok(Map::new());
    }
    // Like Go's json.Decoder, only the first value counts.
    let first = serde_json::Deserializer::from_slice(&data)
        .into_iter::<Value>()
        .next();
    let reason = match first {
        Some(Ok(Value::Object(root))) => return Ok(root),
        Some(Ok(Value::Null)) | None => return Ok(Map::new()),
        Some(Ok(other)) => format!(
            "json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            go_kind(&other)
        ),
        Some(Err(e)) => e.to_string(),
    };
    Err(Error::Message(format!(
        "malformed settings {}: {reason}",
        path.display()
    )))
}

fn go_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn write_settings_atomic(path: &Path, root: &Map<String, Value>) -> Result<(), Error> {
    let dir = parent_dir(path);
    fs::create_dir_all(dir)?;
    let mut b = gojson::marshal_indent(&Value::Object(root.clone()), "  ");
    b.push('\n');

    let (tmp_path, tmp) = create_temp(dir, "settings-", ".json.tmp")?;
    let result = write_and_rename(tmp, &tmp_path, path, b.as_bytes());
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_and_rename(
    mut tmp: fs::File,
    tmp_path: &Path,
    path: &Path,
    b: &[u8],
) -> Result<(), Error> {
    tmp.write_all(b)?;
    tmp.sync_all()?;
    drop(tmp);
    fs::rename(tmp_path, path)?;
    Ok(())
}

// parent_dir is Go's filepath.Dir for a file path.
pub(crate) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Creates a new file `<prefix><random><suffix>` in dir with mode 0600, as
/// Go's os.CreateTemp does.
pub(crate) fn create_temp(
    dir: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<(PathBuf, fs::File), Error> {
    let mut tries = 0;
    loop {
        let name = format!("{prefix}{}{suffix}", rand::random::<u32>());
        let path = dir.join(name);
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists && tries < 10000 => tries += 1,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Writes data to path, creating it with mode (before umask) when missing,
/// as Go's os.WriteFile does.
pub(crate) fn write_file(path: &Path, data: &[u8], mode: u32) -> Result<(), Error> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    f.write_all(data)?;
    Ok(())
}

/// The command of our SessionStart hook in settings_path, or "" when it is
/// not registered.
pub fn registered_command(settings_path: &Path) -> String {
    let Ok(root) = read_settings(settings_path) else {
        return String::new();
    };
    hook_commands(&root)
        .find(|cmd| is_hook_command(cmd))
        .unwrap_or("")
        .to_string()
}
