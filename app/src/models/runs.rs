//! Pure logic for the Runs page (the sidebar's View all): every workspace's runs as
//! `DispatchRunRow` draws them, each with the repository it came from (named as the sidebar
//! names its repository groups), their filter fields, search, and sort. Nothing here performs
//! IO beyond the sidebar's cached look-up of a worktree's main checkout.

use std::collections::HashMap;

use crate::client::{DispatchRun, Workspace};

use super::filter::{FilterOption, FilterSpec, Filters};
use super::sidebar::main_checkout;
use super::sort::{SortDirection, SortValue, sort_rows};

pub const EM_DASH: &str = "\u{2014}";

/// The Runs table's columns: Run, Repository, Status, Agent, Changes, and Started.
pub const COLUMNS: [&str; 6] = ["Run", "Repository", "Status", "Agent", "Changes", "Started"];
/// Changes is the one column that does not sort.
pub const SORTABLE: [bool; 6] = [true, true, true, true, false, true];

/// One run in the Runs table.
#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    pub id: String,
    /// The prompt on one line, or the short run id.
    pub title: String,
    pub prompt: String,
    pub status: String,
    pub agent: String,
    pub model: String,
    pub tier: String,
    pub branch: String,
    pub card_slug: String,
    /// Where the run came from: its repository's main checkout, or, when the run names no
    /// folder, its workspace id. The Repository filter matches on it.
    pub source_key: String,
    /// The repository's folder name, else the workspace's name, else its id.
    pub source: String,
    /// The repository's full path (and the workspace, when it is named apart), for a tooltip.
    pub source_title: String,
    /// `agent / model`, and with the provider as its tooltip.
    pub agent_label: String,
    pub agent_title: String,
    pub files_changed: Option<i32>,
    pub insertions: Option<i32>,
    pub deletions: Option<i32>,
    /// `createdAt || startedAt`, which Started shows and sorts by.
    pub started: String,
    pub active: bool,
}

impl RunRow {
    /// A run whose workspace is unknown.
    pub fn new(run: &DispatchRun) -> Self {
        Self::of(run, "", "")
    }

    /// `run`, from the workspace named `workspace_name` at `workspace_path`. The repository is
    /// the first of the run's repo, its folder, and the workspace's path, at its main checkout,
    /// as `SidebarRun::from_dispatch` finds it.
    pub fn of(run: &DispatchRun, workspace_name: &str, workspace_path: &str) -> Self {
        let repo = [run.repo.as_str(), run.dir.as_str(), workspace_path]
            .into_iter()
            .map(|path| path.trim().trim_end_matches('/'))
            .find(|path| !path.is_empty())
            .map(main_checkout)
            .unwrap_or_default();
        let folder = repo.rsplit('/').next().unwrap_or_default();
        let workspace_name = workspace_name.trim();
        let source = [folder, workspace_name, run.project_id.as_str()]
            .into_iter()
            .find(|name| !name.is_empty())
            .unwrap_or(EM_DASH)
            .to_string();
        let source_title = match (repo.is_empty(), workspace_name.is_empty()) {
            (false, false) if workspace_name != folder => format!("{repo} · {workspace_name}"),
            (false, _) => repo.clone(),
            (true, _) => source.clone(),
        };
        let source_key = if repo.is_empty() {
            run.project_id.clone()
        } else {
            repo
        };
        let title = run.prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        Self {
            id: run.id.clone(),
            title: if title.is_empty() {
                short_run_id(&run.id).to_string()
            } else {
                title
            },
            prompt: run.prompt.clone(),
            status: run.status.clone(),
            agent: run.agent.clone(),
            model: run.model.clone(),
            tier: run.tier.clone(),
            branch: run.branch.clone(),
            card_slug: run.card_slug.clone(),
            source_key,
            source,
            source_title,
            agent_label: dotted(&[&run.agent, &run.model]),
            agent_title: dotted(&[&run.agent, &run.model, &run.provider]),
            files_changed: run.files_changed,
            insertions: run.insertions,
            deletions: run.deletions,
            started: if run.created_at.is_empty() {
                run.started_at.clone()
            } else {
                run.created_at.clone()
            },
            active: is_run_active(&run.status),
        }
    }

    pub fn has_changes(&self) -> bool {
        self.files_changed.is_some() || self.insertions.is_some() || self.deletions.is_some()
    }

    /// `FileChangeLine`'s parts: `N file(s)`, `+ins`, and `−del`, each only when known.
    pub fn change_parts(&self) -> (Option<String>, Option<String>, Option<String>) {
        (
            self.files_changed.map(|count| {
                if count == 1 {
                    "1 file".to_string()
                } else {
                    format!("{count} files")
                }
            }),
            self.insertions.map(|count| format!("+{count}")),
            self.deletions.map(|count| format!("\u{2212}{count}")),
        )
    }

