//! Sidebar grouping, display preferences, and the tests ported from
//! `packages/features/dispatch/src/sidebar-groups.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::client::{DispatchRun, DispatchSection, Workspace};
use crate::format::prompt_first_line;

pub const DISPLAY_PREFERENCE: &str = "pwdispatch.sidebar.display";
pub const COLLAPSED_PREFERENCE: &str = "pwdispatch.sidebar.collapsed";

/// How long a pointer must rest on a run before its transcript card opens: long enough that
/// passing down the list opens nothing, short enough that a pause still does.
pub const HOVER_OPEN_MS: u64 = 900;
/// How long the card stays up while the pointer crosses from the row into it.
pub const HOVER_CLOSE_MS: u64 = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    Conversation,
    Workspace,
    Date,
    Status,
    Agent,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    Updated,
    Created,
    Name,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Desc,
    Asc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    All,
    Unmerged,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarDisplay {
    pub group_by: GroupBy,
    pub sort_by: SortBy,
    pub direction: Direction,
    pub show: Show,
}

impl Default for SidebarDisplay {
    fn default() -> Self {
        Self {
            group_by: GroupBy::Conversation,
            sort_by: SortBy::Updated,
            direction: Direction::Desc,
            show: Show::All,
        }
    }
}

impl GroupBy {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "conversation" => Self::Conversation,
            "workspace" => Self::Workspace,
            "date" => Self::Date,
            "status" => Self::Status,
            "agent" => Self::Agent,
            "none" => Self::None,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Workspace => "workspace",
            Self::Date => "date",
            Self::Status => "status",
            Self::Agent => "agent",
            Self::None => "none",
        }
    }
}

impl SortBy {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "updated" => Self::Updated,
            "created" => Self::Created,
            "name" => Self::Name,
            _ => return None,
        })
    }
}

impl Direction {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "desc" => Self::Desc,
            "asc" => Self::Asc,
            _ => return None,
        })
    }
}

