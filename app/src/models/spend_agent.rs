//! The Spend detail page's figures: every run an agent spent, as table rows.
//!
//! The rows come from the runs the app already holds, with the same usage fields the Spend
//! page's totals sum. A row is a run: when it started, its conversation, repository, model,
//! and tier, its tokens (input, cache, output), its cost and where the cost came from, and its
//! status. The page's Group by folds them into sections (conversation, repository, model, or
//! day), each ending in a subtotal row; the totals over the rows on show sit above the table.

use std::collections::HashMap;
use std::ops::Range;

use super::filter::{FilterOption, FilterSpec, Filters};
use super::run_page::stored_usage;
use super::runs::{EM_DASH, RunRow, is_run_active, matches_query, parse_time};
use super::sort::{SortDirection, SortValue, sort_rows};
use super::spend::{
    API_EQUIVALENT, SpendTotals, TokenCounts, cost_source_label, format_cost, token_breakdown,
    total_tokens,
};
use crate::client::{DispatchRun, Workspace};

/// A day or a conversation the run has no date or name for.
const UNKNOWN_DAY: &str = "Unknown date";

/// What the rows are folded by. Without one, every run is a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    None,
    Conversation,
    Repository,
    Model,
    Day,
}

impl GroupBy {
    pub const ALL: [GroupBy; 5] = [
        GroupBy::None,
        GroupBy::Conversation,
        GroupBy::Repository,
        GroupBy::Model,
        GroupBy::Day,
    ];

    pub fn id(self) -> &'static str {
        match self {
            GroupBy::None => "none",
            GroupBy::Conversation => "conversation",
            GroupBy::Repository => "repository",
            GroupBy::Model => "model",
            GroupBy::Day => "day",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::None => "None",
            GroupBy::Conversation => "Conversation",
            GroupBy::Repository => "Repository",
            GroupBy::Model => "Model",
            GroupBy::Day => "Day",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|by| by.id() == id)
            .unwrap_or_default()
    }
}

/// The page's one filter: Group by, one at a time, None while nothing is picked.
pub fn group_spec() -> FilterSpec {
    FilterSpec::multi(
        "group",
        "Group by",
        GroupBy::ALL
            .iter()
            .map(|by| FilterOption::new(by.id(), by.label()))
            .collect(),
    )
    .single()
    .plural("groupings")
}

/// The grouping the filter holds.
pub fn group_value(filters: &Filters) -> GroupBy {
    filters
        .values_of("group")
        .first()
        .map(|id| GroupBy::from_id(id))
        .unwrap_or_default()
}

/// One run's spend.
#[derive(Clone, Debug, PartialEq)]
pub struct SpendEntry {
    pub run_id: String,
    pub started_ms: Option<i64>,
    /// `2026-11-15 19:33`, or an em dash when the run has no readable date.
    pub date: String,
    /// `2026-11-15`, or [`UNKNOWN_DAY`].
    pub day: String,
    /// The conversation's id, or the run's own when it names none.
    pub conversation_key: String,
    /// The conversation's name, else the run's prompt on one line.
    pub conversation: String,
    pub repository_key: String,
    pub repository: String,
    pub repository_title: String,
    pub model: String,
    pub tier: String,
    pub status: String,
    /// What the run reported; none when it ended without usage.
    pub tokens: Option<TokenCounts>,
    pub cost_usd: Option<f64>,
    pub cost_source: String,
    /// The cost leaves out an attempt no price covered: it is a floor.
    pub cost_partial: bool,
    /// Still working: its figures are what it has used so far.
    pub live: bool,
}

impl SpendEntry {
    /// Input, cache (read and write), output; all zero when nothing was reported.
    pub fn figures(&self) -> (i64, i64, i64) {
        match &self.tokens {
            Some(tokens) => (
                tokens.input,
                tokens.cache_read.saturating_add(tokens.cache_write),
                tokens.output,
            ),
            None => (0, 0, 0),
        }
    }

