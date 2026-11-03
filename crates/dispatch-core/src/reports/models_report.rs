use crate::{
    Error,
    catalog::{
        Catalog, CatalogModel, CatalogSource, MODELS_DEV_URL, ModelEstimate, effort_of,
        parse_models_dev, prices_path,
    },
    pass_model::{pass_probability, theta_uncertainty},
    router::pass_threshold,
    tiers::{Entry, Table},
    tiers_build::band_difficulty,
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BandPass {
    pub tier: String,
    pub d: f64,
    pub p_pass: f64,
    pub low: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogEntry {
    pub agent: String,
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tier: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    pub bands: Vec<BandPass>,
    #[serde(flatten)]
    pub estimate: ModelEstimate,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogReport {
    pub generated: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prices_fetched: String,
    pub sources: Vec<CatalogSource>,
    pub models: usize,
    pub scores: usize,
    pub eci: usize,
    pub bars: BTreeMap<String, f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority: String,
    pub discover: bool,
    pub entries: Vec<CatalogEntry>,
}

fn tier_effort(tier: &str) -> &'static str {
    match tier {
        "easy" => "low",
        "hard" => "high",
        _ => "medium",
    }
}

pub fn report_catalog(cat: &Catalog, table: &Table, discovered: &[Entry]) -> CatalogReport {
    let bars = BTreeMap::from([
        ("speed".into(), pass_threshold("speed")),
        ("balanced".into(), pass_threshold("balanced")),
        ("quality".into(), pass_threshold("quality")),
    ]);
    let mut r = CatalogReport {
        generated: cat.generated.clone(),
        prices_fetched: cat.prices_fetched.clone(),
        sources: cat.sources.clone(),
        models: cat.models.len(),
        scores: cat.scores.len(),
        eci: cat.eci.len(),
        bars,
        priority: String::new(),
        discover: false,
        entries: Vec::new(),
    };
    let mut seen = HashSet::new();

    for tier in ["easy", "normal", "hard"] {
        if let Some(entries) = table.tiers.get(tier) {
            for e in entries {
                let mut effort = effort_of(&e.model);
                if effort.is_empty() {
                    effort = &e.effort;
                }
                let resolved_effort = if effort.is_empty() && !e.efforts.is_empty() {
                    tier_effort(tier)
                } else {
                    effort
                };
                seen.insert(format!("{}\0{}", e.agent, e.model));
                r.entries
                    .push(build_catalog_entry(cat, e, tier, resolved_effort));
            }
        }
    }
    for e in discovered {
        if seen.insert(format!("{}\0{}", e.agent, e.model)) {
            r.entries
                .push(build_catalog_entry(cat, e, "", effort_of(&e.model)));
        }
    }
    r
}

fn build_catalog_entry(cat: &Catalog, e: &Entry, tier: &str, effort: &str) -> CatalogEntry {
    let est = cat.estimate(&e.model, effort, e.strength);
    let sigma = theta_uncertainty(&est.theta_source, est.extrapolated_levels);
    let mut bands = Vec::with_capacity(3);
    for band in ["easy", "normal", "hard"] {
        let d = band_difficulty(band);
        let (p_pass, low) = pass_probability(est.theta, d, sigma);
        bands.push(BandPass {
            tier: band.into(),
            d,
            p_pass,
            low,
        });
    }
    CatalogEntry {
        agent: e.agent.clone(),
        model: e.model.clone(),
        tier: tier.into(),
        effort: effort.into(),
        bands,
        estimate: est,
    }
}

#[derive(Serialize)]
struct RefreshedPricesDoc<'a> {
    fetched: String,
    models: &'a [CatalogModel],
}

pub async fn refresh_catalog(
    client: &reqwest::Client,
    state_dir: &Path,
    now: DateTime<Utc>,
) -> Result<usize, Error> {
    refresh_catalog_from(client, MODELS_DEV_URL, state_dir, now).await
}

pub async fn refresh_catalog_from(
    client: &reqwest::Client,
    url: &str,
    state_dir: &Path,
    now: DateTime<Utc>,
) -> Result<usize, Error> {
    let Some(path) = prices_path(state_dir) else {
        return Err(Error::Message("no dispatch state directory".into()));
    };
    let res = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Message(e.to_string()))?;
    let status = res.status();
    if status != reqwest::StatusCode::OK {
        let status_str = match status.canonical_reason() {
            Some(reason) => format!("{} {reason}", status.as_u16()),
            None => status.as_u16().to_string(),
        };
        return Err(Error::Message(format!("{url}: {status_str}")));
    }
    const LIMIT: usize = 32 << 20;
    let mut raw = Vec::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Message(e.to_string()))?;
        let take = (LIMIT - raw.len()).min(chunk.len());
        raw.extend_from_slice(&chunk[..take]);
        if raw.len() >= LIMIT {
            break;
        }
    }
    let models = parse_models_dev(&raw)?;
    let count = models.len();
    let doc = RefreshedPricesDoc {
        fetched: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        models: &models,
    };
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    doc.serialize(&mut ser)?;
    if let Some(parent) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, buf).await?;
    Ok(count)
}
