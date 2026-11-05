use crate::{
    Error,
    adapters::Model,
    catalog::{Catalog, base_model},
    config::Policy,
    pass_model::difficulty_tier,
    router::{Agent, RouteOptions, RouteReport, route},
    tiers::{Entry, Table, provider_of},
    tiers_build::{build_tiers, unique_entries},
};
use std::collections::HashSet;
use std::future::Future;

pub const OPEN_CODE_AGENT: &str = "opencode";
pub const OPEN_CODE_PICK_NOTE: &str =
    "the tier has no OpenCode entry: OpenCode's models ranked for the tier";

pub trait OpenCodeAdapter: Send + Sync {
    fn installed(&self) -> bool;
    fn models(&self) -> impl Future<Output = Result<Vec<Model>, Error>> + Send;
}

impl<T: OpenCodeAdapter + ?Sized> OpenCodeAdapter for &T {
    fn installed(&self) -> bool {
        (**self).installed()
    }
    fn models(&self) -> impl Future<Output = Result<Vec<Model>, Error>> + Send {
        (**self).models()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TieredEntry {
    pub entry: Entry,
    pub tier: String,
    pub note: String,
}

pub fn agent_pool(agent: &str, table: &Table, listed: &[Entry]) -> Vec<Entry> {
    let mut out = Vec::new();
    for tier in ["easy", "normal", "hard"] {
        if let Some(entries) = table.tiers.get(tier) {
            for e in entries {
                if e.agent == agent {
                    out.push(e.clone());
                }
            }
        }
    }
    for e in listed {
        if e.agent == agent {
            out.push(e.clone());
        }
    }
    unique_entries(&out)
}

pub fn merge_agent(table: &Table, built: &Table, agent: &str) -> Table {
    let mut out = Table::default();
    for tier in ["easy", "normal", "hard"] {
        let mut rows = Vec::new();
        if let Some(entries) = table.tiers.get(tier) {
            for e in entries {
                if e.agent != agent {
                    rows.push(e.clone());
                }
            }
        }
        if let Some(entries) = built.tiers.get(tier) {
            for e in entries {
                if e.agent == agent {
                    rows.push(e.clone());
                }
            }
        }
        if !rows.is_empty() {
            out.tiers.insert(tier.into(), rows);
        }
    }
    out
}

fn policy_reason(policy: &Policy, provider: &str, model: &str, agent: &str) -> String {
    let mut p = policy.clone();
    let base = base_model(model);
    if base != model {
        for list in [&mut p.allow.models, &mut p.deny.models] {
            if list.iter().any(|v| v == base) {
                list.push(model.into());
            }
        }
    }
    p.reason(provider, model, agent)
}

fn listed_entries(models: Vec<Model>) -> Vec<Entry> {
    models
        .into_iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| Entry {
            agent: OPEN_CODE_AGENT.into(),
            provider: provider_of(OPEN_CODE_AGENT, &m.id),
            model: m.id,
            ..Default::default()
        })
        .collect()
}

pub async fn open_code_entries<A: OpenCodeAdapter>(
    adapter: Option<&A>,
    table: &Table,
    policy: &Policy,
    cat: &Catalog,
    options: &RouteOptions,
    mut entries: Vec<TieredEntry>,
) -> Vec<TieredEntry> {
    let Some(a) = adapter.filter(|a| a.installed()) else {
        return entries;
    };
    if !options.tier.is_empty()
        && options.tier != "auto"
        && table
            .tiers
            .get(&options.tier)
            .is_some_and(|es| es.iter().any(|e| e.agent == OPEN_CODE_AGENT))
    {
        return entries;
    }
    let Ok(ms) = a.models().await else {
        return entries;
    };
    let listed = listed_entries(ms);
    if options.tier.is_empty() || options.tier == "auto" {
        let known: HashSet<String> = entries
            .iter()
            .map(|e| format!("{}/{}", e.entry.agent, e.entry.model))
            .collect();
        for e in agent_pool(OPEN_CODE_AGENT, &Table::default(), &listed) {
            if !known.contains(&format!("{}/{}", e.agent, e.model)) {
                entries.push(TieredEntry {
                    entry: e,
                    tier: "normal".into(),
                    note: String::new(),
                });
            }
        }
        return entries;
    }
    let pool: Vec<Entry> = agent_pool(OPEN_CODE_AGENT, table, &listed)
        .into_iter()
        .filter(|e| policy_reason(policy, &e.provider, &e.model, &e.agent).is_empty())
        .collect();
    entries.retain(|e| e.entry.agent != OPEN_CODE_AGENT);
    let priority = if options.filters.priority.is_empty() {
        "balanced"
    } else {
        &options.filters.priority
    };
    if let Some(built) = build_tiers(&pool, cat, priority).tiers.get(&options.tier) {
        for b in built {
            entries.push(TieredEntry {
                entry: b.entry.clone(),
                tier: options.tier.clone(),
                note: OPEN_CODE_PICK_NOTE.into(),
            });
        }
    }
    entries
}

pub async fn route_with_opencode<A: OpenCodeAdapter>(
    table: &Table,
    policy: &Policy,
    agents: &[Agent],
    options: &RouteOptions,
    cat: &Catalog,
    opencode: Option<&A>,
) -> Result<RouteReport, Error> {
    if options.filters.agent != OPEN_CODE_AGENT || !options.filters.model.is_empty() {
        return route(table, policy, agents, options, cat);
    }
    let Some(oc) = opencode.filter(|a| a.installed()) else {
        return route(table, policy, agents, options, cat);
    };
    if !agents
        .iter()
        .any(|a| a.name == OPEN_CODE_AGENT && a.installed)
    {
        return route(table, policy, agents, options, cat);
    }

    let mut adj_table = table.clone();
    let mut adj_policy = policy.clone();
    let mut adj_options = options.clone();
    if adj_options.tier.is_empty()
        && adj_options.difficulty.is_some()
        && adj_options.filters.model.is_empty()
        && adj_options.filters.provider.is_empty()
    {
        adj_options.tier = "auto".into();
    }
    let mut eff_tier: String = if adj_options.tier == "auto" && adj_options.difficulty.is_none() {
        "normal".into()
    } else {
        adj_options.tier.clone()
    };
    if adj_options.filters.model.is_empty() && adj_options.filters.provider.is_empty() {
        let band = if eff_tier == "auto" {
            difficulty_tier(adj_options.difficulty.unwrap_or(-1.8))
        } else if matches!(eff_tier.as_str(), "easy" | "normal" | "hard") {
            eff_tier.as_str()
        } else {
            "normal"
        };
        if let Some(board_entries) = adj_options
            .role_board
            .tiers
            .get(band)
            .filter(|e| !e.is_empty())
        {
            let band_owned = band.to_owned();
            adj_table.tiers.clear();
            adj_table
                .tiers
                .insert(band_owned.clone(), board_entries.clone());
            adj_options.tier.clone_from(&band_owned);
            eff_tier = band_owned;
            adj_policy.discover = false;
        }
    }

    if !eff_tier.is_empty()
        && eff_tier != "auto"
        && adj_table
            .tiers
            .get(&eff_tier)
            .is_some_and(|es| es.iter().any(|e| e.agent == OPEN_CODE_AGENT))
    {
        let mut adj_agents = agents.to_vec();
        for agent in &mut adj_agents {
            if agent.name == OPEN_CODE_AGENT {
                agent.discovered.clear();
            }
        }
        return route(&adj_table, &adj_policy, &adj_agents, &adj_options, cat);
    }

    let Ok(ms) = oc.models().await else {
        return route(&adj_table, &adj_policy, agents, &adj_options, cat);
    };
    let listed = listed_entries(ms);

    if eff_tier.is_empty() || eff_tier == "auto" {
        let known: HashSet<String> = adj_table
            .tiers
            .values()
            .flatten()
            .map(|e| format!("{}/{}", e.agent, e.model))
            .collect();
        for e in agent_pool(OPEN_CODE_AGENT, &Table::default(), &listed) {
            if !known.contains(&format!("{}/{}", e.agent, e.model)) {
                adj_table.tiers.entry("normal".into()).or_default().push(e);
            }
        }
        return route(&adj_table, &adj_policy, agents, &adj_options, cat);
    }

    let mut adj_agents = agents.to_vec();
    for a in &mut adj_agents {
        if a.name == OPEN_CODE_AGENT {
            a.discovered.clear();
        }
    }
    let pool: Vec<Entry> = agent_pool(OPEN_CODE_AGENT, &adj_table, &listed)
        .into_iter()
        .filter(|e| policy_reason(&adj_policy, &e.provider, &e.model, &e.agent).is_empty())
        .collect();
    let priority = if adj_options.filters.priority.is_empty() {
        "balanced"
    } else {
        &adj_options.filters.priority
    };
    let built = build_tiers(&pool, cat, priority)
        .tiers
        .remove(&eff_tier)
        .unwrap_or_default();
    let tier_rows = adj_table.tiers.entry(eff_tier).or_default();
    tier_rows.retain(|e| e.agent != OPEN_CODE_AGENT);
    let added = !built.is_empty();
    for b in built {
        tier_rows.push(b.entry);
    }
    let mut rep = route(&adj_table, &adj_policy, &adj_agents, &adj_options, cat)?;
    if added
        && rep
            .candidates
            .iter()
            .any(|c| c.filters.agent == OPEN_CODE_AGENT)
    {
        if rep.note.is_empty() {
            rep.note = OPEN_CODE_PICK_NOTE.into();
        } else {
            rep.note = format!("{OPEN_CODE_PICK_NOTE}; {}", rep.note);
        }
    }
    Ok(rep)
}
