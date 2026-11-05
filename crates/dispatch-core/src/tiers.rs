use crate::{
    Error,
    adapters::Capabilities,
    catalog::{base_model, effort_of},
    types::{Store, Tier},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    pub agent: String,
    pub model: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Table {
    pub tiers: BTreeMap<String, Vec<Entry>>,
}

pub trait CapabilityAdapter {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
}

impl<T: CapabilityAdapter + ?Sized> CapabilityAdapter for &T {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownAdapter {
    pub name: String,
    pub capabilities: Capabilities,
}

impl CapabilityAdapter for KnownAdapter {
    fn name(&self) -> &str {
        &self.name
    }
    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }
}

pub fn known_adapters() -> Vec<KnownAdapter> {
    let mut names: Vec<String> = [
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
    for custom in crate::adapters::acp::custom_agents().into_keys() {
        if !names.contains(&custom) {
            names.push(custom);
        }
    }
    names
        .into_iter()
        .map(|name| {
            let capabilities = crate::adapters::probe::capabilities(&name);
            KnownAdapter { name, capabilities }
        })
        .collect()
}

pub fn provider_of(agent: &str, model: &str) -> String {
    let m = model.to_lowercase();
    let a = agent.to_lowercase();
    let provider = if m.contains("gemini") {
        "google"
    } else if m.contains("claude") {
        "anthropic"
    } else if m.contains("grok") {
        "xai"
    } else if m.contains("composer") {
        "cursor"
    } else if m.contains("gpt-")
        || m.starts_with("gpt")
        || m.contains("codex")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
    {
        "openai"
    } else {
        match a.as_str() {
            "codex" => "openai",
            "grok" => "xai",
            "cursor" => "cursor",
            "antigravity" | "gemini" => "google",
            "claude" => "anthropic",
            "qwen" => "alibaba",
            "kimi" => "moonshotai",
            "copilot" => "github-copilot",
            "opencode" | "pi" => m.split_once('/').map_or("", |(p, _)| p),
            _ => "",
        }
    };
    provider.into()
}

pub fn valid_effort(e: &str) -> bool {
    matches!(e, "low" | "medium" | "high" | "xhigh" | "max")
}

pub fn default_tiers() -> Result<Table, Error> {
    Table::embedded()
}

pub fn default_json() -> &'static [u8] {
    include_bytes!("default.json")
}

pub fn write_default(path: &Path) -> Result<(), Error> {
    if std::fs::metadata(path).is_ok() {
        return Err(Error::Message(format!("{} already exists", path.display())));
    }
    std::fs::write(path, default_json())?;
    Ok(())
}

pub fn load_tiers(path: &Path) -> Result<Table, Error> {
    if path.as_os_str().is_empty() {
        return Table::embedded();
    }
    match std::fs::read_to_string(path) {
        Ok(text) => {
            Table::parse(&text).map_err(|e| Error::Message(format!("{}: {e}", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Table::embedded(),
        Err(e) => Err(e.into()),
    }
}

pub fn load_tiers_from_store(store: Option<&dyn Store>) -> Result<Table, Error> {
    let Some(s) = store else {
        return Table::embedded();
    };
    let rows = s.list_tiers()?;
    if rows.is_empty() {
        return Table::embedded();
    }
    Ok(table_from_rows(&rows))
}

fn stored_model(e: &Entry) -> String {
    if !e.effort.is_empty() && effort_of(&e.model).is_empty() {
        format!("{}-{}", e.model, e.effort)
    } else {
        e.model.clone()
    }
}

pub fn table_from_rows(rows: &[Tier]) -> Table {
    let mut cp = rows.to_vec();
    cp.sort_by_key(|r| r.position);
    let known = known_adapters();
    let mut t = Table::default();
    for r in cp {
        let p = if r.provider.is_empty() {
            provider_of(&r.agent, &r.model)
        } else {
            r.provider
        };
        let (model, effort) = if known
            .iter()
            .any(|a| a.name == r.agent && a.capabilities.effort_mode == "config")
        {
            (
                base_model(&r.model).to_owned(),
                effort_of(&r.model).to_owned(),
            )
        } else {
            (r.model, String::new())
        };
        t.tiers.entry(r.name).or_default().push(Entry {
            provider: p,
            agent: r.agent,
            model,
            effort,
            efforts: Vec::new(),
        });
    }
    t
}

fn entry_efforts(e: &Entry) -> Vec<&str> {
    let mut out: Vec<&str> = e.efforts.iter().map(String::as_str).collect();
    if !e.effort.is_empty() {
        out.push(&e.effort);
    }
    out
}

impl Table {
    pub fn embedded() -> Result<Self, Error> {
        Self::parse(include_str!("default.json"))
    }

    pub fn load(path: &Path) -> Result<Self, Error> {
        load_tiers(path)
    }

    pub fn load_from_store(store: Option<&dyn Store>) -> Result<Self, Error> {
        load_tiers_from_store(store)
    }

    pub fn parse(input: &str) -> Result<Self, Error> {
        let table: Self = serde_json::from_str(input)?;
        table.validated()
    }

    pub fn rows(&self) -> Vec<Tier> {
        let mut out = Vec::new();
        for tier in ["easy", "normal", "hard"] {
            if let Some(entries) = self.tiers.get(tier) {
                for (i, e) in entries.iter().enumerate() {
                    let provider = if e.provider.is_empty() {
                        provider_of(&e.agent, &e.model)
                    } else {
                        e.provider.clone()
                    };
                    out.push(Tier {
                        name: tier.into(),
                        provider,
                        agent: e.agent.clone(),
                        model: stored_model(e),
                        position: i as i64,
                    });
                }
            }
        }
        out
    }

    pub fn from_rows(rows: &[Tier]) -> Self {
        table_from_rows(rows)
    }

    pub fn capability_problems<A: CapabilityAdapter>(&self, adapters: &[A]) -> Vec<String> {
        let mut problems = Vec::new();
        for (tier, entries) in &self.tiers {
            for e in entries {
                for a in adapters {
                    if a.name() != e.agent {
                        continue;
                    }
                    let caps = a.capabilities();
                    for effort in entry_efforts(e) {
                        let mut supported = valid_effort(effort);
                        match caps.effort_mode.as_str() {
                            "config" => {
                                supported = supported && caps.efforts.iter().any(|v| v == effort);
                            }
                            "model-suffix" => {}
                            _ => {
                                supported = supported && effort_of(&e.model) == effort;
                            }
                        }
                        if !supported {
                            problems.push(format!(
                                "{tier}: {}/{}: effort {effort} unsupported by agent",
                                e.agent, e.model
                            ));
                        }
                    }
                }
            }
        }
        problems
    }

    pub fn validated(mut self) -> Result<Self, Error> {
        for (tier, entries) in &mut self.tiers {
            if !matches!(tier.as_str(), "easy" | "normal" | "hard") {
                return Err(Error::Message(format!("unknown tier {tier:?}")));
            }
            for e in entries {
                if e.provider.is_empty() {
                    e.provider = provider_of(&e.agent, &e.model);
                }
                for effort in entry_efforts(e) {
                    if !valid_effort(effort) {
                        return Err(Error::Message(format!(
                            "tier {tier}: invalid effort {effort:?}"
                        )));
                    }
                }
                let suffix = effort_of(&e.model);
                if !e.effort.is_empty() && !suffix.is_empty() && e.effort != suffix {
                    return Err(Error::Message(format!(
                        "tier {tier}: effort {:?} disagrees with model {}",
                        e.effort, e.model
                    )));
                }
                if !e.effort.is_empty() && !e.efforts.is_empty() && !e.efforts.contains(&e.effort) {
                    return Err(Error::Message(format!(
                        "tier {tier}: effort {:?} not in supported efforts",
                        e.effort
                    )));
                }
                let derived = provider_of("", &e.model);
                if !e.provider.is_empty() && !derived.is_empty() && e.provider != derived {
                    return Err(Error::Message(format!(
                        "tier {tier}: provider {:?} disagrees with model {}",
                        e.provider, e.model
                    )));
                }
                if e.agent.is_empty() || e.model.is_empty() {
                    return Err(Error::Message(format!(
                        "tier {tier:?}: entries need agent and model"
                    )));
                }
            }
        }
        Ok(self)
    }
}
