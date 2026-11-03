//! Spend figures and the Spend page's row rules.
//!
//! Ports `packages/features/dispatch/src/spend.ts` and the usage-tab calculations
//! in `index.tsx` (`linearPace`, quota, pace, window, task price, `agentFilters`).

use std::cmp::Ordering;
use std::collections::HashSet;

use super::filter::{FilterOption, FilterSpec, Filters};
pub use super::sort::SortDirection;
use super::sort::{SortValue, sort_rows as sort_by_value};
use crate::client::{DispatchAgent, DispatchCapacityKey, DispatchQuotaSnapshot, DispatchStatsRow};

pub const API_EQUIVALENT: &str =
    "API-equivalent cost: what these tokens cost at API prices, not what a subscription bills.";

pub const SINCE_OPTIONS: &[(&str, &str)] = &[
    ("24h", "Last 24 hours"),
    ("7d", "Last 7 days"),
    ("30d", "Last 30 days"),
    ("all", "All time"),
];

pub const DEFAULT_SINCE: &str = "7d";

pub const QUOTA_OPTIONS: &[&str] = &["Available", "Low", "Cooldown", "Unknown"];

pub const PACE_OPTIONS: &[&str] = &["ahead", "on pace", "behind", "unknown"];

pub const RUN_OPTIONS: &[(&str, &str)] = &[
    ("Ran", "Ran in the window"),
    ("No runs", "No runs in the window"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Agent,
    Task,
    Tokens,
    Cost,
}

/// The sort each column's header offers, in column order: Usage, Pace, and Window do not sort.
pub const COLUMN_SORT: [Option<SortKey>; 7] = [
    Some(SortKey::Agent),
    None,
    None,
    None,
    Some(SortKey::Task),
    Some(SortKey::Tokens),
    Some(SortKey::Cost),
];

#[derive(Clone, Debug, PartialEq)]
pub struct TokenCounts {
    pub input: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub output: i64,
    pub reasoning: i64,
}

impl Default for TokenCounts {
    fn default() -> Self {
        Self {
            input: 0,
            cache_read: 0,
            cache_write: 0,
            output: 0,
            reasoning: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub tokens: TokenCounts,
    pub cost_usd: Option<f64>,
    pub cost_source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpendGroup {
    pub usage_runs: i64,
    pub tokens: TokenCounts,
    pub cost_usd: f64,
    pub cost_runs: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub label: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpendCells {
    pub tokens: Option<Cell>,
    pub cost: Option<Cell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextUsage {
    pub label: String,
    pub title: String,
    pub percent: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatalogModel {
    pub agent: String,
    pub model: String,
    pub cost_per_task: Option<f64>,
    pub cost_source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskPrice {
    pub label: String,
    pub title: String,
    pub min: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpendRow {
    pub agent: String,
    /// Filter key: the agent's provider, else the first snapshot's.
    pub provider: String,
    /// Trailing label: the latest snapshot's provider, else the agent's.
    pub display_provider: String,
    pub quota: String,
    pub pace_label: String,
    pub ran: String,
    pub usage_label: String,
    pub usage_aria: String,
    pub source_tooltip: Option<String>,
    pub cooldown_tooltip: Option<String>,
    pub window_label: String,
    pub window_tooltip: Option<String>,
    pub task: Option<Cell>,
    pub tokens: Option<Cell>,
    pub cost: Option<Cell>,
    pub sort_task: f64,
    pub sort_tokens: f64,
    pub sort_cost: f64,
}

/// Every token sent or received, cache included. Reasoning is part of output.
pub fn total_tokens(tokens: &TokenCounts) -> i64 {
    tokens
        .input
        .saturating_add(tokens.cache_read)
        .saturating_add(tokens.cache_write)
        .saturating_add(tokens.output)
}

/// A token count as 950, 12k, or 6.5M.
pub fn format_tokens(n: i64) -> String {
    let n = n as f64;
    if n >= 1_000_000.0 {
        format!("{}M", fixed(n / 1_000_000.0, 1))
    } else if n >= 10_000.0 {
        format!("{}k", (n / 1000.0).round() as i64)
    } else if n >= 1000.0 {
        format!("{}k", fixed(n / 1000.0, 1))
    } else {
        format!("{}", n.round() as i64)
    }
}

/// Dollars, with sub-cent amounts kept readable.
pub fn format_cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "<$0.01".to_string()
    } else {
        format_dollars(usd)
    }
}

pub fn format_dollars(usd: f64) -> String {
    format!("${}", fixed(usd, 2))
}

/// The token breakdown, for a tooltip.
pub fn token_breakdown(tokens: &TokenCounts) -> String {
    let mut parts = vec![format!("Input {}", format_tokens(tokens.input))];
    if tokens.cache_read > 0 {
        parts.push(format!("Cache read {}", format_tokens(tokens.cache_read)));
    }
    if tokens.cache_write > 0 {
        parts.push(format!("Cache write {}", format_tokens(tokens.cache_write)));
    }
    let reasoning = if tokens.reasoning > 0 {
        format!(" ({} reasoning)", format_tokens(tokens.reasoning))
    } else {
        String::new()
    };
    parts.push(format!(
        "Output {}{reasoning}",
        format_tokens(tokens.output)
    ));
    parts.join(" · ")
}

/// How full the context was at the last model call.
pub fn context_usage(tokens: i64, window: i64) -> Option<ContextUsage> {
    if tokens <= 0 {
        return None;
    }
    if window <= 0 {
        return Some(ContextUsage {
            label: format_tokens(tokens),
            title: "Tokens in context at the last model call.".to_string(),
            percent: None,
        });
    }
    let percent = (((tokens as f64) / (window as f64)) * 100.0)
        .round()
        .min(100.0)
        .max(0.0) as i64;
    Some(ContextUsage {
        label: format!(
            "{} / {} ({percent}%)",
            format_tokens(tokens),
            format_tokens(window)
        ),
        title: format!(
            "{} of {} tokens in context at the last model call.",
            group_int(tokens),
            group_int(window)
        ),
        percent: Some(percent),
    })
}

/// Where a run's cost comes from, in plain words.
pub fn cost_source_label(source: &str) -> &'static str {
    match source {
        "agent" => "Reported by the agent CLI",
        "models.dev price" => "Estimated from models.dev list prices",
        "agent and models.dev price" => {
            "Reported by the agent CLI and estimated from models.dev list prices"
        }
        _ => "",
    }
}

/// Short price source used on Cost / task.
pub fn task_source_label(source: &str) -> &str {
    if source == "cursorbench" {
        "CursorBench"
    } else if source == "models.dev price" {
        "models.dev"
    } else {
        source
    }
}

/// A run's cost and its tooltip. Unknown when an attempt could not be priced.
pub fn run_cost(usage: &Usage) -> Cell {
    match usage.cost_usd {
        None => Cell {
            label: "Unknown".to_string(),
            title: "No price is known for this model.".to_string(),
        },
        Some(usd) => {
            let source = cost_source_label(&usage.cost_source);
            let title = [API_EQUIVALENT, source]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            Cell {
                label: format_cost(usd),
                title,
            }
        }
    }
}

/// A group's spend: tokens and the cost of its priced runs, with tooltips.
pub fn spend_cells(group: &SpendGroup, runs: i64) -> SpendCells {
    if group.usage_runs == 0 {
        return SpendCells {
            tokens: None,
            cost: None,
        };
    }
    let tokens = Cell {
        label: format_tokens(total_tokens(&group.tokens)),
        title: format!(
            "{}. {} of {runs} runs reported usage.",
            token_breakdown(&group.tokens),
            group.usage_runs
        ),
    };
    if group.cost_runs == 0 {
        return SpendCells {
            tokens: Some(tokens),
            cost: None,
        };
    }
    let priced = if group.cost_runs < runs {
        format!(
            " Sums the {} of {runs} runs that could be priced.",
            group.cost_runs
        )
    } else {
        String::new()
    };
    SpendCells {
        tokens: Some(tokens),
        cost: Some(Cell {
            label: format_cost(group.cost_usd),
            title: format!("{API_EQUIVALENT}{priced}"),
        }),
    }
}

pub fn known_text(value: &str) -> Option<String> {
    let text = value.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(text.to_string())
    }
}

pub fn format_used_percent(used: Option<f64>) -> String {
    match used {
        Some(value) if value.is_finite() => format!("{value:.1}%"),
        _ => "unknown".to_string(),
    }
}

/// RFC 3339 / ISO-8601 instant as unix milliseconds. A date with no zone is UTC.
pub fn parse_time_ms(value: &str) -> Option<i64> {
    let text = value.trim();
    if text.is_empty() {
        return None;
    }
    let (date, time_and_zone) = if let Some((date, rest)) = split_date_time(text) {
        (date, Some(rest))
    } else {
        (text, None)
    };
    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut hour = 0_i64;
    let mut minute = 0_i64;
    let mut second = 0_i64;
    let mut millis = 0_i64;
    let mut offset_ms = 0_i64;
    if let Some(rest) = time_and_zone {
        let (time, offset) = split_zone(rest)?;
        if time.is_empty() {
            return None;
        }
        let mut bits = time.split(':');
        hour = bits.next()?.parse().ok()?;
        minute = bits.next().unwrap_or("0").parse().ok()?;
        let seconds = bits.next().unwrap_or("0");
        if bits.next().is_some() || !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
            return None;
        }
        let (sec, frac) = seconds.split_once('.').unwrap_or((seconds, ""));
        second = sec.parse().ok()?;
        if second > 60 {
            return None;
        }
        if !frac.is_empty() {
            let mut digits: String = frac
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .take(3)
                .collect();
            if digits.is_empty() {
                return None;
            }
            while digits.len() < 3 {
                digits.push('0');
            }
            millis = digits.parse().ok()?;
        }
        offset_ms = offset;
    }
    let days = days_from_civil(year, month, day);
    let utc = days * 86_400_000 + hour * 3_600_000 + minute * 60_000 + second * 1000 + millis;
    Some(utc - offset_ms)
}

pub fn format_duration_ms(ms: i64) -> String {
    let sec = (ms as f64 / 1000.0).round().max(0.0) as i64;
    if sec < 60 {
        return format!("{sec}s");
    }
    let min = sec / 60;
    if min < 60 {
        return format!("{min}m");
    }
    let hours = min / 60;
    let rem = min % 60;
    if rem == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {rem}m")
    }
}

/// Relative clock text. Empty stays unknown; an unparseable value stays as stored.
pub fn relative_when(value: &str, now_ms: i64) -> String {
    let text = value.trim();
    if text.is_empty() {
        return "unknown".to_string();
    }
    let Some(time) = parse_time_ms(text) else {
        return text.to_string();
    };
    let delta = time - now_ms;
    let abs = delta.abs();
    if abs < 1000 {
        return "now".to_string();
    }
    let label = format_duration_ms(abs);
    if delta > 0 {
        format!("in {label}")
    } else {
        format!("{label} ago")
    }
}

/// Local calendar text for a cooldown tooltip. Empty is an em dash.
pub fn format_when(value: &str) -> String {
    let text = value.trim();
    if text.is_empty() {
        return "—".to_string();
    }
    let Some(ms) = parse_time_ms(text) else {
        return text.to_string();
    };
    local_date_time(ms).unwrap_or_else(|| text.to_string())
}

/// Linear pace from a sampled point. Missing window or timestamps stay unknown.
pub fn linear_pace(
    used: f64,
    window_minutes: i32,
    resets_at: &str,
    sampled_at: &str,
) -> Option<f64> {
    let reset = parse_time_ms(resets_at)?;
    let at = parse_time_ms(sampled_at)?;
    if !used.is_finite() || window_minutes <= 0 {
        return None;
    }
    let window_ms = f64::from(window_minutes) * 60_000.0;
    let remaining = (reset - at).max(0) as f64;
    let elapsed = (window_ms - remaining).max(0.0);
    Some(used - (elapsed / window_ms) * 100.0)
}

pub fn pool_matches_agent(key: &str, agent: &str) -> bool {
    if key == agent {
        return true;
    }
    key.split([':', '|']).next().unwrap_or("") == agent
}

pub fn pace_option_label(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

pub fn parse_catalog_entries(json: &str) -> Vec<CatalogModel> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(entries) = value.get("entries").and_then(|item| item.as_array()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let model = entry
                .get("model")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            if model.is_empty() {
                return None;
            }
            Some(CatalogModel {
                agent: entry
                    .get("agent")
                    .and_then(|item| item.as_str())
                    .unwrap_or("")
                    .to_string(),
                model: model.to_string(),
                cost_per_task: entry.get("costPerTask").and_then(|item| item.as_f64()),
                cost_source: entry
                    .get("costSource")
                    .and_then(|item| item.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

/// Cheapest to dearest catalog price for one agent's models.
pub fn agent_task_price(entries: &[CatalogModel], agent: &str) -> Option<TaskPrice> {
    let mut priced: Vec<&CatalogModel> = entries
        .iter()
        .filter(|entry| entry.agent == agent && entry.cost_per_task.is_some())
        .collect();
    if priced.is_empty() {
        return None;
    }
    priced.sort_by(|left, right| {
        left.cost_per_task
            .partial_cmp(&right.cost_per_task)
            .unwrap_or(Ordering::Equal)
    });
    let min = priced[0].cost_per_task.unwrap_or(0.0);
    let max = priced[priced.len() - 1].cost_per_task.unwrap_or(min);
    let title = priced
        .iter()
        .map(|entry| {
            let cost = entry.cost_per_task.unwrap_or(0.0);
            let source = task_source_label(&entry.cost_source);
            if source.is_empty() {
                format!("{}: {}", entry.model, format_dollars(cost))
            } else {
                format!("{}: {} ({source})", entry.model, format_dollars(cost))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let label = if min == max {
        format_dollars(min)
    } else {
        format!("{}–{}", format_dollars(min), format_dollars(max))
    };
    Some(TaskPrice { label, title, min })
}

/// `agentFilters`: Provider over the rows' providers, then Quota, Pace, the Spend window (one
/// at a time, Last 7 days without one), and Dispatch runs.
pub fn filter_specs(rows: &[SpendRow]) -> [FilterSpec; 5] {
    let options = |values: &[(&str, &str)]| {
        values
            .iter()
            .map(|(id, label)| FilterOption::new(*id, *label))
            .collect()
    };
    [
        FilterSpec::values(
            "provider",
            "Provider",
            rows.iter().map(|row| row.provider.as_str()),
        ),
        FilterSpec::multi(
            "quota",
            "Quota",
            QUOTA_OPTIONS
                .iter()
                .map(|value| FilterOption::new(*value, *value))
                .collect(),
        ),
        FilterSpec::multi(
            "pace",
            "Pace",
            PACE_OPTIONS
                .iter()
                .map(|value| FilterOption::new(*value, pace_option_label(value)))
                .collect(),
        ),
        FilterSpec::multi("since", "Spend window", options(SINCE_OPTIONS))
            .single()
            .plural("windows"),
        FilterSpec::multi("runs", "Dispatch runs", options(RUN_OPTIONS)).plural("run states"),
    ]
}

/// The Spend window the page reads its spend over: the field's pick, else Last 7 days.
pub fn since_value(filters: &Filters) -> &str {
    filters
        .values_of("since")
        .first()
        .map(String::as_str)
        .unwrap_or(DEFAULT_SINCE)
}

/// The search over agent and provider, and every filter but the Spend window, which picks
/// the figures instead.
pub fn row_matches(row: &SpendRow, query: &str, filters: &Filters) -> bool {
    matches_query(query, &[&row.agent, &row.provider])
        && filters.matches(|field| match field {
            "provider" => Some(vec![row.provider.as_str()]),
            "quota" => Some(vec![row.quota.as_str()]),
            "pace" => Some(vec![row.pace_label.as_str()]),
            "runs" => Some(vec![row.ran.as_str()]),
            _ => None,
        })
}

/// The rows as the header sorts them: the agent's name, the cheapest task (the dearest number
/// when unpriced), and the window's tokens and cost (-1 with no run that reported them).
pub fn sort_rows(rows: &[SpendRow], key: SortKey, direction: SortDirection) -> Vec<SpendRow> {
    sort_by_value(rows, direction, |row| match key {
        SortKey::Agent => SortValue::Text(row.agent.clone()),
        SortKey::Task => SortValue::Number(row.sort_task),
        SortKey::Tokens => SortValue::Number(row.sort_tokens),
        SortKey::Cost => SortValue::Number(row.sort_cost),
    })
}

/// One row per installed agent, in name order, across every workspace's spend.
pub fn agent_rows(
    agents: &[DispatchAgent],
    snapshots: &[DispatchQuotaSnapshot],
    pools: &[DispatchCapacityKey],
    stats: &[DispatchStatsRow],
    catalog: &[CatalogModel],
    now_ms: i64,
) -> Vec<SpendRow> {
    let installed: HashSet<&str> = agents
        .iter()
        .filter(|agent| agent.installed)
        .map(|agent| agent.name.as_str())
        .collect();
    let mut names: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for agent in agents {
        if seen.insert(agent.name.clone()) {
            names.push(agent.name.clone());
        }
    }
    for snapshot in snapshots {
        if seen.insert(snapshot.agent.clone()) {
            names.push(snapshot.agent.clone());
        }
    }
    names.retain(|name| installed.contains(name.as_str()));
    names.sort();
    names
        .into_iter()
        .map(|name| build_row(&name, agents, snapshots, pools, stats, catalog, now_ms))
        .collect()
}

fn build_row(
    name: &str,
    agents: &[DispatchAgent],
    snapshots: &[DispatchQuotaSnapshot],
    pools: &[DispatchCapacityKey],
    stats: &[DispatchStatsRow],
    catalog: &[CatalogModel],
    now_ms: i64,
) -> SpendRow {
    let agent = agents.iter().find(|agent| agent.name == name);
    let latest = latest_snapshot(snapshots, name);
    let provider = agent
        .and_then(|agent| known_text(&agent.provider))
        .or_else(|| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.agent == name)
                .and_then(|snapshot| known_text(&snapshot.provider))
        })
        .unwrap_or_default();
    let display_provider = latest
        .and_then(|snapshot| known_text(&snapshot.provider))
        .or_else(|| agent.and_then(|agent| known_text(&agent.provider)))
        .unwrap_or_default();
    let used = latest
        .and_then(|snapshot| snapshot.used_percent)
        .or_else(|| agent.and_then(|agent| agent.used_percent))
        .filter(|value| value.is_finite());
    let matched_pools: Vec<&DispatchCapacityKey> = pools
        .iter()
        .filter(|pool| pool_matches_agent(&pool.key, name))
        .collect();
    let pace = matched_pools
        .iter()
        .find_map(|pool| reported_pace(&pool.pace))
        .or_else(|| pace_from_snapshot(latest));
    let pace_label = pace.unwrap_or_else(|| "unknown".to_string());
    let cooldown_until = agent
        .map(|agent| agent.cooldown_until.as_str())
        .unwrap_or("");
    let cooling = parse_time_ms(cooldown_until).is_some_and(|time| time > now_ms);
    let cooldown_tooltip = if cooling {
        Some(format!("Cooldown until {}", format_when(cooldown_until)))
    } else {
        None
    };
    let quota = if cooling {
        "Cooldown"
    } else if used.is_none() {
        "Unknown"
    } else if used.unwrap_or(0.0) >= 90.0 {
        "Low"
    } else {
        "Available"
    };
    let window_name = latest.and_then(|snapshot| quota_window(snapshot));
    let reset = latest.and_then(|snapshot| {
        parse_time_ms(&snapshot.resets_at)
            .map(|_| format!("resets {}", relative_when(&snapshot.resets_at, now_ms)))
    });
    let window_label = dotted(&[
        window_name.as_deref().unwrap_or(""),
        reset.as_deref().unwrap_or(""),
    ]);
    let window_tooltip = window_facts(&matched_pools);
    let usage_label = match used {
        Some(value) => format_used_percent(Some(value)),
        None => "--".to_string(),
    };
    let source_tooltip = latest
        .and_then(|snapshot| known_text(&snapshot.source))
        .map(|source| format!("Source: {source}"));
    let stat = stats.iter().find(|row| row.key == name);
    let runs = stat.map(|row| i64::from(row.runs)).unwrap_or(0);
    let ran = if runs > 0 { "Ran" } else { "No runs" };
    let (tokens, cost, sort_tokens, sort_cost) = match stat {
        Some(row) => {
            let group = group_from_row(row);
            let cells = spend_cells(&group, runs);
            let sort_tokens = if row.usage_runs > 0 {
                total_tokens(&group.tokens) as f64
            } else {
                -1.0
            };
            let sort_cost = if row.cost_runs > 0 {
                row.cost_usd
            } else {
                -1.0
            };
            (cells.tokens, cells.cost, sort_tokens, sort_cost)
        }
        None => (None, None, -1.0, -1.0),
    };
    let task_price = agent_task_price(catalog, name);
    let sort_task = task_price
        .as_ref()
        .map(|price| price.min)
        .unwrap_or(f64::MAX);
    let task = task_price.map(|price| Cell {
        label: price.label,
        title: price.title,
    });
    SpendRow {
        agent: name.to_string(),
        provider,
        display_provider,
        quota: quota.to_string(),
        pace_label,
        ran: ran.to_string(),
        usage_label,
        usage_aria: format!("{name} quota usage: {}", format_used_percent(used)),
        source_tooltip,
        cooldown_tooltip,
        window_label,
        window_tooltip,
        task,
        tokens,
        cost,
        sort_task,
        sort_tokens,
        sort_cost,
    }
}

fn group_from_row(row: &DispatchStatsRow) -> SpendGroup {
    SpendGroup {
        usage_runs: i64::from(row.usage_runs),
        tokens: TokenCounts {
            input: row.input_tokens.0,
            cache_read: row.cache_read_tokens.0,
            cache_write: row.cache_write_tokens.0,
            output: row.output_tokens.0,
            reasoning: row.reasoning_tokens.0,
        },
        cost_usd: row.cost_usd,
        cost_runs: i64::from(row.cost_runs),
    }
}

fn latest_snapshot<'a>(
    snapshots: &'a [DispatchQuotaSnapshot],
    name: &str,
) -> Option<&'a DispatchQuotaSnapshot> {
    let mut matched: Vec<&DispatchQuotaSnapshot> = snapshots
        .iter()
        .filter(|snapshot| snapshot.agent == name)
        .collect();
    matched.sort_by(|left, right| right.sampled_at.cmp(&left.sampled_at));
    matched.into_iter().next()
}

fn reported_pace(pace: &str) -> Option<String> {
    let trimmed = pace.trim();
    let label = if trimmed.is_empty() {
        "unknown"
    } else {
        trimmed
    };
    known_text(label)
}

fn pace_from_snapshot(snapshot: Option<&DispatchQuotaSnapshot>) -> Option<String> {
    let snapshot = snapshot?;
    let used = snapshot.used_percent?;
    let delta = linear_pace(
        used,
        snapshot.window_minutes,
        &snapshot.resets_at,
        &snapshot.sampled_at,
    )?;
    Some(
        if delta > 2.0 {
            "ahead"
        } else if delta < -2.0 {
            "behind"
        } else {
            "on pace"
        }
        .to_string(),
    )
}

fn quota_window(snapshot: &DispatchQuotaSnapshot) -> Option<String> {
    if let Some(name) = known_text(&snapshot.window) {
        return Some(name);
    }
    if snapshot.window_minutes > 0 {
        Some(format!("{} min", snapshot.window_minutes))
    } else {
        None
    }
}

fn window_facts(pools: &[&DispatchCapacityKey]) -> Option<String> {
    if pools.is_empty() {
        return None;
    }
    let many = pools.len() > 1;
    let mut facts = Vec::new();
    for pool in pools {
        let prefix = if many {
            format!("{} ", pool.key)
        } else {
            String::new()
        };
        facts.push(format!("{prefix}Windows: {}", pool.windows));
        facts.push(format!("{prefix}Hit 100%: {}", pool.hit_full));
        let leftover = if pool.leftover_pct.is_some_and(|value| value.is_finite()) {
            Some(format_used_percent(pool.leftover_pct))
        } else {
            let trimmed = pool.leftover.trim();
            let label = if trimmed.is_empty() {
                "unknown"
            } else {
                trimmed
            };
            known_text(label)
        };
        if let Some(leftover) = leftover {
            facts.push(format!("{prefix}Leftover: {leftover}"));
        }
    }
    if facts.is_empty() {
        None
    } else {
        Some(facts.join(" / "))
    }
}

fn matches_query(query: &str, values: &[&str]) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || values
            .iter()
            .any(|value| value.to_lowercase().contains(&needle))
}

fn dotted(parts: &[&str]) -> String {
    let visible: Vec<&str> = parts
        .iter()
        .copied()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if visible.is_empty() {
        "—".to_string()
    } else {
        visible.join(" / ")
    }
}

fn fixed(value: f64, digits: u32) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let scale = 10_f64.powi(digits as i32);
    let rounded = (value * scale).round();
    let negative = rounded.is_sign_negative() && rounded != 0.0;
    let abs = rounded.abs() as u128;
    let factor = 10_u128.pow(digits);
    let whole = abs / factor;
    let frac = abs % factor;
    let sign = if negative { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{frac:0width$}", width = digits as usize)
    }
}

fn group_int(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let grouped: String = out.chars().rev().collect();
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}

fn split_date_time(value: &str) -> Option<(&str, &str)> {
    value
        .split_once('T')
        .or_else(|| value.split_once('t'))
        .or_else(|| value.split_once(' '))
}

fn split_zone(rest: &str) -> Option<(&str, i64)> {
    if let Some(time) = rest.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    if let Some(index) = rest.rfind(['+', '-'])
        && index > 0
    {
        let time = &rest[..index];
        let zone = &rest[index..];
        let sign = if zone.starts_with('+') { 1 } else { -1 };
        let zone = &zone[1..];
        let (hour, minute) = if let Some((hour, minute)) = zone.split_once(':') {
            (hour.parse::<i64>().ok()?, minute.parse::<i64>().ok()?)
        } else if zone.len() == 4 {
            (zone[..2].parse().ok()?, zone[2..].parse().ok()?)
        } else if zone.len() == 2 {
            (zone.parse().ok()?, 0)
        } else {
            return None;
        };
        if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
            return None;
        }
        return Some((time, sign * (hour * 3_600_000 + minute * 60_000)));
    }
    Some((rest, 0))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut year = i64::from(year);
    let month = i64::from(month);
    let day = i64::from(day);
    if month <= 2 {
        year -= 1;
    }
    let era = year.div_euclid(400);
    let year_of_era = (year - era * 400) as u64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year as u64;
    era * 146_097 + day_of_era as i64 - 719_468
}

fn local_date_time(ms: i64) -> Option<String> {
    let secs = ms.div_euclid(1000);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let instant = secs as libc::time_t;
    let parsed = unsafe { libc::localtime_r(&instant, &mut tm) };
    if parsed.is_null() {
        return None;
    }
    let hour24 = tm.tm_hour;
    let ampm = if hour24 < 12 { "AM" } else { "PM" };
    let hour12 = match hour24 % 12 {
        0 => 12,
        hour => hour,
    };
    Some(format!(
        "{}/{}/{}, {}:{:02}:{:02} {}",
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_year + 1900,
        hour12,
        tm.tm_min,
        tm.tm_sec,
        ampm
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ProtoI64;

    fn tokens(
        input: i64,
        cache_read: i64,
        cache_write: i64,
        output: i64,
        reasoning: i64,
    ) -> TokenCounts {
        TokenCounts {
            input,
            cache_read,
            cache_write,
            output,
            reasoning,
        }
    }

    #[test]
    fn formats_tokens_and_cost_the_way_the_tables_show_them() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(1500), "1.5k");
        assert_eq!(format_tokens(44608), "45k");
        assert_eq!(format_tokens(6_486_752), "6.5M");
        assert_eq!(format_cost(0.004), "<$0.01");
        assert_eq!(format_cost(3.912308), "$3.91");
        let counts = tokens(100, 900, 0, 40, 30);
        assert_eq!(total_tokens(&counts), 1040);
        assert_eq!(
            token_breakdown(&counts),
            "Input 100 · Cache read 900 · Output 40 (30 reasoning)"
        );
    }

    #[test]
    fn prices_a_run_from_its_reported_cost() {
        let usage = Usage {
            tokens: tokens(15418, 29056, 0, 134, 64),
            cost_usd: Some(0.092336),
            cost_source: "agent".into(),
        };
        assert_eq!(run_cost(&usage).label, "$0.09");
        assert!(run_cost(&usage).title.contains("Reported by the agent CLI"));
        let unknown = Usage {
            cost_usd: None,
            ..usage
        };
        assert_eq!(unknown.cost_usd, None);
        assert_eq!(run_cost(&unknown).label, "Unknown");
        assert_eq!(
            run_cost(&unknown).title,
            "No price is known for this model."
        );
    }

    #[test]
    fn shows_context_out_of_its_window_or_the_count_alone() {
        let full = context_usage(46200, 272000).unwrap();
        assert_eq!(full.label, "46k / 272k (17%)");
        assert_eq!(full.percent, Some(17));
        assert_eq!(
            full.title,
            "46,200 of 272,000 tokens in context at the last model call."
        );
        let open = context_usage(46200, 0).unwrap();
        assert_eq!(open.label, "46k");
        assert_eq!(open.percent, None);
        assert!(context_usage(0, 272000).is_none());
        assert_eq!(context_usage(300000, 272000).unwrap().percent, Some(100));
    }

    #[test]
    fn shows_tokens_and_a_cost_only_for_priced_runs() {
        let group = SpendGroup {
            usage_runs: 3,
            tokens: tokens(1000, 0, 0, 500, 0),
            cost_usd: 2.5,
            cost_runs: 2,
        };
        let cells = spend_cells(&group, 4);
        let tokens_cell = cells.tokens.unwrap();
        assert_eq!(tokens_cell.label, "1.5k");
        assert!(tokens_cell.title.contains("3 of 4 runs reported usage"));
        let cost = cells.cost.unwrap();
        assert_eq!(cost.label, "$2.50");
        assert!(cost.title.contains("2 of 4 runs"));
        assert!(cost.title.starts_with(API_EQUIVALENT));
        assert!(
            spend_cells(
                &SpendGroup {
                    usage_runs: 0,
                    ..group.clone()
                },
                4
            )
            .tokens
            .is_none()
        );
        assert!(
            spend_cells(
                &SpendGroup {
                    cost_runs: 0,
                    ..group
                },
                4
            )
            .cost
            .is_none()
        );
    }

    #[test]
    fn parses_rfc3339_and_speaks_relative_time() {
        assert_eq!(parse_time_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_time_ms("2000-01-01T00:00:00Z"), Some(946_684_800_000));
        assert_eq!(relative_when("", 0), "unknown");
        assert_eq!(relative_when("not-a-date", 0), "not-a-date");
        assert_eq!(relative_when("1970-01-01T00:00:00.400Z", 0), "now");
        assert_eq!(relative_when("1970-01-01T00:01:30Z", 0), "in 1m");
        assert_eq!(relative_when("1970-01-01T00:02:00Z", 0), "in 2m");
        assert_eq!(relative_when("1970-01-01T00:00:00Z", 90_000), "1m ago");
        assert_eq!(relative_when("1970-01-01T00:00:00Z", 120_000), "2m ago");
        assert_eq!(format_when(""), "—");
    }

    #[test]
    fn classifies_pace_from_the_sample_or_the_pool() {
        assert_eq!(
            linear_pace(60.0, 60, "1970-01-01T00:30:00Z", "1970-01-01T00:00:00Z"),
            Some(10.0)
        );
        assert_eq!(
            linear_pace(40.0, 60, "1970-01-01T00:30:00Z", "1970-01-01T00:00:00Z"),
            Some(-10.0)
        );
        assert!(linear_pace(10.0, 0, "1970-01-01T00:30:00Z", "1970-01-01T00:00:00Z").is_none());
        assert!(pool_matches_agent("codex:work", "codex"));
        assert!(pool_matches_agent("codex|weekly", "codex"));
        assert!(!pool_matches_agent("codexextra", "codex"));
    }

    #[test]
    fn prices_an_agents_task_from_its_catalog_models() {
        let entries = vec![
            CatalogModel {
                agent: "codex".into(),
                model: "big".into(),
                cost_per_task: Some(0.4),
                cost_source: "models.dev price".into(),
            },
            CatalogModel {
                agent: "codex".into(),
                model: "small".into(),
                cost_per_task: Some(0.05),
                cost_source: "cursorbench".into(),
            },
            CatalogModel {
                agent: "codex".into(),
                model: "free".into(),
                cost_per_task: None,
                cost_source: "models.dev price".into(),
            },
            CatalogModel {
                agent: "grok".into(),
                model: "m".into(),
                cost_per_task: Some(1.0),
                cost_source: "models.dev price".into(),
            },
        ];
        let codex = agent_task_price(&entries, "codex").unwrap();
        assert_eq!(codex.label, "$0.05–$0.40");
        assert_eq!(
            codex.title,
            "small: $0.05 (CursorBench)\nbig: $0.40 (models.dev)"
        );
        assert_eq!(codex.min, 0.05);
        assert_eq!(agent_task_price(&entries, "grok").unwrap().label, "$1.00");
        assert!(agent_task_price(&entries, "cursor").is_none());
        let parsed = parse_catalog_entries(
            r#"{"entries":[{"agent":"codex","model":"small","costPerTask":0.05,"costSource":"cursorbench"},{"model":""}]}"#,
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].model, "small");
    }

    #[test]
    fn lists_installed_agents_with_quota_pace_window_and_spend() {
        let agents = vec![
            DispatchAgent {
                name: "codex".into(),
                provider: "openai".into(),
                installed: true,
                cooldown_until: "1970-01-01T00:00:30Z".into(),
                ..DispatchAgent::default()
            },
            DispatchAgent {
                name: "claude".into(),
                provider: "anthropic".into(),
                installed: false,
                ..DispatchAgent::default()
            },
            DispatchAgent {
                name: "grok".into(),
                provider: "xai".into(),
                used_percent: Some(92.0),
                installed: true,
                ..DispatchAgent::default()
            },
        ];
        let snapshots = vec![DispatchQuotaSnapshot {
            agent: "codex".into(),
            provider: "openai".into(),
            window: "5h".into(),
            source: "cli".into(),
            used_percent: Some(40.0),
            window_minutes: 60,
            resets_at: "1970-01-01T00:30:00Z".into(),
            sampled_at: "1970-01-01T00:00:00Z".into(),
            ..DispatchQuotaSnapshot::default()
        }];
        let pools = vec![DispatchCapacityKey {
            key: "codex:main".into(),
            windows: 4,
            hit_full: 1,
            leftover_pct: Some(12.5),
            pace: "  ".into(),
            ..DispatchCapacityKey::default()
        }];
        let stats = vec![DispatchStatsRow {
            key: "codex".into(),
            runs: 4,
            usage_runs: 3,
            input_tokens: ProtoI64(1000),
            output_tokens: ProtoI64(500),
            cost_usd: 2.5,
            cost_runs: 2,
            ..DispatchStatsRow::default()
        }];
        let rows = agent_rows(&agents, &snapshots, &pools, &stats, &[], 0);
        assert_eq!(
            rows.iter()
                .map(|row| row.agent.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "grok"]
        );
        let codex = &rows[0];
        assert_eq!(codex.provider, "openai");
        assert_eq!(codex.usage_label, "40.0%");
        assert_eq!(codex.usage_aria, "codex quota usage: 40.0%");
        assert_eq!(codex.source_tooltip.as_deref(), Some("Source: cli"));
        assert_eq!(codex.pace_label, "behind");
        assert_eq!(codex.quota, "Cooldown");
        assert!(codex.cooldown_tooltip.is_some());
        assert_eq!(codex.window_label, "5h / resets in 30m");
        assert_eq!(
            codex.window_tooltip.as_deref(),
            Some("Windows: 4 / Hit 100%: 1 / Leftover: 12.5%")
        );
        assert_eq!(codex.tokens.as_ref().unwrap().label, "1.5k");
        assert_eq!(codex.cost.as_ref().unwrap().label, "$2.50");
        assert_eq!(codex.ran, "Ran");
        assert_eq!(rows[1].quota, "Low");
        assert_eq!(rows[1].usage_label, "92.0%");
        assert_eq!(rows[1].pace_label, "unknown");
        assert_eq!(rows[1].ran, "No runs");
        assert!(rows[1].tokens.is_none());
        assert_eq!(rows[1].window_label, "—");
    }

    #[test]
    fn filters_by_provider_quota_pace_runs_and_search() {
        let agents = vec![
            DispatchAgent {
                name: "codex".into(),
                provider: "openai".into(),
                used_percent: Some(10.0),
                installed: true,
                ..DispatchAgent::default()
            },
            DispatchAgent {
                name: "grok".into(),
                provider: "xai".into(),
                used_percent: Some(95.0),
                installed: true,
                ..DispatchAgent::default()
            },
        ];
        let rows = agent_rows(&agents, &[], &[], &[], &[], 0);
        let specs = filter_specs(&rows);
        assert_eq!(
            specs[0]
                .options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>(),
            vec!["openai", "xai"]
        );
        assert_eq!(specs[2].options[1].label, "On pace");
        let mut filters = Filters::default();
        assert_eq!(since_value(&filters), "7d");
        assert!(row_matches(&rows[0], "Open", &filters));
        assert!(!row_matches(&rows[0], "xai", &filters));
        filters.add(&specs[1], &["Low".into()]);
        assert!(!row_matches(&rows[0], "", &filters));
        assert!(row_matches(&rows[1], "", &filters));
        filters.clear();
        filters.add(&specs[0], &["openai".into()]);
        filters.add(&specs[2], &["unknown".into()]);
        filters.add(&specs[4], &["No runs".into()]);
        assert!(row_matches(&rows[0], "", &filters));
        assert!(!row_matches(&rows[1], "", &filters));
        filters.add(&specs[3], &["24h".into()]);
        filters.add(&specs[3], &["30d".into()]);
        assert_eq!(since_value(&filters), "30d");
        assert_eq!(filters.clauses.len(), 4);
        assert!(row_matches(&rows[0], "", &filters));
    }

    #[test]
    fn sorts_missing_spend_after_real_numbers_when_ascending() {
        let rows = agent_rows(
            &[
                DispatchAgent {
                    name: "b".into(),
                    installed: true,
                    ..DispatchAgent::default()
                },
                DispatchAgent {
                    name: "a".into(),
                    installed: true,
                    ..DispatchAgent::default()
                },
            ],
            &[],
            &[],
            &[DispatchStatsRow {
                key: "a".into(),
                runs: 1,
                usage_runs: 1,
                output_tokens: ProtoI64(10),
                cost_usd: 1.0,
                cost_runs: 1,
                ..DispatchStatsRow::default()
            }],
            &[],
            0,
        );
        let rows = sort_rows(&rows, SortKey::Agent, SortDirection::Ascending);
        assert_eq!(rows[0].agent, "a");
        let rows = sort_rows(&rows, SortKey::Tokens, SortDirection::Ascending);
        assert_eq!(rows[0].agent, "b");
        assert_eq!(rows[0].sort_tokens, -1.0);
        let rows = sort_rows(&rows, SortKey::Cost, SortDirection::Descending);
        assert_eq!(rows[0].agent, "a");
    }

    #[test]
    fn splits_the_filter_provider_from_the_latest_snapshot_label() {
        let rows = agent_rows(
            &[DispatchAgent {
                name: "codex".into(),
                provider: "openai".into(),
                installed: true,
                ..DispatchAgent::default()
            }],
            &[
                DispatchQuotaSnapshot {
                    agent: "codex".into(),
                    provider: "old".into(),
                    sampled_at: "1970-01-01T00:00:00Z".into(),
                    ..DispatchQuotaSnapshot::default()
                },
                DispatchQuotaSnapshot {
                    agent: "codex".into(),
                    provider: "azure".into(),
                    sampled_at: "1970-01-02T00:00:00Z".into(),
                    ..DispatchQuotaSnapshot::default()
                },
            ],
            &[],
            &[],
            &[],
            0,
        );
        assert_eq!(rows[0].provider, "openai");
        assert_eq!(rows[0].display_provider, "azure");
    }
}
