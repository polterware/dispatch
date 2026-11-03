use crate::{
    adapters::usage::Usage as AgentUsage,
    catalog::{Catalog, CatalogModel, PriceTier, catalog_key},
    types::{Run, Usage},
};
use serde::Deserialize;

/// Where a run's cost comes from.
pub const COST_AGENT: &str = "agent";
pub const COST_CATALOG: &str = "models.dev price";
pub const COST_MIXED: &str = "agent and models.dev price";

#[derive(Deserialize)]
struct RefreshedPrices {
    #[serde(default)]
    fetched: String,
    #[serde(default)]
    models: Vec<CatalogModel>,
}

/// The embedded snapshot, with prices from a refresh in `state_dir` when
/// there is one. A missing or broken refresh keeps the snapshot.
pub fn load_catalog(state_dir: &str) -> Catalog {
    let mut c = Catalog::embedded().unwrap_or_default();
    if !state_dir.is_empty()
        && let Ok(b) = std::fs::read(std::path::Path::new(state_dir).join("models.json"))
        && let Ok(r) = serde_json::from_slice::<RefreshedPrices>(&b)
        && !r.models.is_empty()
    {
        c.models = r.models;
        c.prices_fetched = r.fetched;
    }
    c.calibrate();
    c
}

/// The exact key, then shorter ones with trailing segments dropped.
fn lookup_keys(key: &str) -> Vec<&str> {
    let mut keys = vec![key];
    let mut k = key;
    while let Some(i) = k.rfind('-').filter(|i| *i > 0) {
        k = &k[..i];
        keys.push(k);
    }
    keys
}

fn price<'a>(c: &'a Catalog, key: &str) -> Option<&'a CatalogModel> {
    lookup_keys(key)
        .into_iter()
        .find_map(|k| c.models.iter().find(|m| m.id == k))
}

/// Prices one attempt: the CLI's own price when it gave one, else its tokens
/// at the model's list prices. A model with no cache price pays input price
/// for cache tokens; an attempt whose context passed a price tier's size pays
/// that tier. None when nothing prices it.
pub fn usage_cost(c: &Catalog, model: &str, u: &AgentUsage) -> Option<(f64, &'static str)> {
    if let Some(cost) = u.cost_usd {
        return Some((cost, COST_AGENT));
    }
    let m = price(c, &catalog_key(model))?;
    let mut p = PriceTier {
        size: 0,
        input: m.input,
        output: m.output,
        cache_read: m.cache_read,
        cache_write: m.cache_write,
    };
    for t in &m.tiers {
        if u.context_tokens > t.size {
            p = t.clone();
        }
    }
    let read = p.cache_read.unwrap_or(p.input);
    let write = p.cache_write.unwrap_or(p.input);
    let cost = (u.input_tokens as f64 * p.input
        + u.cache_read_tokens as f64 * read
        + u.cache_write_tokens as f64 * write
        + u.output_tokens as f64 * p.output)
        / 1e6;
    Some((cost, COST_CATALOG))
}

/// The window an attempt ran in: the CLI's, else the catalog's for its
/// model, else zero.
fn context_window(c: &Catalog, model: &str, u: &AgentUsage) -> i64 {
    if u.context_window > 0 {
        return u.context_window;
    }
    price(c, &catalog_key(model)).map_or(0, |m| m.context)
}

/// Adds one attempt's reported usage, run on `model`, to the run.
pub fn add_usage(r: &mut Run, model: &str, u: Option<&AgentUsage>, cat: &Catalog) {
    let Some(u) = u else {
        return;
    };
    let priced = usage_cost(cat, model, u);
    let s = r.usage.get_or_insert_with(|| {
        let mut s = Usage::default();
        if let Some((_, source)) = priced {
            s.cost_usd = Some(0.0);
            s.cost_source = source.into();
        }
        s
    });
    s.input_tokens += u.input_tokens;
    s.cache_read_tokens += u.cache_read_tokens;
    s.cache_write_tokens += u.cache_write_tokens;
    s.output_tokens += u.output_tokens;
    s.reasoning_tokens += u.reasoning_tokens;
    if u.context_tokens > 0 {
        s.context_tokens = u.context_tokens;
        s.context_window = context_window(cat, model, u);
    }
    match priced {
        None => {
            s.cost_usd = None;
            s.cost_source.clear();
        }
        Some((cost, source)) => {
            if let Some(total) = &mut s.cost_usd {
                *total += cost;
                if s.cost_source != source {
                    s.cost_source = COST_MIXED.into();
                }
            }
        }
    }
}
