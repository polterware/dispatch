//! What can be done to a repository, from the new run's repository picker (its row's ⋯ or a
//! right click) and from the sidebar's repository group menu alike: one list and one set of
//! effects, so the two never drift. Reveal in Finder, Copy path, Open in the first editor and
//! terminal installed, then, set apart, Remove repository, which asks first because it deletes
//! the repository's runs and the worktrees Dispatch cut for them.

use std::path::Path;
use std::process::Command;
use std::rc::Rc;

use gpui::{App, ClipboardItem, Entity, SharedString, Window};

use crate::client::RemoveWorkspaceRequest;
use crate::models::sidebar::removable_repository;
use crate::state::AppState;
use crate::ui::{MenuEntry, MenuItem, PropertyOption, ToastTone};

pub const REVEAL: &str = "repository-reveal";
pub const COPY_PATH: &str = "repository-copy-path";
pub const OPEN_EDITOR: &str = "repository-open-editor";
pub const OPEN_TERMINAL: &str = "repository-open-terminal";
pub const REMOVE: &str = "repository-remove";

/// Editors tried in order for Open in editor; the first installed one opens the folder.
const EDITORS: &[&str] = &[
    "Cursor",
    "Visual Studio Code",
    "Zed",
    "Windsurf",
    "Sublime Text",
    "Nova",
];

/// Terminals tried in order for Open in terminal; Terminal always exists on macOS.
const TERMINALS: &[&str] = &["Ghostty", "iTerm", "Warp", "WezTerm", "Terminal"];

/// A repository an action works on: its folder, its name, and the workspace it was added as,
/// which only Remove needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub dir: String,
    pub name: String,
    pub workspace_id: Option<String>,
}

impl Repository {
    /// The repository at `dir`, removable when a standalone workspace was added for it.
    pub fn at(dir: &str, name: &str, state: &AppState) -> Self {
        Self {
            dir: dir.to_string(),
            name: name.to_string(),
            workspace_id: removable_repository(dir, state.workspaces()),
        }
    }
}

/// One entry of the list: its id, its label, whether it reads as danger, and whether a rule
/// sets it apart from the entries before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub id: &'static str,
    pub label: String,
    pub danger: bool,
    pub rule_before: bool,
}

/// The actions `repository` offers, in order. Open in editor appears only when one of
/// [`EDITORS`] is installed, and Remove only when the repository can be removed.
pub fn actions(repository: &Repository) -> Vec<Action> {
    let plain = |id, label: String| Action {
        id,
        label,
        danger: false,
        rule_before: false,
    };
    let mut list = vec![
        plain(REVEAL, "Reveal in Finder".into()),
        plain(COPY_PATH, "Copy path".into()),
    ];
    if let Some(editor) = installed(EDITORS) {
        list.push(plain(OPEN_EDITOR, format!("Open in {editor}")));
    }
    if let Some(terminal) = installed(TERMINALS) {
        list.push(plain(OPEN_TERMINAL, format!("Open in {terminal}")));
    }
    if repository.workspace_id.is_some() {
        list.push(Action {
            id: REMOVE,
            label: "Remove repository".into(),
            danger: true,
            rule_before: true,
        });
    }
    list
}

/// `option` with the actions as its row menu.
pub fn with_actions(mut option: PropertyOption, repository: &Repository) -> PropertyOption {
    for action in actions(repository) {
        if action.rule_before {
            option = option.row_rule();
        }
        option = option.row_action(action.id, action.label, action.danger);
    }
    option
}

/// The actions as a menu's rows, each running [`perform`].
pub fn menu_items(repository: &Repository, state: &Entity<AppState>) -> Vec<MenuItem> {
    let mut items = Vec::new();
    for action in actions(repository) {
        if action.rule_before {
            items.push(MenuItem::Separator);
        }
        let (repository, state, id) = (repository.clone(), state.clone(), action.id);
        items.push(MenuItem::Action(MenuEntry {
            label: action.label.into(),
            icon: None,
            danger: action.danger,
            on_select: Rc::new(move |_: &mut Window, cx: &mut App| {
                perform(id, &repository, &state, cx);
            }),
        }));
    }
    items
}

