//! Sidebar grouping, display preferences, and the tests ported from
//! `packages/features/dispatch/src/sidebar-groups.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::client::{DispatchRun, DispatchSection, Workspace};
use crate::format::prompt_first_line;

pub const DISPLAY_PREFERENCE: &str = "pwdispatch.sidebar.display";
pub const COLLAPSED_PREFERENCE: &str = "pwdispatch.sidebar.collapsed";
/// Each run's mark as its page last showed it, so a row says when it changed since.
pub const SEEN_PREFERENCE: &str = "pwdispatch.sidebar.seen";
/// How many rows each group shows, by group id, for the groups a Show more revealed.
pub const REVEALED_PREFERENCE: &str = "pwdispatch.sidebar.revealed";

/// How many rows a group shows at first, and how many more each Show more adds.
pub const SIDEBAR_PAGE_ROWS: usize = 5;

/// How long a pointer must rest on a run before its transcript card opens: long enough that
/// passing down the list opens nothing, short enough that a pause still does.
pub const HOVER_OPEN_MS: u64 = 900;
/// How long the card stays up while the pointer crosses from the row into it.
pub const HOVER_CLOSE_MS: u64 = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    Conversation,
    /// One group a repository. A Display saved before runs grouped so says `workspace`.
    #[serde(alias = "workspace")]
    Repository,
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
            // `workspace` is what a Display saved before runs grouped by repository.
            "repository" | "workspace" => Self::Repository,
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
            Self::Repository => "repository",
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
    /// The repository the run works in: its main checkout, else its folder.
    pub repo: String,
    pub branch: String,
    pub created_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub merged_at: String,
    pub merge_sha: String,
    pub worktree_status: String,
    pub conversation_id: String,
    pub caller_agent: String,
    /// The earlier runs of the chat this row stands for, oldest first: the runs it followed
    /// up on, which the sidebar folds into this one row.
    pub earlier: Vec<String>,
}

/// What a run last changed to: its status, and when it ended and merged.
pub fn run_mark(run: &SidebarRun) -> String {
    format!(
        "{}|{}|{}",
        run.status.trim().to_ascii_lowercase(),
        run.finished_at.trim(),
        run.merged_at.trim()
    )
}

/// Whether a run has an update its page has not shown: it is no longer working, and it ended,
/// failed, or merged since its page was last open. A working run has nothing to read yet.
pub fn has_unseen_update(run: &SidebarRun, seen: &HashMap<String, String>) -> bool {
    !crate::models::runs::is_run_active(&run.status) && seen.get(&run.id) != Some(&run_mark(run))
}

impl SidebarRun {
    pub fn from_dispatch(run: &DispatchRun, workspace_name: &str, workspace_path: &str) -> Self {
        let repo = [run.repo.as_str(), run.dir.as_str(), workspace_path]
            .into_iter()
            .map(|path| path.trim().trim_end_matches('/'))
            .find(|path| !path.is_empty())
            .map(main_checkout)
            .unwrap_or_default();
        Self {
            id: run.id.clone(),
            prompt: run.prompt.clone(),
            status: run.status.clone(),
            agent: run.agent.clone(),
            model: run.model.clone(),
            project_id: run.project_id.clone(),
            workspace_name: workspace_name.to_string(),
            repo,
            branch: run.branch.clone(),
            created_at: run.created_at.clone(),
            started_at: run.started_at.clone(),
            finished_at: run.finished_at.clone(),
            merged_at: run.merged_at.clone(),
            merge_sha: run.merge_sha.clone(),
            worktree_status: run.worktree_status.clone(),
            conversation_id: run.conversation_id.clone(),
            caller_agent: run.caller_agent.clone(),
            earlier: Vec::new(),
        }
    }
}

