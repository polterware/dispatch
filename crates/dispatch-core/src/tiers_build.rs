use crate::{
    catalog::{Catalog, catalog_key, effort_of},
    router::{AutoInfo, Candidate, price_auto},
    tiers::{Entry, Table, known_adapters, provider_of},
};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

pub const BUILD_PER_TIER: usize = 3;

pub fn band_difficulty(tier: &str) -> f64 {
    match tier {
        "easy" => -2.8,
        "hard" => -0.8,
        _ => -1.8,
    }
}

pub fn unique_entries(entries: &[Entry]) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in entries {
        if entry.agent.is_empty()
            || entry.model.is_empty()
            || !seen.insert((entry.agent.clone(), entry.model.clone()))
        {
            continue;
        }
        let mut e = entry.clone();
        if e.provider.is_empty() {
            e.provider = provider_of(&e.agent, &e.model);
        }
        out.push(e);
    }
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct BuiltEntry {
    #[serde(flatten)]
    pub entry: Entry,
    pub auto: AutoInfo,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TierBuild {
    #[serde(skip)]
    pub table: Table,
    pub tiers: BTreeMap<String, Vec<BuiltEntry>>,
}

pub fn build_tiers(entries: &[Entry], cat: &Catalog, priority: &str) -> TierBuild {
    let known = known_adapters();
    let mut expanded = Vec::new();
    for e in unique_entries(entries) {
        let suffix = effort_of(&e.model);
        let mut efforts: Vec<String> = if !suffix.is_empty() {
            vec![suffix.into()]
        } else if !e.efforts.is_empty() {
            e.efforts.clone()
        } else {
            known
                .iter()
                .find(|a| a.name == e.agent)
                .map(|a| a.capabilities.efforts.clone())
                .unwrap_or_default()
        };
        if efforts.is_empty() {
            efforts.push(e.effort.clone());
        }
        for effort in efforts {
            let mut variant = e.clone();
            variant.effort = effort;
            expanded.push(variant);
        }
    }
    let mut out = TierBuild::default();
    for tier in ["easy", "normal", "hard"] {
        let d = band_difficulty(tier);
        let candidates: Vec<Candidate> = expanded
            .iter()
            .map(|e| Candidate {
                model: e.model.clone(),
                effort: e.effort.clone(),
                strength: e.strength,
                ..Default::default()
            })
            .collect();
        let priced = price_auto(&candidates, d, priority, cat);
        let mut order: Vec<usize> = (0..priced.len()).collect();
        order.sort_by(|a, b| match (&priced[*a].auto, &priced[*b].auto) {
            (Some(a), Some(b)) => b.qualified.cmp(&a.qualified).then_with(|| {
                if a.qualified && a.effective_cost != b.effective_cost {
                    a.effective_cost
                        .partial_cmp(&b.effective_cost)
                        .unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    b.p_pass_low.total_cmp(&a.p_pass_low)
                }
            }),
            _ => std::cmp::Ordering::Equal,
        });
        let mut bases = HashSet::new();
        for i in order {
            let e = &expanded[i];
            let estimate = cat.estimate(&e.model, &priced[i].effort, e.strength);
            let base = if estimate.matched.is_empty() {
                catalog_key(&e.model)
            } else {
                estimate.matched
            };
            if !bases.insert(base) {
                continue;
            }
            if let Some(auto) = priced[i].auto.clone() {
                out.table
                    .tiers
                    .entry(tier.into())
                    .or_default()
                    .push(e.clone());
                let built = out.tiers.entry(tier.into()).or_default();
                built.push(BuiltEntry {
                    entry: e.clone(),
                    auto,
                });
                if built.len() == BUILD_PER_TIER {
                    break;
                }
            }
        }
    }
    out
}