impl Show {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "all" => Self::All,
            "unmerged" => Self::Unmerged,
            "active" => Self::Active,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarRun {
    pub id: String,
    pub prompt: String,
    pub status: String,
    pub agent: String,
    pub model: String,
    pub project_id: String,
    pub workspace_name: String,
    pub branch: String,
    pub created_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub merged_at: String,
    pub merge_sha: String,
    pub worktree_status: String,
    pub conversation_id: String,
    pub caller_agent: String,
}

impl SidebarRun {
    pub fn from_dispatch(run: &DispatchRun, workspace_name: &str) -> Self {
        Self {
            id: run.id.clone(),
            prompt: run.prompt.clone(),
            status: run.status.clone(),
            agent: run.agent.clone(),
            model: run.model.clone(),
            project_id: run.project_id.clone(),
            workspace_name: workspace_name.to_string(),
            branch: run.branch.clone(),
            created_at: run.created_at.clone(),
            started_at: run.started_at.clone(),
            finished_at: run.finished_at.clone(),
            merged_at: run.merged_at.clone(),
            merge_sha: run.merge_sha.clone(),
            worktree_status: run.worktree_status.clone(),
            conversation_id: run.conversation_id.clone(),
            caller_agent: run.caller_agent.clone(),
        }
    }
}

/// Joins each run to its workspace name. A missing workspace leaves the name empty
/// so grouping can fall back to the project id.
pub fn sidebar_runs(workspaces: &[Workspace], runs: &[DispatchRun]) -> Vec<SidebarRun> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    for workspace in workspaces {
        names.insert(workspace.id.as_str(), workspace.name.as_str());
    }
    runs.iter()
        .map(|run| {
            let name = names.get(run.project_id.as_str()).copied().unwrap_or("");
            SidebarRun::from_dispatch(run, name)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Conversation,
    Workspace,
    Date,
    Status,
    Agent,
    None,
    Section,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarGroup {
    pub id: String,
    pub kind: GroupKind,
    pub title: String,
    pub agent: Option<String>,
    pub runs: Vec<SidebarRun>,
}

pub fn sidebar_run_title(run: &SidebarRun) -> String {
    prompt_first_line(&run.prompt, &run.id)
}

/// Validate persisted preferences field by field, including older or malformed values.
pub fn parse_sidebar_display(raw: &str) -> SidebarDisplay {
    let mut display = SidebarDisplay::default();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return display;
    };
    let Some(object) = value.as_object() else {
        return display;
    };
    if let Some(parsed) = object
        .get("groupBy")
        .and_then(|v| v.as_str())
        .and_then(GroupBy::parse)
    {
        display.group_by = parsed;
    }
    if let Some(parsed) = object
        .get("sortBy")
        .and_then(|v| v.as_str())
        .and_then(SortBy::parse)
    {
        display.sort_by = parsed;
    }
    if let Some(parsed) = object
        .get("direction")
        .and_then(|v| v.as_str())
        .and_then(Direction::parse)
    {
        display.direction = parsed;
    }
    if let Some(parsed) = object
        .get("show")
        .and_then(|v| v.as_str())
        .and_then(Show::parse)
    {
        display.show = parsed;
    }
    display
}

pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Custom sections come first. Display filters apply to every group, including sections.
/// `now_ms` is the local clock used for the Date buckets.
pub fn group_sidebar_runs(
    runs: &[SidebarRun],
    sections: &[DispatchSection],
    display: &SidebarDisplay,
    now_ms: i64,
) -> Vec<SidebarGroup> {
    let compare = |a: &SidebarRun, b: &SidebarRun| -> std::cmp::Ordering {
        let base = match display.sort_by {
            SortBy::Name => sidebar_run_title(a).cmp(&sidebar_run_title(b)),
            SortBy::Created => timestamp_ms(&a.created_at).cmp(&timestamp_ms(&b.created_at)),
            SortBy::Updated => updated_ms(a).cmp(&updated_ms(b)),
        };
        let directed = match display.direction {
            Direction::Asc => base,
            Direction::Desc => base.reverse(),
        };
        if directed == std::cmp::Ordering::Equal {
            a.id.cmp(&b.id)
        } else {
            directed
        }
    };

    let mut ordered: Vec<&DispatchSection> = sections.iter().collect();
    ordered.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));

    let mut custom: Vec<SidebarGroup> = ordered
        .iter()
        .map(|section| SidebarGroup {
            id: format!("section:{}", section.id),
            kind: GroupKind::Section,
            title: section.name.clone(),
            agent: None,
            runs: Vec::new(),
        })
        .collect();

    let mut earliest: HashMap<String, usize> = HashMap::new();
    for (index, run) in runs.iter().enumerate() {
        match earliest.get(&run.conversation_id).copied() {
            None => {
                earliest.insert(run.conversation_id.clone(), index);
            }
            Some(prev) => {
                let first = &runs[prev];
                let created = timestamp_ms(&run.created_at);
                let first_created = timestamp_ms(&first.created_at);
                if created < first_created
                    || (run.created_at == first.created_at && run.id < first.id)
                {
                    earliest.insert(run.conversation_id.clone(), index);
                }
            }
        }
    }

    let mut automatic: Vec<SidebarGroup> = Vec::new();
    for run in runs {
        if !passes_show(run, display.show) {
            continue;
        }
        if let Some(index) = section_index(&ordered, run) {
            custom[index].runs.push(run.clone());
            continue;
        }
        let (key, title, agent) = automatic_key(run, display.group_by, &earliest, runs, now_ms);
        let id = format!("{}:{key}", display.group_by.as_str());
        if let Some(group) = automatic.iter_mut().find(|group| group.id == id) {
            group.runs.push(run.clone());
        } else {
            automatic.push(SidebarGroup {
                id,
                kind: kind_of(display.group_by),
                title,
                agent,
                runs: vec![run.clone()],
            });
        }
    }

