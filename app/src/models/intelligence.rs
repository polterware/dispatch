//! Pure logic for Intelligence: catalog mapping, skill score, filters, and price.
//!
//! The table is the catalog (`ListModelCatalog`) alone: the core already lists the models the
//! agent CLIs report in it. The catalog has one entry per agent and model, so a model two
//! agents offer (Cursor's `muse-spark-1.3-medium` and OpenCode's `opencode/muse-spark-1.3`,
//! Codex's `gpt-6-astra` and OpenCode's `openai/gpt-6-astra`) comes once per agent, and once
//! per effort; [`group_models`] folds them into one [`ModelRow`] per model (the catalog key's
//! model, provider and effort taken off), holding every effort and every agent that offers it.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::Value;

use super::filter::{FilterOption, FilterSpec, Filters};
pub use super::sort::SortDirection;
use super::sort::{SortValue, sort_rows};

/// Band difficulties the core prices: easy −2.8, normal −1.8, hard −0.8.
pub const BANDS: [&str; 3] = ["easy", "normal", "hard"];

pub const COLUMNS: [SkillColumn; 5] = [
    SkillColumn::Score,
    SkillColumn::Model,
    SkillColumn::Easy,
    SkillColumn::Normal,
    SkillColumn::Hard,
];

/// The catalog's effort levels, lowest first (`catalog.rs`'s `LEVELS`); medium is θ's anchor.
const LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// The θ an effort level adds away from medium, for a θ read from no CursorBench run.
const EFFORT_STEP: f64 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SkillColumn {
    Score,
    Model,
    Easy,
    Normal,
    Hard,
}