    fn field(&self, field: &str) -> Option<&str> {
        match field {
            "status" => Some(&self.status),
            "agent" => Some(&self.agent),
            "tier" => Some(&self.tier),
            "repository" => Some(&self.source_key),
            _ => None,
        }
    }
}

/// The first 12 characters of a run id.
pub fn short_run_id(id: &str) -> &str {
    id.char_indices().nth(12).map_or(id, |(end, _)| &id[..end])
}

/// Every workspace's runs, each joined to its workspace's name and path (empty when the
/// workspace is gone), whichever workspace the app has selected.
pub fn all_rows(workspaces: &[Workspace], runs: &[DispatchRun]) -> Vec<RunRow> {
    let places: HashMap<&str, (&str, &str)> = workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.id.as_str(),
                (workspace.name.as_str(), workspace.path.as_str()),
            )
        })
        .collect();
    runs.iter()
        .map(|run| {
            let (name, path) = places
                .get(run.project_id.as_str())
                .copied()
                .unwrap_or(("", ""));
            RunRow::of(run, name, path)
        })
        .collect()
}

/// The Repository field: each repository the rows came from, labeled by its folder's name,
/// in the order they first appear. Empty until a filter is added, which keeps every run.
pub fn repository_spec(rows: &[RunRow]) -> FilterSpec {
    let mut seen = std::collections::HashSet::new();
    let options = rows
        .iter()
        .filter(|row| !row.source_key.is_empty() && seen.insert(row.source_key.as_str()))
        .map(|row| FilterOption::new(row.source_key.as_str(), row.source.as_str()))
        .collect();
    FilterSpec::multi("repository", "Repository", options).plural("repositories")
}

/// `valueFilterField` for Repository, Status, Agent, and Tier: each field's distinct values.
pub fn filter_specs(rows: &[RunRow]) -> [FilterSpec; 4] {
    [
        repository_spec(rows),
        FilterSpec::values(
            "status",
            "Status",
            rows.iter().map(|row| row.status.as_str()),
        ),
        FilterSpec::values("agent", "Agent", rows.iter().map(|row| row.agent.as_str())),
        FilterSpec::values("tier", "Tier", rows.iter().map(|row| row.tier.as_str())),
    ]
}

/// The rows the clauses and the search keep, in the column's order when one is sorted.
pub fn visible_rows(
    rows: &[RunRow],
    filters: &Filters,
    query: &str,
    sort: Option<(usize, SortDirection)>,
) -> Vec<RunRow> {
    let kept: Vec<RunRow> = rows
        .iter()
        .filter(|row| {
            filters.matches(|field| row.field(field).map(|value| vec![value]))
                && matches_query(
                    query,
                    &[
                        &row.prompt,
                        &row.agent,
                        &row.model,
                        &row.tier,
                        &row.branch,
                        &row.card_slug,
                        &row.source,
                        &row.source_key,
                        &row.id,
                        &row.status,
                    ],
                )
        })
        .cloned()
        .collect();
    match sort {
        Some((column, direction)) if SORTABLE.get(column).copied().unwrap_or(false) => {
            sort_rows(&kept, direction, |row| sort_value(row, column))
        }
        _ => kept,
    }
}

fn sort_value(row: &RunRow, column: usize) -> SortValue {
    let text = match column {
        0 => &row.prompt,
        1 => &row.source,
        2 => &row.status,
        3 => &row.agent_label,
        5 => &row.started,
        _ => return SortValue::Missing,
    };
    if text.is_empty() {
        SortValue::Missing
    } else {
        SortValue::Text(text.clone())
    }
}

pub fn format_duration(ms: i64) -> String {
    let sec = if ms <= 0 {
        0
    } else {
        ((ms as f64) / 1000.0).round() as i64
    };
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

pub fn format_ago(value: &str, now_ms: i64) -> String {
    match parse_time(value) {
        Some(time) => format!("{} ago", format_duration(now_ms - time)),
        None => EM_DASH.to_string(),
    }
}

/// `pending`, `running`, and `queued` only. `starting` is not a live clock.
pub fn is_run_active(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "pending" | "running" | "queued"
    )
}

pub fn dotted(parts: &[&str]) -> String {
    let visible: Vec<&str> = parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();
    if visible.is_empty() {
        EM_DASH.to_string()
    } else {
        visible.join(" / ")
    }
}

pub fn matches_query(query: &str, values: &[&str]) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || values
            .iter()
            .any(|value| value.to_lowercase().contains(&needle))
}

