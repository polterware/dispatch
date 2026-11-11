use crate::{
    Error,
    router::valid_priority,
    types::{DisabledModel, Store},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const CLASSIFIER_OFF: &str = "";
pub const CLASSIFIER_JEV: &str = "jev";
pub const CLASSIFIER_OPENAI: &str = "openai";
pub const CLASSIFIER_CLI: &str = "cli";

pub fn valid_classifier(name: &str) -> bool {
    matches!(
        name,
        CLASSIFIER_OFF | CLASSIFIER_JEV | CLASSIFIER_OPENAI | CLASSIFIER_CLI
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority: String,
    pub discover: bool,
    pub roles: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub binaries: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub acp_agents: BTreeMap<String, Vec<String>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub classifier: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub classifier_model: String,
    /// The agent the CLI classifier asks, `agent` or `agent/model`; empty asks the easy
    /// tier's first installed agent.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub classifier_agent: String,
}

pub type ConfigFile = Policy;

impl Default for Policy {
    fn default() -> Self {
        Self {
            priority: String::new(),
            discover: true,
            roles: true,
            binaries: BTreeMap::new(),
            acp_agents: BTreeMap::new(),
            classifier: String::new(),
            classifier_model: String::new(),
            classifier_agent: String::new(),
        }
    }
}

pub fn policy_config_path(home: &Path) -> PathBuf {
    home.join("config.json")
}

pub fn default_policy() -> Policy {
    Policy::default()
}

pub fn validate_config(f: &Policy) -> Result<(), Error> {
    if !f.priority.is_empty() && !valid_priority(&f.priority) {
        return Err(Error::Message(format!(
            "invalid priority {:?} (want speed|balanced|quality)",
            f.priority
        )));
    }
    if !valid_classifier(&f.classifier) {
        return Err(Error::Message(format!(
            "invalid classifier {:?} (want off|jev|openai|cli)",
            f.classifier
        )));
    }
    Ok(())
}

pub fn apply_agent_config(state_dir: &Path) -> Result<(), Error> {
    crate::adapters::claude_limits::set_state_dir(state_dir);
    crate::classifier::set_keys_dir(state_dir);
    let f = load(&policy_config_path(state_dir))?;
    crate::adapters::acp::set_custom_agents(f.acp_agents.into_iter().collect());
    Ok(())
}

impl Policy {
    pub fn parse(input: &str) -> Result<Self, Error> {
        if input.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut value: serde_json::Value = serde_json::from_str(input)?;
        let Some(root) = value.as_object_mut() else {
            return Err(Error::Message("config must be an object".into()));
        };
        for (key, kind) in [
            ("priority", "a string"),
            ("discover", "a boolean"),
            ("roles", "a boolean"),
            ("classifier", "a string"),
        ] {
            if let Some(v) = root.get(key) {
                let valid = if kind == "a boolean" {
                    v.is_boolean()
                } else {
                    v.is_string()
                };
                if !valid {
                    return Err(Error::Message(format!("{key} must be {kind}")));
                }
            }
        }
        root.remove("allow");
        root.remove("deny");
        if let Some(v) = root.get("binaries") {
            if v.is_null() {
                root.remove("binaries");
            } else if !v
                .as_object()
                .is_some_and(|m| m.values().all(|x| x.is_string()))
            {
                return Err(Error::Message(
                    "binaries must be an object of agent to path".into(),
                ));
            }
        }
        if let Some(v) = root.get("acp_agents") {
            if v.is_null() {
                root.remove("acp_agents");
            } else if !v.as_object().is_some_and(|m| {
                m.values().all(|x| {
                    x.as_array()
                        .is_some_and(|a| a.iter().all(|s| s.is_string()))
                })
            }) {
                return Err(Error::Message(
                    "acp_agents must be an object of agent to command".into(),
                ));
            }
        }
        for key in ["classifier_model", "classifier_agent"] {
            if let Some(v) = root.get(key) {
                if v.is_null() {
                    root.remove(key);
                } else if !v.is_string() {
                    return Err(Error::Message(format!("{key} must be a string")));
                }
            }
        }
        let mut p: Self = serde_json::from_value(value)?;
        for (name, command) in &p.acp_agents {
            crate::adapters::acp::valid_custom_agent(name, command)
                .map_err(|e| Error::Message(format!("acp_agents: {e}")))?;
        }
        if p.classifier == "off" {
            p.classifier.clear();
        }
        if !valid_classifier(&p.classifier) {
            return Err(Error::Message(format!(
                "invalid classifier {:?} (want off|jev|openai|cli)",
                p.classifier
            )));
        }
        if !p.priority.is_empty() && !valid_priority(&p.priority) {
            return Err(Error::Message(format!(
                "invalid priority {:?} (want speed|balanced|quality)",
                p.priority
            )));
        }
        Ok(p)
    }
}

/// Normalizes a `(provider, agent, model)` triple to `(provider, base_model)` with no
/// effort suffix or `<provider>/` prefix.
pub fn normalize_model_key(provider: &str, agent: &str, model: &str) -> (String, String) {
    let model = model.trim();
    let provider = provider.trim();
    let mut p = if !provider.is_empty() {
        provider.to_ascii_lowercase()
    } else {
        crate::tiers::provider_of(agent.trim(), model)
    };
    if p.is_empty() {
        if let Some((head, rest)) = model.split_once('/')
            && !head.trim().is_empty()
            && !rest.trim().is_empty()
        {
            p = head.trim().to_ascii_lowercase();
        } else if !agent.trim().is_empty() {
            p = agent.trim().to_ascii_lowercase();
        }
    }
    let m = if model.is_empty() {
        String::new()
    } else {
        let base = crate::catalog::base_model(model);
        base.rsplit('/').next().unwrap_or(base).to_owned()
    };
    (p, m)
}

/// The key Settings > Models stores for one agent's model switch: the model id the agent's
/// CLI lists, without its effort suffix (`openai/gpt-5-high` becomes `openai/gpt-5`, and
/// Antigravity's `gemini-4-argon-low` becomes `gemini-4-argon`).
pub fn agent_model_key(model: &str) -> String {
    crate::catalog::base_model(model.trim()).to_owned()
}

/// Whether this setting applies to `agent`: an empty `agent` is the legacy setting for every
/// agent, and one with an agent applies to that agent alone.
pub fn applies_to_agent(d: &DisabledModel, agent: &str) -> bool {
    d.agent.is_empty() || d.agent.eq_ignore_ascii_case(agent.trim())
}

fn is_agent_model_disabled(d: &DisabledModel, agent: &str, model: &str) -> bool {
    !d.agent.is_empty()
        && applies_to_agent(d, agent)
        && d.model.eq_ignore_ascii_case(&agent_model_key(model))
}

pub fn is_provider_disabled(disabled: &[DisabledModel], provider: &str) -> bool {
    let p = provider.trim();
    !p.is_empty()
        && disabled
            .iter()
            .any(|d| d.agent.is_empty() && d.model.is_empty() && d.provider.eq_ignore_ascii_case(p))
}

pub fn is_model_disabled(
    disabled: &[DisabledModel],
    provider: &str,
    agent: &str,
    model: &str,
) -> bool {
    !disabled_reason(disabled, provider, agent, model).is_empty()
}

pub fn is_agent_disabled(disabled: &[String], agent: &str) -> bool {
    let a = agent.trim();
    !a.is_empty() && disabled.iter().any(|d| d.eq_ignore_ascii_case(a))
}

/// Why `agent` is disabled, or empty when enabled.
pub fn disabled_agent_reason(disabled: &[String], agent: &str) -> String {
    if is_agent_disabled(disabled, agent) {
        return format!("agent {} is disabled", agent.trim().to_ascii_lowercase());
    }
    String::new()
}

/// Why `provider` or `model` is disabled, or empty when enabled.
pub fn disabled_reason(
    disabled: &[DisabledModel],
    provider: &str,
    agent: &str,
    model: &str,
) -> String {
    if disabled.is_empty() {
        return String::new();
    }
    let (p, m) = normalize_model_key(provider, agent, model);
    if !p.is_empty() && is_provider_disabled(disabled, &p) {
        return format!("provider {p} is disabled");
    }
    if !m.is_empty()
        && disabled.iter().any(|d| {
            if !d.agent.is_empty() {
                is_agent_model_disabled(d, agent, model)
            } else {
                d.model.eq_ignore_ascii_case(&m)
                    && (d.provider.is_empty() || d.provider.eq_ignore_ascii_case(&p))
            }
        })
    {
        if let Some(d) = disabled
            .iter()
            .find(|d| !d.agent.is_empty() && is_agent_model_disabled(d, agent, model))
        {
            return format!(
                "model {} is disabled for agent {}",
                d.model,
                d.agent.trim().to_ascii_lowercase()
            );
        }
        if p.is_empty() {
            return format!("model {m} is disabled");
        }
        return format!("model {p}/{m} is disabled");
    }
    String::new()
}

/// Catches a requested filter that names a disabled provider or model so `--relax`
/// does not silently substitute a different model.
pub fn disabled_request_reason(
    disabled: &[DisabledModel],
    provider: &str,
    agent: &str,
    model: &str,
) -> String {
    if disabled.is_empty() {
        return String::new();
    }
    if !provider.trim().is_empty() && is_provider_disabled(disabled, provider) {
        return format!(
            "provider {} is disabled",
            provider.trim().to_ascii_lowercase()
        );
    }
    if !model.trim().is_empty() {
        if let Some(d) = disabled
            .iter()
            .find(|d| is_agent_model_disabled(d, agent, model))
        {
            return format!(
                "model {} is disabled for agent {}",
                d.model,
                d.agent.trim().to_ascii_lowercase()
            );
        }
        let (p, m) = normalize_model_key(provider, agent, model);
        if !p.is_empty() && is_provider_disabled(disabled, &p) {
            return format!("provider {p} is disabled");
        }
        if let Some(d) = disabled.iter().find(|d| {
            d.agent.is_empty()
                && !d.model.is_empty()
                && d.model.eq_ignore_ascii_case(&m)
                && (p.is_empty() || d.provider.is_empty() || d.provider.eq_ignore_ascii_case(&p))
        }) {
            let dp = if !p.is_empty() { &p } else { &d.provider };
            if dp.is_empty() {
                return format!("model {m} is disabled");
            }
            return format!("model {dp}/{m} is disabled");
        }
    }
    String::new()
}

/// Carries legacy `deny.providers` and `deny.models` from `config.json` into `store` once,
/// and drops `allow` and `deny` from the file.
pub fn migrate_legacy_deny(path: &Path, store: &dyn Store) -> Result<(), Error> {
    // Several settings and routing reads can arrive together at startup.
    static MIGRATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = MIGRATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if text.trim().is_empty() {
        return Ok(());
    }
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(());
    };
    let Some(mut raw) = val.as_object().cloned() else {
        return Ok(());
    };
    if !raw.contains_key("deny") && !raw.contains_key("allow") {
        return Ok(());
    }
    if let Some(deny) = raw.get("deny").and_then(|v| v.as_object()) {
        if let Some(providers) = deny.get("providers").and_then(|v| v.as_array()) {
            for p in providers.iter().filter_map(|v| v.as_str()) {
                let p = p.trim();
                if !p.is_empty() {
                    store.set_model_enabled(p, "", false)?;
                }
            }
        }
        if let Some(models) = deny.get("models").and_then(|v| v.as_array()) {
            let tiers = store.list_tiers().unwrap_or_default();
            for m in models.iter().filter_map(|v| v.as_str()) {
                let m = m.trim();
                if m.is_empty() {
                    continue;
                }
                let (mut p, base) = normalize_model_key("", "", m);
                if p.is_empty()
                    && let Some(t) = tiers.iter().find(|t| {
                        let (_, tb) = normalize_model_key(&t.provider, &t.agent, &t.model);
                        tb.eq_ignore_ascii_case(&base)
                    })
                {
                    p = normalize_model_key(&t.provider, &t.agent, &t.model).0;
                }
                store.set_model_enabled(&p, &base, false)?;
            }
        }
    }
    raw.remove("allow");
    raw.remove("deny");
    write_policy_raw(path, &raw)?;
    Ok(())
}

pub fn load_with_store(path: &Path, store: &dyn Store) -> Result<Policy, Error> {
    migrate_legacy_deny(path, store)?;
    load(path)
}

pub fn load(path: &Path) -> Result<Policy, Error> {
    match std::fs::read_to_string(path) {
        Ok(value) => {
            Policy::parse(&value).map_err(|e| Error::Message(format!("{}: {e}", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
        Err(e) => Err(e.into()),
    }
}

fn read_policy_raw(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if text.trim().is_empty() {
                return Ok(serde_json::Map::new());
            }
            let val: serde_json::Value = serde_json::from_str(&text)
                .map_err(|_| Error::Message(format!("{}: invalid config", path.display())))?;
            val.as_object()
                .cloned()
                .ok_or_else(|| Error::Message(format!("{}: invalid config", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_policy_raw(
    path: &Path,
    raw: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec_pretty(raw)?;
    bytes.push(b'\n');
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir)?;

    let (tmp_path, mut tmp_file) = create_temp_config(dir)?;
    let write_res = (|| -> Result<(), Error> {
        tmp_file.write_all(&bytes)?;
        tmp_file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        tmp_file.sync_all()?;
        drop(tmp_file);
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    write_res
}

fn create_temp_config(dir: &Path) -> Result<(PathBuf, std::fs::File), Error> {
    for _ in 0..32 {
        let suffix: u64 = rand::random();
        let candidate = dir.join(format!(".config-{suffix:016x}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Message(
        "failed to allocate temporary config file".into(),
    ))
}

pub fn write_config(path: &Path, f: &Policy) -> Result<(), Error> {
    let mut raw = read_policy_raw(path)?;
    raw.insert("priority".into(), serde_json::to_value(&f.priority)?);
    raw.insert("discover".into(), serde_json::to_value(f.discover)?);
    raw.insert("roles".into(), serde_json::to_value(f.roles)?);
    raw.remove("subagents");
    raw.remove("explore");
    raw.remove("allow");
    raw.remove("deny");
    for (k, v) in [
        ("classifier", &f.classifier),
        ("classifier_model", &f.classifier_model),
        ("classifier_agent", &f.classifier_agent),
    ] {
        if v.is_empty() {
            raw.remove(k);
        } else {
            raw.insert(k.into(), serde_json::to_value(v)?);
        }
    }
    if !f.binaries.is_empty() {
        raw.insert("binaries".into(), serde_json::to_value(&f.binaries)?);
    } else {
        raw.remove("binaries");
    }
    if !f.acp_agents.is_empty() {
        raw.insert("acp_agents".into(), serde_json::to_value(&f.acp_agents)?);
    } else {
        raw.remove("acp_agents");
    }
    write_policy_raw(path, &raw)
}

pub fn write_config_key<V: Serialize>(path: &Path, key: &str, value: V) -> Result<(), Error> {
    let mut raw = read_policy_raw(path)?;
    raw.insert(key.into(), serde_json::to_value(value)?);
    write_policy_raw(path, &raw)
}
