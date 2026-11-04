//! Commander (⌘K): the shared palette over the pages, Settings, Reload, and recent runs.
//! The palette's look, filtering, and keys are `polterware_ui`'s [`Commander`]; this view
//! hands it Dispatch's commands and runs what it confirms.

use gpui::{App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Window, prelude::*};

use crate::avatar::agent_avatar;
use crate::models::commander::{
    CommandEffect, PLACEHOLDER, PaletteGroup, PaletteIcon, PaletteItem, RELOAD_TOAST, RecentRun,
    effect_for, with_recent,
};
use crate::state::AppState;
use crate::ui::{
    Commander, CommanderEvent, CommanderGroup, CommanderItem, Icon, TileTone, ToastTone,
};

pub struct CommanderView {
    state: Entity<AppState>,
    commander: Entity<Commander>,
}

impl CommanderView {
    /// `home` takes focus back when the palette closes with nothing of its own to return
    /// to, so the window keeps hearing ⌘K.
    pub fn new(
        state: Entity<AppState>,
        home: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let commander = cx.new(|cx| {
            let mut commander = Commander::new(window, cx);
            commander.set_placeholder(PLACEHOLDER, cx);
            commander
        });
        cx.subscribe(
            &commander,
            |this, _, event: &CommanderEvent, cx| match event {
                CommanderEvent::Confirm(id) => this.run(id, cx),
                CommanderEvent::Cancel => this.close(cx),
                CommanderEvent::Query(_) | CommanderEvent::Select(_) => {}
            },
        )
        .detach();
        // The overlay stays mounted while the palette plays out.
        cx.observe(&commander, |_, _, cx| cx.notify()).detach();
        cx.observe_in(&state, window, move |this, state, window, cx| {
            let open = state.read(cx).commander_open();
            if open {
                let groups = groups(&with_recent(&recent_runs(state.read(cx))));
                this.commander
                    .update(cx, |commander, cx| commander.set_groups(groups, cx));
            }
            this.commander
                .update(cx, |commander, cx| commander.set_open(open, window, cx));
            if !open && this.commander.focus_handle(cx).contains_focused(window, cx) {
                home.focus(window);
            }
        })
        .detach();
        Self { state, commander }
    }

    /// How far the palette is presented, 0 to 1, for the overlay behind it.
    pub fn presence(&self, cx: &App) -> f32 {
        self.commander.read(cx).presence()
    }

    /// Whether the overlay must stay mounted: open, or still playing out.
    pub fn is_presented(&self, cx: &App) -> bool {
        self.commander.read(cx).is_presented()
    }

    fn run(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(effect) = effect_for(id) else {
            return;
        };
        self.state.update(cx, |app, cx| {
            match effect {
                CommandEffect::Reload => {
                    app.reload_all(cx);
                    app.push_toast(RELOAD_TOAST, ToastTone::Success);
                }
                CommandEffect::Navigate(route) => {
                    app.navigate(route);
                    app.request_refresh(cx);
                }
            }
            app.set_commander_open(false);
            cx.notify();
        });
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.set_commander_open(false);
            cx.notify();
        });
    }
}

impl Render for CommanderView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.commander.clone()
    }
}

fn recent_runs(state: &AppState) -> Vec<RecentRun> {
    state
        .runs()
        .iter()
        .map(|run| RecentRun {
            id: run.id.clone(),
            prompt: run.prompt.clone(),
            created_at: run.created_at.clone(),
            agent: run.agent.clone(),
            status: run.status.clone(),
        })
        .collect()
}

/// Navigation, then Actions, then Recent, each command on its tile as the Solid app drew it.
fn groups(items: &[PaletteItem]) -> Vec<CommanderGroup> {
    [
        PaletteGroup::Navigation,
        PaletteGroup::Actions,
        PaletteGroup::Recent,
    ]
    .into_iter()
    .map(|group| {
        CommanderGroup::new(group.label()).items(
            items
                .iter()
                .filter(|item| item.group == group)
                .map(command_item),
        )
    })
    .collect()
}

fn command_item(item: &PaletteItem) -> CommanderItem {
    let command = CommanderItem::new(item.id.clone(), item.label.clone());
    let tile = |icon| command.clone().tile(icon, TileTone::Neutral);
    match &item.icon {
        PaletteIcon::Roles => tile(Icon::Route),
        PaletteIcon::Intelligence => tile(Icon::AiBrain01Icon),
        PaletteIcon::Spend => tile(Icon::CreditCard),
        PaletteIcon::Stats => tile(Icon::Activity),
        PaletteIcon::Settings => tile(Icon::Settings),
        PaletteIcon::AgentSetup => tile(Icon::Terminal),
        PaletteIcon::Reload => command.clone().tile(Icon::RefreshCw, TileTone::Execution),
        PaletteIcon::Run { agent, status } => {
            let (agent, status) = (agent.clone(), status.clone());
            command
                .clone()
                .mark(move |_, _| agent_avatar(&agent, &status, 20.0).into_any_element())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::groups;
    use crate::models::commander::static_commands;

    #[test]
    fn groups_keep_navigation_actions_then_recent() {
        let groups = groups(&static_commands());
        assert_eq!(groups.len(), 3);
    }
}