    for group in custom.iter_mut().chain(automatic.iter_mut()) {
        group.runs.sort_by(|a, b| compare(a, b));
    }
    automatic.sort_by(|a, b| {
        if display.group_by == GroupBy::Conversation {
            if a.id == "conversation:" {
                return std::cmp::Ordering::Greater;
            }
            if b.id == "conversation:" {
                return std::cmp::Ordering::Less;
            }
        }
        let by_run = compare(&a.runs[0], &b.runs[0]);
        if by_run == std::cmp::Ordering::Equal {
            a.id.cmp(&b.id)
        } else {
            by_run
        }
    });
    custom.append(&mut automatic);
    custom
}

pub fn conversation_ref(group_id: &str) -> Option<&str> {
    let rest = group_id.strip_prefix("conversation:")?;
    if rest.is_empty() { None } else { Some(rest) }
}

pub fn section_ref(group_id: &str) -> Option<&str> {
    group_id.strip_prefix("section:")
}

fn kind_of(group_by: GroupBy) -> GroupKind {
    match group_by {
        GroupBy::Conversation => GroupKind::Conversation,
        GroupBy::Workspace => GroupKind::Workspace,
        GroupBy::Date => GroupKind::Date,
        GroupBy::Status => GroupKind::Status,
        GroupBy::Agent => GroupKind::Agent,
        GroupBy::None => GroupKind::None,
    }
}

fn automatic_key(
    run: &SidebarRun,
    group_by: GroupBy,
    earliest: &HashMap<String, usize>,
    runs: &[SidebarRun],
    now_ms: i64,
) -> (String, String, Option<String>) {
    match group_by {
        GroupBy::Conversation => {
            let key = run.conversation_id.clone();
            if key.is_empty() {
                (String::new(), "Other runs".to_string(), None)
            } else {
                let first = &runs[earliest[&key]];
                (
                    key,
                    sidebar_run_title(first),
                    Some(first.caller_agent.clone()),
                )
            }
        }
        GroupBy::Workspace => {
            let key = run.project_id.clone();
            let title = if run.workspace_name.trim().is_empty() {
                key.clone()
            } else {
                run.workspace_name.clone()
            };
            (key, title, None)
        }
        GroupBy::Date => {
            let title = date_group(&run.created_at, now_ms).to_string();
            (title.clone(), title, None)
        }
        GroupBy::Status => {
            let title = status_group(run).to_string();
            (title.clone(), title, None)
        }
        GroupBy::Agent => {
            let key = run.agent.clone();
            let title = if key.is_empty() {
                "Unknown agent".to_string()
            } else {
                key.clone()
            };
            (key, title, None)
        }
        GroupBy::None => ("all".to_string(), "All runs".to_string(), None),
    }
}

fn section_index(sections: &[&DispatchSection], run: &SidebarRun) -> Option<usize> {
    if let Some(index) = sections.iter().position(|section| {
        section
            .items
            .iter()
            .any(|item| item.kind == "run" && item.item_ref == run.id)
    }) {
        return Some(index);
    }
    if run.conversation_id.is_empty() {
        return None;
    }
    sections.iter().position(|section| {
        section
            .items
            .iter()
            .any(|item| item.kind == "conversation" && item.item_ref == run.conversation_id)
    })
}

fn passes_show(run: &SidebarRun, show: Show) -> bool {
    match show {
        Show::All => true,
        Show::Unmerged => !merged(run),
        Show::Active => matches!(status_group(run), "Running" | "Needs merge"),
    }
}

fn merged(run: &SidebarRun) -> bool {
    !run.merged_at.is_empty() || !run.merge_sha.is_empty() || run.worktree_status == "merged"
}

fn status_group(run: &SidebarRun) -> &'static str {
    let status = run.status.to_ascii_lowercase();
    if status == "pending" || status == "running" {
        "Running"
    } else if matches!(
        status.as_str(),
        "failed" | "cancelled" | "timeout" | "timed_out" | "orphaned"
    ) {
        "Failed"
    } else if status == "completed" && !run.branch.is_empty() && !merged(run) {
        "Needs merge"
    } else {
        "Done"
    }
}

