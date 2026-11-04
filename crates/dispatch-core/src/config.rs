use crate::{Error, router::valid_priority};
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rules {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.models.is_empty() && self.agents.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority: String,
    pub discover: bool,
    pub roles: bool,
    #[serde(skip_serializing_if = "Rules::is_empty")]
    pub allow: Rules,
    #[serde(skip_serializing_if = "Rules::is_empty")]
    pub deny: Rules,
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
            allow: Rules::default(),
            deny: Rules::default(),
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
        for key in ["allow", "deny"] {
            if let Some(v) = root.get(key) {
                let Some(obj) = v.as_object() else {
                    return Err(Error::Message(format!("{key} must be an object")));
                };
                for dimension in ["providers", "models", "agents"] {
                    if let Some(list) = obj.get(dimension)
                        && !list
                            .as_array()
                            .is_some_and(|a| a.iter().all(|x| x.is_string()))
                    {
                        return Err(Error::Message(format!(
                            "{key} {dimension} must be an array of strings"
                        )));
                    }
                }
            }
        }
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

    pub fn reason(&self, provider: &str, model: &str, agent: &str) -> String {
        for (name, list, value) in [
            ("providers", &self.deny.providers, provider),
            ("models", &self.deny.models, model),
            ("agents", &self.deny.agents, agent),
        ] {
            if list.iter().any(|s| s == value) {
                return format!("policy deny.{name} includes {value}");
            }
        }
        for (name, list, value) in [
            ("providers", &self.allow.providers, provider),
            ("models", &self.allow.models, model),
            ("agents", &self.allow.agents, agent),
        ] {
            if !list.is_empty() && !list.iter().any(|s| s == value) {
                return format!("policy allow.{name} requires one of {}", list.join(","));
            }
        }
        String::new()
    }
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
    raw.insert("allow".into(), serde_json::to_value(&f.allow)?);
    raw.insert("deny".into(), serde_json::to_value(&f.deny)?);
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
