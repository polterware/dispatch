//! Spend figures and the Spend page's row rules.
//!
//! Ports `packages/features/dispatch/src/spend.ts` and the usage-tab calculations in
//! `index.tsx`. The page shows only what Dispatch's runs used: the agents that ran in the
//! Spend window, dearest first, under the window's totals. Account limits live with the tiers.

use std::cmp::Ordering;
use std::collections::HashSet;

use super::filter::{FilterOption, FilterSpec, Filters};
use crate::client::{DispatchAgent, DispatchStatsRow};

pub const API_EQUIVALENT: &str =
    "API-equivalent cost: what these tokens cost at API prices, not what a subscription bills.";

pub const SINCE_OPTIONS: &[(&str, &str)] = &[
    ("24h", "Last 24 hours"),
    ("7d", "Last 7 days"),
    ("30d", "Last 30 days"),
    ("all", "All time"),
];

pub const DEFAULT_SINCE: &str = "7d";

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
pub struct SpendRow {
    pub agent: String,
    /// The agent's provider, empty when unknown.
    pub provider: String,
    /// Dispatch runs in the Spend window.
    pub runs: i64,
    pub tokens: Option<Cell>,
    pub cost: Option<Cell>,
    /// The window's tokens, when a run reported usage.
    pub counts: Option<TokenCounts>,
    /// The window's API-equivalent cost, when a run could be priced.
    pub cost_usd: Option<f64>,
    /// The runs whose cost is in `cost_usd`.
    pub cost_runs: i64,
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

/// `agentFilters`: Provider over the rows' providers, then the Spend window (one at a time,
/// Last 7 days without one).
pub fn filter_specs(rows: &[SpendRow]) -> [FilterSpec; 2] {
    [
        FilterSpec::values(
            "provider",
            "Provider",
            rows.iter().map(|row| row.provider.as_str()),
        ),
        FilterSpec::multi(
            "since",
            "Spend window",
            SINCE_OPTIONS
                .iter()
                .map(|(id, label)| FilterOption::new(*id, *label))
                .collect(),
        )
        .single()
        .plural("windows"),
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

/// The search over agent and provider, and the Provider filter; the Spend window picks the
/// figures instead.
pub fn row_matches(row: &SpendRow, query: &str, filters: &Filters) -> bool {
    matches_query(query, &[&row.agent, &row.provider])
        && filters.matches(|field| match field {
            "provider" => Some(vec![row.provider.as_str()]),
            _ => None,
        })
}

/// The agents Dispatch ran in the window, dearest first: by cost (unpriced after priced), then
/// tokens, then name.
pub fn spent_rows(rows: &[SpendRow]) -> Vec<SpendRow> {
    let mut spent: Vec<SpendRow> = rows.iter().filter(|row| row.runs > 0).cloned().collect();
    let tokens = |row: &SpendRow| row.counts.as_ref().map(total_tokens).unwrap_or(-1);
    spent.sort_by(|left, right| {
        let cost = |row: &SpendRow| row.cost_usd.unwrap_or(-1.0);
        cost(right)
            .partial_cmp(&cost(left))
            .unwrap_or(Ordering::Equal)
            .then_with(|| tokens(right).cmp(&tokens(left)))
            .then_with(|| left.agent.cmp(&right.agent))
    });
    spent
}

/// The window's totals over the rows on show.
#[derive(Clone, Debug, PartialEq)]
pub struct SpendTotals {
    pub runs: i64,
    pub tokens: i64,
    /// The API-equivalent cost of the priced runs, when any was priced.
    pub cost_usd: Option<f64>,
    pub cost_runs: i64,
}

impl SpendTotals {
    /// The cost of a priced run on average.
    pub fn per_run(&self) -> Option<f64> {
        let cost = self.cost_usd?;
        (self.cost_runs > 0).then(|| cost / self.cost_runs as f64)
    }
}

pub fn spend_totals(rows: &[SpendRow]) -> SpendTotals {
    let mut totals = SpendTotals {
        runs: 0,
        tokens: 0,
        cost_usd: None,
        cost_runs: 0,
    };
    for row in rows {
        totals.runs += row.runs;
        totals.tokens += row.counts.as_ref().map(total_tokens).unwrap_or(0);
        if let Some(cost) = row.cost_usd {
            totals.cost_usd = Some(totals.cost_usd.unwrap_or(0.0) + cost);
            totals.cost_runs += row.cost_runs;
        }
    }
    totals
}

/// One row per installed agent, in name order, with its spend across every workspace.
pub fn agent_rows(agents: &[DispatchAgent], stats: &[DispatchStatsRow]) -> Vec<SpendRow> {
    // Before the agents' list first answers, every agent that ran has a card, its provider
    // still unknown.
    if agents.is_empty() {
        let mut rows: Vec<SpendRow> = stats
            .iter()
            .map(|row| {
                let agent = DispatchAgent {
                    name: row.key.clone(),
                    ..DispatchAgent::default()
                };
                build_row(&agent, stats)
            })
            .collect();
        rows.sort_by(|left, right| left.agent.cmp(&right.agent));
        return rows;
    }
    let mut seen = HashSet::new();
    let mut rows: Vec<SpendRow> = agents
        .iter()
        .filter(|agent| agent.installed && seen.insert(agent.name.clone()))
        .map(|agent| build_row(agent, stats))
        .collect();
    rows.sort_by(|left, right| left.agent.cmp(&right.agent));
    rows
}

fn build_row(agent: &DispatchAgent, stats: &[DispatchStatsRow]) -> SpendRow {
    let stat = stats.iter().find(|row| row.key == agent.name);
    let runs = stat.map(|row| i64::from(row.runs)).unwrap_or(0);
    let (tokens, cost, counts, cost_usd, cost_runs) = match stat {
        Some(row) => {
            let group = group_from_row(row);
            let cells = spend_cells(&group, runs);
            let counts = (row.usage_runs > 0).then(|| group.tokens.clone());
            let cost_usd = (row.cost_runs > 0).then_some(row.cost_usd);
            (
                cells.tokens,
                cells.cost,
                counts,
                cost_usd,
                i64::from(row.cost_runs),
            )
        }
        None => (None, None, None, None, 0),
    };
    SpendRow {
        agent: agent.name.clone(),
        provider: known_text(&agent.provider).unwrap_or_default(),
        runs,
        tokens,
        cost,
        counts,
        cost_usd,
        cost_runs,
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

fn matches_query(query: &str, values: &[&str]) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || values
            .iter()
            .any(|value| value.to_lowercase().contains(&needle))
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
    fn keeps_the_agents_that_ran_dearest_first_and_totals_them() {
        let agent = |name: &str| DispatchAgent {
            name: name.into(),
            installed: true,
            ..DispatchAgent::default()
        };
        let stat = |key: &str, runs: i32, tokens: i64, cost: Option<f64>| DispatchStatsRow {
            key: key.into(),
            runs,
            usage_runs: runs,
            output_tokens: ProtoI64(tokens),
            cost_usd: cost.unwrap_or(0.0),
            cost_runs: if cost.is_some() { runs } else { 0 },
            ..DispatchStatsRow::default()
        };
        let rows = agent_rows(
            &[agent("a"), agent("b"), agent("c"), agent("idle")],
            &[
                stat("a", 2, 100, Some(1.0)),
                stat("b", 1, 900, None),
                stat("c", 3, 50, Some(4.0)),
            ],
        );
        let spent = spent_rows(&rows);
        assert_eq!(
            spent
                .iter()
                .map(|row| row.agent.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );
        let totals = spend_totals(&spent);
        assert_eq!(totals.runs, 6);
        assert_eq!(totals.tokens, 1050);
        assert_eq!(totals.cost_usd, Some(5.0));
        assert_eq!(totals.cost_runs, 5);
        assert_eq!(totals.per_run(), Some(1.0));
        assert_eq!(spend_totals(&[]).cost_usd, None);
        assert_eq!(spend_totals(&[]).per_run(), None);
    }

    #[test]
    fn lists_installed_agents_with_their_spend_and_filters_by_provider() {
        let agents = vec![
            DispatchAgent {
                name: "codex".into(),
                provider: "openai".into(),
                installed: true,
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
                provider: "unknown".into(),
                installed: true,
                ..DispatchAgent::default()
            },
        ];
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
        let rows = agent_rows(&agents, &stats);
        assert_eq!(
            rows.iter()
                .map(|row| row.agent.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "grok"]
        );
        let codex = &rows[0];
        assert_eq!(codex.provider, "openai");
        assert_eq!(codex.runs, 4);
        assert_eq!(codex.tokens.as_ref().unwrap().label, "1.5k");
        assert_eq!(codex.cost.as_ref().unwrap().label, "$2.50");
        assert_eq!(codex.cost_usd, Some(2.5));
        assert_eq!(codex.counts.as_ref().map(total_tokens), Some(1500));
        assert_eq!(rows[1].provider, "");
        assert_eq!(rows[1].runs, 0);
        assert!(rows[1].tokens.is_none());

        let specs = filter_specs(&rows);
        let mut filters = Filters::default();
        assert!(row_matches(&rows[0], "Open", &filters));
        assert!(!row_matches(&rows[0], "xai", &filters));
        filters.add(&specs[0], &["openai".into()]);
        assert!(row_matches(&rows[0], "", &filters));
        assert!(!row_matches(&rows[1], "", &filters));

        assert_eq!(since_value(&filters), "7d");
        filters.add(&specs[1], &["24h".into()]);
        filters.add(&specs[1], &["30d".into()]);
        assert_eq!(since_value(&filters), "30d");
        assert!(row_matches(&rows[0], "", &filters));
    }
}