fn updated_ms(run: &SidebarRun) -> i64 {
    [
        timestamp_ms(&run.created_at),
        timestamp_ms(&run.started_at),
        timestamp_ms(&run.finished_at),
        timestamp_ms(&run.merged_at),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn date_group(created_at: &str, now_ms: i64) -> &'static str {
    let created = timestamp_ms(created_at);
    let (year, month, day) = local_ymd(now_ms);
    let today = local_unix_ms(year, month, day, 0, 0, 0);
    let yesterday = local_unix_ms(year, month, day - 1, 0, 0, 0);
    let week = local_unix_ms(year, month, day - 7, 0, 0, 0);
    if created >= today {
        "Today"
    } else if created >= yesterday {
        "Yesterday"
    } else if created >= week {
        "Previous 7 days"
    } else {
        "Older"
    }
}

/// `Date.parse` for the ISO timestamps the core and the original tests use.
/// Invalid input is 0, matching `Date.parse(value) || 0`.
fn timestamp_ms(value: &str) -> i64 {
    let bytes = value.trim().as_bytes();
    if bytes.len() < 10 {
        return 0;
    }
    let Some(year) = parse_digits(&bytes[0..4]) else {
        return 0;
    };
    let Some(month) = parse_digits(&bytes[5..7]) else {
        return 0;
    };
    let Some(day) = parse_digits(&bytes[8..10]) else {
        return 0;
    };
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return 0;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return 0;
    }
    let mut hour = 0i32;
    let mut minute = 0i32;
    let mut second = 0i32;
    let mut millis = 0i32;
    let mut index = 10usize;
    if matches!(bytes.get(10), Some(b'T' | b't' | b' ')) {
        if bytes.len() < 19 {
            return 0;
        }
        let Some(parsed_hour) = parse_digits(&bytes[11..13]) else {
            return 0;
        };
        let Some(parsed_minute) = parse_digits(&bytes[14..16]) else {
            return 0;
        };
        let Some(parsed_second) = parse_digits(&bytes[17..19]) else {
            return 0;
        };
        if bytes.get(13) != Some(&b':') || bytes.get(16) != Some(&b':') {
            return 0;
        }
        hour = parsed_hour;
        minute = parsed_minute;
        second = parsed_second;
        index = 19;
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if start == index {
                return 0;
            }
            let mut padded = value.trim()[start..index].to_string();
            while padded.len() < 3 {
                padded.push('0');
            }
            millis = padded[..3].parse().unwrap_or(0);
        }
    }
    let mut offset_minutes = 0i32;
    if index < bytes.len() {
        match bytes[index] {
            b'Z' | b'z' => index += 1,
            b'+' | b'-' => {
                let sign = if bytes[index] == b'+' { 1 } else { -1 };
                index += 1;
                if index + 5 <= bytes.len() && bytes.get(index + 2) == Some(&b':') {
                    let Some(offset_hour) = parse_digits(&bytes[index..index + 2]) else {
                        return 0;
                    };
                    let Some(offset_minute) = parse_digits(&bytes[index + 3..index + 5]) else {
                        return 0;
                    };
                    offset_minutes = sign * (offset_hour * 60 + offset_minute);
                    index += 5;
                } else if index + 4 <= bytes.len() {
                    let Some(offset_hour) = parse_digits(&bytes[index..index + 2]) else {
                        return 0;
                    };
                    let Some(offset_minute) = parse_digits(&bytes[index + 2..index + 4]) else {
                        return 0;
                    };
                    offset_minutes = sign * (offset_hour * 60 + offset_minute);
                    index += 4;
                } else {
                    return 0;
                }
            }
            _ => return 0,
        }
    }
    if index != bytes.len() {
        return 0;
    }
    let days = days_from_civil(year, month, day);
    let clock = days * 86_400_000
        + hour as i64 * 3_600_000
        + minute as i64 * 60_000
        + second as i64 * 1_000
        + millis as i64;
    clock - offset_minutes as i64 * 60_000
}