/// RFC3339 / ISO-8601 with a timezone. The fraction is truncated to milliseconds.
pub fn parse_time(value: &str) -> Option<i64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year = parse_i32(&bytes[0..4])?;
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    if bytes[10] != b'T' && bytes[10] != b't' && bytes[10] != b' ' {
        return None;
    }
    let month = parse_u32(&bytes[5..7])?;
    let day = parse_u32(&bytes[8..10])?;
    let hour = parse_u32(&bytes[11..13])?;
    let minute = parse_u32(&bytes[14..16])?;
    let second = parse_u32(&bytes[17..19])?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut index = 19;
    let mut millis: i64 = 0;
    if index < bytes.len() && (bytes[index] == b'.' || bytes[index] == b',') {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == start {
            return None;
        }
        let digits = &value[start..index];
        let mut padded = digits.to_string();
        while padded.len() < 3 {
            padded.push('0');
        }
        millis = padded[..3].parse().ok()?;
    }
    if index >= bytes.len() {
        return None;
    }
    let offset_minutes = parse_offset(&value[index..])?;
    let days = days_from_civil(year, month, day);
    let local_ms = days * 86_400_000
        + hour as i64 * 3_600_000
        + minute as i64 * 60_000
        + second as i64 * 1000
        + millis;
    Some(local_ms - offset_minutes * 60_000)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = (year - era * 400) as u32;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era as i64 * 146_097 + day_of_era as i64 - 719_468
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn parse_offset(rest: &str) -> Option<i64> {
    if rest == "Z" || rest == "z" {
        return Some(0);
    }
    let bytes = rest.as_bytes();
    if bytes.len() != 5 && bytes.len() != 6 {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hour, minute) = if bytes.len() == 6 {
        if bytes[3] != b':' {
            return None;
        }
        (parse_u32(&bytes[1..3])?, parse_u32(&bytes[4..6])?)
    } else {
        (parse_u32(&bytes[1..3])?, parse_u32(&bytes[3..5])?)
    };
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * (hour as i64 * 60 + minute as i64))
}

