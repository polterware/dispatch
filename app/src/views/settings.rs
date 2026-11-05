//! Settings: one page per concern, with a rail.
//!
//! Agent setup, routing policy, classifier keys, and the key's test classify all go
//! through `CoreClient` on the background executor. Key values are never copied
//! into a label, a toast, or the view's own fields.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, MouseButton, Render, SharedString,
    WeakEntity, Window, div, prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::client::{
    ApplyAgentSetupRequest, ClassifyRequest, ConfigRequest, ConfigResponse, CoreClient, CoreError,
    GetAgentSetupRequest, ListAgentsRequest, ListModelsRequest, at_once,
};
use crate::models::settings::{
    CLASSIFIERS, CliState, Dimension, InstallState, ListedModel, Policy, RuleSide, SAMPLE_TASK,
    agent_label, bool_wire, classifier_trailing, classify_meta, cli_description, draft_ready,
    env_key_sentence, fixed2, folder_agents_label, hook_note, hook_points_elsewhere, key_chip,
    key_placeholder, key_test_enabled, key_uses_replace, keyed_classifier, model_agent,
    opencode_description, parse_agent_setup, parse_classify, parse_cli_model, parse_key_map,
    parse_key_status_value, parse_policy, policy_options, priority_label, priority_value,
    prob_line, radio_value, role_answer_line, skill_readers, skills_need_install, token_delta,
    trimmed_draft, with_token, without_token,
};
use crate::motion::{Animator, Spring, SpringTier, SpringValue};
use crate::state::{AppState, Route, SettingsPage, ToastTone};
use crate::theme::{StatusTone, Theme};
use crate::ui::parts::settings::{radio_mark, settings_row, settings_section, wash};
use crate::ui::patterns::{
    AnchorSlot, SETTINGS_SECTION_GAP, SettingsNavItem, settings_dialog, window_anchor,
};
use crate::ui::{
    ButtonKind, Icon, IconTile, Menu, MenuItem, Popover, Switch, TextInput, TileTone, button, chip,
    icon, icon_button,
};
use crate::ui::{SkeletonLayout, page_skeleton};

/// A token option's leading mark: an agent's bot or a provider's tile.
const TOKEN_MARK: f32 = 20.0;
/// Wide enough for `cursor-grok-4.5-high-fast` beside its mark.
const TOKEN_PANEL_WIDTH: f32 = 280.0;
const RAIL_ROW: f32 = 32.0;
const RAIL_GAP: f32 = 2.0;
/// The editor's height before a frame has measured it: one row of controls.
const EDITOR_HEIGHT: f32 = 44.0;
/// The token picker's trigger, the row's "+" icon button, which the picker lines up with.
const TOKEN_TRIGGER: f32 = 28.0;

const RAIL: [(SettingsPage, &str, Icon); 4] = [
    (SettingsPage::Agents, "Agents", Icon::Settings),
    (SettingsPage::Classifier, "Classifier", Icon::AiBrain01Icon),
    (SettingsPage::PickRole, "Pick a role", Icon::Route),
    (SettingsPage::RoutingPolicy, "Routing policy", Icon::Sliders),
];

#[derive(Clone, Copy, PartialEq, Eq)]
struct TokenSlot {
    side: RuleSide,
    dimension: Dimension,
}

pub struct SettingsView {
    state: Entity<AppState>,
    started: bool,
    epoch: u64,
    policy: Option<Policy>,
    policy_ticket: u64,
    keys: BTreeMap<String, String>,
    keys_ready: bool,
    keys_inflight: bool,
    cli_model: Option<crate::models::settings::CliModel>,
    agents: Vec<String>,
    models: Vec<ListedModel>,
    setup: Option<crate::models::settings::AgentSetup>,
    setup_ticket: u64,
    busy: String,
    busy_ticket: u64,
    classify_report: Option<crate::models::settings::ClassifyView>,
    jev_key: Entity<TextInput>,
    openai_key: Entity<TextInput>,
    jev_placeholder: &'static str,
    openai_placeholder: &'static str,
    token_draft: Entity<TextInput>,
    /// Each agent's session hook switch, made when the setup first lists the agent.
    hooks: BTreeMap<String, Entity<Switch>>,
    opencode: Entity<Switch>,
    roles: Entity<Switch>,
    discover: Entity<Switch>,
    priority_menu: Entity<Menu>,
    priority_anchor: AnchorSlot,
    token_popover: Entity<Popover>,
    token_anchor: AnchorSlot,
    token_slot: Option<TokenSlot>,
    /// The agent and model the CLI classifier asks, picked from what the CLIs list.
    cli_popover: Entity<Popover>,
    cli_anchor: AnchorSlot,
    cli_filter: Entity<TextInput>,
    /// The open classifier editor's content, as the last frame laid it out.
    editor_measure: AnchorSlot,
    /// How far the dialog is in, 0 to 1.
    open: SpringValue,
    option_hover: [SpringValue; 4],
    option_press: [SpringValue; 4],
    enter: SpringValue,
    editor: Spring,
    editor_slot: Option<usize>,
    result_enter: SpringValue,
    shown: Option<SettingsPage>,
    animator: Animator,
}

