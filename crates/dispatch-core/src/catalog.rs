use crate::Error;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

pub const CATALOG_PROVIDERS: [&str; 10] = [
    "openai",
    "google",
    "xai",
    "anthropic",
    "deepseek",
    "zai",
    "moonshotai",
    "mistral",
    "alibaba",
    "meta",
];

const LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
const COSTS: [f64; 6] = [0.4, 0.6, 1.0, 1.6, 2.5, 3.5];

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

pub fn effort_of(model: &str) -> &str {
    let Some((_, suffix)) = model.rsplit_once('-') else {
        return "";
    };
    match suffix.to_ascii_lowercase().as_str() {
        "none" => "none",
        "minimal" => "minimal",
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => "",
    }
}

pub fn base_model(model: &str) -> &str {
    let effort = effort_of(model);
    if effort.is_empty() {
        model
    } else {
        model.strip_suffix(&format!("-{effort}")).unwrap_or(model)
    }
}

fn rank(effort: &str) -> Option<usize> {
    LEVELS.iter().position(|&v| v == effort)
}

pub(crate) fn effort_rank(effort: &str) -> usize {
    rank(effort).unwrap_or(0)
}

fn cost_factor(effort: &str) -> Option<f64> {
    rank(effort).map(|i| COSTS[i])
}

fn logit(p: f64) -> f64 {
    let p = p.clamp(0.0001, 0.9999);
    (p / (1.0 - p)).ln()
}

pub fn catalog_key(model: &str) -> String {
    base_model(model.trim())
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .replace(' ', "-")
}

pub(crate) fn lookup_keys(key: &str) -> Vec<&str> {
    let mut out = vec![key];
    let mut current = key;
    while let Some(i) = current.rfind('-') {
        if i == 0 {
            break;
        }
        current = &current[..i];
        out.push(current);
    }
    out
}

/// Catalog keys an agent may name differently, mapped to the id the catalog lists. The
/// `-free` entry is listed for completeness only: [`price_keys`] keeps a free name on
/// free ids, so it stays unknown until the catalog lists a free entry of its own.
const PRICE_ALIASES: [(&str, &str); 2] = [
    ("deepseek-v4.1-flash", "deepseek-flash"),
    ("deepseek-v4.1-flash-free", "deepseek-flash"),
];

