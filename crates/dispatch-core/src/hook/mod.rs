//! The SessionStart hooks that point agents at the dispatch skill, one per
//! agent that has them, and the enabled flag.
//!
//! Each agent keeps its hooks in its own file and shape: Claude Code, Gemini
//! CLI, and Qwen Code in their settings.json, Codex in hooks.json, Grok in a
//! file of its own under ~/.grok/hooks, all as groups of `{type, command,
//! timeout}`; Cursor as a flat list in hooks.json; Copilot as `{type, bash,
//! timeoutSec}` in a file of its own under ~/.copilot/hooks.

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

/// What agent's hook prints while dispatch is on: the context line in the
/// shape that agent reads. Claude Code and Codex add plain output to the
/// session; Gemini CLI, Qwen Code, and Grok read `hookSpecificOutput`; Cursor
/// reads `additional_context`, and sets `env` (the caller variables) for the
/// session; Copilot reads `additionalContext`.
pub fn context_output(agent: &str, env: Map<String, Value>) -> String {
    let mut body = Map::new();
    match agent {
        "cursor" => {
            body.insert("additional_context".into(), CONTEXT_MESSAGE.into());
            if !env.is_empty() {
                body.insert("env".into(), Value::Object(env));
            }
        }
        "copilot" => {
            body.insert("additionalContext".into(), CONTEXT_MESSAGE.into());
        }
        "gemini" | "qwen" | "grok" => {
            let mut out = Map::new();
            out.insert("hookEventName".into(), SESSION_START.into());
            out.insert("additionalContext".into(), CONTEXT_MESSAGE.into());
            body.insert("hookSpecificOutput".into(), Value::Object(out));
        }
        _ => return CONTEXT_MESSAGE.to_string(),
    }
    Value::Object(body).to_string() + "\n"
}

/// Whether agent's hook keeps quiet under Grok, which runs Claude Code's and
/// Cursor's hooks as well as its own: Grok's own hook speaks when it is
/// registered, else Claude Code's, else Cursor's, so a session reads the line
/// once.
pub fn quiet_under_grok(home: &Path, agent: &str) -> bool {
    let registered = |a: &str| agent_hook(home, a).is_some_and(|h| !h.registered().is_empty());
    match agent {
        "" | "claude" => registered("grok"),
        "cursor" => registered("grok") || registered("claude"),
        _ => false,
    }
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
    let mut cmd = cmd.trim();
    if let Some((head, agent)) = cmd.rsplit_once(AGENT_FLAG)
        && !agent.is_empty()
        && agent.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        cmd = head.trim_end();
    }
    let cmd = cmd.trim_matches('\'');
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

// hook_commands yields the command of every hook action under hooks.<event>.
fn hook_commands<'a>(
    root: &'a Map<String, Value>,
    layout: Layout,
    event: &str,
) -> impl Iterator<Item = &'a str> {
    let list = root
        .get("hooks")
        .and_then(Value::as_object)
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let actions: Vec<&Map<String, Value>> = match layout {
        Layout::Grouped => list
            .filter_map(|g| g.as_object()?.get("hooks")?.as_array())
            .flatten()
            .filter_map(Value::as_object)
            .collect(),
        Layout::Flat | Layout::Copilot => list.filter_map(Value::as_object).collect(),
    };
    let key = layout.command_key();
    actions
        .into_iter()
        .map(move |a| a.get(key).and_then(Value::as_str).unwrap_or(""))
}

/// Reports whether the SessionStart hook is registered in settings_path.
pub fn is_installed(settings_path: &Path) -> Result<bool, Error> {
    let root = read_settings(settings_path)?;
    Ok(hook_commands(&root, Layout::Grouped, SESSION_START).any(is_hook_command))
}

/// Adds or updates our SessionStart hook in settings_path atomically and
/// idempotently. It preserves unknown keys and other hooks.
pub fn install(settings_path: &Path, binary_path: &str) -> Result<(), Error> {
    let spec = Spec::grouped(SESSION_START, DEFAULT_TIMEOUT);
    install_at(settings_path, &spec, &hook_command(binary_path))
}

/// Removes our SessionStart hook from settings_path atomically. It preserves
/// all other hooks, other hook events, and unknown keys.
pub fn uninstall(settings_path: &Path) -> Result<(), Error> {
    let spec = Spec::grouped(SESSION_START, DEFAULT_TIMEOUT);
    uninstall_at(settings_path, &spec, false)
}

/// How an agent's hooks file lists a command under `hooks.<event>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Groups, each with a `hooks` list of `{type, command, timeout}`.
    Grouped,
    /// `{command, timeout}` entries, under `version: 1` (Cursor).
    Flat,
    /// `{type, bash, timeoutSec}` entries, under `version: 1` (Copilot).
    Copilot,
}