fn parse_digits(bytes: &[u8]) -> Option<i32> {
    if bytes.is_empty() || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut value = 0i32;
    for byte in bytes {
        value = value * 10 + i32::from(byte - b'0');
    }
    Some(value)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = (year - era * 400) as u64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year as u64;
    era * 146097 + day_of_era as i64 - 719468
}

fn local_unix_ms(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_sec = second;
    tm.tm_isdst = -1;
    let secs = unsafe { libc::mktime(&mut tm) };
    if secs < 0 { 0 } else { secs as i64 * 1000 }
}

fn local_ymd(ms: i64) -> (i32, i32, i32) {
    let secs = ms.div_euclid(1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let parsed = unsafe { libc::localtime_r(&secs, &mut tm) };
    if parsed.is_null() {
        (1970, 1, 1)
    } else {
        (tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
    }
}

fn format_utc_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400) as i32;
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days: i64) -> (i32, i32, i32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let day_of_era = (z - era * 146097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as i32, day as i32)
}

#[cfg(test)]
mod tests {
    use super::{
        Direction, GroupBy, Show, SidebarDisplay, SidebarRun, SortBy, format_utc_iso,
        group_sidebar_runs, local_unix_ms, parse_sidebar_display, timestamp_ms,
    };
    use crate::client::{DispatchSection, DispatchSectionItem};

    fn run(id: &str, edit: impl FnOnce(&mut SidebarRun)) -> SidebarRun {
        let mut run = SidebarRun {
            id: id.to_string(),
            prompt: id.to_string(),
            status: "completed".to_string(),
            agent: String::new(),
            model: String::new(),
            project_id: String::new(),
            workspace_name: String::new(),
            branch: String::new(),
            created_at: "2026-11-02T09:00:00Z".to_string(),
            started_at: String::new(),
            finished_at: String::new(),
            merged_at: String::new(),
            merge_sha: String::new(),
            worktree_status: String::new(),
            conversation_id: String::new(),
            caller_agent: String::new(),
        };
        edit(&mut run);
        run
    }

    fn display(edit: impl FnOnce(&mut SidebarDisplay)) -> SidebarDisplay {
        let mut display = SidebarDisplay::default();
        edit(&mut display);
        display
    }

    fn section(id: &str, position: i32, items: Vec<DispatchSectionItem>) -> DispatchSection {
        DispatchSection {
            id: id.to_string(),
            name: id.to_string(),
            position,
            created_at: String::new(),
            items,
        }
    }

    fn item(kind: &str, item_ref: &str) -> DispatchSectionItem {
        DispatchSectionItem {
            kind: kind.to_string(),
            item_ref: item_ref.to_string(),
            position: 0,
        }
    }

    fn ids(runs: &[SidebarRun]) -> Vec<&str> {
        runs.iter().map(|run| run.id.as_str()).collect()
    }

    #[test]
    fn groups_conversations_across_workspaces_by_their_earliest_prompt_and_caller() {
        let runs = vec![
            run("later", |run| {
                run.conversation_id = "chat".into();
                run.prompt = "Later task".into();
                run.caller_agent = "codex".into();
                run.project_id = "two".into();
                run.created_at = "2026-11-02T10:00:00Z".into();
            }),
            run("other", |run| {
                run.created_at = "2026-11-02T12:00:00Z".into();
            }),
            run("first", |run| {
                run.conversation_id = "chat".into();
                run.prompt = "  Original title  \nMore".into();
                run.caller_agent = "claude".into();
                run.project_id = "one".into();
            }),
        ];
        let groups = group_sidebar_runs(&runs, &[], &SidebarDisplay::default(), 0);
        let headings: Vec<(String, Option<String>)> = groups
            .iter()
            .map(|group| (group.title.clone(), group.agent.clone()))
            .collect();
        assert_eq!(
            headings,
            vec![
                ("Original title".to_string(), Some("claude".to_string())),
                ("Other runs".to_string(), None),
            ]
        );
        assert_eq!(ids(&groups[0].runs), ["later", "first"]);
        let ascending = display(|display| display.direction = Direction::Asc);
        assert_eq!(
            group_sidebar_runs(&runs, &[], &ascending, 0)[1].title,
            "Other runs"
        );
    }

