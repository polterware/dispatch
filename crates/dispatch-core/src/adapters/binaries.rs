use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

static OVERRIDES: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub fn default_binaries() -> HashMap<String, String> {
    [
        ("codex", "codex"),
        ("cursor", "cursor-agent"),
        ("grok", "grok"),
        ("antigravity", "agy"),
        ("opencode", "opencode"),
        ("claude", "claude"),
        ("pi", "pi"),
        ("gemini", "gemini"),
        ("qwen", "qwen"),
        ("copilot", "copilot"),
        ("kimi", "kimi"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
pub fn set_binaries(overrides: HashMap<String, String>) {
    if let Ok(mut guard) = OVERRIDES.write() {
        *guard = overrides
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
    }
}
pub fn bin(agent: &str) -> String {
    OVERRIDES
        .read()
        .ok()
        .and_then(|g| g.get(agent).cloned())
        .or_else(|| {
            super::acp::custom_agents()
                .get(agent)
                .and_then(|c| c.first())
                .cloned()
        })
        .or_else(|| default_binaries().remove(agent))
        .unwrap_or_default()
}
fn executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
}
fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    path.into()
}
pub fn valid_binary(path: &str) -> Result<(), Error> {
    let path = expand(path);
    if !path.is_absolute() {
        return Err(Error::Message("invalid argument".into()));
    }
    if !executable(&path) {
        return Err(Error::Message("file does not exist".into()));
    }
    Ok(())
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detection {
    pub agent: String,
    pub default: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub configured: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suggest: String,
}
pub fn detect() -> Vec<Detection> {
    let mut out = Vec::new();
    let mut agents: Vec<String> = [
        "codex",
        "claude",
        "cursor",
        "grok",
        "antigravity",
        "opencode",
        "pi",
        "gemini",
        "qwen",
        "copilot",
        "kimi",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    agents.extend(super::acp::custom_agents().into_keys());
    for agent in &agents {
        let configured = OVERRIDES
            .read()
            .ok()
            .and_then(|g| g.get(agent).cloned())
            .unwrap_or_default();
        let mut d = Detection {
            agent: agent.clone(),
            default: bin(agent),
            configured,
            source: "missing".into(),
            ..Detection::default()
        };
        if !d.configured.is_empty() {
            if executable(Path::new(&d.configured)) {
                d.path = d.configured.clone();
                d.source = "configured".into();
            } else {
                d.source = "invalid".into();
            }
        } else {
            let binary = Path::new(&d.default);
            let found = if d.default.contains('/') {
                executable(binary).then(|| binary.to_path_buf())
            } else {
                std::env::var_os("PATH")
                    .into_iter()
                    .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
                    .map(|p| p.join(binary))
                    .find(|p| executable(p))
            };
            if let Some(path) = found {
                d.path = path.to_string_lossy().into_owned();
                d.source = "PATH".into();
            }
        }
        if d.path.is_empty() {
            let locations: &[&str] = match agent.as_str() {
                "opencode" => &[
                    "~/.opencode/bin/opencode",
                    "/Applications/OpenCode.app/Contents/Resources/opencode-cli",
                    "/opt/homebrew/bin/opencode",
                    "/usr/local/bin/opencode",
                ],
                "claude" => &[
                    "~/.local/bin/claude",
                    "~/.claude/local/claude",
                    "/opt/homebrew/bin/claude",
                ],
                "pi" => &["~/.local/bin/pi", "/opt/homebrew/bin/pi"],
                "kimi" => &["~/.kimi-code/bin/kimi", "~/.local/bin/kimi"],
                _ => &[],
            };
            if let Some(p) = locations.iter().map(|p| expand(p)).find(|p| executable(p)) {
                d.suggest = p.to_string_lossy().into_owned();
                if d.source == "missing" {
                    d.source = "found".into();
                }
            }
        }
        out.push(d);
    }
    out
}