fn parse_u32(bytes: &[u8]) -> Option<u32> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_i32(bytes: &[u8]) -> Option<i32> {
    parse_u32(bytes).map(|value| value as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::filter::FILTER_IS_NOT;

    fn run(id: &str, prompt: &str, status: &str, agent: &str, created: &str) -> DispatchRun {
        DispatchRun {
            id: id.into(),
            project_id: "ws".into(),
            prompt: prompt.into(),
            status: status.into(),
            agent: agent.into(),
            model: "m".into(),
            created_at: created.into(),
            ..DispatchRun::default()
        }
    }

    #[test]
    fn durations_match_the_dispatch_clock() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(-40), "0s");
        assert_eq!(format_duration(59_400), "59s");
        assert_eq!(format_duration(59_600), "1m");
        assert_eq!(format_duration(90_000), "1m");
        assert_eq!(format_duration(3_600_000), "1h");
        assert_eq!(format_duration(5_400_000), "1h 30m");
        assert_eq!(format_ago("", 10_000), EM_DASH);
        assert_eq!(format_ago("1970-01-01T00:00:00Z", 10_000), "10s ago");
        assert_eq!(format_ago("1970-01-01T00:00:20Z", 10_000), "0s ago");
    }

    #[test]
    fn parse_time_requires_a_real_civil_date_and_a_timezone() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2000-01-01T00:00:00Z"), Some(946_684_800_000));
        assert_eq!(
            parse_time("2000-01-01T10:00:00-03:00"),
            parse_time("2000-01-01T13:00:00Z")
        );
        assert_eq!(parse_time("1970-01-01T00:00:00.250Z"), Some(250));
        assert_eq!(parse_time("1970-01-01T00:00:00.25Z"), Some(250));
        assert_eq!(parse_time("1970-01-01T00:00:60Z"), Some(60_000));
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("nope"), None);
        assert_eq!(parse_time("1970-13-01T00:00:00Z"), None);
        assert_eq!(parse_time("1970-01-01T00:00:00"), None);
        assert_eq!(parse_time("1970-02-29T00:00:00Z"), None);
        assert_eq!(parse_time("1900-02-29T00:00:00Z"), None);
        assert!(parse_time("2000-02-29T00:00:00Z").is_some());
    }

    fn workspace(id: &str, name: &str, path: &str) -> Workspace {
        Workspace {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            ..Workspace::default()
        }
    }

    #[test]
    fn rows_cover_every_workspace_and_title_the_prompt_on_one_line() {
        let mut other = run("b", "x", "completed", "codex", "");
        other.project_id = "other".into();
        let mut gone = run("d", "y", "completed", "codex", "");
        gone.project_id = "gone".into();
        let rows = all_rows(
            &[
                workspace("ws", "App", "/nowhere/app"),
                workspace("other", "Library", "/nowhere/lib"),
            ],
            &[
                run("abcdefghijklmnop", "", "running", "claude", ""),
                other,
                run("c", "Line one\n  line two", "completed", "codex", ""),
                gone,
            ],
        );
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].title, "abcdefghijkl");
        assert!(rows[0].active);
        assert_eq!(rows[2].title, "Line one line two");
        assert_eq!(rows[2].agent_label, "codex / m");
        // Each row says where it came from: the repository's folder, else the workspace.
        assert_eq!(rows[0].source, "app");
        assert_eq!(rows[0].source_key, "/nowhere/app");
        assert_eq!(rows[0].source_title, "/nowhere/app · App");
        assert_eq!(rows[1].source, "lib");
        assert_eq!(rows[3].source, "gone");
        assert_eq!(rows[3].source_key, "gone");
    }

    #[test]
    fn a_run_names_its_own_folder_over_its_workspace() {
        let mut own = run("a", "p", "completed", "codex", "");
        own.dir = "/nowhere/tools/".into();
        let row = RunRow::of(&own, "App", "/nowhere/app");
        assert_eq!(row.source, "tools");
        assert_eq!(row.source_key, "/nowhere/tools");
    }

    #[test]
    fn the_repository_filter_lists_each_repository_once_and_keeps_its_runs() {
        let mut lib = run("b", "x", "completed", "codex", "");
        lib.project_id = "other".into();
        let rows = all_rows(
            &[
                workspace("ws", "App", "/nowhere/app"),
                workspace("other", "Library", "/nowhere/lib"),
            ],
            &[
                run("a", "p", "completed", "codex", ""),
                lib,
                run("c", "q", "failed", "claude", ""),
            ],
        );
        let [repository, ..] = filter_specs(&rows);
        let options: Vec<(&str, &str)> = repository
            .options
            .iter()
            .map(|option| (option.id.as_str(), option.label.as_str()))
            .collect();
        assert_eq!(options, [("/nowhere/app", "app"), ("/nowhere/lib", "lib")]);
        let ids = |rows: Vec<RunRow>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
        let mut filters = Filters::default();
        assert_eq!(
            ids(visible_rows(&rows, &filters, "", None)),
            ["a", "b", "c"]
        );
        let clause = filters.add(&repository, &["/nowhere/lib".into()]);
        assert_eq!(ids(visible_rows(&rows, &filters, "", None)), ["b"]);
        filters.set_operator(&clause, FILTER_IS_NOT);
        assert_eq!(ids(visible_rows(&rows, &filters, "", None)), ["a", "c"]);
    }

    #[test]
    fn changes_name_files_and_use_a_real_minus() {
        let mut row = RunRow::new(&run("a", "p", "completed", "codex", ""));
        assert!(!row.has_changes());
        row.files_changed = Some(1);
        row.deletions = Some(4);
        assert_eq!(
            row.change_parts(),
            (Some("1 file".into()), None, Some("\u{2212}4".into()))
        );
    }

    #[test]
    fn filters_search_and_sort_compose() {
        let rows = all_rows(
            &[],
            &[
                run(
                    "a",
                    "Fix the parser",
                    "completed",
                    "codex",
                    "2026-01-02T00:00:00Z",
                ),
                run(
                    "b",
                    "Write tests",
                    "failed",
                    "claude",
                    "2026-01-03T00:00:00Z",
                ),
                run(
                    "c",
                    "Fix the lexer",
                    "completed",
                    "claude",
                    "2026-01-01T00:00:00Z",
                ),
            ],
        );
        let specs = filter_specs(&rows);
        assert_eq!(specs[0].options.len(), 1);
        assert_eq!(specs[1].options.len(), 2);
        let mut filters = Filters::default();
        let agent = filters.add(&specs[2], &["claude".into()]);
        let ids = |rows: Vec<RunRow>| rows.into_iter().map(|row| row.id).collect::<Vec<_>>();
        assert_eq!(ids(visible_rows(&rows, &filters, "", None)), ["b", "c"]);
        assert_eq!(ids(visible_rows(&rows, &filters, "FIX", None)), ["c"]);
        filters.set_operator(&agent, FILTER_IS_NOT);
        assert_eq!(ids(visible_rows(&rows, &filters, "", None)), ["a"]);
        filters.clear();
        assert_eq!(
            ids(visible_rows(
                &rows,
                &filters,
                "",
                Some((5, SortDirection::Ascending))
            )),
            ["c", "a", "b"]
        );
        assert_eq!(
            ids(visible_rows(
                &rows,
                &filters,
                "",
                Some((4, SortDirection::Ascending))
            )),
            ["a", "b", "c"]
        );
    }
}