    pub fn has_usage(&self) -> bool {
        self.tokens.is_some()
    }
}

/// `agent`'s runs among `runs`, as rows. `conversation_name` gives a conversation's name.
pub fn entries(
    agent: &str,
    runs: &[DispatchRun],
    workspaces: &[Workspace],
    conversation_name: &dyn Fn(&str) -> Option<String>,
) -> Vec<SpendEntry> {
    runs.iter()
        .filter(|run| run.agent == agent)
        .map(|run| {
            let (name, path) = workspaces
                .iter()
                .find(|workspace| workspace.id == run.project_id)
                .map(|workspace| (workspace.name.as_str(), workspace.path.as_str()))
                .unwrap_or(("", ""));
            entry(run, &RunRow::of(run, name, path), conversation_name)
        })
        .collect()
}

fn entry(
    run: &DispatchRun,
    row: &RunRow,
    conversation_name: &dyn Fn(&str) -> Option<String>,
) -> SpendEntry {
    let usage = stored_usage(&run.usage);
    let started_ms = parse_time(&row.started);
    let (date, day) = match started_ms {
        Some(_) => {
            let text = row.started.trim();
            (
                text.get(..16).unwrap_or(text).replace('T', " "),
                text.get(..10).unwrap_or(text).to_string(),
            )
        }
        None => (EM_DASH.to_string(), UNKNOWN_DAY.to_string()),
    };
    let named = if run.conversation_id.is_empty() {
        None
    } else {
        conversation_name(&run.conversation_id).filter(|name| !name.trim().is_empty())
    };
    SpendEntry {
        run_id: run.id.clone(),
        started_ms,
        date,
        day,
        conversation_key: if run.conversation_id.is_empty() {
            run.id.clone()
        } else {
            run.conversation_id.clone()
        },
        conversation: named.unwrap_or_else(|| row.title.clone()),
        repository_key: row.source_key.clone(),
        repository: row.source.clone(),
        repository_title: row.source_title.clone(),
        model: run.model.trim().to_string(),
        tier: run.tier.trim().to_string(),
        status: run.status.clone(),
        tokens: usage.as_ref().map(|usage| TokenCounts {
            input: usage.input_tokens,
            cache_read: usage.cache_read_tokens,
            cache_write: usage.cache_write_tokens,
            output: usage.output_tokens,
            reasoning: usage.reasoning_tokens,
        }),
        cost_usd: usage.as_ref().and_then(|usage| usage.cost_usd),
        cost_source: usage
            .as_ref()
            .map(|usage| usage.cost_source.clone())
            .unwrap_or_default(),
        cost_partial: run.usage.cost_partial,
        live: is_run_active(&run.status),
    }
}

/// The table's columns, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpendColumn {
    Date,
    Conversation,
    Repository,
    Model,
    Tier,
    Input,
    Cache,
    Output,
    Cost,
    Status,
}

pub const COLUMNS: [SpendColumn; 10] = [
    SpendColumn::Date,
    SpendColumn::Conversation,
    SpendColumn::Repository,
    SpendColumn::Model,
    SpendColumn::Tier,
    SpendColumn::Input,
    SpendColumn::Cache,
    SpendColumn::Output,
    SpendColumn::Cost,
    SpendColumn::Status,
];