impl SkillColumn {
    pub fn title(self) -> &'static str {
        match self {
            Self::Score => "Score",
            Self::Model => "Model",
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

/// An agent on this machine that offers a model: the model as that agent names it, the
/// effort it runs it at ("" for the agent's default), and the tier it holds, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub agent: String,
    pub model: String,
    pub effort: String,
    pub tier: String,
}

/// One row of the table: a model, with each effort the agents here run it at (lowest first,
/// the agents' default before them) and every agent that offers it (those in a tier first).
/// Its Score and bands are its best effort's, `entry` (the highest θ, as Auto keeps a model's
/// best effort), at `effort`.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRow {
    pub name: String,
    pub effort: String,
    pub entry: CatalogEntry,
    pub efforts: Vec<CatalogEntry>,
    pub offers: Vec<Offer>,
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

/// An effort's place, lowest first, the agents' default ("") before every named level.
fn effort_rank(effort: &str) -> usize {
    LEVELS
        .iter()
        .position(|level| *level == effort)
        .map_or(0, |rank| rank + 1)
}

/// The catalog's entries folded to one row per model, in the catalog's order: the catalog
/// key's model (`gemini-4-argon` of `gemini-4-argon@high`), the same through every agent and
/// provider. Each effort keeps its first entry, whose θ every agent shares. An entry with no
/// key stands alone, by its agent and model.
pub fn group_models(entries: &[CatalogEntry]) -> Vec<ModelRow> {
    let mut rows: Vec<ModelRow> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        let (name, effort) = match entry.key.split_once('@') {
            Some((name, effort)) if !name.is_empty() => (name.to_string(), effort.to_string()),
            _ => (entry.model.clone(), entry.effort.clone()),
        };
        let group = if entry.key.is_empty() {
            format!("{}:{}", entry.agent, entry.model)
        } else {
            name.clone()
        };
        let offer = Offer {
            agent: entry.agent.clone(),
            model: entry.model.clone(),
            effort: effort.clone(),
            tier: entry.tier.clone(),
        };
        let at = match index.get(&group) {
            Some(&at) => at,
            None => {
                index.insert(group, rows.len());
                rows.push(ModelRow {
                    name,
                    effort: String::new(),
                    entry: entry.clone(),
                    efforts: Vec::new(),
                    offers: Vec::new(),
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        row.offers.push(offer);
        if !row.efforts.iter().any(|known| known.effort == effort) {
            let mut kept = entry.clone();
            kept.effort = effort;
            row.efforts.push(kept);
        }
    }
    for row in &mut rows {
        row.efforts.sort_by_key(|entry| effort_rank(&entry.effort));
        // The best effort: the highest θ, the lower effort on a tie.
        if let Some(best) = row
            .efforts
            .iter()
            .fold(None::<&CatalogEntry>, |best, entry| match best {
                Some(best) if best.theta >= entry.theta => Some(best),
                _ => Some(entry),
            })
        {
            row.entry = best.clone();
            row.effort = best.effort.clone();
        }
        // Stable: those in a tier first, else the catalog's order.
        row.offers.sort_by_key(|offer| offer.tier.is_empty());
    }
    rows
}

pub fn row_id(row: &ModelRow) -> String {
    row.name.clone()
}

/// An effort as the dialog and the tooltips name it.
pub fn effort_label(effort: &str) -> String {
    if effort.is_empty() {
        "Default".to_string()
    } else {
        band_label(effort)
    }
}

/// How the Score's effort reads: the one effort, or the best of several.
pub fn best_effort_line(row: &ModelRow) -> String {
    let at = if row.effort.is_empty() {
        "its default effort".to_string()
    } else {
        format!("{} effort", row.effort)
    };
    match row.efforts.len() {
        0 | 1 => format!("At {at}"),
        count => format!("At {at}, its best of {count}"),
    }
}

/// An offer as the agent runs it: the model, and its effort when the name does not say it.
pub fn offer_name(offer: &Offer) -> String {
    if offer.effort.is_empty() || offer.model.ends_with(&format!("-{}", offer.effort)) {
        offer.model.clone()
    } else {
        format!("{} \u{00b7} {}", offer.model, offer.effort)
    }
}

/// The agents that offer the row, each once, in the offers' order.
pub fn row_agents(row: &ModelRow) -> Vec<&str> {
    let mut agents: Vec<&str> = Vec::new();
    for offer in &row.offers {
        if !agents.contains(&offer.agent.as_str()) {
            agents.push(&offer.agent);
        }
    }
    agents
}

/// The tiers the row holds, each once; a row in none is Found or Installed.
pub fn row_tiers(row: &ModelRow, discover: bool) -> Vec<String> {
    let mut tiers: Vec<String> = Vec::new();
    for offer in &row.offers {
        if !offer.tier.is_empty() && !tiers.contains(&offer.tier) {
            tiers.push(offer.tier.clone());
        }
    }
    if tiers.is_empty() {
        tiers.push(tier_label("", discover));
    }
    tiers
}

/// How the dialog says what bright means in the band rows: the lower bound meets the
/// policy priority's bar.
pub fn bar_note(catalog: &Catalog) -> String {
    format!(
        "Bright where the lower bound meets the {} bar ({}).",
        display_priority(&catalog.priority),
        percent_text(policy_bar(catalog))
    )
}

/// The dialog's line on who offers the model.
pub fn offered_line(row: &ModelRow) -> String {
    match row_agents(row).len() {
        1 => "One agent here offers it.".to_string(),
        count => format!("{count} agents here offer it."),
    }
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

fn logit(p: f64) -> f64 {
    let p = p.clamp(0.0001, 0.9999);
    (p / (1.0 - p)).ln()
}

/// What the model's dialog says of its Score: the score, θ and where it comes from, how far
/// under θ the lower bounds sit, how the source becomes θ, and the catalog's date and sources.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreDetail {
    pub title: String,
    /// Which effort the Score is read at: [`best_effort_line`].
    pub effort: String,
    pub theta: String,
    pub source: String,
    /// `θ − u`, read back from a band's lower bound, or None with no band.
    pub bound: Option<String>,
    pub how: String,
    pub note: String,
}

pub fn score_detail(row: &ModelRow, catalog: &Catalog) -> ScoreDetail {
    let entry = &row.entry;
    let source = entry.theta_source.trim();
    let mut how = if source.starts_with("cursorbench") {
        let shift = entry
            .score
            .map(|score| entry.theta - logit(score))
            .unwrap_or(0.0);
        if shift.abs() >= 0.005 {
            format!(
                "\u{03b8} is the logit of CursorBench's pass rate at the nearest effort it ran, moved {EFFORT_STEP:.2} a level to this one."
            )
        } else {
            "\u{03b8} is the logit of CursorBench's pass rate at this effort, so the Score is that rate.".to_string()
        }
    } else if source.starts_with("eci") {
        "No CursorBench run: its Epoch Capabilities Index, put on CursorBench's scale by a line fitted on the models that have both.".to_string()
    } else if source == "tiers.json strength" {
        "No benchmark knows it: the tier table's strength, \u{2212}0.7 and 0.35 a step from 3."
            .to_string()
    } else {
        "No benchmark knows it: the field's middle, \u{03b8} \u{2212}0.7.".to_string()
    };
    let levels = LEVELS
        .iter()
        .position(|level| *level == row.effort)
        .map(|rank| rank as i64 - 2)
        .unwrap_or(0);
    if !source.starts_with("cursorbench") && levels != 0 {
        how.push_str(&format!(
            " Each effort level from medium adds {EFFORT_STEP:.2}."
        ));
    }
    let bound = entry
        .bands
        .iter()
        .find(|band| band.low > 0.0 && band.low < 1.0)
        .map(|band| entry.theta - band.d - logit(band.low))
        .map(|margin| format!("\u{03b8} \u{2212} {margin:.2}"));
    let mut note: Vec<String> = Vec::new();
    if !catalog.generated.is_empty() {
        note.push(format!("Catalog {}", catalog.generated));
    }
    let sources = skill_sources(&catalog.sources);
    if !sources.is_empty() {
        note.push(sources);
    }
    ScoreDetail {
        title: format!("Score {}", skill_score(entry.theta)),
        effort: best_effort_line(row),
        theta: theta_text(entry.theta),
        source: theta_source_label(&entry.theta_source, entry.score),
        bound,
        how,
        note: note.join(" \u{00b7} "),
    }
}

/// The sources θ comes from, by publisher: "Epoch AI, CursorBench" and "Epoch AI, Epoch
/// Capabilities Index" read "Epoch AI: CursorBench, Epoch Capabilities Index". A source of
/// prices alone (models.dev) is left out: the Score is skill only.
fn skill_sources(sources: &[CatalogSource]) -> String {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for source in sources {
        let name = source.name.trim();
        if name.is_empty() || source.fields.trim().starts_with("prices") {
            continue;
        }
        let (publisher, work) = match name.split_once(", ") {
            Some((publisher, work)) => (publisher.to_string(), Some(work.to_string())),
            None => (name.to_string(), None),
        };
        let at = match groups.iter().position(|(known, _)| *known == publisher) {
            Some(at) => at,
            None => {
                groups.push((publisher, Vec::new()));
                groups.len() - 1
            }
        };
        groups[at].1.extend(work);
    }
    groups
        .into_iter()
        .map(|(publisher, works)| {
            if works.is_empty() {
                publisher
            } else {
                format!("{publisher}: {}", works.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether the Score is a guess: no benchmark knows the model, so θ is the field's middle.
/// The table draws such a Score dimmed.
pub fn is_guess(row: &ModelRow) -> bool {
    theta_family(&row.entry.theta_source) == "Unknown"
}

/// What the dialog's agent list closes on: whether Auto routes to it.
pub fn routing_note(row: &ModelRow, discover: bool) -> &'static str {
    let tiered = row.offers.iter().any(|offer| !offer.tier.is_empty());
    match (tiered, discover) {
        (_, true) => "Auto can route to it through any of these.",
        (true, false) => "Auto routes to it through the tier table alone: discover is off.",
        (false, false) => "Auto routes to it only with discover on.",
    }
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

/// `modelFieldValues`: the row's values for each model filter: every agent that offers it,
/// every tier it holds, and in "bar" every band whose lower bound meets the policy's bar.
pub fn matches_model_filters(row: &ModelRow, filters: &Filters, bar: f64, discover: bool) -> bool {
    if filters.is_empty() {
        return true;
    }
    let tiers = row_tiers(row, discover);
    let skill = theta_family(&row.entry.theta_source);
    let bands = bands_meeting_bar(&row.entry, bar);
    filters.matches(|field| match field {
        "agent" => Some(row_agents(row)),
        "tier" => Some(tiers.iter().map(String::as_str).collect()),
        "skill" => Some(vec![skill.as_str()]),
        "bar" => Some(bands.iter().map(String::as_str).collect()),
        _ => None,
    })
}

/// The search reads the model, its effort and key, and each offer's agent, name, and tier.
pub fn matches_search(row: &ModelRow, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let found = |text: &str| text.to_lowercase().contains(&needle);
    found(&row.name)
        || row
            .efforts
            .iter()
            .any(|entry| found(&entry.effort) || found(&entry.key))
        || row
            .offers
            .iter()
            .any(|offer| found(&offer.agent) || found(&offer.model) || found(&offer.tier))
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
pub fn model_filter_fields(rows: &[ModelRow], discover: bool) -> [FilterSpec; 4] {
    [
        FilterSpec::values(
            "agent",
            "Agent",
            rows.iter()
                .flat_map(|row| row.offers.iter().map(|offer| offer.agent.as_str())),
        ),
        FilterSpec::multi(
            "tier",
            "Tier",
            distinct(
                rows.iter().flat_map(|row| row_tiers(row, discover)),
                &["easy", "normal", "hard", "Found", "Installed"],
            ),
        ),
        FilterSpec::multi(
            "skill",
            "Skill source",
            distinct(
                rows.iter().map(|row| theta_family(&row.entry.theta_source)),
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

/// The rows the search and the filters keep, in the order of `sort`.
pub fn visible_rows(
    rows: &[ModelRow],
    query: &str,
    filters: &Filters,
    bar: f64,
    discover: bool,
    sort: SkillSort,
) -> Vec<ModelRow> {
    let kept: Vec<ModelRow> = rows
        .iter()
        .filter(|row| {
            matches_search(row, query) && matches_model_filters(row, filters, bar, discover)
        })
        .cloned()
        .collect();
    match sort {
        Some((column, direction)) => sort_rows(&kept, direction, |row| sort_value(row, column)),
        None => kept,
    }
}

/// Each column's `sortBy`.
fn sort_value(row: &ModelRow, column: SkillColumn) -> SortValue {
    match column {
        SkillColumn::Score => SortValue::Number(row.entry.theta),
        SkillColumn::Model => SortValue::Text(row.name.clone()),
        SkillColumn::Easy | SkillColumn::Normal | SkillColumn::Hard => {
            let tier = column.band().unwrap_or("");
            SortValue::Number(
                band_of(&row.entry, tier)
                    .map(|band| band.low)
                    .unwrap_or(-1.0),
            )
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
        BandPass, CatalogEntry, CatalogSource, ModelRow, SkillColumn, SortDirection,
        agent_task_price, best_effort_line, empty_catalog, group_models, map_dispatch_catalog,
        map_refresh, matches_model_filters, offer_name, offered_line, refresh_failure,
        routing_note, row_tiers, score_detail, skill_score, theta_source_label, visible_rows,
    };

    fn row(entry: CatalogEntry) -> ModelRow {
        group_models(&[entry]).remove(0)
    }

    fn keyed(agent: &str, model: &str, key: &str, tier: &str) -> CatalogEntry {
        let mut entry = entry(agent, model, tier, "eci", None, None, "", vec![]);
        entry.key = key.to_string();
        entry
    }

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
        let (strong, found) = (row(strong), row(found));
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
    fn folds_a_model_into_one_row_whatever_agent_provider_or_effort() {
        let at = |agent: &str, model: &str, key: &str, tier: &str, theta: f64| {
            let mut entry = keyed(agent, model, key, tier);
            entry.theta = theta;
            entry
        };
        let rows = group_models(&[
            at("codex", "gpt-6-astra", "gpt-6-astra@", "", -0.1),
            at(
                "cursor",
                "muse-spark-1.3-medium",
                "muse-spark-1.3@medium",
                "",
                -0.6,
            ),
            at("opencode", "openai/gpt-6-astra", "gpt-6-astra@", "", -0.1),
            at(
                "opencode",
                "opencode/muse-spark-1.3",
                "muse-spark-1.3@",
                "",
                -0.7,
            ),
            at(
                "claude",
                "claude-opus-5-5",
                "claude-opus-5-5@medium",
                "normal",
                0.1,
            ),
            at(
                "claude",
                "claude-opus-5-5",
                "claude-opus-5-5@high",
                "hard",
                0.24,
            ),
            at(
                "cursor",
                "claude-opus-5-5-xhigh",
                "claude-opus-5-5@xhigh",
                "",
                0.2,
            ),
            at("pi", "local", "", "", -0.7),
        ]);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        // One row per model, in the catalog's order; an entry with no key alone.
        assert_eq!(
            names,
            ["gpt-6-astra", "muse-spark-1.3", "claude-opus-5-5", "local"]
        );

        // Cursor's effort and OpenCode's provider/model are the same model.
        let spark = &rows[1];
        assert_eq!(offered_line(spark), "2 agents here offer it.");
        let efforts: Vec<&str> = spark.efforts.iter().map(|e| e.effort.as_str()).collect();
        assert_eq!(efforts, ["", "medium"]);
        // Its Score is its best effort's.
        assert_eq!(spark.effort, "medium");
        assert_eq!(best_effort_line(spark), "At medium effort, its best of 2");
        assert!(matches_model_filters(
            spark,
            &clause("agent", &["opencode"]),
            0.8,
            true
        ));

        let opus = &rows[2];
        let efforts: Vec<&str> = opus.efforts.iter().map(|e| e.effort.as_str()).collect();
        assert_eq!(efforts, ["medium", "high", "xhigh"]);
        assert_eq!(opus.effort, "high");
        assert!((opus.entry.theta - 0.24).abs() < 1e-9);
        // Those in a tier lead, and an offer names its effort when its model does not.
        let offers: Vec<String> = opus.offers.iter().map(offer_name).collect();
        assert_eq!(
            offers,
            [
                "claude-opus-5-5 \u{00b7} medium",
                "claude-opus-5-5 \u{00b7} high",
                "claude-opus-5-5-xhigh",
            ]
        );
        assert_eq!(row_tiers(opus, true), ["normal", "hard"]);

        let astra = &rows[0];
        assert_eq!(astra.offers.len(), 2);
        assert_eq!(best_effort_line(astra), "At its default effort");
        assert_eq!(row_tiers(&rows[3], true), ["Found"]);
    }

    #[test]
    fn the_dialog_says_how_theta_was_read() {
        let mut catalog = empty_catalog();
        catalog.generated = "2026-10-31".into();
        let source = |name: &str, fields: &str| CatalogSource {
            name: name.into(),
            url: String::new(),
            license: String::new(),
            fields: fields.into(),
        };
        catalog.sources = vec![
            source("models.dev", "prices and limits"),
            source("Epoch AI, CursorBench", "scores"),
            source("Epoch AI, Epoch Capabilities Index", "eci"),
        ];
        let mut bench = entry(
            "claude",
            "claude-opus-5-5",
            "",
            "cursorbench high",
            Some(0.56),
            None,
            "",
            vec![BandPass {
                tier: "hard".into(),
                d: -0.8,
                p_pass: 0.0,
                low: 0.0,
            }],
        );
        bench.key = "claude-opus-5-5@high".into();
        bench.theta = (0.56_f64 / 0.44).ln();
        // The lower bound sits 0.3 under θ for CursorBench.
        bench.bands[0].low = 1.0 / (1.0 + (-(bench.theta + 0.8 - 0.3)).exp());
        let detail = score_detail(&row(bench), &catalog);
        assert_eq!(detail.title, "Score 56");
        assert_eq!(detail.source, "CursorBench 56% high");
        assert_eq!(detail.bound.as_deref(), Some("\u{03b8} \u{2212} 0.30"));
        assert!(
            detail.how.contains("so the Score is that rate"),
            "{}",
            detail.how
        );
        assert_eq!(
            detail.note,
            "Catalog 2026-10-31 \u{00b7} Epoch AI: CursorBench, Epoch Capabilities Index"
        );

        let mut eci = entry(
            "codex",
            "gpt-6-sol",
            "",
            "eci",
            Some(163.8),
            None,
            "",
            vec![],
        );
        eci.key = "gpt-6-sol@high".into();
        let detail = score_detail(&row(eci), &catalog);
        assert!(
            detail.how.contains("Epoch Capabilities Index"),
            "{}",
            detail.how
        );
        assert!(
            detail
                .how
                .contains("Each effort level from medium adds 0.15")
        );
        assert!(detail.bound.is_none());
    }

    #[test]
    fn the_dialog_says_whether_auto_routes_to_it() {
        let tiered = row(keyed("cursor", "m", "m@", "normal"));
        let found = row(keyed("cursor", "m", "m@", ""));
        assert_eq!(
            routing_note(&found, true),
            "Auto can route to it through any of these."
        );
        assert!(routing_note(&tiered, false).contains("tier table alone"));
        assert_eq!(
            routing_note(&found, false),
            "Auto routes to it only with discover on."
        );
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
        let rows = visible_rows(
            &group_models(&[low, high]),
            "",
            &Filters::default(),
            0.8,
            true,
            Some((SkillColumn::Score, SortDirection::Descending)),
        );
        assert_eq!(rows[0].name, "high");
        assert_eq!(rows[1].name, "low");
    }

    #[test]
    fn orders_by_model_name_and_finds_a_model_by_its_efforts() {
        let rows = group_models(&[
            keyed("claude", "claude-sonnet-5-5", "claude-sonnet-5-5@low", ""),
            keyed("claude", "claude-opus-5-5", "claude-opus-5-5@xhigh", ""),
            keyed("claude", "claude-opus-5-5", "claude-opus-5-5@medium", ""),
        ]);
        let sorted = visible_rows(
            &rows,
            "",
            &Filters::default(),
            0.8,
            true,
            Some((SkillColumn::Model, SortDirection::Ascending)),
        );
        let names: Vec<&str> = sorted.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["claude-opus-5-5", "claude-sonnet-5-5"]);
        let found = visible_rows(&rows, "xhigh", &Filters::default(), 0.8, true, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "claude-opus-5-5");
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
