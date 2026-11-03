//! Pure logic for Commander (⌘K).
//!
//! Dispatch's palette is the shared command palette with `runAgent` and `searchAll`
//! off: a fuzzy list of each page, Settings, Agent setup, Reload, and recent runs.
//! Arrow keys wrap, Enter runs the lit row, and a new result set re-lights the first row.

use std::collections::HashSet;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::format::prompt_first_line;
use crate::state::{Route, SettingsPage};

/// Top inset of the results scroller, matching the palette's `pt-1`.
pub const TOP_PAD: f32 = 4.0;
/// One command row. The sliding highlight is this tall.
pub const COMMAND_ROW_H: f32 = 32.0;
/// Group heading row.
pub const GROUP_HEADER_H: f32 = 24.0;
/// Gap between groups.
pub const GROUP_GAP: f32 = 8.0;
/// How many runs the Recent group keeps. Newest first.
pub const RECENT_LIMIT: usize = 12;

pub const PLACEHOLDER: &str = "Go to a page or run a command…";
pub const EMPTY_LABEL: &str = "No matches";
pub const RELOAD_TOAST: &str = "Reloaded.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteGroup {
    Navigation,
    Actions,
    Recent,
}

impl PaletteGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Actions => "Actions",
            Self::Recent => "Recent",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Navigation => 0,
            Self::Actions => 1,
            Self::Recent => 2,
        }
    }
}

/// Which mark a row paints. The view maps these onto shared icons or the avatar seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteIcon {
    Roles,
    Intelligence,
    Spend,
    Stats,
    Settings,
    AgentSetup,
    Reload,
    Run { agent: String, status: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteItem {
    pub id: String,
    pub label: String,
    pub group: PaletteGroup,
    pub icon: PaletteIcon,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandEffect {
    Navigate(Route),
    Reload,
}

/// A run the palette can list under Recent. `created_at` is the core's timestamp string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentRun {
    pub id: String,
    pub prompt: String,
    pub created_at: String,
    pub agent: String,
    pub status: String,
}

/// The search field. The cursor is a byte offset on a char boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryEditor {
    text: String,
    cursor: usize,
    selected: bool,
}

impl Default for QueryEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryEditor {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            selected: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor.min(self.text.len())
    }

    pub fn selected(&self) -> bool {
        self.selected && !self.text.is_empty()
    }

    pub fn before_cursor(&self) -> &str {
        &self.text[..self.cursor()]
    }

    pub fn after_cursor(&self) -> &str {
        &self.text[self.cursor()..]
    }

    pub fn insert(&mut self, raw: &str) {
        let clean: String = raw
            .chars()
            .filter(|ch| *ch != '\n' && *ch != '\r')
            .collect();
        if clean.is_empty() {
            return;
        }
        if self.selected {
            self.text.clear();
            self.cursor = 0;
            self.selected = false;
        }
        let cursor = self.cursor();
        self.text.insert_str(cursor, &clean);
        self.cursor = cursor + clean.len();
    }

    pub fn backspace(&mut self) {
        if self.selected {
            self.clear();
            return;
        }
        let cursor = self.cursor();
        let start = prev_boundary(&self.text, cursor);
        if start == cursor {
            return;
        }
        self.text.replace_range(start..cursor, "");
        self.cursor = start;
    }

    pub fn delete_forward(&mut self) {
        if self.selected {
            self.clear();
            return;
        }
        let cursor = self.cursor();
        let end = next_boundary(&self.text, cursor);
        if end == cursor {
            return;
        }
        self.text.replace_range(cursor..end, "");
    }

    pub fn move_left(&mut self) {
        self.selected = false;
        self.cursor = prev_boundary(&self.text, self.cursor());
    }

    pub fn move_right(&mut self) {
        self.selected = false;
        self.cursor = next_boundary(&self.text, self.cursor());
    }

    pub fn move_to_start(&mut self) {
        self.selected = false;
        self.cursor = 0;
    }

    pub fn move_to_end(&mut self) {
        self.selected = false;
        self.cursor = self.text.len();
    }

    pub fn select_all(&mut self) {
        self.selected = !self.text.is_empty();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.selected = false;
    }
}