impl Layout {
    fn command_key(self) -> &'static str {
        match self {
            Layout::Copilot => "bash",
            Layout::Grouped | Layout::Flat => "command",
        }
    }

    fn timeout_key(self) -> &'static str {
        match self {
            Layout::Copilot => "timeoutSec",
            Layout::Grouped | Layout::Flat => "timeout",
        }
    }

    fn versioned(self) -> bool {
        self != Layout::Grouped
    }
}

/// Where and how one agent's hook is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spec {
    layout: Layout,
    event: &'static str,
    /// In the agent's unit: seconds, or milliseconds for Gemini CLI and Qwen Code.
    timeout: u64,
}

impl Spec {
    const fn grouped(event: &'static str, timeout: u64) -> Self {
        Spec {
            layout: Layout::Grouped,
            event,
            timeout,
        }
    }
}

// action is our hook entry in spec's shape.
fn action(spec: &Spec, command: &str) -> Value {
    let mut action = Map::new();
    if spec.layout != Layout::Flat {
        action.insert("type".into(), Value::String("command".into()));
    }
    action.insert(
        spec.layout.command_key().into(),
        Value::String(command.into()),
    );
    action.insert(spec.layout.timeout_key().into(), Value::from(spec.timeout));
    Value::Object(action)
}

fn install_at(path: &Path, spec: &Spec, command: &str) -> Result<(), Error> {
    let mut root = read_settings(path)?;
    let layout = spec.layout;
    if layout.versioned() && !root.contains_key("version") {
        root.insert("version".into(), Value::from(1));
    }
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        return Err(Error::Message(format!(
            "malformed settings {}: 'hooks' is not an object",
            path.display()
        )));
    };
    let event = spec.event;
    let list = hooks
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = list.as_array_mut() else {
        return Err(Error::Message(format!(
            "malformed settings {}: 'hooks.{event}' is not an array",
            path.display()
        )));
    };

    let (command_key, timeout_key) = (layout.command_key(), layout.timeout_key());
    let ours = |action: &Map<String, Value>| {
        is_hook_command(
            action
                .get(command_key)
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
    };
    // Where our entry is: its group and place in it, or its place in a flat list.
    let found = match layout {
        Layout::Grouped => list.iter().enumerate().find_map(|(g, group)| {
            let inner = group.get("hooks")?.as_array()?;
            let i = inner.iter().position(|h| h.as_object().is_some_and(ours))?;
            Some((Some(g), i))
        }),
        Layout::Flat | Layout::Copilot => list
            .iter()
            .position(|h| h.as_object().is_some_and(ours))
            .map(|i| (None, i)),
    };

    match found {
        Some((group, i)) => {
            let entry = match group {
                Some(g) => &mut list[g]["hooks"][i],
                None => &mut list[i],
            };
            let Some(a) = entry.as_object_mut() else {
                return Ok(());
            };
            let cmd = a.get(command_key).and_then(Value::as_str).unwrap_or("");
            let timeout = a.get(timeout_key).and_then(Value::as_u64);
            if cmd == command && timeout == Some(spec.timeout) {
                return Ok(());
            }
            a.insert(command_key.into(), Value::String(command.into()));
            a.insert(timeout_key.into(), Value::from(spec.timeout));
        }
        None => {
            let entry = action(spec, command);
            if layout == Layout::Grouped {
                let mut group = Map::new();
                group.insert("hooks".into(), Value::Array(vec![entry]));
                list.push(Value::Object(group));
            } else {
                list.push(entry);
            }
        }
    }

    write_settings_atomic(path, &root)
}

// uninstall_at removes our entries. When own_file is set and nothing but an
// empty hooks list and the version are left, the file goes too.
fn uninstall_at(path: &Path, spec: &Spec, own_file: bool) -> Result<(), Error> {
    let mut root = read_settings(path)?;
    if root.is_empty() {
        return Ok(());
    }
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let event = spec.event;
    let Some(list) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let command_key = spec.layout.command_key();
    let ours = |h: &Value| {
        h.as_object().is_some_and(|a| {
            is_hook_command(a.get(command_key).and_then(Value::as_str).unwrap_or(""))
        })
    };

    let mut modified = false;
    let mut kept = Vec::new();
    for mut g in std::mem::take(list) {
        if spec.layout != Layout::Grouped {
            if ours(&g) {
                modified = true;
            } else {
                kept.push(g);
            }
            continue;
        }
        let Some(inner) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            kept.push(g);
            continue;
        };
        let before = inner.len();
        inner.retain(|h| !ours(h));
        if inner.len() == before {
            kept.push(g);
            continue;
        }
        modified = true;
        if !inner.is_empty() {
            kept.push(g);
        }
    }
    if !modified {
        *list = kept;
        return Ok(());
    }

    if kept.is_empty() {
        hooks.remove(event);
        if hooks.is_empty() {
            root.remove("hooks");
        }
    } else {
        *list = kept;
    }

    if own_file && root.keys().all(|k| k == "version") {
        return match fs::remove_file(path) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    write_settings_atomic(path, &root)
}

