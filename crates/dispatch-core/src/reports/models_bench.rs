use crate::{
    catalog::{Catalog, CatalogSource, catalog_key, effort_rank, lookup_keys},
    tiers::Entry,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BenchLevel {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    pub score: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub date: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelBenchmark {
    pub agent: String,
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    pub cursor_bench: Vec<BenchLevel>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor_bench_as: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eci: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub eci_as: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BenchmarkReport {
    pub generated: String,
    pub sources: Vec<CatalogSource>,
    pub models: Vec<ModelBenchmark>,
}

pub fn benchmark_models(cat: &Catalog, models: &[Entry]) -> BenchmarkReport {
    let sources: Vec<CatalogSource> = cat
        .sources
        .iter()
        .filter(|src| src.name.contains("CursorBench") || src.name.contains("Capabilities Index"))
        .cloned()
        .collect();
    let mut r = BenchmarkReport {
        generated: cat.generated.clone(),
        sources,
        models: Vec::new(),
    };
    let mut seen = HashSet::new();
    for e in models {
        let id = format!("{}\0{}", e.agent, e.model);
        if !seen.insert(id) {
            continue;
        }
        let key = catalog_key(&e.model);
        let mut b = ModelBenchmark {
            agent: e.agent.clone(),
            model: e.model.clone(),
            provider: e.provider.clone(),
            cursor_bench: Vec::new(),
            ..Default::default()
        };
        for k in lookup_keys(&key) {
            for s in &cat.scores {
                if s.model == k {
                    b.cursor_bench.push(BenchLevel {
                        effort: s.effort.clone(),
                        score: s.score,
                        date: s.date.clone(),
                    });
                }
            }
            if !b.cursor_bench.is_empty() {
                if k != key {
                    b.cursor_bench_as = k.into();
                }
                break;
            }
        }
        b.cursor_bench.sort_by_key(|l| effort_rank(&l.effort));

        'eci: for k in lookup_keys(&key) {
            for s in &cat.eci {
                if s.model == k {
                    b.eci = Some(s.eci);
                    if k != key {
                        b.eci_as = k.into();
                    }
                    break 'eci;
                }
            }
        }
        r.models.push(b);
    }

    let best = |b: &ModelBenchmark| -> f64 {
        b.cursor_bench
            .iter()
            .fold(-1.0f64, |acc, l| acc.max(l.score))
    };
    let eci = |b: &ModelBenchmark| -> f64 { b.eci.unwrap_or(-1.0) };

    r.models.sort_by(|a, b| {
        let ba = best(a);
        let bb = best(b);
        if ba != bb {
            return bb.total_cmp(&ba);
        }
        let ea = eci(a);
        let eb = eci(b);
        if ea != eb {
            return eb.total_cmp(&ea);
        }
        format!("{}{}", a.agent, a.model).cmp(&format!("{}{}", b.agent, b.model))
    });
    r
}