    #[test]
    fn places_sections_first_by_position_and_retains_empty_sections() {
        let runs = vec![
            run("a", |run| run.conversation_id = "chat".into()),
            run("b", |run| run.conversation_id = "chat".into()),
            run("c", |_| {}),
        ];
        let sections = vec![
            section("later", 2, vec![item("conversation", "chat")]),
            section("first", 0, vec![item("run", "c")]),
            section("empty", 1, vec![]),
        ];
        let groups = group_sidebar_runs(&runs, &sections, &SidebarDisplay::default(), 0);
        let group_ids: Vec<&str> = groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(
            group_ids,
            ["section:first", "section:empty", "section:later"]
        );
        let grouped: Vec<Vec<&str>> = groups.iter().map(|group| ids(&group.runs)).collect();
        assert_eq!(grouped, vec![vec!["c"], vec![], vec!["a", "b"]]);
        let section_ids: Vec<&str> = sections.iter().map(|section| section.id.as_str()).collect();
        assert_eq!(section_ids, ["later", "first", "empty"]);
        assert_eq!(ids(&runs), ["a", "b", "c"]);
    }

    #[test]
    fn lets_an_explicit_run_membership_override_its_conversation() {
        let sections = vec![
            section("chat", 0, vec![item("conversation", "chat")]),
            section("row", 1, vec![item("run", "a")]),
        ];
        let runs = vec![
            run("a", |run| run.conversation_id = "chat".into()),
            run("b", |run| run.conversation_id = "chat".into()),
        ];
        let groups = group_sidebar_runs(&runs, &sections, &SidebarDisplay::default(), 0);
        let grouped: Vec<Vec<&str>> = groups.iter().map(|group| ids(&group.runs)).collect();
        assert_eq!(grouped, vec![vec!["b"], vec!["a"]]);
    }

    #[test]
    fn uses_workspace_names_and_run_agents_for_the_corresponding_groups() {
        let runs = vec![
            run("a", |run| {
                run.project_id = "one".into();
                run.workspace_name = "First".into();
                run.agent = "codex".into();
            }),
            run("b", |run| {
                run.project_id = "two".into();
                run.workspace_name = "Second".into();
                run.agent = "claude".into();
            }),
        ];
        let by_workspace = display(|display| display.group_by = GroupBy::Workspace);
        let titles = |groups: Vec<super::SidebarGroup>| -> Vec<String> {
            groups.into_iter().map(|group| group.title).collect()
        };
        assert_eq!(
            titles(group_sidebar_runs(&runs, &[], &by_workspace, 0)),
            ["First", "Second"]
        );
        let by_agent = display(|display| display.group_by = GroupBy::Agent);
        assert_eq!(
            titles(group_sidebar_runs(&runs, &[], &by_agent, 0)),
            ["codex", "claude"]
        );
    }

    #[test]
    fn buckets_dates_at_local_calendar_boundaries() {
        let now = local_unix_ms(2026, 11, 2, 12, 0, 0);
        let at = |day: i32| format_utc_iso(local_unix_ms(2026, 11, day, 0, 0, 0));
        let runs = vec![
            run("today", |run| run.created_at = at(2)),
            run("yesterday", |run| run.created_at = at(1)),
            run("week", |run| run.created_at = at(-5)),
            run("old", |run| run.created_at = at(-6)),
        ];
        let by_date = display(|display| display.group_by = GroupBy::Date);
        let groups = group_sidebar_runs(&runs, &[], &by_date, now);
        let titles: Vec<&str> = groups.iter().map(|group| group.title.as_str()).collect();
        assert_eq!(titles, ["Today", "Yesterday", "Previous 7 days", "Older"]);
    }