/// Runs the action named `id` on `repository`. Remove asks first.
pub fn perform(id: &str, repository: &Repository, state: &Entity<AppState>, cx: &mut App) {
    let dir = repository.dir.clone();
    let opened = match id {
        REVEAL => open(&["-R", &dir]),
        COPY_PATH => {
            cx.write_to_clipboard(ClipboardItem::new_string(dir.clone()));
            Ok(())
        }
        OPEN_EDITOR => match installed(EDITORS) {
            Some(app) => open(&["-a", app, &dir]),
            None => Err("No editor found".into()),
        },
        OPEN_TERMINAL => match installed(TERMINALS) {
            Some(app) => open(&["-a", app, &dir]),
            None => Err("No terminal found".into()),
        },
        REMOVE => {
            state.update(cx, |app, cx| {
                app.ask_remove_repository(repository.clone());
                cx.notify();
            });
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(message) = opened {
        state.update(cx, |app, cx| {
            app.push_toast(message, ToastTone::Danger);
            cx.notify();
        });
    }
}

/// Removes the repository the app asked about: the core forgets its workspace and deletes
/// its runs and their worktrees (refusing while one still works).
pub fn remove(repository: Repository, state: Entity<AppState>, cx: &mut App) {
    let Some(workspace_id) = repository.workspace_id.clone() else {
        return;
    };
    let Some(client) = state.read(cx).client() else {
        state.update(cx, |app, cx| {
            app.push_toast("Dispatch core is not running", ToastTone::Danger);
            cx.notify();
        });
        return;
    };
    let request = RemoveWorkspaceRequest {
        id: workspace_id.clone(),
    };
    let job = cx
        .background_executor()
        .spawn(async move { client.remove_workspace(&request) });
    cx.spawn(async move |cx| {
        let result = job.await;
        let _ = state.update(cx, |app, cx| {
            match result {
                Ok(response) => {
                    let deleted = match response.deleted_runs {
                        0 => String::new(),
                        1 => " and its run".to_string(),
                        n => format!(" and its {n} runs"),
                    };
                    app.forget_workspace(&workspace_id);
                    app.forget_workspace_runs(&workspace_id, &repository.dir);
                    app.push_toast(
                        format!("Removed {}{deleted}", repository.name),
                        ToastTone::Success,
                    );
                    app.request_refresh(cx);
                }
                Err(error) => {
                    app.push_toast(super::core_message(error), ToastTone::Danger);
                }
            }
            cx.notify();
        });
    })
    .detach();
}

/// The first of `apps` installed in /Applications, ~/Applications, or the system's.
fn installed(apps: &[&'static str]) -> Option<&'static str> {
    let home = std::env::var("HOME").unwrap_or_default();
    let roots = [
        "/Applications".to_string(),
        format!("{home}/Applications"),
        "/System/Applications/Utilities".to_string(),
    ];
    apps.iter().copied().find(|app| {
        roots
            .iter()
            .any(|root| Path::new(root).join(format!("{app}.app")).exists())
    })
}

fn open(args: &[&str]) -> Result<(), SharedString> {
    match Command::new("/usr/bin/open").args(args).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(_) | Err(_) => Err("Could not open the repository".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository(workspace: Option<&str>) -> Repository {
        Repository {
            dir: "/tmp/repo".into(),
            name: "repo".into(),
            workspace_id: workspace.map(str::to_string),
        }
    }

    #[test]
    fn remove_comes_last_behind_a_rule() {
        let list = actions(&repository(Some("ws-1")));
        assert_eq!(list[0].id, REVEAL);
        assert_eq!(list[1].id, COPY_PATH);
        let last = list.last().expect("actions");
        assert_eq!(last.id, REMOVE);
        assert!(last.danger && last.rule_before);
        assert!(
            list[..list.len() - 1]
                .iter()
                .all(|a| !a.danger && !a.rule_before)
        );
    }

    #[test]
    fn a_repository_no_workspace_holds_cannot_be_removed() {
        assert!(actions(&repository(None)).iter().all(|a| a.id != REMOVE));
    }
}
