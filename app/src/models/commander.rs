//! Dispatch's commands for Commander (⌘K): each page, Settings, Agent setup, Reload, and
//! recent runs, and what each one does. Filtering, the highlight, and the keys are the shared
//! `polterware_ui::Commander`'s.

use std::collections::HashSet;

use crate::format::prompt_first_line;
use crate::state::{Route, SettingsPage};

/// How many runs the Recent group keeps. Newest first.
pub const RECENT_LIMIT: usize = 12;

pub const PLACEHOLDER: &str = "Go to a page or run a command…";
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

#[cfg(test)]
mod tests {
    use super::{
        CommandEffect, PLACEHOLDER, PaletteGroup, PaletteItem, RECENT_LIMIT, RELOAD_TOAST,
        RecentRun, effect_for, static_commands, with_recent,
    };
    use crate::state::{Route, SettingsPage};

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
    fn each_command_has_its_effect() {
        assert_eq!(
            effect_for("nav.roles"),
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
}