fn prev_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(idx, _)| (idx < offset).then_some(idx))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(idx, _)| (idx > offset).then_some(idx))
        .unwrap_or(text.len())
}

/// Pages, Settings, Agent setup, and Reload. Ids and labels match Dispatch's `COMMANDS`.
pub fn static_commands() -> Vec<PaletteItem> {
    vec![
        command(
            "nav.roles",
            "Roles",
            PaletteGroup::Navigation,
            PaletteIcon::Roles,
        ),
        command(
            "nav.models",
            "Intelligence",
            PaletteGroup::Navigation,
            PaletteIcon::Intelligence,
        ),
        command(
            "nav.usage",
            "Spend",
            PaletteGroup::Navigation,
            PaletteIcon::Spend,
        ),
        command(
            "nav.stats",
            "Stats",
            PaletteGroup::Navigation,
            PaletteIcon::Stats,
        ),
        command(
            "nav.settings",
            "Settings",
            PaletteGroup::Navigation,
            PaletteIcon::Settings,
        ),
        command(
            "nav.agent-setup",
            "Agent setup",
            PaletteGroup::Navigation,
            PaletteIcon::AgentSetup,
        ),
        command(
            "action.reload",
            "Reload",
            PaletteGroup::Actions,
            PaletteIcon::Reload,
        ),
    ]
}

fn command(id: &str, label: &str, group: PaletteGroup, icon: PaletteIcon) -> PaletteItem {
    PaletteItem {
        id: id.to_string(),
        label: label.to_string(),
        group,
        icon,
    }
}

/// Static commands, then up to [`RECENT_LIMIT`] runs, newest `created_at` first.
pub fn with_recent(runs: &[RecentRun]) -> Vec<PaletteItem> {
    let mut items = static_commands();
    items.extend(recent_items(runs));
    items
}

fn recent_items(runs: &[RecentRun]) -> Vec<PaletteItem> {
    let mut indexed: Vec<(usize, &RecentRun)> = runs
        .iter()
        .enumerate()
        .filter(|(_, run)| !run.id.trim().is_empty() && !run.created_at.trim().is_empty())
        .collect();
    indexed.sort_by(|(left_i, left), (right_i, right)| {
        right
            .created_at
            .cmp(&left.created_at)
            .then(left_i.cmp(right_i))
    });
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, run) in indexed {
        if !seen.insert(run.id.clone()) {
            continue;
        }
        if out.len() == RECENT_LIMIT {
            break;
        }
        out.push(PaletteItem {
            id: format!("recent.run:{}", run.id),
            label: prompt_first_line(&run.prompt, &run.id),
            group: PaletteGroup::Recent,
            icon: PaletteIcon::Run {
                agent: run.agent.clone(),
                status: run.status.clone(),
            },
        });
    }
    out
}

/// Case-insensitive subsequence score. An empty query matches everything at score 0.
/// Consecutive characters and word starts score higher, so a typed prefix wins.
pub fn fuzzy_score(label: &str, query: &str) -> Option<i32> {
    let needle = fold(query.trim());
    if needle.is_empty() {
        return Some(0);
    }
    let hay = fold(label);
    let mut score = 0_i32;
    let mut from = 0_usize;
    let mut previous: Option<usize> = None;
    for needle_ch in needle {
        let found = hay[from..]
            .iter()
            .position(|ch| *ch == needle_ch)
            .map(|offset| from + offset)?;
        score += 1;
        if previous == Some(found.saturating_sub(1)) {
            score += 8;
        }
        if found == 0 {
            score += 6;
        } else if !hay[found - 1].is_alphanumeric() {
            score += 4;
        }
        previous = Some(found);
        from = found + 1;
    }
    Some(score)
}

