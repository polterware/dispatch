//! Pure logic for Intelligence: catalog mapping, skill score, filters, and price.
//!
//! The table is the catalog (`ListModelCatalog`) alone, as Electron's: the core already lists
//! the models the agent CLIs report in it.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::Value;

use super::filter::{FilterOption, FilterSpec, Filters};
pub use super::sort::SortDirection;
use super::sort::{SortValue, sort_rows};

/// Band difficulties the core prices: easy −2.8, normal −1.8, hard −0.8.
pub const BANDS: [&str; 3] = ["easy", "normal", "hard"];

pub const COLUMNS: [SkillColumn; 6] = [
    SkillColumn::Score,
    SkillColumn::Model,
    SkillColumn::Theta,
    SkillColumn::Easy,
    SkillColumn::Normal,
    SkillColumn::Hard,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SkillColumn {
    Score,
    Model,
    Theta,
    Easy,
    Normal,
    Hard,
}

impl SkillColumn {
    pub fn title(self) -> &'static str {
        match self {
            Self::Score => "Score",
            Self::Model => "Model",
            Self::Theta => "Skill (\u{03b8})",
            Self::Easy => "Easy",
            Self::Normal => "Normal",
            Self::Hard => "Hard",
        }
    }

    pub fn band(self) -> Option<&'static str> {
        match self {
            Self::Easy => Some("easy"),
            Self::Normal => Some("normal"),
            Self::Hard => Some("hard"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BandPass {
    pub tier: String,
    pub d: f64,
    pub p_pass: f64,
    pub low: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogEntry {
    pub agent: String,
    pub model: String,
    pub tier: String,
    pub effort: String,
    pub key: String,
    pub theta: f64,
    pub theta_source: String,
    pub score: Option<f64>,
    pub cost_per_task: Option<f64>,
    pub cost_source: String,
    pub bands: Vec<BandPass>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogSource {
    pub name: String,
    pub url: String,
    pub license: String,
    pub fields: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub generated: String,
    pub prices_fetched: String,
    pub sources: Vec<CatalogSource>,
    pub bars: BTreeMap<String, f64>,
    pub priority: String,
    pub discover: bool,
    pub entries: Vec<CatalogEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskPrice {
    pub label: String,
    pub title: String,
    pub min: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RefreshReport {
    pub ok: bool,
    pub error: String,
    pub models: Option<i64>,
}

pub fn empty_catalog() -> Catalog {
    Catalog {
        generated: String::new(),
        prices_fetched: String::new(),
        sources: Vec::new(),
        bars: BTreeMap::new(),
        priority: "balanced".to_string(),
        discover: true,
        entries: Vec::new(),
    }
}

pub fn row_id(entry: &CatalogEntry) -> String {
    format!("{}:{}", entry.agent, entry.model)
}

/// 100·σ(θ): the chance, in points, to pass a task of difficulty 0.
pub fn skill_score(theta: f64) -> i32 {
    if !theta.is_finite() {
        return 0;
    }
    (100.0 / (1.0 + (-theta).exp())).round() as i32
}

pub fn skill_digits(theta: f64) -> String {
    format!("{:>2}", skill_score(theta))
}

pub fn percent_text(value: f64) -> String {
    let rounded = if value.is_finite() {
        (value * 100.0).round() as i64
    } else {
        0
    };
    format!("{rounded}%")
}

pub fn theta_text(theta: f64) -> String {
    format!("{theta:.2}")
}

pub fn score_title(theta: f64) -> String {
    format!(
        "Skill score {}: 100\u{00b7}\u{03c3}(\u{03b8}), the chance to pass a task at d 0",
        skill_score(theta)
    )
}

pub fn band_title(pass: &BandPass, bar: f64) -> String {
    format!(
        "P(pass) at d {:.1}: {}, lower bound {} (bar {})",
        pass.d,
        percent_text(pass.p_pass),
        percent_text(pass.low),
        percent_text(bar)
    )
}

/// Where a model's θ comes from, with the benchmark figure it was read from.
pub fn theta_source_label(source: &str, score: Option<f64>) -> String {
    let source = source.trim();
    if let Some(rest) = source.strip_prefix("cursorbench") {
        let head = match score {
            Some(score) => format!("CursorBench {}", percent_text(score)),
            None => "CursorBench".to_string(),
        };
        join_head(&head, rest.trim())
    } else if let Some(rest) = source.strip_prefix("eci") {
        let head = match score {
            Some(score) => format!("ECI {}", score.round() as i64),
            None => "ECI".to_string(),
        };
        join_head(&head, rest.trim())
    } else if source == "tiers.json strength" {
        "Tier strength".to_string()
    } else {
        "Unknown (median)".to_string()
    }
}

fn join_head(head: &str, rest: &str) -> String {
    if rest.is_empty() {
        head.to_string()
    } else {
        format!("{head} {rest}")
    }
}

/// The benchmark family a θ comes from, as the Skill filter names it.
pub fn theta_family(source: &str) -> String {
    let source = source.trim();
    if source.starts_with("cursorbench") {
        "CursorBench".to_string()
    } else if source.starts_with("eci") {
        "ECI".to_string()
    } else if source == "tiers.json strength" {
        "Tier strength".to_string()
    } else {
        "Unknown".to_string()
    }
}

pub fn band_label(tier: &str) -> String {
    let mut chars = tier.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub fn display_priority(priority: &str) -> String {
    priority
        .split(' ')
        .map(band_label)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A model's tier, else Found when Auto routes to reported models, else Installed.
pub fn tier_label(tier: &str, discover: bool) -> String {
    if !tier.is_empty() {
        tier.to_string()
    } else if discover {
        "Found".to_string()
    } else {
        "Installed".to_string()
    }
}

pub fn tier_chip_title(tier: &str, discover: bool) -> Option<&'static str> {
    if tier.is_empty() && !discover {
        Some("Auto routes to it only with discover on")
    } else {
        None
    }
}

pub fn policy_bar(catalog: &Catalog) -> f64 {
    catalog
        .bars
        .get(&catalog.priority)
        .copied()
        .filter(|bar| bar.is_finite())
        .unwrap_or(0.8)
}

fn bands_meeting_bar(entry: &CatalogEntry, bar: f64) -> Vec<String> {
    entry
        .bands
        .iter()
        .filter(|band| band.low >= bar)
        .map(|band| band_label(&band.tier))
        .collect()
}

pub fn band_of<'a>(entry: &'a CatalogEntry, tier: &str) -> Option<&'a BandPass> {
    entry.bands.iter().find(|band| band.tier == tier)
}

/// `modelFieldValues`: the entry's value for each model filter; "bar" holds every band whose
/// lower bound meets the policy's bar.
pub fn matches_model_filters(
    entry: &CatalogEntry,
    filters: &Filters,
    bar: f64,
    discover: bool,
) -> bool {
    if filters.is_empty() {
        return true;
    }
    let tier = tier_label(&entry.tier, discover);
    let skill = theta_family(&entry.theta_source);
    let bands = bands_meeting_bar(entry, bar);
    filters.matches(|field| match field {
        "agent" => Some(vec![entry.agent.as_str()]),
        "tier" => Some(vec![tier.as_str()]),
        "skill" => Some(vec![skill.as_str()]),
        "bar" => Some(bands.iter().map(String::as_str).collect()),
        _ => None,
    })
}

pub fn matches_search(entry: &CatalogEntry, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    [&entry.model, &entry.agent, &entry.tier, &entry.key]
        .iter()
        .any(|name| name.to_lowercase().contains(&needle))
}

pub fn empty_title(filtered: bool, has_entries: bool, loaded: bool) -> &'static str {
    if filtered && has_entries {
        "No matches"
    } else if loaded {
        "No models to price"
    } else {
        "Loading models"
    }
}

pub fn cost_source_label(source: &str) -> String {
    if source == "cursorbench" {
        "CursorBench".to_string()
    } else if source == "models.dev price" {
        "models.dev".to_string()
    } else {
        source.to_string()
    }
}

fn money(usd: f64) -> String {
    format!("${usd:.2}")
}

/// What one task costs on an agent's catalog models, cheapest to dearest.
pub fn agent_task_price(entries: &[CatalogEntry], agent: &str) -> Option<TaskPrice> {
    let mut priced: Vec<&CatalogEntry> = entries
        .iter()
        .filter(|entry| entry.agent == agent && entry.cost_per_task.is_some())
        .collect();
    if priced.is_empty() {
        return None;
    }
    priced.sort_by(|left, right| {
        cmp_f64(
            left.cost_per_task.unwrap_or(0.0),
            right.cost_per_task.unwrap_or(0.0),
        )
    });
    let min = priced[0].cost_per_task.unwrap_or(0.0);
    let max = priced[priced.len() - 1].cost_per_task.unwrap_or(0.0);
    let title = priced
        .iter()
        .map(|entry| {
            let cost = entry.cost_per_task.unwrap_or(0.0);
            let source = cost_source_label(&entry.cost_source);
            if source.is_empty() {
                format!("{}: {}", entry.model, money(cost))
            } else {
                format!("{}: {} ({source})", entry.model, money(cost))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let label = if min == max {
        money(min)
    } else {
        format!("{}\u{2013}{}", money(min), money(max))
    };
    Some(TaskPrice { label, title, min })
}

pub fn map_dispatch_catalog(json: &str) -> Catalog {
    let parsed = serde_json::from_str::<Value>(json).unwrap_or(Value::Null);
    let Some(body) = parsed.as_object() else {
        return empty_catalog();
    };
    let mut bars = BTreeMap::new();
    if let Some(raw_bars) = body.get("bars").and_then(Value::as_object) {
        for (priority, bar) in raw_bars {
            bars.insert(priority.clone(), json_f64(Some(bar)));
        }
    }
    let sources = body
        .get("sources")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|item| {
                    let row = item.as_object();
                    CatalogSource {
                        name: json_string(row.and_then(|row| row.get("name"))),
                        url: json_string(row.and_then(|row| row.get("url"))),
                        license: json_string(row.and_then(|row| row.get("license"))),
                        fields: json_string(row.and_then(|row| row.get("fields"))),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let entries = body
        .get("entries")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|item| {
                    let row = item.as_object()?;
                    let model = json_string(row.get("model"));
                    if model.is_empty() {
                        return None;
                    }
                    let bands = row
                        .get("bands")
                        .and_then(Value::as_array)
                        .map(|bands| {
                            bands
                                .iter()
                                .map(|band| {
                                    let band = band.as_object();
                                    BandPass {
                                        tier: json_string(band.and_then(|band| band.get("tier"))),
                                        d: json_f64(band.and_then(|band| band.get("d"))),
                                        p_pass: json_f64(band.and_then(|band| band.get("pPass"))),
                                        low: json_f64(band.and_then(|band| band.get("low"))),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(CatalogEntry {
                        agent: json_string(row.get("agent")),
                        model,
                        tier: json_string(row.get("tier")),
                        effort: json_string(row.get("effort")),
                        key: json_string(row.get("key")),
                        theta: json_f64(row.get("theta")),
                        theta_source: json_string(row.get("thetaSource")),
                        score: json_f64_opt(row.get("score")),
                        cost_per_task: json_f64_opt(row.get("costPerTask")),
                        cost_source: json_string(row.get("costSource")),
                        bands,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let priority = json_string(body.get("priority"));
    Catalog {
        generated: json_string(body.get("generated")),
        prices_fetched: json_string(body.get("pricesFetched")),
        sources,
        bars,
        priority: if priority.is_empty() {
            "balanced".to_string()
        } else {
            priority
        },
        discover: json_not_false(body.get("discover")),
        entries,
    }
}

pub fn map_refresh(ok: bool, json: &str) -> RefreshReport {
    let parsed = serde_json::from_str::<Value>(json).unwrap_or(Value::Null);
    RefreshReport {
        ok,
        error: json_string(parsed.get("error")),
        models: json_i64_opt(parsed.get("models")),
    }
}

pub fn refresh_failure(report: &RefreshReport) -> Option<String> {
    if report.ok && report.error.is_empty() {
        None
    } else if !report.error.is_empty() {
        Some(report.error.clone())
    } else {
        Some("Could not refresh the model catalog.".to_string())
    }
}

/// `modelFilterFields`: Agent, Tier, Skill source, and Meets the bar, each over the values the
/// catalog holds, in the order Electron lists them.
pub fn model_filter_fields(entries: &[CatalogEntry], discover: bool) -> [FilterSpec; 4] {
    [
        FilterSpec::values(
            "agent",
            "Agent",
            entries.iter().map(|entry| entry.agent.as_str()),
        ),
        FilterSpec::multi(
            "tier",
            "Tier",
            distinct(
                entries
                    .iter()
                    .map(|entry| tier_label(&entry.tier, discover)),
                &["easy", "normal", "hard", "Found", "Installed"],
            ),
        ),
        FilterSpec::multi(
            "skill",
            "Skill source",
            distinct(
                entries
                    .iter()
                    .map(|entry| theta_family(&entry.theta_source)),
                &["CursorBench", "ECI", "Tier strength", "Unknown"],
            ),
        ),
        FilterSpec::multi(
            "bar",
            "Meets the bar",
            BANDS
                .iter()
                .map(|tier| {
                    let label = band_label(tier);
                    FilterOption::new(label.clone(), label)
                })
                .collect(),
        )
        .plural("bands"),
    ]
}

/// The default order: Score, high to low (`defaultSort`).
pub const DEFAULT_SORT: SkillSort = Some((SkillColumn::Score, SortDirection::Descending));

/// The column the table is sorted by, if any.
pub type SkillSort = Option<(SkillColumn, SortDirection)>;

/// The catalog rows the search and the filters keep, in the order of `sort`.
pub fn visible_entries(
    entries: &[CatalogEntry],
    query: &str,
    filters: &Filters,
    bar: f64,
    discover: bool,
    sort: SkillSort,
) -> Vec<CatalogEntry> {
    let rows: Vec<CatalogEntry> = entries
        .iter()
        .filter(|entry| {
            matches_search(entry, query) && matches_model_filters(entry, filters, bar, discover)
        })
        .cloned()
        .collect();
    match sort {
        Some((column, direction)) => sort_rows(&rows, direction, |entry| sort_value(entry, column)),
        None => rows,
    }
}

/// Each column's `sortBy`.
fn sort_value(entry: &CatalogEntry, column: SkillColumn) -> SortValue {
    match column {
        SkillColumn::Score | SkillColumn::Theta => SortValue::Number(entry.theta),
        SkillColumn::Model => SortValue::Text(entry.model.clone()),
        SkillColumn::Easy | SkillColumn::Normal | SkillColumn::Hard => {
            let tier = column.band().unwrap_or("");
            SortValue::Number(band_of(entry, tier).map(|band| band.low).unwrap_or(-1.0))
        }
    }
}

fn cmp_f64(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

/// `distinct(values, order)`: each value once, by its place in `order` (one not listed first).
fn distinct(values: impl IntoIterator<Item = String>, order: &[&str]) -> Vec<FilterOption> {
    let mut set: Vec<String> = Vec::new();
    for value in values {
        if !value.is_empty() && !set.contains(&value) {
            set.push(value);
        }
    }
    let rank = |value: &str| order.iter().position(|item| *item == value);
    set.sort_by(|left, right| match (rank(left), rank(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    set.into_iter()
        .map(|value| FilterOption::new(value.clone(), value))
        .collect()
}

fn json_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

fn json_f64(value: Option<&Value>) -> f64 {
    json_f64_opt(value).unwrap_or(0.0)
}

fn json_f64_opt(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64().filter(|number| number.is_finite()),
        _ => None,
    }
}

fn json_i64_opt(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|n| n as i64)),
        _ => None,
    }
}

fn json_not_false(value: Option<&Value>) -> bool {
    !matches!(value, Some(Value::Bool(false)))
}

#[cfg(test)]
mod tests {
    use super::super::filter::{FilterSpec, Filters};
    use super::{
        BandPass, CatalogEntry, SkillColumn, SortDirection, agent_task_price, map_dispatch_catalog,
        map_refresh, matches_model_filters, refresh_failure, skill_score, theta_source_label,
        visible_entries,
    };

    fn entry(
        agent: &str,
        model: &str,
        tier: &str,
        theta_source: &str,
        score: Option<f64>,
        cost: Option<f64>,
        cost_source: &str,
        bands: Vec<BandPass>,
    ) -> CatalogEntry {
        CatalogEntry {
            agent: agent.to_string(),
            model: model.to_string(),
            tier: tier.to_string(),
            effort: String::new(),
            key: model.to_string(),
            theta: 0.0,
            theta_source: theta_source.to_string(),
            score,
            cost_per_task: cost,
            cost_source: cost_source.to_string(),
            bands,
        }
    }

    fn clause(field: &str, values: &[&str]) -> Filters {
        let spec = FilterSpec::multi(field, field, Vec::new());
        let mut filters = Filters::default();
        let values: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
        filters.add(&spec, &values);
        filters
    }

    #[test]
    fn names_the_benchmark_each_theta_comes_from_with_its_figure() {
        assert_eq!(
            theta_source_label("cursorbench high", Some(0.62)),
            "CursorBench 62% high"
        );
        assert_eq!(
            theta_source_label("eci (as gpt-5.6)", Some(141.2)),
            "ECI 141 (as gpt-5.6)"
        );
        assert_eq!(
            theta_source_label("tiers.json strength", None),
            "Tier strength"
        );
        assert_eq!(theta_source_label("unknown", None), "Unknown (median)");
    }

    #[test]
    fn filters_models_by_tier_skill_source_and_the_bar() {
        let strong = entry(
            "codex",
            "m",
            "hard",
            "cursorbench high",
            None,
            Some(0.2),
            "",
            vec![
                BandPass {
                    tier: "easy".into(),
                    d: -2.8,
                    p_pass: 0.95,
                    low: 0.9,
                },
                BandPass {
                    tier: "hard".into(),
                    d: -0.8,
                    p_pass: 0.7,
                    low: 0.6,
                },
            ],
        );
        let found = entry(
            "grok",
            "m",
            "",
            "eci",
            None,
            None,
            "",
            vec![BandPass {
                tier: "easy".into(),
                d: -2.8,
                p_pass: 0.8,
                low: 0.7,
            }],
        );
        assert!(matches_model_filters(
            &found,
            &clause("tier", &["Found"]),
            0.8,
            true
        ));
        assert!(matches_model_filters(
            &found,
            &clause("tier", &["Installed"]),
            0.8,
            false
        ));
        assert!(!matches_model_filters(
            &strong,
            &clause("tier", &["Found"]),
            0.8,
            false
        ));
        assert!(matches_model_filters(
            &strong,
            &clause("skill", &["CursorBench"]),
            0.8,
            false
        ));
        assert!(matches_model_filters(
            &strong,
            &clause("bar", &["Easy"]),
            0.8,
            false
        ));
        assert!(!matches_model_filters(
            &strong,
            &clause("bar", &["Hard"]),
            0.8,
            false
        ));
    }

    #[test]
    fn prices_an_agents_task_from_its_catalog_models_cheapest_to_dearest() {
        let entries = [
            entry(
                "codex",
                "big",
                "",
                "unknown",
                None,
                Some(0.4),
                "models.dev price",
                vec![],
            ),
            entry(
                "codex",
                "small",
                "",
                "unknown",
                None,
                Some(0.05),
                "cursorbench",
                vec![],
            ),
            entry("codex", "free", "", "unknown", None, None, "", vec![]),
            entry("grok", "m", "", "unknown", None, Some(1.0), "", vec![]),
        ];
        let price = agent_task_price(&entries, "codex").unwrap();
        assert_eq!(price.label, "$0.05\u{2013}$0.40");
        assert_eq!(
            price.title,
            "small: $0.05 (CursorBench)\nbig: $0.40 (models.dev)"
        );
        assert_eq!(price.min, 0.05);
        assert_eq!(agent_task_price(&entries, "grok").unwrap().label, "$1.00");
        assert!(agent_task_price(&entries, "cursor").is_none());
    }

    #[test]
    fn skill_score_rises_with_theta_on_a_0_to_100_scale() {
        assert_eq!(skill_score(0.0), 50);
        assert_eq!(skill_score(0.49), 62);
        assert_eq!(skill_score(-0.71), 33);
        assert!(skill_score(1.0) > skill_score(0.5));
    }

    #[test]
    fn reads_the_catalog_with_benchmark_scores_and_band_estimates() {
        let view = map_dispatch_catalog(
            r#"{
              "generated": "2026-10-31",
              "bars": { "speed": 0.7, "balanced": 0.8 },
              "sources": [{ "name": "CursorBench", "url": "https://epoch.ai", "license": "CC-BY", "fields": "score" }],
              "entries": [
                { "agent": "codex", "model": "m", "tier": "easy", "effort": "low", "key": "m@low", "theta": -1.2, "thetaSource": "cursorbench low", "score": 0.23, "costPerTask": 0.08, "costSource": "cursorbench", "bands": [{ "tier": "easy", "d": -2.8, "pPass": 0.82, "low": 0.78 }] },
                { "agent": "grok", "model": "g", "theta": 0, "thetaSource": "unknown", "bands": [] },
                { "agent": "x" }
              ]
            }"#,
        );
        assert!((view.bars["balanced"] - 0.8).abs() < 1e-9);
        assert_eq!(view.entries.len(), 2);
        assert_eq!(view.entries[0].score, Some(0.23));
        assert_eq!(view.entries[0].cost_per_task, Some(0.08));
        assert_eq!(view.entries[0].bands[0].tier, "easy");
        assert!((view.entries[0].bands[0].p_pass - 0.82).abs() < 1e-9);
        assert!((view.entries[0].bands[0].low - 0.78).abs() < 1e-9);
        assert!(view.entries[1].score.is_none());
        assert_eq!(view.entries[1].tier, "");
        assert!(map_dispatch_catalog("{}").discover);
        assert!(map_dispatch_catalog(r#"{"discover":true}"#).discover);
        assert!(!map_dispatch_catalog(r#"{"discover":false}"#).discover);
        assert_eq!(map_dispatch_catalog("{}").priority, "balanced");
    }

    #[test]
    fn orders_the_table_by_skill_descending() {
        let mut low = entry("codex", "low", "", "unknown", None, None, "", vec![]);
        low.theta = -0.71;
        let mut high = entry("grok", "high", "", "eci", None, None, "", vec![]);
        high.theta = 0.49;
        let rows = visible_entries(
            &[low, high],
            "",
            &Filters::default(),
            0.8,
            true,
            Some((SkillColumn::Score, SortDirection::Descending)),
        );
        assert_eq!(rows[0].model, "high");
        assert_eq!(rows[1].model, "low");
    }

    #[test]
    fn refresh_failure_uses_the_catalog_error() {
        let ok = map_refresh(true, r#"{"source":"https://models.dev","models":12}"#);
        assert!(refresh_failure(&ok).is_none());
        assert_eq!(ok.models, Some(12));
        let failed = map_refresh(false, r#"{"error":"models.dev: 500"}"#);
        assert_eq!(refresh_failure(&failed).as_deref(), Some("models.dev: 500"));
        assert_eq!(
            refresh_failure(&map_refresh(false, "{}")).as_deref(),
            Some("Could not refresh the model catalog.")
        );
    }
}
