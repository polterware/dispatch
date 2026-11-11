use crate::{
    adapters::usage::Usage as AgentUsage,
    catalog::{Catalog, CatalogModel, PriceTier, catalog_key},
    types::Run,
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

/// Prices one attempt: the CLI's own price when it gave one, else its tokens
/// at the model's list prices. A model with no cache price pays input price
/// for cache tokens; an attempt whose context passed a price tier's size pays
/// that tier. An image price the CLI named is added where it gave no total of
/// its own, which already covers whatever it billed. None when nothing prices it.
pub fn usage_cost(c: &Catalog, model: &str, u: &AgentUsage) -> Option<(f64, &'static str)> {
    if let Some(cost) = u.cost_usd {
        return Some((cost, COST_AGENT));
    }
    let images = u.image_cost_usd.filter(|c| c.is_finite() && *c > 0.0);
    let tokens = c.price(&catalog_key(model)).map(|m| {
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
        (u.input_tokens as f64 * p.input
            + u.cache_read_tokens as f64 * read
            + u.cache_write_tokens as f64 * write
            + u.output_tokens as f64 * p.output)
            / 1e6
    });
    match (tokens, images) {
        (Some(tokens), Some(images)) => Some((tokens + images, COST_MIXED)),
        (Some(tokens), None) => Some((tokens, COST_CATALOG)),
        (None, Some(images)) => Some((images, COST_AGENT)),
        (None, None) => None,
    }
}

/// The window an attempt ran in: the CLI's, else the catalog's for its
/// model, else zero.
fn context_window(c: &Catalog, model: &str, u: &AgentUsage) -> i64 {
    if u.context_window > 0 {
        return u.context_window;
    }
    c.price(&catalog_key(model)).map_or(0, |m| m.context)
}

/// Adds one attempt's reported usage, run on `model`, to the run: its tokens always, and its
/// cost when a price covers it. An attempt no price covers leaves what the others cost and
/// marks the cost partial, rather than dropping it.
pub fn add_usage(r: &mut Run, model: &str, u: Option<&AgentUsage>, cat: &Catalog) {
    let Some(u) = u else {
        return;
    };
    let priced = usage_cost(cat, model, u);
    // Tokens no price covers stay unpriced even when an image price covered the images.
    let tokens_unpriced = u.cost_usd.is_none() && cat.price(&catalog_key(model)).is_none();
    let s = r.usage.get_or_insert_with(Default::default);
    let unpriced_before = s.cost_usd.is_none()
        && s.input_tokens + s.cache_read_tokens + s.cache_write_tokens + s.output_tokens > 0;
    s.input_tokens += u.input_tokens;
    s.cache_read_tokens += u.cache_read_tokens;
    s.cache_write_tokens += u.cache_write_tokens;
    s.output_tokens += u.output_tokens;
    s.reasoning_tokens += u.reasoning_tokens;
    s.image_generations += u.image_generations;
    if u.context_tokens > 0 {
        s.context_tokens = u.context_tokens;
        s.context_window = context_window(cat, model, u);
    }
    match priced {
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
                s.cost_source = source.into();
                s.cost_partial |= unpriced_before;
            }
        },
    }
    if tokens_unpriced && u.image_cost_usd.is_some() && priced.is_some() {
        s.cost_partial = true;
    }
}