impl SpendColumn {
    pub fn title(self) -> &'static str {
        match self {
            SpendColumn::Date => "Date",
            SpendColumn::Conversation => "Conversation",
            SpendColumn::Repository => "Repository",
            SpendColumn::Model => "Model",
            SpendColumn::Tier => "Tier",
            SpendColumn::Input => "Input",
            SpendColumn::Cache => "Cache",
            SpendColumn::Output => "Output",
            SpendColumn::Cost => "Cost",
            SpendColumn::Status => "Status",
        }
    }

    /// A figure: right-aligned, in tabular digits.
    pub fn is_figure(self) -> bool {
        matches!(
            self,
            SpendColumn::Input | SpendColumn::Cache | SpendColumn::Output | SpendColumn::Cost
        )
    }

    /// What the column sorts a row by.
    pub fn sort_value(self, entry: &SpendEntry) -> SortValue {
        let text = |value: &str| {
            if value.is_empty() {
                SortValue::Missing
            } else {
                SortValue::Text(value.to_string())
            }
        };
        let tokens = |value: i64| match entry.tokens {
            Some(_) => SortValue::Number(value as f64),
            None => SortValue::Missing,
        };
        let (input, cache, output) = entry.figures();
        match self {
            SpendColumn::Date => entry
                .started_ms
                .map_or(SortValue::Missing, |ms| SortValue::Number(ms as f64)),
            SpendColumn::Conversation => text(&entry.conversation),
            SpendColumn::Repository => text(&entry.repository),
            SpendColumn::Model => text(&entry.model),
            SpendColumn::Tier => text(&entry.tier),
            SpendColumn::Input => tokens(input),
            SpendColumn::Cache => tokens(cache),
            SpendColumn::Output => tokens(output),
            SpendColumn::Cost => entry.cost_usd.map_or(SortValue::Missing, SortValue::Number),
            SpendColumn::Status => text(&entry.status),
        }
    }
}

/// The page's sort: a header's pick, else newest first.
pub type Sort = Option<(SpendColumn, SortDirection)>;

/// The page opens sorted by Date, newest first.
pub const DEFAULT_SORT: (SpendColumn, SortDirection) =
    (SpendColumn::Date, SortDirection::Descending);

/// The rows the search keeps, in the sort's order.
pub fn visible_entries(entries: &[SpendEntry], query: &str, sort: Sort) -> Vec<SpendEntry> {
    let kept: Vec<SpendEntry> = entries
        .iter()
        .filter(|entry| {
            matches_query(
                query,
                &[
                    &entry.conversation,
                    &entry.repository,
                    &entry.model,
                    &entry.tier,
                    &entry.status,
                    &entry.day,
                ],
            )
        })
        .cloned()
        .collect();
    let (column, direction) = sort.unwrap_or(DEFAULT_SORT);
    sort_rows(&kept, direction, |entry| column.sort_value(entry))
}

/// What a set of rows add up to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntryTotals {
    pub runs: i64,
    /// Runs that reported usage.
    pub usage_runs: i64,
    /// Ended runs that reported none: their tokens and cost are unknown, not zero.
    pub unreported_runs: i64,
    pub live_runs: i64,
    /// Runs with usage but no price.
    pub unpriced_runs: i64,
    /// Priced runs whose cost leaves out an attempt no price covered.
    pub partial_cost_runs: i64,
    pub counts: TokenCounts,
    /// The priced runs' cost, when any was priced.
    pub cost_usd: Option<f64>,
    pub cost_runs: i64,
}