    #[test]
    fn classifies_every_status_and_only_offers_completed_unmerged_branches_for_merge() {
        let mut runs = [
            "pending",
            "running",
            "failed",
            "cancelled",
            "timed_out",
            "orphaned",
            "TIMEOUT",
        ]
        .into_iter()
        .map(|status| {
            run(status, |run| {
                run.status = status.into();
                run.branch = "branch".into();
            })
        })
        .collect::<Vec<_>>();
        runs.push(run("needs", |run| run.branch = "branch".into()));
        runs.push(run("merged", |run| {
            run.branch = "branch".into();
            run.merged_at = "2026-11-02T10:00:00Z".into();
        }));
        runs.push(run("read-only", |_| {}));
        let by_status = display(|display| display.group_by = GroupBy::Status);
        let groups = group_sidebar_runs(&runs, &[], &by_status, 0);
        let find = |title: &str| -> Vec<&str> {
            groups
                .iter()
                .find(|group| group.title == title)
                .map(|group| ids(&group.runs))
                .unwrap_or_default()
        };
        assert_eq!(find("Running"), ["pending", "running"]);
        assert_eq!(find("Needs merge"), ["needs"]);
        assert_eq!(find("Done"), ["merged", "read-only"]);
        assert_eq!(find("Failed").len(), 5);
    }

    #[test]
    fn filters_custom_and_automatic_groups_and_preserves_a_conversations_original_title() {
        let runs = vec![
            run("first", |run| {
                run.conversation_id = "chat".into();
                run.prompt = "Original".into();
                run.created_at = "2026-11-01T09:00:00Z".into();
                run.merged_at = "2026-11-01T10:00:00Z".into();
            }),
            run("next", |run| {
                run.conversation_id = "chat".into();
                run.branch = "branch".into();
            }),
            run("active", |run| run.status = "running".into()),
            run("failed", |run| run.status = "failed".into()),
        ];
        let sections = vec![section("custom", 0, vec![item("run", "failed")])];
        let unmerged = display(|display| display.show = Show::Unmerged);
        let unmerged_groups = group_sidebar_runs(&runs, &sections, &unmerged, 0);
        let title = unmerged_groups
            .iter()
            .find(|group| {
                group.kind == super::GroupKind::Conversation && group.id == "conversation:chat"
            })
            .map(|group| group.title.as_str());
        assert_eq!(title, Some("Original"));
        let mut flat: Vec<&str> = unmerged_groups
            .iter()
            .flat_map(|group| ids(&group.runs))
            .collect();
        flat.sort();
        assert_eq!(flat, ["active", "failed", "next"]);

        let active = display(|display| display.show = Show::Active);
        let active_groups = group_sidebar_runs(&runs, &sections, &active, 0);
        assert!(active_groups[0].runs.is_empty());
        let mut flat: Vec<&str> = active_groups
            .iter()
            .flat_map(|group| ids(&group.runs))
            .collect();
        flat.sort();
        assert_eq!(flat, ["active", "next"]);
    }

    #[test]
    fn treats_merge_metadata_as_merged_even_if_its_timestamp_is_absent() {
        let runs = vec![
            run("sha", |run| run.merge_sha = "abc".into()),
            run("tree", |run| run.worktree_status = "merged".into()),
            run("unmerged", |_| {}),
        ];
        let show = display(|display| {
            display.show = Show::Unmerged;
            display.group_by = GroupBy::None;
        });
        let groups = group_sidebar_runs(&runs, &[], &show, 0);
        let grouped = ids(&groups[0].runs);
        assert_eq!(grouped, ["unmerged"]);
    }