/// One agent's SessionStart hook: the file it lives in and how it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHook {
    pub agent: &'static str,
    /// The file holding the hook.
    pub path: PathBuf,
    /// Whether the file is Dispatch's alone, so removing the hook removes it.
    pub own_file: bool,
    spec: Spec,
}

/// The agents with a SessionStart hook, in the order Settings lists them.
pub const HOOK_AGENTS: [&str; 7] = [
    "claude", "codex", "cursor", "gemini", "qwen", "copilot", "grok",
];

/// Follows the binary and HOOK_SUFFIX for every agent but Claude Code, which
/// keeps the bare command it always had.
pub const AGENT_FLAG: &str = " --agent ";

fn user_home(home: &Path) -> PathBuf {
    if home.as_os_str().is_empty()
        && let Some(u) = dirs::home_dir()
    {
        return u;
    }
    home.to_path_buf()
}

/// The hook for agent under home (an empty home means the user's), or None
/// for an agent without SessionStart hooks.
pub fn agent_hook(home: &Path, agent: &str) -> Option<AgentHook> {
    let home = user_home(home);
    let (agent, path, own_file, spec) = match agent {
        "claude" => (
            "claude",
            settings_path(&home),
            false,
            Spec::grouped(SESSION_START, DEFAULT_TIMEOUT),
        ),
        "codex" => {
            let dir = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .filter(|d| d.is_absolute())
                .unwrap_or_else(|| home.join(".codex"));
            (
                "codex",
                dir.join("hooks.json"),
                false,
                Spec::grouped(SESSION_START, DEFAULT_TIMEOUT),
            )
        }
        "cursor" => (
            "cursor",
            home.join(".cursor").join("hooks.json"),
            false,
            Spec {
                layout: Layout::Flat,
                event: "sessionStart",
                timeout: DEFAULT_TIMEOUT,
            },
        ),
        "gemini" => (
            "gemini",
            home.join(".gemini").join("settings.json"),
            false,
            Spec::grouped(SESSION_START, DEFAULT_TIMEOUT * 1000),
        ),
        "qwen" => (
            "qwen",
            home.join(".qwen").join("settings.json"),
            false,
            Spec::grouped(SESSION_START, DEFAULT_TIMEOUT * 1000),
        ),
        "copilot" => (
            "copilot",
            home.join(".copilot").join("hooks").join("dispatch.json"),
            true,
            Spec {
                layout: Layout::Copilot,
                event: "sessionStart",
                timeout: DEFAULT_TIMEOUT,
            },
        ),
        "grok" => (
            "grok",
            home.join(".grok").join("hooks").join("dispatch.json"),
            true,
            Spec::grouped(SESSION_START, DEFAULT_TIMEOUT),
        ),
        _ => return None,
    };
    Some(AgentHook {
        agent,
        path,
        own_file,
        spec,
    })
}

/// The hook command agent runs for binary: `<binary> hook`, with
/// `--agent <agent>` for every agent but Claude Code.
pub fn agent_hook_command(binary_path: &str, agent: &str) -> String {
    let command = hook_command(binary_path);
    if agent == "claude" {
        return command;
    }
    format!("{command}{AGENT_FLAG}{agent}")
}

impl AgentHook {
    /// The command of our hook in the agent's file, or "" when it is not there
    /// (or the file cannot be read).
    pub fn registered(&self) -> String {
        let Ok(root) = read_settings(&self.path) else {
            return String::new();
        };
        hook_commands(&root, self.spec.layout, self.spec.event)
            .find(|cmd| is_hook_command(cmd))
            .unwrap_or("")
            .to_string()
    }

    /// Adds or updates the hook for binary, keeping the rest of the file.
    pub fn install(&self, binary_path: &str) -> Result<(), Error> {
        install_at(
            &self.path,
            &self.spec,
            &agent_hook_command(binary_path, self.agent),
        )
    }

    /// Removes the hook, keeping the rest of the file.
    pub fn uninstall(&self) -> Result<(), Error> {
        uninstall_at(&self.path, &self.spec, self.own_file)
    }
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
    hook_commands(&root, Layout::Grouped, SESSION_START)
        .find(|cmd| is_hook_command(cmd))
        .unwrap_or("")
        .to_string()
}