impl EntryTotals {
    pub fn of<'a>(entries: impl IntoIterator<Item = &'a SpendEntry>) -> Self {
        let mut totals = Self::default();
        for entry in entries {
            totals.runs += 1;
            if entry.live {
                totals.live_runs += 1;
            }
            match &entry.tokens {
                Some(tokens) => {
                    totals.usage_runs += 1;
                    totals.counts.input = totals.counts.input.saturating_add(tokens.input);
                    totals.counts.cache_read =
                        totals.counts.cache_read.saturating_add(tokens.cache_read);
                    totals.counts.cache_write =
                        totals.counts.cache_write.saturating_add(tokens.cache_write);
                    totals.counts.output = totals.counts.output.saturating_add(tokens.output);
                    totals.counts.reasoning =
                        totals.counts.reasoning.saturating_add(tokens.reasoning);
                    if entry.cost_usd.is_none() {
                        totals.unpriced_runs += 1;
                    }
                }
                None if !entry.live => totals.unreported_runs += 1,
                None => {}
            }
            if let Some(cost) = entry.cost_usd {
                totals.cost_usd = Some(totals.cost_usd.unwrap_or(0.0) + cost);
                totals.cost_runs += 1;
                if entry.cost_partial {
                    totals.partial_cost_runs += 1;
                }
            }
        }
        totals
    }

    /// Every token sent or received, cache included.
    pub fn tokens(&self) -> i64 {
        total_tokens(&self.counts)
    }

    /// Input, cache (read and write), output.
    pub fn figures(&self) -> (i64, i64, i64) {
        (
            self.counts.input,
            self.counts
                .cache_read
                .saturating_add(self.counts.cache_write),
            self.counts.output,
        )
    }

    /// The Spend page's totals tiles' figures.
    pub fn spend_totals(&self) -> SpendTotals {
        SpendTotals {
            runs: self.runs,
            unreported_runs: self.unreported_runs,
            live_runs: self.live_runs,
            partial_cost_runs: self.partial_cost_runs,
            tokens: self.tokens(),
            cost_usd: self.cost_usd,
            cost_runs: self.cost_runs,
        }
    }

    /// What qualifies the cost beside it: `partial`, and how many runs had no price.
    pub fn cost_note(&self) -> Option<String> {
        let mut notes = Vec::new();
        if self.partial_cost_runs > 0 {
            notes.push("partial".to_string());
        }
        if self.unpriced_runs > 0 {
            notes.push(format!("{} unpriced", self.unpriced_runs));
        }
        (!notes.is_empty()).then(|| notes.join(" \u{00b7} "))
    }
}

/// A run's cost cell: the figure, what qualifies it, and its tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct CostCell {
    pub label: String,
    /// The cost's source, `partial` when it is a floor, or `unpriced`.
    pub note: Option<String>,
    pub title: String,
}

pub fn cost_cell(entry: &SpendEntry) -> CostCell {
    let Some(usd) = entry.cost_usd else {
        return if entry.has_usage() {
            CostCell {
                label: EM_DASH.to_string(),
                note: Some("unpriced".to_string()),
                title: "No price is known for this model.".to_string(),
            }
        } else {
            CostCell {
                label: EM_DASH.to_string(),
                note: None,
                title: if entry.live {
                    "The run has not reported usage yet.".to_string()
                } else {
                    "The run ended before its agent reported any usage.".to_string()
                },
            }
        };
    };
    // The source (agent, models.dev) is in the tooltip; the cell notes only a partial cost,
    // so the column stays narrow enough for the table to fit.
    let mut notes: Vec<&str> = Vec::new();
    if entry.cost_partial {
        notes.push("partial");
    }
    let mut title = vec![API_EQUIVALENT.to_string()];
    let label = cost_source_label(&entry.cost_source);
    if !label.is_empty() {
        title.push(format!("{label}."));
    }
    if entry.cost_partial {
        title.push("An attempt no price covered is left out, so the cost is a floor.".into());
    }
    if entry.live {
        title.push("The run is still working: this is what it has used so far.".into());
    }
    CostCell {
        label: format_cost(usd),
        note: (!notes.is_empty()).then(|| notes.join(" \u{00b7} ")),
        title: title.join(" "),
    }
}

/// A run's tokens as a tooltip, or none when it reported no usage.
pub fn tokens_title(entry: &SpendEntry) -> Option<String> {
    entry.tokens.as_ref().map(token_breakdown)
}

/// One line of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// The run at this index of the entries.
    Entry(usize),
    /// The totals of the section at this index.
    Subtotal(usize),
}

/// One group of rows and what they add up to.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub key: String,
    pub label: String,
    pub totals: EntryTotals,
    /// Its lines: its runs, then its subtotal.
    pub lines: Range<usize>,
}

/// The table's lines, and its sections when the rows are grouped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub lines: Vec<Line>,
    pub sections: Vec<Section>,
}