/// The usage the run's current attempt reported so far in its agent log: the last
/// `{"dispatch":"usage"}` line since the last attempt marker (written as the CLI streams it),
/// else what the CLI's own output says, for the agents whose usage is read from it.
pub fn attempt_usage_from_log(agent: &str, log: &str) -> Option<AgentUsage> {
    let segment = log
        .rsplit("\u{2500}\u{2500} dispatch: ")
        .next()
        .unwrap_or(log);
    for line in segment.lines().rev() {
        if !line.contains("\"dispatch\":\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let n = |k: &str| v.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
        return Some(AgentUsage {
            input_tokens: n("input"),
            cache_read_tokens: n("cacheRead"),
            cache_write_tokens: n("cacheWrite"),
            output_tokens: n("output"),
            reasoning_tokens: n("reasoning"),
            cost_usd: v
                .get("costUsd")
                .and_then(serde_json::Value::as_f64)
                .filter(|c| c.is_finite()),
            context_tokens: n("context"),
            context_window: n("window"),
            image_generations: n("images"),
            image_cost_usd: v
                .get("imageCostUsd")
                .and_then(serde_json::Value::as_f64)
                .filter(|c| c.is_finite()),
        });
    }
    use crate::adapters::usage::{usage_claude, usage_cursor, usage_grok, usage_pi};
    match agent {
        "claude" => usage_claude(segment),
        "pi" => usage_pi(segment),
        "cursor" => usage_cursor(segment),
        "grok" => usage_grok(segment),
        _ => None,
    }
}

/// The run's usage counting its current attempt so far, read from its agent log under
/// `state_dir`: what a run still working has used, or what an orphan used before the core
/// went away. None when the log says nothing more than the run already holds.
pub fn usage_with_log(run: &Run, state_dir: &str, cat: &Catalog) -> Option<crate::types::Usage> {
    if state_dir.is_empty() || run.id.is_empty() {
        return None;
    }
    let log = std::fs::read_to_string(super::agent_log_path(state_dir, &run.id)).ok()?;
    let u = attempt_usage_from_log(&run.agent, &log)?;
    let mut with = run.clone();
    add_usage(&mut with, &run.model, Some(&u), cat);
    with.usage
}

#[cfg(test)]
mod log_tests {
    use super::{attempt_usage_from_log, usage_cost};
    use crate::{adapters::usage::Usage as AgentUsage, catalog::Catalog};

    #[test]
    fn the_current_attempt_reads_its_last_usage_line() {
        let log = concat!(
            "{\"dispatch\":\"usage\",\"input\":5,\"output\":1}\n",
            "\n\u{2500}\u{2500} dispatch: a/m failed (rate_limit); trying b/n \u{2500}\u{2500}\n",
            "{\"dispatch\":\"usage\",\"input\":10,\"output\":2,\"costUsd\":null}\n",
            "{\"dispatch\":\"usage\",\"input\":30,\"output\":4,\"cacheRead\":7}\n",
            "some agent text\n",
        );
        let u = attempt_usage_from_log("acp-agent", log).expect("usage");
        assert_eq!(
            (u.input_tokens, u.output_tokens, u.cache_read_tokens),
            (30, 4, 7)
        );
        assert!(attempt_usage_from_log("acp-agent", "no usage here").is_none());
    }

    #[test]
    fn an_image_price_counts_only_where_the_cli_gave_no_total() {
        let catalog = Catalog::embedded().expect("embedded catalog");
        // A CLI that names an image price and no total is priced by that price.
        let named = AgentUsage {
            image_generations: 2,
            image_cost_usd: Some(0.08),
            ..AgentUsage::default()
        };
        let (cost, source) = usage_cost(&catalog, "no-such-provider/no-such-model", &named)
            .expect("the image price");
        assert_eq!(source, super::COST_AGENT);
        assert!((cost - 0.08).abs() < 1e-9, "{cost}");
        // Its own total already covers its images, so a named price is not added again.
        let totalled = AgentUsage {
            cost_usd: Some(0.50),
            image_cost_usd: Some(0.08),
            ..AgentUsage::default()
        };
        let (cost, _) = usage_cost(
            &catalog,
            "opencode/opencode-go/deepseek-v4.1-flash",
            &totalled,
        )
        .expect("the CLI's total");
        assert!((cost - 0.50).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn a_run_on_a_differently_named_model_gets_the_catalog_price() {
        let catalog = Catalog::embedded().expect("embedded catalog");
        let usage = AgentUsage {
            input_tokens: 1_000_000,
            ..AgentUsage::default()
        };
        let (cost, source) =
            usage_cost(&catalog, "opencode/opencode-go/deepseek-v4.1-flash", &usage)
                .expect("a price from the catalog");
        assert_eq!(source, super::COST_CATALOG);
        assert!((cost - 0.15).abs() < 1e-9, "{cost}");
    }
}