    #[test]
    fn sorts_groups_and_rows_by_the_latest_timestamp_including_activity_fields() {
        for field in ["started", "finished", "merged"] {
            let runs = vec![
                run("old", |run| run.conversation_id = "one".into()),
                run("new", |run| {
                    run.conversation_id = "one".into();
                    match field {
                        "started" => run.started_at = "2026-11-02T12:00:00Z".into(),
                        "finished" => run.finished_at = "2026-11-02T12:00:00Z".into(),
                        _ => run.merged_at = "2026-11-02T12:00:00Z".into(),
                    }
                }),
                run("middle", |run| {
                    run.conversation_id = "two".into();
                    run.created_at = "2026-11-02T11:00:00Z".into();
                }),
            ];
            let groups = group_sidebar_runs(&runs, &[], &SidebarDisplay::default(), 0);
            let grouped: Vec<Vec<&str>> = groups.iter().map(|group| ids(&group.runs)).collect();
            assert_eq!(grouped, vec![vec!["new", "old"], vec!["middle"]]);
            let ascending = display(|display| display.direction = Direction::Asc);
            let groups = group_sidebar_runs(&runs, &[], &ascending, 0);
            let grouped: Vec<Vec<&str>> = groups.iter().map(|group| ids(&group.runs)).collect();
            assert_eq!(grouped, vec![vec!["old", "new"], vec!["middle"]]);
        }
    }

    #[test]
    fn sorts_created_dates_and_prompt_titles_in_either_direction_with_deterministic_ties() {
        let runs = vec![
            run("a", |run| {
                run.prompt = "Zebra\nAlpha".into();
                run.created_at = "2026-11-01T09:00:00Z".into();
                run.finished_at = "2026-11-02T12:00:00Z".into();
            }),
            run("b", |run| run.prompt = "Alpha".into()),
            run("c", |run| run.prompt = "Alpha".into()),
        ];
        let created = display(|display| {
            display.group_by = GroupBy::None;
            display.sort_by = SortBy::Created;
        });
        assert_eq!(
            ids(&group_sidebar_runs(&runs, &[], &created, 0)[0].runs),
            ["b", "c", "a"]
        );
        let created_asc = display(|display| {
            display.group_by = GroupBy::None;
            display.sort_by = SortBy::Created;
            display.direction = Direction::Asc;
        });
        assert_eq!(
            ids(&group_sidebar_runs(&runs, &[], &created_asc, 0)[0].runs),
            ["a", "b", "c"]
        );
        let name_asc = display(|display| {
            display.group_by = GroupBy::None;
            display.sort_by = SortBy::Name;
            display.direction = Direction::Asc;
        });
        assert_eq!(
            ids(&group_sidebar_runs(&runs, &[], &name_asc, 0)[0].runs),
            ["b", "c", "a"]
        );
        let name_desc = display(|display| {
            display.group_by = GroupBy::None;
            display.sort_by = SortBy::Name;
        });
        assert_eq!(
            ids(&group_sidebar_runs(&runs, &[], &name_desc, 0)[0].runs),
            ["a", "b", "c"]
        );
        assert!(group_sidebar_runs(&[], &[], &SidebarDisplay::default(), 0).is_empty());
    }

    #[test]
    fn recovers_from_malformed_storage_and_validates_preferences_independently() {
        for raw in ["", "broken", "null", "[]", "42"] {
            assert_eq!(parse_sidebar_display(raw), SidebarDisplay::default());
        }
        assert_eq!(
            parse_sidebar_display(
                r#"{"groupBy":"date","sortBy":"invalid","direction":"asc","show":"active"}"#
            ),
            display(|display| {
                display.group_by = GroupBy::Date;
                display.direction = Direction::Asc;
                display.show = Show::Active;
            })
        );
    }

    #[test]
    fn timestamp_round_trips_utc_and_applies_numeric_offsets() {
        let ms = local_unix_ms(2026, 11, 2, 0, 0, 0);
        assert_eq!(timestamp_ms(&format_utc_iso(ms)), ms);
        assert_eq!(
            timestamp_ms("2026-11-02T09:00:00Z"),
            timestamp_ms("2026-11-02T09:00:00.000Z")
        );
        assert_eq!(timestamp_ms(""), 0);
        assert_eq!(timestamp_ms("broken"), 0);
        assert!(timestamp_ms("2026-11-02T12:00:00Z") > timestamp_ms("2026-11-02T09:00:00Z"));
        assert_eq!(
            timestamp_ms("2026-11-02T11:00:00+02:00"),
            timestamp_ms("2026-11-02T09:00:00Z")
        );
    }
}