/// Whether a dash-separated key segment is a version (`v4`, `4.1`, `1`), not a family.
fn version_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let rest = match chars.next() {
        Some('v' | 'V') => chars.as_str(),
        _ => segment,
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The key with its version segments dropped: `deepseek-v4.1-flash` becomes
/// `deepseek-flash`. None when there is nothing to drop or no family would remain.
fn versionless_key(key: &str) -> Option<String> {
    let segments: Vec<&str> = key.split('-').collect();
    if segments.len() < 3 || !segments.iter().any(|segment| version_segment(segment)) {
        return None;
    }
    let kept: Vec<&str> = segments
        .iter()
        .copied()
        .filter(|segment| !version_segment(segment))
        .collect();
    (kept.len() >= 2 && kept.len() < segments.len()).then(|| kept.join("-"))
}

fn push_key(out: &mut Vec<String>, key: &str) {
    if !key.is_empty() && !out.iter().any(|known| known == key) {
        out.push(key.to_owned());
    }
}

/// The catalog ids to try for `key`, most specific first: the key and its shorter
/// prefixes, the same with dots as dashes, the version-less family, and the explicit
/// aliases. A `-free` key only tries `-free` ids, so a catalog without one keeps it
/// unknown rather than taking its paid sibling's price.
pub(crate) fn price_keys(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for known in lookup_keys(key) {
        push_key(&mut out, known);
    }
    let dotted = key.replace('.', "-");
    if dotted != key {
        for known in lookup_keys(&dotted) {
            push_key(&mut out, known);
        }
    }
    if let Some(versionless) = versionless_key(key) {
        push_key(&mut out, &versionless);
    }
    if let Some((_, alias)) = PRICE_ALIASES.iter().find(|(name, _)| *name == key) {
        push_key(&mut out, alias);
    }
    if key.ends_with("-free") {
        out.retain(|known| known.ends_with("-free"));
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub input: f64,
    pub output: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<PriceTier>,
    pub context: i64,
    pub max_output: i64,
    pub reasoning: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PriceTier {
    pub size: i64,
    pub input: f64,
    pub output: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BenchScore {
    pub model: String,
    pub effort: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_task: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_per_task: Option<f64>,
    pub date: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EciScore {
    pub model: String,
    pub eci: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogSource {
    pub name: String,
    pub url: String,
    pub license: String,
    pub fields: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Catalog {
    pub generated: String,
    pub sources: Vec<CatalogSource>,
    pub models: Vec<CatalogModel>,
    pub scores: Vec<BenchScore>,
    pub eci: Vec<EciScore>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prices_fetched: String,
    #[serde(skip)]
    pub eci_fit: Option<(f64, f64)>,
    #[serde(skip)]
    pub tokens_per_task: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelEstimate {
    pub key: String,
    pub theta: f64,
    pub theta_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_per_task: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cost_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip)]
    pub matched: String,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub extrapolated_levels: usize,
}

#[derive(Serialize, Deserialize)]
struct RefreshedPrices {
    fetched: String,
    models: Vec<CatalogModel>,
}

pub fn prices_path(state_dir: &Path) -> Option<PathBuf> {
    if state_dir.as_os_str().is_empty() {
        None
    } else {
        Some(state_dir.join("models.json"))
    }
}

pub fn load_catalog(state_dir: &Path) -> Result<Catalog, Error> {
    let mut c: Catalog = serde_json::from_str(include_str!("models_snapshot.json"))?;
    if let Some(p) = prices_path(state_dir)
        && let Ok(bytes) = std::fs::read(&p)
        && let Ok(r) = serde_json::from_slice::<RefreshedPrices>(&bytes)
        && !r.models.is_empty()
    {
        c.models = r.models;
        c.prices_fetched = r.fetched;
    }
    c.calibrate();
    Ok(c)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevProvider {
    models: Option<HashMap<String, ModelsDevEntry>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevEntry {
    name: String,
    tool_call: bool,
    reasoning: bool,
    modalities: ModelsDevModalities,
    cost: Option<ModelsDevCost>,
    limit: ModelsDevLimit,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevModalities {
    output: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevCost {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    tiers: Vec<ModelsDevCostTier>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevCostTier {
    input: f64,
    output: f64,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    tier: ModelsDevTierSpec,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevTierSpec {
    #[serde(rename = "type")]
    kind: String,
    size: i64,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModelsDevLimit {
    context: i64,
    output: i64,
}

pub fn parse_models_dev(raw: &[u8]) -> Result<Vec<CatalogModel>, Error> {
    let api: HashMap<String, Option<ModelsDevProvider>> = serde_json::from_slice(raw)?;
    let mut out = Vec::new();
    for provider in CATALOG_PROVIDERS {
        let Some(Some(prov)) = api.get(provider) else {
            continue;
        };
        let Some(models) = &prov.models else {
            continue;
        };
        for (id, m) in models {
            let has_text = m.modalities.output.iter().any(|o| o == "text");
            let Some(cost) = &m.cost else {
                continue;
            };
            if !m.tool_call || !has_text {
                continue;
            }
            let name = if m.name.is_empty() {
                id.clone()
            } else {
                m.name.clone()
            };
            let mut tiers: Vec<PriceTier> = cost
                .tiers
                .iter()
                .filter(|t| t.tier.kind == "context" && t.tier.size > 0)
                .map(|t| PriceTier {
                    size: t.tier.size,
                    input: t.input,
                    output: t.output,
                    cache_read: t.cache_read,
                    cache_write: t.cache_write,
                })
                .collect();
            tiers.sort_by_key(|t| t.size);
            out.push(CatalogModel {
                id: id.clone(),
                provider: provider.into(),
                name,
                input: cost.input,
                output: cost.output,
                cache_read: cost.cache_read,
                cache_write: cost.cache_write,
                tiers,
                context: m.limit.context,
                max_output: m.limit.output,
                reasoning: m.reasoning,
            });
        }
    }
    if out.is_empty() {
        return Err(Error::Message("models.dev returned no usable model".into()));
    }
    out.sort_by(|a, b| a.provider.cmp(&b.provider).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

pub fn save_refreshed_prices(
    state_dir: &Path,
    raw: &[u8],
    now: DateTime<Utc>,
) -> Result<usize, Error> {
    let Some(p) = prices_path(state_dir) else {
        return Err(Error::Message("no dispatch state directory".into()));
    };
    let models = parse_models_dev(raw)?;
    let count = models.len();
    let doc = RefreshedPrices {
        fetched: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        models,
    };
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    doc.serialize(&mut ser)?;
    if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, buf)?;
    Ok(count)
}

/// How long a stored catalog, or the embedded snapshot, stays fresh: the core refreshes
/// its prices once a day.
pub const CATALOG_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn catalog_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|at| at.with_timezone(&Utc))
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|date| date.and_utc())
        })
}

/// Whether `now` is at least [`CATALOG_MAX_AGE`] past the catalog's date: `fetched` when a
/// refresh stored one, else the embedded snapshot's `generated` date. An unreadable date
/// counts as stale.
pub fn catalog_refresh_due(fetched: &str, generated: &str, now: DateTime<Utc>) -> bool {
    match catalog_time(fetched).or_else(|| catalog_time(generated)) {
        Some(at) => {
            now.signed_duration_since(at).num_seconds()
                >= i64::try_from(CATALOG_MAX_AGE.as_secs()).unwrap_or(i64::MAX)
        }
        None => true,
    }
}

#[derive(Deserialize)]
struct SnapshotDate {
    #[serde(default)]
    generated: String,
}

/// [`catalog_refresh_due`] for a state folder's stored prices, falling back to the embedded
/// snapshot's date when it holds no usable copy.
pub fn refresh_due(state_dir: &Path, now: DateTime<Utc>) -> bool {
    let fetched = prices_path(state_dir)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<RefreshedPrices>(&b).ok())
        .filter(|r| !r.models.is_empty())
        .map(|r| r.fetched)
        .unwrap_or_default();
    let generated = serde_json::from_str::<SnapshotDate>(include_str!("models_snapshot.json"))
        .map(|snapshot| snapshot.generated)
        .unwrap_or_default();
    catalog_refresh_due(&fetched, &generated, now)
}

impl Catalog {
    pub fn embedded() -> Result<Self, Error> {
        let mut c: Self = serde_json::from_str(include_str!("models_snapshot.json"))?;
        c.calibrate();
        Ok(c)
    }

    pub fn load(state_dir: &Path) -> Result<Self, Error> {
        load_catalog(state_dir)
    }

    fn score_at(&self, key: &str, effort: &str) -> Option<(&BenchScore, f64, String)> {
        let want = rank(effort).unwrap_or(2);
        let row = self
            .scores
            .iter()
            .filter(|s| s.model == key)
            .min_by_key(|s| rank(&s.effort).unwrap_or(want).abs_diff(want))?;
        let mut shift = 0.0;
        let mut note = String::from("cursorbench");
        if !row.effort.is_empty() {
            note.push_str(&format!(" {}", row.effort));
            let r = rank(&row.effort).unwrap_or(0);
            if r != want {
                shift = (want as f64 - r as f64) * 0.15;
                note.push_str(&format!(" (nearest to {})", LEVELS[want]));
            }
        }
        Some((row, shift, note))
    }

    pub(crate) fn price(&self, key: &str) -> Option<&CatalogModel> {
        price_keys(key)
            .into_iter()
            .find_map(|k| self.models.iter().find(|m| m.id == k))
    }

    pub fn calibrate(&mut self) {
        let pairs: Vec<(f64, f64)> = self
            .eci
            .iter()
            .filter_map(|e| {
                self.score_at(&e.model, "medium")
                    .map(|(s, shift, _)| (e.eci, logit(s.score) + shift))
            })
            .collect();
        if pairs.len() >= 5 {
            let n = pairs.len() as f64;
            let mx = pairs.iter().map(|p| p.0).sum::<f64>() / n;
            let my = pairs.iter().map(|p| p.1).sum::<f64>() / n;
            let sxx = pairs.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>();
            if sxx != 0.0 {
                let slope = pairs.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / sxx;
                self.eci_fit = Some((slope, my - slope * mx));
            }
        }
        let mut ratios: Vec<f64> = self
            .scores
            .iter()
            .filter(|s| s.effort.is_empty() || s.effort == "medium")
            .filter_map(|s| {
                let price = self.price(&s.model)?;
                let blend = (3.0 * price.input + price.output) / 4.0;
                if blend > 0.0 {
                    s.cost_per_task.map(|cost| cost / blend * 1e6)
                } else {
                    None
                }
            })
            .collect();
        ratios.sort_by(f64::total_cmp);
        self.tokens_per_task = if ratios.len() >= 3 {
            ratios[ratios.len() / 2]
        } else {
            40_000.0
        };
    }

    pub fn estimate(&self, model: &str, effort: &str) -> ModelEstimate {
        let effort = if effort.is_empty() {
            effort_of(model)
        } else {
            effort
        };
        let key = catalog_key(model);
        let mut est = ModelEstimate {
            key: format!("{key}@{effort}"),
            ..Default::default()
        };
        for k in lookup_keys(&key) {
            if let Some((s, shift, mut note)) = self.score_at(k, effort) {
                if k != key {
                    note.push_str(&format!(" (as {k})"));
                }
                est.theta = logit(s.score) + shift;
                est.theta_source = note;
                est.matched = k.into();
                est.score = Some(s.score);
                est.extrapolated_levels = (shift / 0.15).abs().round() as usize;
                if let Some(mut cost) = s.cost_per_task {
                    if !s.effort.is_empty()
                        && s.effort != effort
                        && let (Some(a), Some(b)) = (cost_factor(effort), cost_factor(&s.effort))
                    {
                        cost *= a / b;
                    }
                    est.cost_per_task = Some(cost);
                    est.cost_source = "cursorbench".into();
                }
                break;
            }
        }
        if est.theta_source.is_empty()
            && let Some((slope, intercept)) = self.eci_fit
        {
            for k in lookup_keys(&key) {
                if let Some(e) = self.eci.iter().find(|e| e.model == k) {
                    est.theta = slope * e.eci + intercept;
                    est.theta_source = if k == key {
                        "eci".into()
                    } else {
                        format!("eci (as {k})")
                    };
                    est.matched = k.into();
                    est.score = Some(e.eci);
                    break;
                }
            }
        }
        if est.theta_source.is_empty() {
            est.theta = -0.7;
            est.theta_source = "unknown".into();
        }
        if !est.theta_source.starts_with("cursorbench")
            && let Some(r) = rank(effort)
        {
            let levels = r as i32 - 2;
            est.theta += f64::from(levels) * 0.15;
            est.extrapolated_levels = levels.unsigned_abs() as usize;
        }
        if est.cost_per_task.is_none()
            && let Some(m) = self.price(&key)
        {
            let tokens = if self.tokens_per_task == 0.0 {
                40_000.0
            } else {
                self.tokens_per_task
            };
            est.cost_per_task = Some(
                (3.0 * m.input + m.output) / 4.0 * tokens / 1e6
                    * cost_factor(effort).unwrap_or(1.0),
            );
            est.cost_source = "models.dev price".into();
        }
        est
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn embedded() -> Catalog {
        Catalog::embedded().expect("embedded catalog")
    }

    #[test]
    fn a_differently_named_model_prices_as_the_catalog_key() {
        let catalog = embedded();
        let model = catalog
            .price("deepseek-v4.1-flash")
            .expect("deepseek-v4.1-flash prices");
        assert_eq!(model.id, "deepseek-flash");
        assert_eq!(
            (model.input, model.output, model.cache_read),
            (0.15, 0.6, Some(0.003))
        );
    }

    #[test]
    fn an_agents_provider_prefix_still_finds_the_price() {
        let key = catalog_key("opencode-go/deepseek-v4.1-flash");
        assert_eq!(key, "deepseek-v4.1-flash");
        assert_eq!(
            self::embedded().price(&key).map(|m| m.id.as_str()),
            Some("deepseek-flash")
        );
    }

    #[test]
    fn an_exact_key_wins_over_its_shorter_forms() {
        let catalog = embedded();
        assert_eq!(
            catalog.price("deepseek-v4-flash").map(|m| m.id.as_str()),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            catalog.price("deepseek-flash").map(|m| m.id.as_str()),
            Some("deepseek-flash")
        );
    }

    #[test]
    fn dots_read_as_dashes() {
        let catalog = embedded();
        assert_eq!(
            catalog
                .price("deepseek-v4.1-flash-1017")
                .map(|m| m.id.as_str()),
            Some("deepseek-v4-1-flash-1017")
        );
    }

    #[test]
    fn an_unrelated_name_stays_unknown() {
        assert!(embedded().price("acme-mystery-9").is_none());
    }

    #[test]
    fn a_free_model_does_not_take_its_paid_siblings_price() {
        assert!(embedded().price("deepseek-v4.1-flash-free").is_none());
    }

    #[test]
    fn the_catalog_refreshes_after_a_day() {
        let now = Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap();
        // Older than 24 hours: refresh.
        assert!(catalog_refresh_due("2026-11-06T11:59:00Z", "", now));
        // Fresher: keep it.
        assert!(!catalog_refresh_due("2026-11-06T12:01:00Z", "", now));
        // No stored copy: the embedded snapshot's date decides.
        assert!(catalog_refresh_due("", "2026-11-02", now));
        assert!(!catalog_refresh_due("", "2026-11-07", now));
    }
}