/// The runs a group's Delete runs takes: each ended row's run and the earlier runs folded
/// into it. A row still working is left out, since the core refuses to delete it.
pub fn ended_run_ids(runs: &[SidebarRun]) -> Vec<String> {
    let mut ids = Vec::new();
    for run in runs {
        let working = matches!(
            run.status.to_ascii_lowercase().as_str(),
            "pending" | "running" | "queued"
        );
        if working {
            continue;
        }
        for id in run.earlier.iter().chain(std::iter::once(&run.id)) {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

/// The main checkout of the repository `path` is in, as the core names a repository
/// (`worktree::main_root`), read without git so the sidebar can ask for every row: a linked
/// worktree's `.git` file points into the main checkout's `.git/worktrees`, and a Dispatch
/// worktree that is gone is still under its repository's `.dispatch/worktrees`. Any other
/// path is its own. Answers are kept, since a folder's repository does not change.
pub fn main_checkout(path: &str) -> String {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(found) = seen.lock().ok().and_then(|seen| seen.get(path).cloned()) {
        return found;
    }
    let found = dispatch_core::worktree::main_checkout(path);
    if let Ok(mut seen) = seen.lock() {
        seen.insert(path.to_string(), found.clone());
    }
    found
}

/// Joins each run to its workspace name. A missing workspace leaves the name empty
/// so grouping can fall back to the project id.
///
/// A chat of follow-ups is one row: a run a message was sent to folds into the run that
/// message started, which shows the latest status under the chat's first prompt and its
/// first run's created time.
pub fn sidebar_runs(workspaces: &[Workspace], runs: &[DispatchRun]) -> Vec<SidebarRun> {
    let mut names: HashMap<&str, (&str, &str)> = HashMap::new();
    for workspace in workspaces {
        names.insert(
            workspace.id.as_str(),
            (workspace.name.as_str(), workspace.path.as_str()),
        );
    }
    let by_id: HashMap<&str, &DispatchRun> =
        runs.iter().map(|run| (run.id.as_str(), run)).collect();
    let followed: std::collections::HashSet<&str> = runs
        .iter()
        .map(|run| run.resumed_of.trim())
        .filter(|id| by_id.contains_key(id))
        .collect();
    runs.iter()
        .filter(|run| !followed.contains(run.id.as_str()))
        .map(|run| {
            let (name, path) = names
                .get(run.project_id.as_str())
                .copied()
                .unwrap_or(("", ""));
            let mut row = SidebarRun::from_dispatch(run, name, path);
            let mut at = run;
            while let Some(prev) = by_id.get(at.resumed_of.trim()) {
                if row.earlier.iter().any(|id| id == &prev.id) || prev.id == run.id {
                    break;
                }
                row.earlier.insert(0, prev.id.clone());
                at = prev;
            }
            if !row.earlier.is_empty() {
                row.prompt.clone_from(&at.prompt);
                row.created_at.clone_from(&at.created_at);
            }
            row
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    Conversation,
    Repository,
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

/// How much of a group's title its heading tooltip shows: a very long prompt is cut here
/// rather than shown whole.
pub const GROUP_TITLE_TIP_CHARS: usize = 500;

/// A group heading's tooltip title: the group's whole title (its conversation's name, else
/// its first prompt), clamped to [`GROUP_TITLE_TIP_CHARS`].
pub fn group_title_tooltip(title: &str) -> String {
    if title.chars().count() <= GROUP_TITLE_TIP_CHARS {
        return title.to_string();
    }
    let mut cut: String = title.chars().take(GROUP_TITLE_TIP_CHARS).collect();
    cut.push('…');
    cut
}

/// A group's rows as the sidebar lists them: the first `limit` in Display order, plus every
/// row that needs attention, which is never cut. `hidden` counts the rows left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarPage<'a> {
    pub rows: Vec<&'a SidebarRun>,
    pub hidden: usize,
    /// The limit a Show more asks for.
    pub next_limit: usize,
}

pub fn page_sidebar_rows<'a>(
    rows: &'a [SidebarRun],
    limit: usize,
    pinned: impl Fn(&SidebarRun) -> bool,
) -> SidebarPage<'a> {
    let limit = limit.max(1);
    let shown: Vec<&SidebarRun> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, run)| (index < limit || pinned(run)).then_some(run))
        .collect();
    SidebarPage {
        hidden: rows.len() - shown.len(),
        rows: shown,
        next_limit: limit.saturating_add(SIDEBAR_PAGE_ROWS),
    }
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

/// The repository a Group by Repository group stands for: its main checkout's path (or,
/// for runs with no folder, their workspace's id).
pub fn repository_ref(group_id: &str) -> Option<&str> {
    let rest = group_id.strip_prefix("repository:")?;
    if rest.is_empty() { None } else { Some(rest) }
}

/// The workspace Remove repository forgets for a repository group: one added on its own
/// whose folder is that repository (or whose id the group fell back to). A repository a
/// Polterware project brings follows the project, so it has none.
pub fn removable_repository(repository: &str, workspaces: &[Workspace]) -> Option<String> {
    let key = repository.trim_end_matches('/');
    workspaces
        .iter()
        .filter(|workspace| workspace.polterware_project_id.is_empty())
        .find(|workspace| {
            workspace.id == key
                || (!workspace.path.is_empty() && workspace.path.trim_end_matches('/') == key)
        })
        .map(|workspace| workspace.id.clone())
}

/// What the runs list shows besides its groups: one thing at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunsNotice {
    /// Runs are listed and the last refresh worked: nothing more.
    None,
    /// The core is starting and nothing is listed yet: the rows in bones.
    Loading,
    /// Nothing to list: the mascot asleep, saying so.
    Empty,
    /// Nothing listed because the core did not answer: the mascot sad, saying only that.
    Unreachable,
    /// Runs are listed, but the last refresh failed: a line over them, which may be old.
    Stale,
}

/// The notice for a list that holds runs or not (`listed`), while the core starts
/// (`connecting`) or after a refresh that failed (`failed`).
pub fn runs_notice(listed: bool, connecting: bool, failed: bool) -> RunsNotice {
    match (listed, connecting, failed) {
        (false, true, _) => RunsNotice::Loading,
        (false, false, true) => RunsNotice::Unreachable,
        (false, false, false) => RunsNotice::Empty,
        (true, _, true) => RunsNotice::Stale,
        (true, _, false) => RunsNotice::None,
    }
}

fn kind_of(group_by: GroupBy) -> GroupKind {
    match group_by {
        GroupBy::Conversation => GroupKind::Conversation,
        GroupBy::Repository => GroupKind::Repository,
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
        GroupBy::Repository => {
            // One repository is one group, whatever workspace or worktree its runs came
            // through; a run with no folder falls back to its workspace.
            let key = if run.repo.is_empty() {
                run.project_id.clone()
            } else {
                run.repo.clone()
            };
            (key, repository_title(run), None)
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

/// A repository group's title: its folder's name, as harnesses name a project.
fn repository_title(run: &SidebarRun) -> String {
    let name = std::path::Path::new(&run.repo)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    [
        name.as_str(),
        run.workspace_name.trim(),
        run.project_id.as_str(),
    ]
    .into_iter()
    .find(|title| !title.is_empty())
    .unwrap_or("Repository")
    .to_string()
}

/// The section that holds the item itself: a `kind` item ("run" or "conversation") whose ref
/// is one of `refs` (a run row's id and its earlier ones). `None` when the item is in no
/// section, or shows in one only through its conversation.
pub fn holding_section<'a>(
    sections: &'a [DispatchSection],
    kind: &str,
    refs: &[&str],
) -> Option<&'a str> {
    sections
        .iter()
        .find(|section| {
            section
                .items
                .iter()
                .any(|item| item.kind == kind && refs.contains(&item.item_ref.as_str()))
        })
        .map(|section| section.id.as_str())
}

fn section_index(sections: &[&DispatchSection], run: &SidebarRun) -> Option<usize> {
    if let Some(index) = sections.iter().position(|section| {
        section.items.iter().any(|item| {
            item.kind == "run" && (item.item_ref == run.id || run.earlier.contains(&item.item_ref))
        })
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

/// `HH:MM` on this computer's clock for an ISO timestamp, or `None` when it does not parse.
pub fn local_clock(value: &str) -> Option<String> {
    let ms = timestamp_ms(value);
    if ms == 0 {
        return None;
    }
    let secs = ms.div_euclid(1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let parsed = unsafe { libc::localtime_r(&secs, &mut tm) };
    if parsed.is_null() {
        return None;
    }
    Some(format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))
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
        Direction, GroupBy, RunsNotice, Show, SidebarDisplay, SidebarRun, SortBy, format_utc_iso,
        group_sidebar_runs, has_unseen_update, local_clock, local_unix_ms, page_sidebar_rows,
        parse_sidebar_display, run_mark, runs_notice, timestamp_ms,
    };

    #[test]
    fn a_run_shows_an_update_until_its_page_shows_it() {
        let mut seen = std::collections::HashMap::new();
        let working = run("r1", |r| r.status = "running".into());
        assert!(
            !has_unseen_update(&working, &seen),
            "nothing to read while it works"
        );
        let done = run("r1", |r| r.finished_at = "2026-11-04T10:00:00Z".into());
        assert!(
            has_unseen_update(&done, &seen),
            "it ended and its page was not open"
        );
        seen.insert(done.id.clone(), run_mark(&done));
        assert!(!has_unseen_update(&done, &seen), "its page showed the end");
        let merged = run("r1", |r| {
            r.finished_at = "2026-11-04T10:00:00Z".into();
            r.merged_at = "2026-11-04T10:05:00Z".into();
        });
        assert!(
            has_unseen_update(&merged, &seen),
            "a merge after that is news"
        );
    }

    #[test]
    fn an_empty_list_says_one_thing() {
        // The core is down and nothing is listed: that it could not load, not also that it is
        // empty.
        assert_eq!(runs_notice(false, false, true), RunsNotice::Unreachable);
        assert_eq!(runs_notice(false, false, false), RunsNotice::Empty);
        assert_eq!(runs_notice(false, true, true), RunsNotice::Loading);
        assert_eq!(runs_notice(false, true, false), RunsNotice::Loading);
        assert_eq!(runs_notice(true, false, true), RunsNotice::Stale);
        assert_eq!(runs_notice(true, true, false), RunsNotice::None);
    }
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
            repo: String::new(),
            branch: String::new(),
            created_at: "2026-11-02T09:00:00Z".to_string(),
            started_at: String::new(),
            finished_at: String::new(),
            merged_at: String::new(),
            merge_sha: String::new(),
            worktree_status: String::new(),
            conversation_id: String::new(),
            caller_agent: String::new(),
            earlier: Vec::new(),
        };
        edit(&mut run);
        run
    }

    #[test]
    fn a_chat_of_follow_ups_is_one_row_under_its_first_prompt() {
        use super::sidebar_runs;
        use crate::client::DispatchRun;
        let at =
            |id: &str, prompt: &str, created: &str, resumed_of: &str, status: &str| DispatchRun {
                id: id.into(),
                prompt: prompt.into(),
                created_at: created.into(),
                resumed_of: resumed_of.into(),
                status: status.into(),
                ..DispatchRun::default()
            };
        let runs = vec![
            at("c", "and now?", "2026-11-05T12:00:00Z", "b", "RUNNING"),
            at("b", "oi", "2026-11-05T11:00:00Z", "a", "COMPLETED"),
            at(
                "a",
                "Read the views",
                "2026-11-05T10:00:00Z",
                "",
                "COMPLETED",
            ),
            at(
                "z",
                "Something else",
                "2026-11-05T09:00:00Z",
                "",
                "COMPLETED",
            ),
        ];
        let rows = sidebar_runs(&[], &runs);
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["c", "z"]);
        assert_eq!(rows[0].prompt, "Read the views");
        assert_eq!(rows[0].created_at, "2026-11-05T10:00:00Z");
        assert_eq!(rows[0].status, "RUNNING");
        assert_eq!(rows[0].earlier, ["a", "b"]);
        assert!(rows[1].earlier.is_empty());
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
    fn a_group_keeps_its_whole_title_for_the_heading_tooltip() {
        let prompt = format!("Goal: {}", ["in the Dispatch app"; 40].join(" "));
        let runs = vec![run("first", |run| {
            run.conversation_id = "chat".into();
            run.prompt = prompt.clone();
        })];
        let groups = group_sidebar_runs(&runs, &[], &SidebarDisplay::default(), 0);
        assert_eq!(
            groups[0].title, prompt,
            "the group keeps the untruncated title"
        );
        let tip = super::group_title_tooltip(&groups[0].title);
        assert_eq!(tip.chars().count(), super::GROUP_TITLE_TIP_CHARS + 1);
        let cut = tip.strip_suffix('…').unwrap();
        assert!(
            prompt.starts_with(cut),
            "the tooltip keeps the title's start"
        );
        assert_eq!(super::group_title_tooltip("Short task"), "Short task");
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
    fn finds_the_section_that_holds_an_item_itself() {
        let sections = vec![
            section("chat", 0, vec![item("conversation", "c1")]),
            section("row", 1, vec![item("run", "old")]),
        ];
        assert_eq!(
            super::holding_section(&sections, "conversation", &["c1"]),
            Some("chat")
        );
        // A run row holds its section through an earlier run it folds in.
        assert_eq!(
            super::holding_section(&sections, "run", &["new", "old"]),
            Some("row")
        );
        // A run of a conversation in a section is not in it itself.
        assert_eq!(super::holding_section(&sections, "run", &["c1"]), None);
        assert_eq!(
            super::holding_section(&sections, "conversation", &["c2"]),
            None
        );
    }

    #[test]
    fn delete_runs_takes_the_ended_rows_and_their_earlier_runs() {
        let rows = vec![
            run("a", |run| {
                run.status = "COMPLETED".into();
                run.earlier = vec!["a0".into()];
            }),
            run("b", |run| run.status = "RUNNING".into()),
            run("c", |run| run.status = "failed".into()),
        ];
        assert_eq!(super::ended_run_ids(&rows), ["a0", "a", "c"]);
    }

    #[test]
    fn a_worktree_is_grouped_with_its_main_checkout() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("app");
        let linked = root.path().join("lanes").join("core-c");
        std::fs::create_dir_all(main.join(".git").join("worktrees").join("core-c")).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!(
                "gitdir: {}\n",
                main.join(".git").join("worktrees").join("core-c").display()
            ),
        )
        .unwrap();
        let main_path = main.to_string_lossy().into_owned();
        assert_eq!(super::main_checkout(&linked.to_string_lossy()), main_path);
        assert_eq!(super::main_checkout(&main_path), main_path);
        // A Dispatch worktree that is gone still names its repository.
        assert_eq!(
            super::main_checkout("/code/app/.dispatch/worktrees/3d38a36d"),
            "/code/app"
        );
        assert_eq!(super::main_checkout("/gone/lane"), "/gone/lane");
    }

    #[test]
    fn a_repository_group_can_remove_the_workspace_it_was_added_as() {
        let workspace = |id: &str, path: &str, project: &str| crate::client::Workspace {
            id: id.into(),
            path: path.into(),
            polterware_project_id: project.into(),
            ..crate::client::Workspace::default()
        };
        let all = [
            workspace("ws-a", "/code/dispatch", ""),
            workspace("ws-home", "/Users/me", ""),
            workspace("proj-1", "", "pw-1"),
        ];
        assert_eq!(
            super::repository_ref("repository:/code/dispatch"),
            Some("/code/dispatch")
        );
        assert_eq!(super::repository_ref("conversation:c1"), None);
        assert_eq!(
            super::removable_repository("/code/dispatch/", &all).as_deref(),
            Some("ws-a")
        );
        assert_eq!(
            super::removable_repository("/Users/me", &all).as_deref(),
            Some("ws-home")
        );
        assert_eq!(super::removable_repository("proj-1", &all), None);
        assert_eq!(super::removable_repository("/code/other", &all), None);
    }

    #[test]
    fn groups_by_repository_and_by_agent() {
        let runs = vec![
            run("a", |run| {
                run.project_id = "home".into();
                run.workspace_name = "Home".into();
                run.repo = "/code/dispatch".into();
                run.agent = "codex".into();
            }),
            run("b", |run| {
                run.project_id = "two".into();
                run.workspace_name = "Second".into();
                run.agent = "claude".into();
            }),
            run("c", |run| {
                // Another workspace, the same repository: one group.
                run.project_id = "other".into();
                run.repo = "/code/dispatch".into();
                run.agent = "codex".into();
            }),
        ];
        let by_repository = display(|display| display.group_by = GroupBy::Repository);
        let titles = |groups: Vec<super::SidebarGroup>| -> Vec<String> {
            groups.into_iter().map(|group| group.title).collect()
        };
        assert_eq!(
            titles(group_sidebar_runs(&runs, &[], &by_repository, 0)),
            ["dispatch", "Second"]
        );
        let by_agent = display(|display| display.group_by = GroupBy::Agent);
        assert_eq!(
            titles(group_sidebar_runs(&runs, &[], &by_agent, 0)),
            ["codex", "claude"]
        );
        assert_eq!(GroupBy::parse("workspace"), Some(GroupBy::Repository));
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

    #[test]
    fn local_clock_reads_the_time_on_this_computers_clock() {
        let ms = local_unix_ms(2026, 11, 2, 9, 30, 0);
        assert_eq!(local_clock(&format_utc_iso(ms)).as_deref(), Some("09:30"));
        let ms = local_unix_ms(2026, 11, 2, 0, 5, 0);
        assert_eq!(local_clock(&format_utc_iso(ms)).as_deref(), Some("00:05"));
        assert_eq!(local_clock(""), None);
        assert_eq!(local_clock("broken"), None);
    }

    #[test]
    fn pages_a_groups_rows_five_at_a_time_without_cutting_a_pinned_row() {
        let rows: Vec<SidebarRun> = (0..12)
            .map(|index| run(&format!("r{index}"), |_| {}))
            .collect();
        let page = page_sidebar_rows(&rows, 5, |_| false);
        assert_eq!(
            page.rows
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            ["r0", "r1", "r2", "r3", "r4"]
        );
        assert_eq!(page.hidden, 7);
        assert_eq!(page.next_limit, 10);

        let page = page_sidebar_rows(&rows, 10, |_| false);
        assert_eq!(page.rows.len(), 10);
        assert_eq!(page.hidden, 2);
        let page = page_sidebar_rows(&rows, 15, |_| false);
        assert_eq!(page.rows.len(), 12);
        assert_eq!(page.hidden, 0);

        // A row that needs attention stays past the limit, in its Display place, and counts
        // as shown; one already inside the page changes nothing.
        let page = page_sidebar_rows(&rows, 5, |run| run.id == "r9");
        assert_eq!(page.rows.len(), 6);
        assert_eq!(page.rows[5].id, "r9");
        assert_eq!(page.hidden, 6);
        let page = page_sidebar_rows(&rows, 5, |run| run.id == "r2");
        assert_eq!(page.rows.len(), 5);
        assert_eq!(page.hidden, 7);
    }
}
