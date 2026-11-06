use crate::{
    adapters::usage::Usage as AdapterUsage,
    catalog::{Catalog, PriceTier, catalog_key},
    types::{Run, Usage},
};

pub const COST_AGENT: &str = "agent";
pub const COST_CATALOG: &str = "models.dev price";
pub const COST_MIXED: &str = "agent and models.dev price";

pub fn total_tokens(u: &Usage) -> i64 {
    u.input_tokens + u.cache_read_tokens + u.cache_write_tokens + u.output_tokens
}

pub trait UsageExt {
    fn total_tokens(&self) -> i64;
}

impl UsageExt for Usage {
    fn total_tokens(&self) -> i64 {
        total_tokens(self)
    }
}

pub fn usage_cost(cat: &Catalog, model: &str, u: &AdapterUsage) -> Option<(f64, String)> {
    if let Some(cost) = u.cost_usd {
        return Some((cost, COST_AGENT.into()));
    }
    let m = cat.price(&catalog_key(model))?;
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
    Some((cost, COST_CATALOG.into()))
}

pub fn context_window(cat: &Catalog, model: &str, u: &AdapterUsage) -> i64 {
    if u.context_window > 0 {
        return u.context_window;
    }
    cat.price(&catalog_key(model)).map_or(0, |m| m.context)
}

pub trait CatalogSpendExt {
    fn usage_cost(&self, model: &str, u: &AdapterUsage) -> Option<(f64, String)>;
    fn context_window(&self, model: &str, u: &AdapterUsage) -> i64;
}

impl CatalogSpendExt for Catalog {
    fn usage_cost(&self, model: &str, u: &AdapterUsage) -> Option<(f64, String)> {
        usage_cost(self, model, u)
    }

    fn context_window(&self, model: &str, u: &AdapterUsage) -> i64 {
        context_window(self, model, u)
    }
}

/// Adds one attempt's reported usage, run on `model`, to the run: tokens always, and its cost
/// when a price covers it.
pub fn add_usage(run: &mut Run, model: &str, u: Option<&AdapterUsage>, cat: &Catalog) {
    let Some(u) = u else {
        return;
    };
    let priced = usage_cost(cat, model, u);
    let s = run.usage.get_or_insert_with(Usage::default);
    // Tokens counted before this attempt that no price covered.
    let unpriced_before = s.cost_usd.is_none() && total_tokens(s) > 0;
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
        // An attempt no price covers leaves what the others cost, marked partial.
        None => s.cost_partial |= s.cost_usd.is_some(),
        Some((cost, source)) => match &mut s.cost_usd {
            Some(total) => {
                *total += cost;
                if s.cost_source != source {
                    s.cost_source = COST_MIXED.into();
                }
            }
            None => {
                s.cost_usd = Some(cost);
                s.cost_source = source;
                s.cost_partial |= unpriced_before;
            }
        },
    }
}

pub trait RunSpendExt {
    fn add_usage(&mut self, model: &str, u: Option<&AdapterUsage>, cat: &Catalog);
}

impl RunSpendExt for Run {
    fn add_usage(&mut self, model: &str, u: Option<&AdapterUsage>, cat: &Catalog) {
        add_usage(self, model, u, cat);
    }
}