/// The group an entry falls in: its key and its label.
fn group_of(entry: &SpendEntry, by: GroupBy) -> (String, String) {
    let or = |value: &str, fallback: &str| {
        if value.trim().is_empty() {
            fallback.to_string()
        } else {
            value.to_string()
        }
    };
    match by {
        GroupBy::None => (String::new(), String::new()),
        GroupBy::Conversation => (entry.conversation_key.clone(), entry.conversation.clone()),
        GroupBy::Repository => (entry.repository_key.clone(), or(&entry.repository, EM_DASH)),
        GroupBy::Model => (entry.model.clone(), or(&entry.model, "Unknown model")),
        GroupBy::Day => (entry.day.clone(), entry.day.clone()),
    }
}

/// `entries` (already in the page's order) as lines. Without a grouping each run is a line.
/// With one, the runs fall into sections in the order each group first appears, keeping their
/// order inside it, and each section ends with its subtotal line.
pub fn layout(entries: &[SpendEntry], by: GroupBy) -> Layout {
    if by == GroupBy::None {
        return Layout {
            lines: (0..entries.len()).map(Line::Entry).collect(),
            sections: Vec::new(),
        };
    }
    let mut order: Vec<(String, String, Vec<usize>)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let (key, label) = group_of(entry, by);
        match seen.get(&key) {
            Some(slot) => order[*slot].2.push(index),
            None => {
                seen.insert(key.clone(), order.len());
                order.push((key, label, vec![index]));
            }
        }
    }
    let mut lines = Vec::with_capacity(entries.len() + order.len());
    let mut sections = Vec::with_capacity(order.len());
    for (key, label, members) in order {
        let start = lines.len();
        let totals = EntryTotals::of(members.iter().map(|index| &entries[*index]));
        lines.extend(members.into_iter().map(Line::Entry));
        lines.push(Line::Subtotal(sections.len()));
        sections.push(Section {
            key,
            label,
            totals,
            lines: start..lines.len(),
        });
    }
    Layout { lines, sections }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{DispatchUsage, ProtoI64};

    fn run(id: &str, agent: &str, created: &str) -> DispatchRun {
        DispatchRun {
            id: id.into(),
            agent: agent.into(),
            model: "gpt-5".into(),
            tier: "normal".into(),
            status: "done".into(),
            prompt: format!("prompt {id}"),
            created_at: created.into(),
            ..DispatchRun::default()
        }
    }

    fn priced(mut run: DispatchRun, input: i64, output: i64, cost: f64) -> DispatchRun {
        run.usage = DispatchUsage {
            input_tokens: ProtoI64(input),
            output_tokens: ProtoI64(output),
            cost_usd: Some(cost),
            cost_source: "models.dev price".into(),
            ..DispatchUsage::default()
        };
        run
    }

    fn entry_of(runs: &[DispatchRun]) -> Vec<SpendEntry> {
        entries("codex", runs, &[], &|_| None)
    }

    #[test]
    fn keeps_only_the_agents_runs_with_their_usage() {
        let runs = vec![
            priced(run("a", "codex", "2026-11-14T10:00:00Z"), 100, 50, 0.5),
            priced(run("b", "claude", "2026-11-14T11:00:00Z"), 9, 9, 9.0),
            run("c", "codex", "2026-11-15T09:30:00Z"),
        ];
        let rows = entry_of(&runs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].run_id, "a");
        assert_eq!(rows[0].date, "2026-11-14 10:00");
        assert_eq!(rows[0].day, "2026-11-14");
        assert_eq!(rows[0].figures(), (100, 0, 50));
        assert_eq!(rows[0].cost_usd, Some(0.5));
        assert!(rows[1].tokens.is_none());
        assert_eq!(rows[1].conversation, "prompt c");
    }

    #[test]
    fn names_a_conversation_and_groups_runs_by_its_id() {
        let mut first = priced(run("a", "codex", "2026-11-14T10:00:00Z"), 1, 1, 0.1);
        first.conversation_id = "conv-1".into();
        let rows = entries("codex", &[first], &[], &|id| {
            (id == "conv-1").then(|| "Fix the bug".to_string())
        });
        assert_eq!(rows[0].conversation, "Fix the bug");
        assert_eq!(rows[0].conversation_key, "conv-1");
    }

    #[test]
    fn sorts_newest_first_and_by_a_column() {
        let runs = vec![
            priced(run("a", "codex", "2026-11-14T10:00:00Z"), 100, 50, 0.5),
            priced(run("b", "codex", "2026-11-16T10:00:00Z"), 10, 5, 2.0),
            run("c", "codex", "2026-11-15T09:30:00Z"),
        ];
        let rows = entry_of(&runs);
        let ids = |rows: &[SpendEntry]| {
            rows.iter()
                .map(|row| row.run_id.as_str())
                .collect::<Vec<_>>()
                .join("")
        };
        assert_eq!(ids(&visible_entries(&rows, "", None)), "bca");
        let by_cost = visible_entries(
            &rows,
            "",
            Some((SpendColumn::Cost, SortDirection::Descending)),
        );
        // The run with no price goes last either way.
        assert_eq!(ids(&by_cost), "bac");
        let by_cost = visible_entries(
            &rows,
            "",
            Some((SpendColumn::Cost, SortDirection::Ascending)),
        );
        assert_eq!(ids(&by_cost), "abc");
        assert_eq!(ids(&visible_entries(&rows, "PROMPT c", None)), "c");
    }

    #[test]
    fn totals_count_unreported_unpriced_and_partial_runs() {
        let mut partial = priced(run("p", "codex", "2026-11-14T10:00:00Z"), 100, 0, 1.0);
        partial.usage.cost_partial = true;
        let mut unpriced = priced(run("u", "codex", "2026-11-14T11:00:00Z"), 40, 10, 0.0);
        unpriced.usage.cost_usd = None;
        unpriced.usage.cost_source = String::new();
        let mut live = run("l", "codex", "2026-11-14T12:00:00Z");
        live.status = "running".into();
        let rows = entry_of(&[
            partial,
            unpriced,
            run("n", "codex", "2026-11-14T13:00:00Z"),
            live,
        ]);
        let totals = EntryTotals::of(&rows);
        assert_eq!(totals.runs, 4);
        assert_eq!(totals.usage_runs, 2);
        assert_eq!(totals.unreported_runs, 1);
        assert_eq!(totals.live_runs, 1);
        assert_eq!(totals.unpriced_runs, 1);
        assert_eq!(totals.partial_cost_runs, 1);
        assert_eq!(totals.cost_usd, Some(1.0));
        assert_eq!(totals.cost_runs, 1);
        assert_eq!(totals.tokens(), 150);
        assert_eq!(
            totals.cost_note().as_deref(),
            Some("partial \u{00b7} 1 unpriced")
        );
        let tiles = totals.spend_totals();
        assert_eq!((tiles.runs, tiles.tokens, tiles.cost_runs), (4, 150, 1));
        assert_eq!(EntryTotals::of(&[]).cost_usd, None);
        assert_eq!(EntryTotals::of(&[]).cost_note(), None);
    }

    #[test]
    fn explains_each_cost() {
        let rows = entry_of(&[
            priced(run("a", "codex", "2026-11-14T10:00:00Z"), 1, 1, 0.004),
            run("b", "codex", "2026-11-14T10:00:00Z"),
        ]);
        let cell = cost_cell(&rows[0]);
        assert_eq!(cell.label, "<$0.01");
        assert_eq!(cell.note, None);
        assert!(cell.title.starts_with(API_EQUIVALENT));
        let none = cost_cell(&rows[1]);
        assert_eq!((none.label.as_str(), none.note), (EM_DASH, None));

        let mut unpriced = rows[0].clone();
        unpriced.cost_usd = None;
        assert_eq!(cost_cell(&unpriced).note.as_deref(), Some("unpriced"));
        let mut partial = rows[0].clone();
        partial.cost_partial = true;
        partial.cost_source = "agent".into();
        let cell = cost_cell(&partial);
        assert_eq!(cell.note.as_deref(), Some("partial"));
        assert!(cell.title.contains("floor"));
    }

    fn grouped_rows() -> Vec<SpendEntry> {
        let mut a = priced(run("a", "codex", "2026-11-16T10:00:00Z"), 100, 10, 1.0);
        a.conversation_id = "c1".into();
        let mut b = priced(run("b", "codex", "2026-11-16T08:00:00Z"), 200, 20, 2.0);
        b.conversation_id = "c2".into();
        b.model = "o3".into();
        let mut c = priced(run("c", "codex", "2026-11-15T08:00:00Z"), 300, 30, 4.0);
        c.conversation_id = "c1".into();
        let d = run("d", "codex", "2026-11-15T07:00:00Z");
        visible_entries(&entry_of(&[a, b, c, d]), "", None)
    }

    #[test]
    fn without_a_grouping_every_run_is_a_line() {
        let rows = grouped_rows();
        let layout = layout(&rows, GroupBy::None);
        assert_eq!(
            layout.lines,
            vec![
                Line::Entry(0),
                Line::Entry(1),
                Line::Entry(2),
                Line::Entry(3)
            ]
        );
        assert!(layout.sections.is_empty());
    }

    #[test]
    fn groups_by_conversation_with_subtotals_in_first_seen_order() {
        let rows = grouped_rows();
        // Newest first: a (c1), b (c2), c (c1), d (its own).
        let layout = layout(&rows, GroupBy::Conversation);
        assert_eq!(layout.sections.len(), 3);
        let first = &layout.sections[0];
        assert_eq!(first.key, "c1");
        assert_eq!(first.lines, 0..3);
        assert_eq!(
            layout.lines[..3],
            [Line::Entry(0), Line::Entry(2), Line::Subtotal(0)]
        );
        assert_eq!(first.totals.runs, 2);
        assert_eq!(first.totals.cost_usd, Some(5.0));
        assert_eq!(first.totals.tokens(), 440);
        assert_eq!(first.totals.figures(), (400, 0, 40));
        assert_eq!(layout.sections[1].key, "c2");
        assert_eq!(layout.sections[1].lines, 3..5);
        let last = &layout.sections[2];
        assert_eq!(last.key, "d");
        assert_eq!(last.totals.cost_usd, None);
        assert_eq!(last.totals.unreported_runs, 1);
        assert_eq!(layout.lines.len(), rows.len() + 3);
        assert_eq!(*layout.lines.last().unwrap(), Line::Subtotal(2));
    }

    #[test]
    fn groups_by_model_day_and_repository() {
        let rows = grouped_rows();
        let by_model = layout(&rows, GroupBy::Model);
        let labels: Vec<_> = by_model.sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["gpt-5", "o3"]);
        assert_eq!(by_model.sections[0].totals.runs, 3);
        assert_eq!(by_model.sections[1].totals.cost_usd, Some(2.0));

        let by_day = layout(&rows, GroupBy::Day);
        let labels: Vec<_> = by_day.sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["2026-11-16", "2026-11-15"]);
        assert_eq!(by_day.sections[0].totals.cost_usd, Some(3.0));
        assert_eq!(by_day.sections[1].totals.runs, 2);

        let by_repo = layout(&rows, GroupBy::Repository);
        assert_eq!(by_repo.sections.len(), 1);
        assert_eq!(by_repo.sections[0].totals.runs, 4);
        // Subtotals add up to the page's totals.
        let total: f64 = by_day
            .sections
            .iter()
            .filter_map(|section| section.totals.cost_usd)
            .sum();
        assert_eq!(Some(total), EntryTotals::of(&rows).cost_usd);
    }

    #[test]
    fn reads_the_grouping_from_the_filter() {
        let spec = group_spec();
        let mut filters = Filters::default();
        assert_eq!(group_value(&filters), GroupBy::None);
        filters.add(&spec, &["model".into()]);
        assert_eq!(group_value(&filters), GroupBy::Model);
        assert_eq!(GroupBy::from_id("nonsense"), GroupBy::None);
        assert_eq!(spec.options.len(), 5);
    }
}