/// Fuzzy-filters `items`, keeping Navigation, then Actions, then Recent.
/// Inside a group, a higher score comes first and ties keep catalog order.
pub fn filter_items(items: &[PaletteItem], query: &str) -> Vec<PaletteItem> {
    let mut scored: Vec<(usize, i32, PaletteItem)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            fuzzy_score(&item.label, query).map(|score| (index, score, item.clone()))
        })
        .collect();
    scored.sort_by(|left, right| {
        left.2
            .group
            .rank()
            .cmp(&right.2.group.rank())
            .then(right.1.cmp(&left.1))
            .then(left.0.cmp(&right.0))
    });
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Contiguous group ranges in `items`, which [`filter_items`] already orders.
pub fn group_spans(items: &[PaletteItem]) -> Vec<(PaletteGroup, Range<usize>)> {
    let mut spans: Vec<(PaletteGroup, Range<usize>)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match spans.last_mut() {
            Some((group, range)) if *group == item.group => range.end = index + 1,
            _ => spans.push((item.group, index..index + 1)),
        }
    }
    spans
}

/// Y of a command row inside the results column. `lengths` is each group's row count.
pub fn highlight_y(lengths: &[usize], index: usize) -> f32 {
    let mut y = TOP_PAD;
    let mut seen = 0_usize;
    for (group_index, len) in lengths.iter().enumerate() {
        if group_index > 0 {
            y += GROUP_GAP;
        }
        y += GROUP_HEADER_H;
        if index < seen + *len {
            y += (index - seen) as f32 * COMMAND_ROW_H;
            return y;
        }
        y += *len as f32 * COMMAND_ROW_H;
        seen += *len;
    }
    y
}

/// Arrow motion. `step` is negative for up. Wraps, and a missing selection
/// lands on the first row going down or the last row going up.
pub fn step_index(active: Option<usize>, count: usize, step: isize) -> Option<usize> {
    if count == 0 || step == 0 {
        return None;
    }
    let direction: isize = if step > 0 { 1 } else { -1 };
    let count_i = count as isize;
    let next = match active {
        None => {
            if direction > 0 {
                0
            } else {
                count_i - 1
            }
        }
        Some(current) => {
            let value = current as isize + direction;
            (value % count_i + count_i) % count_i
        }
    };
    Some(next as usize)
}

pub fn effect_for(id: &str) -> Option<CommandEffect> {
    match id {
        "nav.roles" => Some(CommandEffect::Navigate(Route::Roles)),
        "nav.models" => Some(CommandEffect::Navigate(Route::Intelligence)),
        "nav.usage" => Some(CommandEffect::Navigate(Route::Spend)),
        "nav.stats" => Some(CommandEffect::Navigate(Route::Stats)),
        "nav.settings" | "nav.agent-setup" => Some(CommandEffect::Navigate(Route::Settings(
            SettingsPage::Agents,
        ))),
        "action.reload" => Some(CommandEffect::Reload),
        other => {
            let run_id = other.strip_prefix("recent.run:")?;
            if run_id.is_empty() {
                None
            } else {
                Some(CommandEffect::Navigate(Route::Run(run_id.to_string())))
            }
        }
    }
}