impl SettingsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let view = cx.entity().downgrade();
        let token_draft = token_input(view.clone(), cx);
        let jev_key = key_input("jev", "Paste a key", view.clone(), cx);
        let openai_key = key_input("openai", "Paste a key", view.clone(), cx);
        let opencode = switch(true, view.clone(), cx, |this, on, cx| {
            this.on_opencode(on, cx)
        });
        let roles = switch(true, view.clone(), cx, |this, on, cx| this.on_roles(on, cx));
        let discover = switch(true, view.clone(), cx, |this, on, cx| {
            this.on_discover(on, cx)
        });
        let priority_menu = cx.new(Menu::new);
        let token_popover = cx.new(Popover::new);
        let panel_view = view.clone();
        token_popover.update(cx, |popover, _| {
            popover.set_panel(Rc::new(move |_window, app| token_panel(&panel_view, app)));
            // The "+" ends its row, so the picker grows back over the row.
            popover.set_align_end(Some(TOKEN_TRIGGER));
        });
        let cli_filter = cli_filter_input(view.clone(), cx);
        let cli_popover = cx.new(Popover::new);
        let cli_view = view.clone();
        cli_popover.update(cx, |popover, _| {
            popover.set_panel(Rc::new(move |_window, app| cli_panel(&cli_view, app)));
        });
        cx.observe(&state, |this, _, cx| {
            this.on_app_state(cx);
            cx.notify();
        })
        .detach();
        watch_input(&jev_key, cx);
        watch_input(&openai_key, cx);
        watch_input(&token_draft, cx);
        watch_input(&cli_filter, cx);
        let mut this = Self {
            state,
            started: false,
            epoch: 0,
            policy: None,
            policy_ticket: 0,
            keys: BTreeMap::new(),
            keys_ready: false,
            keys_inflight: false,
            cli_model: None,
            agents: Vec::new(),
            models: Vec::new(),
            setup: None,
            setup_ticket: 0,
            busy: String::new(),
            busy_ticket: 0,
            classify_report: None,
            jev_key,
            openai_key,
            jev_placeholder: "Paste a key",
            openai_placeholder: "Paste a key",
            token_draft,
            hooks: BTreeMap::new(),
            opencode,
            roles,
            discover,
            priority_menu,
            priority_anchor: AnchorSlot::default(),
            token_popover,
            token_anchor: AnchorSlot::default(),
            token_slot: None,
            cli_popover,
            cli_anchor: AnchorSlot::default(),
            cli_filter,
            editor_measure: AnchorSlot::default(),
            open: SpringValue::new(0.0, SpringTier::Slow),
            option_hover: [SpringValue::new(0.0, SpringTier::Fast); 4],
            option_press: [SpringValue::new(0.0, SpringTier::Fast); 4],
            enter: SpringValue::new(0.0, SpringTier::Moderate),
            editor: Spring::collapse(0.0),
            editor_slot: None,
            result_enter: SpringValue::new(0.0, SpringTier::Moderate),
            shown: None,
            animator: Animator::new(),
        };
        this.on_app_state(cx);
        this
    }

    fn on_app_state(&mut self, cx: &mut Context<Self>) {
        let mut reload_policy = false;
        let online = self.client(cx).is_some();
        if !online {
            if self.started {
                self.started = false;
                self.keys_ready = false;
                self.keys_inflight = false;
                self.epoch = self.epoch.wrapping_add(1);
            }
        } else if !self.started {
            self.started = true;
            self.epoch = self.epoch.wrapping_add(1);
            self.load_lists(cx);
            self.load_setup(cx);
            reload_policy = true;
        }
        if reload_policy && online {
            self.load_policy(cx);
        }
        if online
            && matches!(self.page(cx), SettingsPage::Classifier)
            && !self.keys_ready
            && !self.keys_inflight
        {
            self.load_keys(cx);
        }
    }

    /// The open section; while the dialog plays out, the one it showed.
    fn page(&self, cx: &App) -> SettingsPage {
        match self.state.read(cx).route() {
            Route::Settings(page) => page.clone(),
            _ => self.shown.clone().unwrap_or(SettingsPage::Agents),
        }
    }

    /// How far the dialog is presented, 0 to 1, for the scrim behind it.
    pub fn presence(&self) -> f32 {
        self.open.value().clamp(0.0, 1.0)
    }

    /// Whether the dialog must stay mounted: open, or still playing out.
    pub fn is_presented(&self, cx: &App) -> bool {
        matches!(self.state.read(cx).route(), Route::Settings(_))
            || !self.open.settled()
            || self.open.value() > 0.001
    }

    fn client(&self, cx: &App) -> Option<CoreClient> {
        self.state.read(cx).client()
    }

    fn busy(&self) -> bool {
        !self.busy.is_empty()
    }

    fn classifier_on(&self) -> bool {
        self.policy.as_ref().is_some_and(Policy::classifier_on)
    }

    fn toast(&self, message: impl Into<SharedString>, tone: ToastTone, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.push_toast(message, tone);
            cx.notify();
        });
    }

    fn key_status(&self, name: &str) -> String {
        self.keys
            .get(name)
            .cloned()
            .unwrap_or_else(|| "missing".to_string())
    }

    fn key_field(&self, name: &str) -> Entity<TextInput> {
        if name == "openai" {
            self.openai_key.clone()
        } else {
            self.jev_key.clone()
        }
    }

    fn placeholder_of(&self, name: &str) -> &'static str {
        if name == "openai" {
            self.openai_placeholder
        } else {
            self.jev_placeholder
        }
    }

    fn set_placeholder(&mut self, name: &str, placeholder: &'static str) {
        if name == "openai" {
            self.openai_placeholder = placeholder;
        } else {
            self.jev_placeholder = placeholder;
        }
    }

    fn refresh_key_field(&mut self, name: &'static str, cx: &mut Context<Self>) {
        let placeholder = key_placeholder(&self.key_status(name));
        let empty = self.key_field(name).read(cx).text().trim().is_empty();
        if !empty || self.placeholder_of(name) == placeholder {
            return;
        }
        let replacement = key_input(name, placeholder, cx.entity().downgrade(), cx);
        watch_input(&replacement, cx);
        if name == "openai" {
            self.openai_key = replacement;
        } else {
            self.jev_key = replacement;
        }
        self.set_placeholder(name, placeholder);
    }

    fn sync_policy_switches(&mut self, cx: &mut Context<Self>) {
        if let Some(policy) = self.policy.clone() {
            self.roles
                .update(cx, |switch, _| switch.set_on(policy.roles));
            self.discover
                .update(cx, |switch, _| switch.set_on(policy.discover));
        }
    }

    fn sync_setup_switches(&mut self, cx: &mut Context<Self>) {
        if let Some(setup) = self.setup.clone() {
            let view = cx.entity().downgrade();
            for hook in &setup.hooks {
                let agent = hook.agent.clone();
                let toggle = self
                    .hooks
                    .entry(agent.clone())
                    .or_insert_with(|| {
                        switch(hook.on, view.clone(), cx, move |this, on, cx| {
                            this.on_hook(&agent, on, cx)
                        })
                    })
                    .clone();
                toggle.update(cx, |switch, _| switch.set_on(hook.on));
            }
            self.opencode
                .update(cx, |switch, _| switch.set_on(setup.opencode.on));
        }
    }

    fn apply_policy(&mut self, policy: Policy, cx: &mut Context<Self>) {
        let roles = policy.roles;
        let discover = policy.discover;
        self.policy = Some(policy);
        self.roles.update(cx, |switch, _| switch.set_on(roles));
        self.discover
            .update(cx, |switch, _| switch.set_on(discover));
    }

    fn load_lists(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_spawn(async move {
                    at_once(
                        || client.list_agents(&ListAgentsRequest::default()).ok(),
                        || client.list_models(&ListModelsRequest::default()).ok(),
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                if let Some(response) = listed.0 {
                    view.agents = response
                        .agents
                        .into_iter()
                        .map(|agent| agent.name)
                        .filter(|name| !name.is_empty())
                        .collect();
                }
                if let Some(response) = listed.1 {
                    view.models = response
                        .models
                        .into_iter()
                        .map(|model| ListedModel {
                            agent: model.agent,
                            model: model.model,
                            provider: model.provider,
                        })
                        .collect();
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_setup(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.setup_ticket = self.setup_ticket.wrapping_add(1);
        let ticket = self.setup_ticket;
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client
                        .get_agent_setup(&GetAgentSetupRequest::default())
                        .map(|response| response.json)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch || view.setup_ticket != ticket {
                    return;
                }
                match result {
                    Ok(json) => {
                        view.setup = Some(parse_agent_setup(&json));
                        view.sync_setup_switches(cx);
                    }
                    Err(err) => view.toast(
                        core_message(&err, "Could not read the agent setup."),
                        ToastTone::Danger,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_policy(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.policy_ticket = self.policy_ticket.wrapping_add(1);
        let ticket = self.policy_ticket;
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        ..Default::default()
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch || view.policy_ticket != ticket {
                    return;
                }
                match result {
                    Ok(response) => view.apply_policy(parse_policy(&response.json), cx),
                    Err(err) => {
                        view.policy = None;
                        view.toast(
                            core_message(&err, "Could not read the routing policy."),
                            ToastTone::Danger,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_keys(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.keys_inflight = true;
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let keys = client.config(&ConfigRequest {
                        action: "get".to_string(),
                        key: "classifier_keys".to_string(),
                        ..Default::default()
                    });
                    let cli = client.config(&ConfigRequest {
                        action: "get".to_string(),
                        key: "classifier_cli".to_string(),
                        ..Default::default()
                    });
                    (keys, cli)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.keys_inflight = false;
                if view.epoch != epoch {
                    return;
                }
                view.keys_ready = true;
                match result.0 {
                    Ok(response) => view.keys = parse_key_map(&response.json),
                    Err(err) => view.toast(
                        core_message(&err, "Could not read the classifier keys."),
                        ToastTone::Danger,
                        cx,
                    ),
                }
                if let Ok(response) = result.1 {
                    view.cli_model = Some(parse_cli_model(&response.json));
                }
                view.refresh_key_field("jev", cx);
                view.refresh_key_field("openai", cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_setup(
        &mut self,
        key: &str,
        action: &str,
        agent: &str,
        done: &str,
        cx: &mut Context<Self>,
    ) {
        if self.busy() {
            return;
        }
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            self.sync_setup_switches(cx);
            return;
        };
        self.busy = key.to_string();
        self.busy_ticket = self.busy_ticket.wrapping_add(1);
        self.setup_ticket = self.setup_ticket.wrapping_add(1);
        let busy_ticket = self.busy_ticket;
        let setup_ticket = self.setup_ticket;
        let epoch = self.epoch;
        let action = action.to_string();
        let agent = agent.to_string();
        let done = done.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client
                        .apply_agent_setup(&ApplyAgentSetupRequest { action, agent })
                        .map(|response| response.json)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                if view.busy_ticket == busy_ticket {
                    view.busy.clear();
                }
                if view.setup_ticket != setup_ticket {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(json) => {
                        view.setup = Some(parse_agent_setup(&json));
                        view.sync_setup_switches(cx);
                        view.toast(done, ToastTone::Success, cx);
                    }
                    Err(err) => {
                        view.sync_setup_switches(cx);
                        view.toast(
                            core_message(&err, "Could not change the agent setup."),
                            ToastTone::Danger,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn write_values(&mut self, key: &str, values: Vec<String>, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            self.sync_policy_switches(cx);
            return;
        };
        let previous = self.policy.clone();
        if let Some(policy) = self.policy.as_mut() {
            match key {
                "priority" => {
                    policy.priority = values.first().cloned().unwrap_or_default();
                }
                "discover" => {
                    policy.discover = values.first().is_some_and(|value| value == "true");
                }
                "roles" => {
                    policy.roles = values.first().is_some_and(|value| value == "true");
                }
                "classifier" => {
                    let name = values.first().map(String::as_str).unwrap_or("");
                    policy.classifier = if name == "off" {
                        String::new()
                    } else {
                        name.to_string()
                    };
                }
                _ => {}
            }
        }
        self.policy_ticket = self.policy_ticket.wrapping_add(1);
        let ticket = self.policy_ticket;
        let epoch = self.epoch;
        let key = key.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.config(&ConfigRequest {
                        action: "set".to_string(),
                        key,
                        values,
                    })?;
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        ..Default::default()
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch || view.policy_ticket != ticket {
                    return;
                }
                match result {
                    Ok(response) => view.apply_policy(parse_policy(&response.json), cx),
                    Err(err) => {
                        view.policy = previous;
                        view.sync_policy_switches(cx);
                        view.toast(
                            core_message(&err, "Could not save the routing policy."),
                            ToastTone::Danger,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn write_rule_delta(
        &mut self,
        side: RuleSide,
        dimension: Dimension,
        added: Vec<String>,
        removed: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        };
        let previous = self.policy.clone();
        self.policy_ticket = self.policy_ticket.wrapping_add(1);
        let ticket = self.policy_ticket;
        let epoch = self.epoch;
        let key = dimension.key().to_string();
        cx.spawn(async move |this, cx| {
            let result: Result<ConfigResponse, CoreError> = cx
                .background_spawn(async move {
                    if !added.is_empty() {
                        client.config(&ConfigRequest {
                            action: side.action("add"),
                            key: key.clone(),
                            values: added,
                        })?;
                    }
                    if !removed.is_empty() {
                        client.config(&ConfigRequest {
                            action: side.action("rm"),
                            key: key.clone(),
                            values: removed,
                        })?;
                    }
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        ..Default::default()
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch || view.policy_ticket != ticket {
                    return;
                }
                match result {
                    Ok(response) => view.apply_policy(parse_policy(&response.json), cx),
                    Err(err) => {
                        view.policy = previous;
                        view.toast(
                            core_message(&err, "Could not save the routing policy."),
                            ToastTone::Danger,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn change_tokens(
        &mut self,
        side: RuleSide,
        dimension: Dimension,
        next: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(policy) = self.policy.clone() else {
            return;
        };
        let previous = policy.rules(side).get(dimension).to_vec();
        let (added, removed) = token_delta(&previous, &next);
        if added.is_empty() && removed.is_empty() {
            return;
        }
        if let Some(policy) = self.policy.as_mut() {
            *policy.rules_mut(side).get_mut(dimension) = next;
        }
        self.write_rule_delta(side, dimension, added, removed, cx);
    }

    fn add_token(&mut self, token: String, cx: &mut Context<Self>) {
        let token = trimmed_draft(&token);
        if !draft_ready(&token) {
            return;
        }
        let Some(slot) = self.token_slot else { return };
        let Some(policy) = self.policy.clone() else {
            return;
        };
        let next = with_token(policy.rules(slot.side).get(slot.dimension), &token);
        self.change_tokens(slot.side, slot.dimension, next, cx);
        self.token_draft
            .update(cx, |input, cx| input.set_text("", cx));
        self.token_popover.update(cx, |popover, cx| {
            popover.set_open(false);
            cx.notify();
        });
    }

    fn remove_token(
        &mut self,
        side: RuleSide,
        dimension: Dimension,
        token: String,
        cx: &mut Context<Self>,
    ) {
        let Some(policy) = self.policy.clone() else {
            return;
        };
        let next = without_token(policy.rules(side).get(dimension), &token);
        self.change_tokens(side, dimension, next, cx);
    }

    fn write_key(
        &mut self,
        name: &str,
        action: &str,
        secret: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        };
        let values = if action == "key-set" {
            let Some(secret) = secret.and_then(|value| {
                let trimmed = trimmed_draft(&value);
                draft_ready(&trimmed).then_some(trimmed)
            }) else {
                return;
            };
            vec![secret]
        } else {
            Vec::new()
        };
        let saving = action == "key-set";
        let name = name.to_string();
        let action = action.to_string();
        let epoch = self.epoch;
        let key_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.config(&ConfigRequest {
                        action,
                        key: key_name,
                        values,
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                match result {
                    Ok(response) => {
                        let status = parse_key_status_value(&response.json);
                        view.keys.insert(name.clone(), status);
                        view.key_field(&name)
                            .update(cx, |field, cx| field.set_text("", cx));
                        let field = if name == "openai" { "openai" } else { "jev" };
                        view.refresh_key_field(field, cx);
                        view.toast(
                            if saving {
                                "Key saved in the Keychain."
                            } else {
                                "Key removed from the Keychain."
                            },
                            ToastTone::Success,
                            cx,
                        );
                    }
                    Err(err) => view.toast(
                        core_message(
                            &err,
                            if saving {
                                "Could not save the key."
                            } else {
                                "Could not remove the key."
                            },
                        ),
                        ToastTone::Danger,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn submit_key(&mut self, name: &str, draft: String, cx: &mut Context<Self>) {
        self.write_key(name, "key-set", Some(draft), cx);
    }

    fn submit_key_field(&mut self, name: &str, cx: &mut Context<Self>) {
        let draft = trimmed_draft(self.key_field(name).read(cx).text());
        self.submit_key(name, draft, cx);
    }

    fn classify_text(&mut self, prompt: String, cx: &mut Context<Self>) {
        if !draft_ready(&prompt) {
            return;
        }
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        };
        let request = ClassifyRequest {
            prompt,
            ..Default::default()
        };
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.classify(&request) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                match result {
                    Ok(response) => {
                        view.classify_report = Some(parse_classify(&response.json));
                        view.result_enter.snap(0.0);
                        view.result_enter.set_target(1.0);
                    }
                    Err(err) => view.toast(
                        core_message(&err, "Could not classify."),
                        ToastTone::Danger,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_hook(&mut self, agent: &str, on: bool, cx: &mut Context<Self>) {
        if self.busy() {
            if let Some(toggle) = self.hooks.get(agent) {
                toggle.update(cx, |switch, _| switch.set_on(!on));
            }
            cx.notify();
            return;
        }
        let name = agent_label(agent);
        let (action, done) = if on {
            ("hook_on", format!("{name} hook on."))
        } else {
            ("hook_off", format!("{name} hook off."))
        };
        self.apply_setup(&format!("hook:{agent}"), action, agent, &done, cx);
    }

    fn on_opencode(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.busy() {
            self.opencode.update(cx, |switch, _| switch.set_on(!on));
            cx.notify();
            return;
        }
        let (action, done) = if on {
            ("opencode_on", "OpenCode delegates through dispatch.")
        } else {
            ("opencode_off", "OpenCode uses its own subagents.")
        };
        self.apply_setup("opencode", action, "", done, cx);
    }

    fn on_roles(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.client(cx).is_none() {
            self.roles.update(cx, |switch, _| switch.set_on(!on));
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        }
        self.write_values("roles", vec![bool_wire(on)], cx);
    }

    fn on_discover(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.client(cx).is_none() {
            self.discover.update(cx, |switch, _| switch.set_on(!on));
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        }
        self.write_values("discover", vec![bool_wire(on)], cx);
    }

    /// For the screenshot tour: opens Priority (0), the Allow models picker (1), picks the
    /// agent CLI classifier (2), or opens its model picker (3).
    pub(crate) fn tour_open(&mut self, which: usize, cx: &mut Context<Self>) {
        match which {
            0 => self.open_priority(cx),
            1 => self.open_tokens(RuleSide::Allow, Dimension::Models, cx),
            2 => self.choose_classifier("cli", cx),
            _ => self.open_cli_models(cx),
        }
        cx.notify();
    }

    /// For the screenshot tour: closes what [`Self::tour_open`] opened.
    pub(crate) fn tour_close(&mut self, cx: &mut Context<Self>) {
        self.priority_menu.update(cx, |menu, cx| {
            menu.set_open(false);
            cx.notify();
        });
        for popover in [&self.token_popover, &self.cli_popover] {
            popover.update(cx, |popover, cx| {
                popover.set_open(false);
                cx.notify();
            });
        }
        self.token_slot = None;
        cx.notify();
    }

    fn open_cli_models(&mut self, cx: &mut Context<Self>) {
        self.cli_filter
            .update(cx, |input, cx| input.set_text("", cx));
        self.cli_popover.update(cx, |popover, cx| {
            popover.set_open(true);
            cx.notify();
        });
    }

    /// The agent and model pairs the CLIs here list, as `agent/model`, that hold `filter`.
    fn cli_options(&self, filter: &str) -> Vec<(String, String)> {
        let filter = filter.trim().to_lowercase();
        let mut options: Vec<(String, String)> = self
            .models
            .iter()
            .filter(|listed| !listed.agent.is_empty() && !listed.model.is_empty())
            .map(|listed| (listed.agent.clone(), listed.model.clone()))
            .filter(|(agent, model)| {
                filter.is_empty() || format!("{agent} {model}").to_lowercase().contains(&filter)
            })
            .collect();
        options.sort();
        options.dedup();
        options
    }

    /// Picks the first model the filter holds, or the `agent/model` typed.
    fn submit_cli_filter(&mut self, text: String, cx: &mut Context<Self>) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let pick = self
            .cli_options(&text)
            .into_iter()
            .next()
            .map(|(agent, model)| format!("{agent}/{model}"))
            .or_else(|| text.contains('/').then_some(text));
        if let Some(pick) = pick {
            self.pick_cli_model(Some(pick), cx);
        }
    }

    /// Has the CLI classifier ask `agent/model`, or the easy tier's agent with `None`.
    fn pick_cli_model(&mut self, pick: Option<String>, cx: &mut Context<Self>) {
        self.cli_popover.update(cx, |popover, cx| {
            popover.set_open(false);
            cx.notify();
        });
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        };
        let value = pick.unwrap_or_else(|| "default".to_string());
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.config(&ConfigRequest {
                        action: "set".to_string(),
                        key: "classifier_agent".to_string(),
                        values: vec![value],
                    })?;
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        key: "classifier_cli".to_string(),
                        ..Default::default()
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                match result {
                    Ok(response) => view.cli_model = Some(parse_cli_model(&response.json)),
                    Err(err) => view.toast(
                        core_message(&err, "Could not change the classifier's model."),
                        ToastTone::Danger,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_classifier(&mut self, value: &str, cx: &mut Context<Self>) {
        let current = self
            .policy
            .as_ref()
            .map(|policy| policy.classifier.as_str())
            .unwrap_or("");
        if radio_value(current) == radio_value(value) {
            return;
        }
        let stored = if value.is_empty() { "off" } else { value };
        self.write_values("classifier", vec![stored.to_string()], cx);
    }

    fn open_priority(&mut self, cx: &mut Context<Self>) {
        let current = self
            .policy
            .as_ref()
            .map(|policy| priority_value(&policy.priority))
            .unwrap_or("balanced");
        let view = cx.entity().downgrade();
        let items = ["balanced", "speed", "quality"]
            .into_iter()
            .map(|value| {
                let view = view.clone();
                let picked = current == value;
                MenuItem::radio(
                    priority_label(value),
                    picked,
                    Rc::new(move |_, app| {
                        let view = view.clone();
                        app.defer(move |app| {
                            if let Some(view) = view.upgrade() {
                                let _ = view.update(app, |this, cx| {
                                    let now = this
                                        .policy
                                        .as_ref()
                                        .map(|policy| priority_value(&policy.priority))
                                        .unwrap_or("balanced");
                                    if now != value {
                                        this.write_values("priority", vec![value.to_string()], cx);
                                    }
                                });
                            }
                        });
                    }),
                )
            })
            .collect();
        self.priority_menu.update(cx, |menu, cx| {
            menu.set_menu(items);
            menu.set_open(true);
            cx.notify();
        });
    }

    fn open_tokens(&mut self, side: RuleSide, dimension: Dimension, cx: &mut Context<Self>) {
        let slot = Some(TokenSlot { side, dimension });
        if self.token_slot != slot {
            self.token_anchor.clear();
        }
        self.token_slot = slot;
        self.token_draft
            .update(cx, |input, cx| input.set_text("", cx));
        self.token_popover.update(cx, |popover, cx| {
            popover.set_open(true);
            cx.notify();
        });
    }

    fn moving(&self) -> bool {
        !self.open.settled()
            || !self.editor.is_settled()
            || !self.enter.settled()
            || !self.result_enter.settled()
            || self.option_hover.iter().any(|spring| !spring.settled())
            || self.option_press.iter().any(|spring| !spring.settled())
    }

    fn sync_motion(&mut self, window: &mut Window, cx: &App) {
        let page = self.page(cx);
        if self.shown.as_ref() != Some(&page) {
            if self.shown.is_none() {
                self.enter.snap(1.0);
            } else {
                self.enter.snap(0.0);
            }
            self.enter.set_target(1.0);
            self.shown = Some(page.clone());
        }
        let open = matches!(self.state.read(cx).route(), Route::Settings(_));
        self.open.set_target(if open { 1.0 } else { 0.0 });
        let editor_open = matches!(page, SettingsPage::Classifier) && self.classifier_on();
        if editor_open && let Some(policy) = &self.policy {
            self.editor_slot = Some(picked_index(&policy.classifier));
        }
        let measured = self
            .editor_measure
            .get()
            .map_or(EDITOR_HEIGHT, |bounds| f32::from(bounds.size.height));
        self.editor
            .set_target(if editor_open { measured } else { 0.0 });
        let show_result = match page {
            SettingsPage::Classifier => self.classify_report.is_some(),
            _ => false,
        };
        self.result_enter
            .set_target(if show_result { 1.0 } else { 0.0 });
        let dt = self.animator.dt(window, self.moving());
        if dt > 0.0 {
            self.open.step(dt);
            self.editor.step(dt);
            self.enter.step(dt);
            self.result_enter.step(dt);
            for spring in &mut self.option_hover {
                spring.step(dt);
            }
            for spring in &mut self.option_press {
                spring.step(dt);
            }
        }
        if self.moving() {
            let _ = self.animator.dt(window, true);
        }
    }

    fn page_body(
        &self,
        page: &SettingsPage,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match page {
            SettingsPage::Agents => self.agents_page(theme, cx),
            SettingsPage::Classifier => self.classifier_page(theme, cx),
            SettingsPage::PickRole => self.roles_page(theme, cx),
            SettingsPage::RoutingPolicy => self.policy_page(theme, cx),
        }
    }

    fn agents_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.client(cx).is_none() {
            return note("This core does not report the agent setup yet.", theme);
        }
        let Some(setup) = self.setup.clone() else {
            // The core is asked for the agent setup: its rows, in bones.
            return settings_skeleton(5);
        };
        let enabled = !self.busy();
        let view = cx.entity().downgrade();
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(SETTINGS_SECTION_GAP));
        let cli_tone = cli_tone(setup.cli.state);
        let mut cli_trailing = div()
            .flex()
            .items_center()
            .gap(px(theme.space_2))
            .child(chip(setup.cli.state.label(), cli_tone, cx));
        if let Some(label) = setup.cli.state.action_label() {
            cli_trailing = cli_trailing.child(dim_button(
                "settings-link-cli",
                label,
                ButtonKind::Secondary,
                enabled,
                gated(enabled, view.clone(), |this, cx| {
                    this.apply_setup(
                        "cli",
                        "link_cli",
                        "",
                        "dispatch now runs this app's core.",
                        cx,
                    );
                }),
                cx,
            ));
        }
        column = column.child(settings_section(
            Icon::SquareTerminal,
            TileTone::Execution,
            "Command line",
            "Agents delegate by running dispatch in their shell.",
            None,
            settings_row(
                div().font_family(theme.font_mono).child("dispatch"),
                text_or_none(&cli_description(&setup.cli)),
                cli_trailing,
                cx,
            ),
            cx,
        ));

        let install_all = skills_need_install(&setup.skills).then(|| {
            dim_button(
                "settings-install-all",
                "Install for all",
                ButtonKind::Secondary,
                enabled,
                gated(enabled, view.clone(), |this, cx| {
                    this.apply_setup(
                        "skills",
                        "install_skill",
                        "",
                        "Skill installed in every folder.",
                        cx,
                    );
                }),
                cx,
            )
        });
        let mut skills = div().flex().flex_col();
        for skill in &setup.skills {
            let dir = skill.dir.clone();
            let readers = skill_readers(skill, &setup.agents);
            let names: Vec<String> = readers.iter().map(|agent| (*agent).clone()).collect();
            let mut trailing = div()
                .flex()
                .items_center()
                .gap(px(theme.space_2))
                .child(chip(skill.state.label(), install_tone(skill.state), cx));
            if let Some(action) = skill.state.action_label().filter(|_| !skill.extra) {
                // The agent names the folder to a core from before skills folders too.
                let key = skill.agents.first().cloned().unwrap_or_else(|| dir.clone());
                let busy_key = format!("skill:{dir}");
                trailing = trailing.child(dim_button(
                    SharedString::from(format!("settings-skill-{dir}")),
                    action,
                    ButtonKind::Secondary,
                    enabled,
                    gated(enabled, view.clone(), move |this, cx| {
                        this.apply_setup(&busy_key, "install_skill", &key, "Skill installed.", cx);
                    }),
                    cx,
                ));
            }
            if skill.state != InstallState::Missing {
                let dir = dir.clone();
                trailing = trailing.child(dim_button(
                    SharedString::from(format!("settings-skill-remove-{dir}")),
                    "Remove",
                    ButtonKind::Ghost,
                    enabled,
                    gated(enabled, view.clone(), move |this, cx| {
                        this.apply_setup(
                            &format!("skill:{dir}"),
                            "remove_skill",
                            &dir,
                            "Skill removed.",
                            cx,
                        );
                    }),
                    cx,
                ));
            }
            let mut avatars = div().flex().items_center().gap(px(2.0));
            for agent in readers.iter().take(6) {
                avatars = avatars.child(agent_avatar(agent, "", 18.0));
            }
            let readers_line = folder_agents_label(&names);
            // The shared folder this agent's own folder sits beside, and whether it has the
            // skill already.
            let shared_has_it = setup.skills.iter().any(|other| {
                !other.extra
                    && other.state != InstallState::Missing
                    && skill
                        .agents
                        .iter()
                        .any(|agent| other.agents.contains(agent))
            });
            let description = if skill.extra && shared_has_it {
                format!("A second copy: {readers_line} also reads ~/.agents/skills, which has it.")
            } else if skill.extra {
                format!(
                    "{readers_line}'s own folder: a copy in ~/.agents/skills serves it with the others."
                )
            } else {
                format!("Read by {readers_line}.")
            };
            skills = skills.child(settings_row(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme.space_2))
                    .child(avatars)
                    .child(
                        div()
                            .font_family(theme.font_mono)
                            .child(SharedString::from(home_relative(&skill.dir))),
                    ),
                Some(SharedString::from(description)),
                trailing,
                cx,
            ));
        }
        column = column.child(settings_section(
            Icon::BookOpen,
            TileTone::Knowledge,
            "Skill",
            "Teaches agents when to delegate with dispatch run. Most of them read ~/.agents/skills, so one copy there serves them all; Claude Code and Antigravity read their own folders.",
            install_all,
            skills,
            cx,
        ));

        let mut hook_body = div().flex().flex_col();
        if setup.hooks.is_empty() {
            hook_body = hook_body.child(settings_row(
                "No agent here has session hooks",
                None,
                div(),
                cx,
            ));
        }
        for hook in &setup.hooks {
            let mut trailing = div().flex().items_center().gap(px(theme.space_2));
            if hook_points_elsewhere(hook) {
                let agent = hook.agent.clone();
                trailing = trailing.child(dim_button(
                    SharedString::from(format!("settings-point-hook-{agent}")),
                    "Point to this app",
                    ButtonKind::Secondary,
                    enabled,
                    gated(enabled, view.clone(), move |this, cx| {
                        this.apply_setup(
                            &format!("hook:{agent}"),
                            "hook_on",
                            &agent,
                            "The hook runs this app's core.",
                            cx,
                        );
                    }),
                    cx,
                ));
            }
            if let Some(toggle) = self.hooks.get(&hook.agent) {
                let selector = format!("settings-hook-{}", hook.agent);
                trailing = trailing.child(
                    div()
                        .debug_selector(move || selector)
                        .when(!enabled, |row| row.opacity(0.45))
                        .child(toggle.clone()),
                );
            }
            hook_body = hook_body.child(settings_row(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme.space_2))
                    .child(agent_avatar(&hook.agent, "", 20.0))
                    .child(agent_label(&hook.agent)),
                Some(SharedString::from(match hook_note(&hook.agent) {
                    Some(note) => format!("{} · {note}", home_relative(&hook.settings)),
                    None => home_relative(&hook.settings),
                })),
                trailing,
                cx,
            ));
        }
        column = column.child(settings_section(
            Icon::Webhook,
            TileTone::Integration,
            "Session hook",
            "At the start of each session the agent reads one line saying dispatch is here and where its skill is. Each agent keeps its hooks in its own file, so each has its own switch.",
            None,
            hook_body,
            cx,
        ));

        let mut plugin = div()
            .flex()
            .items_center()
            .gap(px(theme.space_2))
            .child(chip(
                setup.opencode.state.label(),
                install_tone(setup.opencode.state),
                cx,
            ));
        if let Some(action) = setup.opencode.state.action_label() {
            plugin = plugin.child(dim_button(
                "settings-opencode-install",
                action,
                ButtonKind::Secondary,
                enabled,
                gated(enabled, view.clone(), |this, cx| {
                    this.apply_setup(
                        "opencode",
                        "install_opencode",
                        "",
                        "OpenCode plugin installed.",
                        cx,
                    );
                }),
                cx,
            ));
        }
        if setup.opencode.state != InstallState::Missing {
            plugin = plugin.child(dim_button(
                "settings-opencode-remove",
                "Remove",
                ButtonKind::Ghost,
                enabled,
                gated(enabled, view, |this, cx| {
                    this.apply_setup(
                        "opencode",
                        "remove_opencode",
                        "",
                        "OpenCode plugin removed.",
                        cx,
                    );
                }),
                cx,
            ));
        }
        let mut plugin_body = div().flex().flex_col().child(settings_row(
            div()
                .flex()
                .items_center()
                .gap(px(theme.space_2))
                .child(agent_avatar("opencode", "", 20.0))
                .child(agent_label("opencode")),
            text_or_none(opencode_description(&setup.opencode)),
            plugin,
            cx,
        ));
        if setup.opencode.state != InstallState::Missing {
            plugin_body = plugin_body.child(settings_row(
                "Delegate OpenCode's subagent work through dispatch",
                None,
                div()
                    .when(!enabled, |row| row.opacity(0.45))
                    .child(self.opencode.clone()),
                cx,
            ));
        }
        column = column.child(settings_section(
            Icon::Plug,
            TileTone::Integration,
            "OpenCode plugin",
            "Replaces OpenCode's subagents with dispatch runs and shows each conversation's runs in OpenCode Desktop. It loads from OpenCode's global config.",
            None,
            plugin_body,
            cx,
        ));
        column.into_any_element()
    }

    fn classifier_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let body = if self.policy.is_none() {
            settings_skeleton(4)
        } else {
            let picked = self
                .policy
                .as_ref()
                .map(|policy| policy.classifier.as_str())
                .unwrap_or("");
            let picked_radio = radio_value(picked);
            let view = cx.entity().downgrade();
            let mut column = div().flex().flex_col().gap(px(theme.space_1));
            for (index, choice) in CLASSIFIERS.iter().enumerate() {
                let on = choice.radio == picked_radio;
                let hover = self.option_hover[index].value();
                let press = self.option_press[index].value();
                let view_hover = view.clone();
                let view_press = view.clone();
                let view_up = view.clone();
                let value = choice.value;
                let trailing = classifier_trailing(
                    choice.value,
                    picked_radio,
                    &self.key_status(choice.value),
                    self.cli_model.as_ref(),
                );
                let mut row = div()
                    .id(("settings-classifier", index))
                    .min_h(px(theme.row_height))
                    .px(px(theme.space_2))
                    .py(px(theme.space_2))
                    .flex()
                    .items_start()
                    .gap(px(theme.space_2))
                    .rounded(px(theme.radius_sm))
                    .bg(row_wash(&theme, hover, press))
                    .cursor_pointer()
                    .on_hover(move |over, _, app| {
                        let over = *over;
                        let _ = view_hover.update(app, |this, cx| {
                            this.option_hover[index].set_target(if over { 1.0 } else { 0.0 });
                            cx.notify();
                        });
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, app| {
                        let _ = view_press.update(app, |this, cx| {
                            this.option_press[index].set_target(1.0);
                            cx.notify();
                        });
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let view_up = view_up.clone();
                        move |_, _, app| {
                            let _ = view_up.update(app, |this, cx| {
                                this.option_press[index].set_target(0.0);
                                cx.notify();
                            });
                        }
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_, _, app| {
                        let _ = view_up.update(app, |this, cx| {
                            this.option_press[index].set_target(0.0);
                            cx.notify();
                        });
                    })
                    .on_click({
                        let view = view.clone();
                        move |_, _, app| {
                            let view = view.clone();
                            app.defer(move |app| {
                                if let Some(view) = view.upgrade() {
                                    let _ = view
                                        .update(app, |this, cx| this.choose_classifier(value, cx));
                                }
                            });
                        }
                    })
                    .child(radio_mark(on, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(theme.font_size_sm))
                                    .text_color(theme.color_text_primary)
                                    .child(choice.label),
                            )
                            .child(
                                div()
                                    .text_size(px(theme.font_size_xs))
                                    .text_color(theme.color_text_muted)
                                    .child(choice.detail),
                            ),
                    );
                if let Some(trailing) = trailing {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_size(px(theme.font_size_xs))
                            .text_color(theme.color_text_muted)
                            .child(trailing),
                    );
                }
                column = column.child(row);
                if self.editor_slot == Some(index)
                    && (self.editor.value() > 0.0 || !self.editor.is_settled())
                {
                    column = column.child(
                        div()
                            .h(px(self.editor.value().max(0.0)))
                            .overflow_hidden()
                            .child(
                                div()
                                    .relative()
                                    .pt(px(theme.space_2))
                                    .pb(px(theme.space_1))
                                    .px(px(theme.space_2))
                                    .child(self.editor_measure.probe())
                                    .child(self.classifier_editor(choice.value, cx)),
                            ),
                    );
                }
            }
            if self.classify_report.is_some() {
                column = column.child(self.classify_result(theme, cx));
            }
            column.into_any_element()
        };
        settings_section(
            Icon::AiBrain01Icon,
            TileTone::Knowledge,
            "Difficulty classifier",
            "What judges how hard a task is before it is routed. Auto exists only with one; without it, every run takes a tier. A key is kept in the macOS Keychain, where only Dispatch reads it, and never shown again.",
            None,
            body,
            cx,
        )
    }

    fn classifier_editor(&self, value: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        if keyed_classifier(value).is_some() {
            self.key_editor(value, cx)
        } else if value == "cli" {
            self.cli_editor(cx)
        } else {
            div().into_any_element()
        }
    }

    fn key_editor(&self, name: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = *Theme::global(cx);
        let Some(keyed) = keyed_classifier(name) else {
            return div().into_any_element();
        };
        let status = self.key_status(name);
        let chip_info = key_chip(&status);
        let tone = if chip_info.ok {
            StatusTone::Ok
        } else {
            StatusTone::Muted
        };
        let ready = draft_ready(self.key_field(name).read(cx).text());
        let view = cx.entity().downgrade();
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme.space_2))
            .child(chip(chip_info.label, tone, cx));
        if status == "env" {
            row = row.child(
                div()
                    .text_size(px(theme.font_size_xs))
                    .text_color(theme.color_text_muted)
                    .child(env_key_sentence(keyed.env)),
            );
        } else {
            row = row.child(div().w(px(224.0)).child(self.key_field(name)));
            let field_name = if name == "openai" { "openai" } else { "jev" };
            if key_uses_replace(&status) {
                row = row
                    .child(dim_button(
                        SharedString::from(format!("settings-key-replace-{field_name}")),
                        "Replace",
                        ButtonKind::Secondary,
                        ready,
                        gated(ready, view.clone(), move |this, cx| {
                            this.submit_key_field(field_name, cx);
                        }),
                        cx,
                    ))
                    .child(dim_button(
                        SharedString::from(format!("settings-key-remove-{field_name}")),
                        "Remove",
                        ButtonKind::Ghost,
                        true,
                        gated(true, view.clone(), move |this, cx| {
                            this.write_key(field_name, "key-clear", None, cx);
                        }),
                        cx,
                    ));
            } else {
                row = row.child(dim_button(
                    SharedString::from(format!("settings-key-save-{field_name}")),
                    "Save",
                    ButtonKind::Secondary,
                    ready,
                    gated(ready, view.clone(), move |this, cx| {
                        this.submit_key_field(field_name, cx);
                    }),
                    cx,
                ));
            }
        }
        let test_on = key_test_enabled(&status);
        row = row.child(dim_button(
            SharedString::from(format!("settings-key-test-{name}")),
            "Test",
            ButtonKind::Ghost,
            test_on,
            gated(test_on, view, |this, cx| {
                this.classify_text(SAMPLE_TASK.to_string(), cx);
            }),
            cx,
        ));
        row.into_any_element()
    }

    fn cli_editor(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = *Theme::global(cx);
        let agent = self
            .cli_model
            .as_ref()
            .map(|model| model.agent.clone())
            .unwrap_or_default();
        let model = self
            .cli_model
            .as_ref()
            .map(|model| model.model.clone())
            .unwrap_or_default();
        let enabled = !agent.is_empty();
        let view = cx.entity().downgrade();
        let label = if agent.is_empty() {
            div()
                .text_size(px(theme.font_size_sm))
                .text_color(theme.color_text_muted)
                .child("Pick a model")
        } else {
            div()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(theme.space_2))
                .child(agent_avatar(&agent, "", 14.0))
                .child(
                    div()
                        .font_family(theme.font_mono)
                        .text_size(px(theme.font_size_sm))
                        .child(agent),
                )
                .when(!model.is_empty(), |row| {
                    row.child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .font_family(theme.font_mono)
                            .text_size(px(theme.font_size_sm))
                            .text_color(theme.color_text_muted)
                            .child(model),
                    )
                })
        };
        let open = gated(true, view.clone(), |this, cx| this.open_cli_models(cx));
        let trigger = div()
            .relative()
            .max_w(px(360.0))
            .child(self.cli_anchor.probe())
            .child(
                div()
                    .id("settings-cli-model")
                    .debug_selector(|| "settings-cli-model".to_string())
                    .h(px(theme.control_height_md))
                    .px(px(theme.space_3))
                    .flex()
                    .items_center()
                    .gap(px(theme.space_2))
                    .rounded_full()
                    .bg(theme.color_surface_subtle)
                    .border_1()
                    .border_color(theme.color_border)
                    .cursor_pointer()
                    .on_click(move |_, window, cx| open(window, cx))
                    .child(label)
                    .child(icon(Icon::ChevronDown, 14.0, cx)),
            );
        let row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme.space_2))
            .child(trigger);
        row.child(dim_button(
            "settings-cli-test",
            "Test",
            ButtonKind::Ghost,
            enabled,
            gated(enabled, view, |this, cx| {
                this.classify_text(SAMPLE_TASK.to_string(), cx);
            }),
            cx,
        ))
        .into_any_element()
    }

    fn roles_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let body = if let Some(policy) = &self.policy {
            let mut column = div()
                .flex()
                .flex_col()
                .gap(px(theme.space_2))
                .child(settings_row(
                    "Pick a role for Auto runs",
                    Some("The classifier chooses a role for an Auto run that names none.".into()),
                    div()
                        .debug_selector(|| "settings-pick-role".to_string())
                        .child(self.roles.clone()),
                    cx,
                ));
            for line in [
                "It reads the run's prompt and picks the role whose When to use fits, at P 0.5 or more; if none fits, the run stays on General. The task's difficulty then picks the column of that role's board, and the run draws its model from there.",
                "A run that names a role with --role always keeps it; the pick only fills in a missing one.",
                "A pick never makes a run read-only, and the role's tier is a floor: a role set to Hard or harder never runs on an easier model, even for an easy task.",
                "Turned off, every Auto run without --role uses General.",
            ] {
                column = column.child(
                    div()
                        .text_size(px(theme.font_size_xs))
                        .line_height(px(18.0))
                        .text_color(theme.color_text_muted)
                        .child(line),
                );
            }
            if let Some(blocked) = policy.pick_blocked() {
                column = column.child(
                    div()
                        .mt(px(theme.space_1))
                        .text_size(px(theme.font_size_xs))
                        .line_height(px(18.0))
                        .text_color(theme.color_text_secondary)
                        .child(blocked),
                );
            }
            column.into_any_element()
        } else {
            settings_skeleton(4)
        };
        settings_section(
            Icon::Bot,
            TileTone::Identity,
            "Roles",
            "A role is a kind of task, such as explorer, implementer or reviewer: when it fits, what a run may touch, and a board of models per difficulty. Edit them on the Roles page.",
            None,
            body,
            cx,
        )
    }

    fn classify_result(&self, theme: Theme, cx: &App) -> gpui::AnyElement {
        let Some(report) = &self.classify_report else {
            return div().into_any_element();
        };
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(theme.space_2))
            .opacity(self.result_enter.value().clamp(0.0, 1.0))
            .text_size(px(theme.font_size_sm))
            .text_color(theme.color_text_secondary);
        let mut line = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme.space_2));
        if !report.tier.is_empty() {
            line = line.child(chip(report.tier.clone(), StatusTone::Muted, cx));
        }
        line = line.child(
            div()
                .font_family(theme.font_mono)
                .child(format!("d {}", fixed2(report.d))),
        );
        if let Some(probs) = prob_line(&report.probs) {
            line = line.child(
                div()
                    .font_family(theme.font_mono)
                    .text_color(theme.color_text_muted)
                    .child(probs),
            );
        }
        line = line.child(
            div()
                .text_color(theme.color_text_muted)
                .child(classify_meta(report)),
        );
        column = column.child(line);
        if let Some(role) = &report.role {
            column = column.child(
                div()
                    .text_color(theme.color_text_muted)
                    .child(role_answer_line(role)),
            );
        }
        column.into_any_element()
    }

    fn policy_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view = cx.entity().downgrade();
        let mut body = div().flex().flex_col();
        if let Some(policy) = &self.policy {
            let priority = priority_label(priority_value(&policy.priority));
            body = body
                .child(settings_row(
                    "Priority",
                    Some(
                        "How sure Auto must be that a model passes before it picks the cheaper one."
                            .into(),
                    ),
                    menu_trigger(
                        "settings-priority",
                        priority.into(),
                        &self.priority_anchor,
                        gated(true, view.clone(), |this, cx| this.open_priority(cx)),
                        theme,
                        cx,
                    ),
                    cx,
                ))
                .child(settings_row(
                    "Discover new models",
                    Some(
                        "Let Auto route to models the agent CLIs report that are not in the tier table."
                            .into(),
                    ),
                    div()
                        .debug_selector(|| "settings-discover".to_string())
                        .child(self.discover.clone()),
                    cx,
                ));
        } else {
            body = body.child(settings_skeleton(4));
        }
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(SETTINGS_SECTION_GAP))
            .child(settings_section(
                Icon::Route,
                TileTone::Architecture,
                "Model choice",
                "How Auto picks a model. One policy, for every repository.",
                None,
                body,
                cx,
            ));
        if self.policy.is_some() {
            for side in [RuleSide::Allow, RuleSide::Deny] {
                let mut rows = div().flex().flex_col();
                for dimension in Dimension::ALL {
                    let values = self
                        .policy
                        .as_ref()
                        .map(|policy| policy.rules(side).get(dimension).to_vec())
                        .unwrap_or_default();
                    rows = rows.child(settings_row(
                        dimension.label(),
                        None,
                        self.token_picker(side, dimension, values, theme, cx),
                        cx,
                    ));
                }
                column = column.child(settings_section(
                    side_icon(side),
                    TileTone::Security,
                    side.title(),
                    side.description(),
                    None,
                    rows,
                    cx,
                ));
            }
        }
        column.into_any_element()
    }

    fn token_picker(
        &self,
        side: RuleSide,
        dimension: Dimension,
        values: Vec<String>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view = cx.entity().downgrade();
        let active = self.token_slot == Some(TokenSlot { side, dimension });
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap(px(theme.space_2));
        if values.is_empty() {
            row = row.child(
                div()
                    .text_size(px(theme.font_size_xs))
                    .text_color(theme.color_text_muted)
                    .child(side.empty_label()),
            );
        }
        for token in &values {
            let token_id = token.clone();
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme.space_1))
                    .child(chip(token.clone(), StatusTone::Muted, cx))
                    .child(icon_button(
                        SharedString::from(format!(
                            "settings-token-rm-{}-{}-{token_id}",
                            side.title(),
                            dimension.key()
                        )),
                        Icon::Close,
                        "Remove",
                        click(view.clone(), move |this, cx| {
                            this.remove_token(side, dimension, token_id.clone(), cx);
                        }),
                        cx,
                    )),
            );
        }
        row.child(
            div()
                .relative()
                .child(icon_button(
                    SharedString::from(format!(
                        "settings-token-add-{}-{}",
                        side.title(),
                        dimension.key()
                    )),
                    Icon::Plus,
                    "Add",
                    click(view, move |this, cx| this.open_tokens(side, dimension, cx)),
                    cx,
                ))
                .when(active, |row| row.child(self.token_anchor.probe())),
        )
        .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_motion(window, cx);
        let theme = *Theme::global(cx);
        let page = self.page(cx);
        let entered = self.enter.value().clamp(0.0, 1.0);
        let active = RAIL
            .iter()
            .find(|(item, _, _)| *item == page)
            .map_or("Agents", |(_, label, _)| *label);
        let items = RAIL
            .iter()
            .map(|(_, label, icon)| SettingsNavItem::new(*label, *label).icon(*icon))
            .collect();
        let select = self.state.clone();
        let close = self.state.clone();
        let mut dialog = settings_dialog(self.presence())
            .items(items)
            .active(active)
            .on_select(Rc::new(
                move |id: &SharedString, _: &mut Window, cx: &mut App| {
                    let Some((page, _, _)) =
                        RAIL.iter().find(|(_, label, _)| *label == id.as_ref())
                    else {
                        return;
                    };
                    select.update(cx, |state, cx| {
                        state.navigate(Route::Settings(page.clone()));
                        cx.notify();
                    });
                },
            ))
            .on_close(Rc::new(move |_, cx| {
                close.update(cx, |state, cx| {
                    state.close_settings();
                    cx.notify();
                });
            }))
            .child(
                div()
                    .opacity(entered)
                    .font_family(theme.font_sans)
                    .child(self.page_body(&page, theme, cx)),
            );
        for overlay in self.overlays(&page) {
            dialog = dialog.overlay(overlay);
        }
        dialog
    }
}

impl SettingsView {
    /// The open page's menus and pickers, each hung from its trigger over the whole dialog,
    /// outside the content's scroll and clip. A closed one draws nothing.
    fn overlays(&self, page: &SettingsPage) -> Vec<gpui::AnyElement> {
        let hung: Vec<(&AnchorSlot, gpui::AnyElement)> = match page {
            SettingsPage::RoutingPolicy => {
                let mut hung = vec![(
                    &self.priority_anchor,
                    self.priority_menu.clone().into_any_element(),
                )];
                if self.token_slot.is_some() {
                    hung.push((
                        &self.token_anchor,
                        self.token_popover.clone().into_any_element(),
                    ));
                }
                hung
            }
            SettingsPage::Classifier => {
                vec![(
                    &self.cli_anchor,
                    self.cli_popover.clone().into_any_element(),
                )]
            }
            _ => Vec::new(),
        };
        hung.into_iter()
            .filter_map(|(anchor, panel)| Some(window_anchor(anchor.get()?, panel)))
            .collect()
    }
}

fn page_index(page: &SettingsPage) -> usize {
    RAIL.iter()
        .position(|(item, _, _)| item == page)
        .unwrap_or(0)
}

fn picked_index(classifier: &str) -> usize {
    let radio = radio_value(classifier);
    CLASSIFIERS
        .iter()
        .position(|choice| choice.radio == radio)
        .unwrap_or(0)
}

fn core_message(err: &CoreError, fallback: &str) -> String {
    match err {
        CoreError::Rpc(message) if !message.trim().is_empty() => message.clone(),
        _ => fallback.to_string(),
    }
}

fn install_tone(state: InstallState) -> StatusTone {
    match state {
        InstallState::Current => StatusTone::Ok,
        InstallState::Outdated => StatusTone::Warn,
        InstallState::Missing => StatusTone::Muted,
    }
}

fn cli_tone(state: CliState) -> StatusTone {
    match state {
        CliState::Linked => StatusTone::Ok,
        CliState::Other => StatusTone::Warn,
        CliState::Missing => StatusTone::Muted,
    }
}

fn row_wash(theme: &Theme, hover: f32, press: f32) -> gpui::Rgba {
    let color = if press >= hover && press > 0.0 {
        theme.color_surface_active
    } else {
        theme.color_surface_hover
    };
    wash(color, hover.max(press))
}

fn note(text: &str, theme: Theme) -> gpui::AnyElement {
    div()
        .text_size(px(theme.font_size_sm))
        .text_color(theme.color_text_muted)
        .child(text.to_string())
        .into_any_element()
}

/// A path under the home folder as `~/…`.
fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => match path.strip_prefix(&home) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_string(),
        },
        _ => path.to_string(),
    }
}

fn text_or_none(text: &str) -> Option<SharedString> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_string().into())
    }
}

/// The allow list's shield, the deny list's wall.
fn side_icon(side: RuleSide) -> Icon {
    match side {
        RuleSide::Allow => Icon::ShieldCheck,
        RuleSide::Deny => Icon::FirewallIcon,
    }
}

fn watch_input(input: &Entity<TextInput>, cx: &mut Context<SettingsView>) {
    cx.observe(input, |_, _, cx| cx.notify()).detach();
}

fn key_input(
    name: &'static str,
    placeholder: &'static str,
    view: WeakEntity<SettingsView>,
    cx: &mut Context<SettingsView>,
) -> Entity<TextInput> {
    cx.new(move |cx| {
        TextInput::new(cx)
            .password(true)
            .placeholder(placeholder)
            .on_submit(move |text, _, app| {
                let draft = trimmed_draft(text);
                let view = view.clone();
                app.defer(move |app| {
                    if let Some(view) = view.upgrade() {
                        let _ = view.update(app, |this, cx| this.submit_key(name, draft, cx));
                    }
                });
            })
    })
}

fn token_input(
    view: WeakEntity<SettingsView>,
    cx: &mut Context<SettingsView>,
) -> Entity<TextInput> {
    cx.new(move |cx| {
        TextInput::new(cx)
            .placeholder("Type a value")
            .on_submit(move |text, _, app| {
                let token = trimmed_draft(text);
                let view = view.clone();
                app.defer(move |app| {
                    if let Some(view) = view.upgrade() {
                        let _ = view.update(app, |this, cx| this.add_token(token, cx));
                    }
                });
            })
    })
}

fn switch(
    on: bool,
    view: WeakEntity<SettingsView>,
    cx: &mut Context<SettingsView>,
    change: impl Fn(&mut SettingsView, bool, &mut Context<SettingsView>) + 'static,
) -> Entity<Switch> {
    let change = Rc::new(change);
    cx.new(move |cx| {
        Switch::new(on, cx).on_change(move |on, _, app| {
            let view = view.clone();
            let change = change.clone();
            app.defer(move |app| {
                if let Some(view) = view.upgrade() {
                    let _ = view.update(app, |this, cx| change(this, on, cx));
                }
            });
        })
    })
}

fn click(
    view: WeakEntity<SettingsView>,
    f: impl Fn(&mut SettingsView, &mut Context<SettingsView>) + 'static,
) -> Rc<dyn Fn(&mut Window, &mut App)> {
    let f = Rc::new(f);
    Rc::new(move |_, app| {
        let view = view.clone();
        let f = f.clone();
        app.defer(move |app| {
            if let Some(view) = view.upgrade() {
                let _ = view.update(app, |this, cx| f(this, cx));
            }
        });
    })
}

fn gated(
    enabled: bool,
    view: WeakEntity<SettingsView>,
    f: impl Fn(&mut SettingsView, &mut Context<SettingsView>) + 'static,
) -> Rc<dyn Fn(&mut Window, &mut App)> {
    click(view, move |this, cx| {
        if enabled {
            f(this, cx);
        }
    })
}

fn dim_button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    kind: ButtonKind,
    enabled: bool,
    on_click: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> gpui::AnyElement {
    div()
        .when(!enabled, |row| row.opacity(0.45))
        .child(button(id, label.to_string(), kind, on_click, cx))
        .into_any_element()
}

/// A select-like trigger, 168px wide. Its menu hangs from `anchor` in the dialog's overlay,
/// so the rows after it never paint over the menu.
fn menu_trigger(
    id: &'static str,
    label: SharedString,
    anchor: &AnchorSlot,
    on_open: Rc<dyn Fn(&mut Window, &mut App)>,
    theme: Theme,
    cx: &App,
) -> gpui::AnyElement {
    div()
        .relative()
        .w(px(168.0))
        .child(anchor.probe())
        .child(
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .h(px(theme.control_height_md))
                .w_full()
                .px(px(theme.space_3))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme.space_2))
                // Round-ended, as every button and field is.
                .rounded_full()
                .bg(theme.color_surface_subtle)
                .border_1()
                .border_color(theme.color_border)
                .cursor_pointer()
                .on_click(move |_, window, cx| on_open(window, cx))
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(theme.font_size_sm))
                        .text_color(theme.color_text_primary)
                        .child(label),
                )
                .child(icon(Icon::ChevronDown, 14.0, cx)),
        )
        .into_any_element()
}

fn token_panel(view: &WeakEntity<SettingsView>, cx: &mut App) -> gpui::AnyElement {
    let Some(view) = view.upgrade() else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let (options, dimension, models, draft) = {
        let settings = view.read(cx);
        let Some(slot) = settings.token_slot else {
            return div().into_any_element();
        };
        let options = settings
            .policy
            .as_ref()
            .map(|policy| {
                let current = policy.rules(slot.side).get(slot.dimension);
                policy_options(slot.dimension, &settings.agents, &settings.models)
                    .into_iter()
                    .filter(|item| !current.iter().any(|have| have == item))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (
            options,
            slot.dimension,
            settings.models.clone(),
            settings.token_draft.clone(),
        )
    };
    let mut list = div()
        .id("settings-token-options")
        .flex()
        .flex_col()
        .max_h(px(220.0))
        .overflow_y_scroll();
    for (index, option) in options.iter().enumerate() {
        let view = view.downgrade();
        let option = option.clone();
        let label = option.clone();
        let mark = match dimension {
            Dimension::Agents => Some(agent_avatar(&option, "", TOKEN_MARK).into_any_element()),
            Dimension::Models => model_agent(&models, &option)
                .map(|agent| agent_avatar(agent, "", TOKEN_MARK).into_any_element()),
            Dimension::Providers => Some(
                IconTile::new(Icon::Cloud)
                    .size(TOKEN_MARK, 12.0)
                    .into_any_element(),
            ),
        };
        list = list.child(
            div()
                .id(("settings-token-option", index))
                .flex_shrink_0()
                .h(px(theme.control_height_md))
                .px(px(theme.space_2))
                .flex()
                .items_center()
                .gap(px(theme.space_2))
                .rounded(px(theme.radius_sm))
                .cursor_pointer()
                .hover(|style| style.bg(theme.color_surface_hover))
                .text_size(px(theme.font_size_sm))
                .on_click(move |_, _, app| {
                    let view = view.clone();
                    let option = option.clone();
                    app.defer(move |app| {
                        if let Some(view) = view.upgrade() {
                            let _ = view.update(app, |this, cx| this.add_token(option, cx));
                        }
                    });
                })
                .children(mark.map(|mark| div().flex_none().child(mark)))
                .child(div().min_w(px(0.0)).truncate().child(label)),
        );
    }
    div()
        .debug_selector(|| "settings-token-panel".to_string())
        .w(px(TOKEN_PANEL_WIDTH))
        .flex()
        .flex_col()
        .gap(px(theme.space_2))
        .child(list)
        .child(draft)
        .into_any_element()
}

/// The CLI classifier's picker: the easy tier's agent, then every agent and model the CLIs
/// here list, under a filter that also takes a typed `agent/model`.
fn cli_panel(view: &WeakEntity<SettingsView>, cx: &mut App) -> gpui::AnyElement {
    let Some(view) = view.upgrade() else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let (options, filter, current) = {
        let settings = view.read(cx);
        let text = settings.cli_filter.read(cx).text().to_string();
        let current = settings
            .cli_model
            .as_ref()
            .map(|model| (model.agent.clone(), model.model.clone()));
        (
            settings.cli_options(&text),
            settings.cli_filter.clone(),
            current,
        )
    };
    let row = |id: gpui::ElementId, on: bool, child: gpui::AnyElement, pick: Option<String>| {
        let view = view.downgrade();
        let selector = format!(
            "settings-cli-option-{}",
            pick.as_deref().unwrap_or("default")
        );
        div()
            .id(id)
            .debug_selector(move || selector)
            .min_h(px(theme.control_height_md))
            .px(px(theme.space_2))
            .flex()
            .items_center()
            .gap(px(theme.space_2))
            .rounded(px(theme.radius_sm))
            .cursor_pointer()
            .hover(|style| style.bg(theme.color_surface_hover))
            .text_size(px(theme.font_size_sm))
            .on_click(move |_, _, app| {
                let view = view.clone();
                let pick = pick.clone();
                app.defer(move |app| {
                    if let Some(view) = view.upgrade() {
                        view.update(app, |this, cx| this.pick_cli_model(pick, cx));
                    }
                });
            })
            .child(div().flex_1().min_w(px(0.0)).child(child))
            .when(on, |row| row.child(icon(Icon::Check, 14.0, cx)))
    };
    let mut list = div()
        .id("settings-cli-options")
        .flex()
        .flex_col()
        .max_h(px(260.0))
        .overflow_y_scroll()
        .child(row(
            "settings-cli-default".into(),
            false,
            div()
                .text_color(theme.color_text_secondary)
                .child("The easy tier's first agent")
                .into_any_element(),
            None,
        ));
    for (index, (agent, model)) in options.iter().enumerate() {
        let on = current
            .as_ref()
            .is_some_and(|(a, m)| a == agent && m == model);
        let label = div()
            .flex()
            .items_center()
            .gap(px(theme.space_2))
            .child(agent_avatar(agent, "", 14.0))
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .font_family(theme.font_mono)
                    .child(model.clone()),
            )
            .into_any_element();
        list = list.child(row(
            ("settings-cli-option", index).into(),
            on,
            label,
            Some(format!("{agent}/{model}")),
        ));
    }
    if options.is_empty() {
        list = list.child(
            div()
                .px(px(theme.space_2))
                .py(px(theme.space_2))
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(
                    "No model the agent CLIs list holds that. Type agent/model and press Return.",
                ),
        );
    }
    div()
        .w(px(320.0))
        .flex()
        .flex_col()
        .gap(px(theme.space_2))
        .child(
            div()
                .debug_selector(|| "settings-cli-filter".to_string())
                .child(filter),
        )
        .child(list)
        .into_any_element()
}

fn cli_filter_input(
    view: WeakEntity<SettingsView>,
    cx: &mut Context<SettingsView>,
) -> Entity<TextInput> {
    cx.new(move |cx| {
        TextInput::new(cx)
            .placeholder("Search models, or type agent/model")
            .on_submit(move |text, _, app| {
                let text = text.to_string();
                let view = view.clone();
                app.defer(move |app| {
                    if let Some(view) = view.upgrade() {
                        view.update(app, |this, cx| this.submit_cli_filter(text, cx));
                    }
                });
            })
    })
}

/// A settings section in bones while the core answers: `rows` rows, no toolbar.
fn settings_skeleton(rows: usize) -> gpui::AnyElement {
    page_skeleton(SkeletonLayout::Table)
        .without_toolbar()
        .rows(rows)
        .into_any_element()
}