fn fold(text: &str) -> Vec<char> {
    text.chars().flat_map(|ch| ch.to_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND_ROW_H, CommandEffect, EMPTY_LABEL, GROUP_GAP, GROUP_HEADER_H, PLACEHOLDER,
        PaletteGroup, PaletteItem, QueryEditor, RECENT_LIMIT, RELOAD_TOAST, RecentRun, TOP_PAD,
        effect_for, filter_items, fuzzy_score, group_spans, highlight_y, static_commands,
        step_index, with_recent,
    };
    use crate::state::{Route, SettingsPage};

    fn item(id: &str, label: &str, group: PaletteGroup) -> PaletteItem {
        PaletteItem {
            id: id.to_string(),
            label: label.to_string(),
            group,
            icon: super::PaletteIcon::Roles,
        }
    }

    #[test]
    fn blank_query_returns_every_command_in_catalog_order() {
        let items = static_commands();
        assert_eq!(filter_items(&items, ""), items);
        assert_eq!(filter_items(&items, "   "), items);
        let spans = group_spans(&items);
        assert_eq!(spans[0].0, PaletteGroup::Navigation);
        assert_eq!(spans[0].1, 0..6);
        assert_eq!(spans[1].0, PaletteGroup::Actions);
        assert_eq!(spans[1].1, 6..7);
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        let items = vec![
            item("nav.inbox", "Inbox", PaletteGroup::Navigation),
            item("nav.activity", "Activity", PaletteGroup::Navigation),
        ];
        let filtered = filter_items(&items, "inbox");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "nav.inbox");
        assert!(filter_items(&items, "zzzz").is_empty());
    }

    #[test]
    fn fuzzy_subsequence_ranks_a_prefix_ahead_of_a_later_word() {
        let items = static_commands();
        let roles = filter_items(&items, "rls");
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].id, "nav.roles");
        assert!(fuzzy_score("Roles", "xyz").is_none());

        let settings = filter_items(&items, "set");
        assert_eq!(settings[0].id, "nav.settings");
        assert!(settings.iter().any(|item| item.id == "nav.agent-setup"));
        assert!(
            filter_items(&items, "reload")
                .iter()
                .any(|item| item.id == "action.reload")
        );
    }

    #[test]
    fn static_commands_match_dispatch_copy() {
        let items = static_commands();
        let pairs: Vec<(&str, &str)> = items
            .iter()
            .map(|item| (item.id.as_str(), item.label.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("nav.roles", "Roles"),
                ("nav.models", "Intelligence"),
                ("nav.usage", "Spend"),
                ("nav.stats", "Stats"),
                ("nav.settings", "Settings"),
                ("nav.agent-setup", "Agent setup"),
                ("action.reload", "Reload"),
            ]
        );
        assert_eq!(PLACEHOLDER, "Go to a page or run a command…");
        assert_eq!(EMPTY_LABEL, "No matches");
        assert_eq!(RELOAD_TOAST, "Reloaded.");
    }

    #[test]
    fn recent_runs_are_newest_first_deduped_and_capped() {
        let mut runs = vec![
            RecentRun {
                id: "old".into(),
                prompt: "first line\nrest".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                agent: "codex".into(),
                status: "merged".into(),
            },
            RecentRun {
                id: "new".into(),
                prompt: "  \n".into(),
                created_at: "2026-06-01T00:00:00Z".into(),
                agent: "grok".into(),
                status: "running".into(),
            },
            RecentRun {
                id: "new".into(),
                prompt: "duplicate".into(),
                created_at: "2026-05-01T00:00:00Z".into(),
                agent: "grok".into(),
                status: "running".into(),
            },
            RecentRun {
                id: " ".into(),
                prompt: "skip".into(),
                created_at: "2026-07-01T00:00:00Z".into(),
                agent: "pi".into(),
                status: "done".into(),
            },
            RecentRun {
                id: "undated".into(),
                prompt: "no stamp".into(),
                created_at: String::new(),
                agent: "claude".into(),
                status: "pending".into(),
            },
        ];
        for index in 0..8 {
            runs.push(RecentRun {
                id: format!("extra-{index}"),
                prompt: format!("extra {index}"),
                created_at: format!("2026-03-{index:02}T00:00:00Z"),
                agent: "cursor".into(),
                status: "merged".into(),
            });
        }
        let items = with_recent(&runs);
        let recent: Vec<&PaletteItem> = items
            .iter()
            .filter(|item| item.group == PaletteGroup::Recent)
            .collect();
        assert_eq!(recent[0].id, "recent.run:new");
        assert_eq!(recent[0].label, "new");
        assert!(
            recent
                .iter()
                .any(|item| item.id == "recent.run:old" && item.label == "first line")
        );
        assert!(recent.iter().all(|item| item.id != "recent.run:undated"));
        let overflow: Vec<RecentRun> = (0..RECENT_LIMIT + 4)
            .map(|index| RecentRun {
                id: format!("cap-{index}"),
                prompt: format!("cap {index}"),
                created_at: format!("2026-08-{index:02}T00:00:00Z"),
                agent: "cursor".into(),
                status: "merged".into(),
            })
            .chain(std::iter::once(RecentRun {
                id: "oldest".into(),
                prompt: "oldest".into(),
                created_at: "2024-01-01T00:00:00Z".into(),
                agent: "pi".into(),
                status: "merged".into(),
            }))
            .collect();
        let capped: Vec<PaletteItem> = with_recent(&overflow)
            .into_iter()
            .filter(|item| item.group == PaletteGroup::Recent)
            .collect();
        assert_eq!(capped.len(), RECENT_LIMIT);
        assert!(capped.iter().all(|item| item.id != "recent.run:oldest"));
        assert_eq!(
            effect_for("recent.run:new"),
            Some(CommandEffect::Navigate(Route::Run("new".into())))
        );
    }

    #[test]
    fn arrows_wrap_and_an_empty_list_does_not_move() {
        assert_eq!(step_index(None, 0, 1), None);
        assert_eq!(step_index(None, 3, 1), Some(0));
        assert_eq!(step_index(None, 3, -1), Some(2));
        assert_eq!(step_index(Some(2), 3, 1), Some(0));
        assert_eq!(step_index(Some(0), 3, -1), Some(2));
        assert_eq!(step_index(Some(1), 3, 0), None);
    }

    #[test]
    fn enter_runs_the_active_match() {
        let filtered = filter_items(&static_commands(), "roles");
        let active = step_index(None, filtered.len(), 1).unwrap();
        assert_eq!(filtered[active].id, "nav.roles");
        assert_eq!(
            effect_for(&filtered[active].id),
            Some(CommandEffect::Navigate(Route::Roles))
        );
        assert_eq!(
            effect_for("nav.models"),
            Some(CommandEffect::Navigate(Route::Intelligence))
        );
        assert_eq!(
            effect_for("nav.usage"),
            Some(CommandEffect::Navigate(Route::Spend))
        );
        assert_eq!(
            effect_for("nav.stats"),
            Some(CommandEffect::Navigate(Route::Stats))
        );
        assert_eq!(
            effect_for("nav.settings"),
            Some(CommandEffect::Navigate(Route::Settings(
                SettingsPage::Agents
            )))
        );
        assert_eq!(
            effect_for("nav.agent-setup"),
            Some(CommandEffect::Navigate(Route::Settings(
                SettingsPage::Agents
            )))
        );
        assert_eq!(effect_for("action.reload"), Some(CommandEffect::Reload));
        assert_eq!(effect_for("recent.run:"), None);
        assert_eq!(effect_for("nav.missing"), None);
    }

    #[test]
    fn highlight_sits_on_the_row_under_its_heading() {
        let y0 = highlight_y(&[2, 1], 0);
        assert_eq!(y0, TOP_PAD + GROUP_HEADER_H);
        let y1 = highlight_y(&[2, 1], 1);
        assert_eq!(y1, y0 + COMMAND_ROW_H);
        let y2 = highlight_y(&[2, 1], 2);
        assert_eq!(y2, y1 + COMMAND_ROW_H + GROUP_GAP + GROUP_HEADER_H);
    }

    #[test]
    fn query_editor_inserts_backspaces_by_grapheme_and_replaces_a_selection() {
        let mut editor = QueryEditor::new();
        editor.insert("in");
        editor.insert("box");
        assert_eq!(editor.text(), "inbox");
        editor.backspace();
        assert_eq!(editor.text(), "inbo");
        editor.insert("é");
        editor.backspace();
        assert_eq!(editor.text(), "inbo");
        editor.insert("a\nb");
        assert_eq!(editor.text(), "inboab");
        editor.select_all();
        assert!(editor.selected());
        editor.insert("Roles");
        assert_eq!(editor.text(), "Roles");
        assert!(!editor.selected());
        editor.move_to_start();
        editor.delete_forward();
        assert_eq!(editor.text(), "oles");
        editor.move_left();
        editor.move_right();
        assert_eq!(editor.cursor(), 1);
    }
}
