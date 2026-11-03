//! Settings: one page per concern, with a rail.
//!
//! Agent setup, routing policy, classifier keys, route, and classify all go
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
    GetAgentSetupRequest, ListAgentsRequest, ListModelsRequest, RouteRequest, Workspace,
};
use crate::models::settings::{
    CLASSIFIERS, CliState, Dimension, InstallState, ListedModel, Policy, RouteView, RuleSide,
    SAMPLE_TASK, agent_label, bool_wire, classifier_trailing, classify_meta, cli_description,
    draft_ready, env_key_sentence, fixed2, folder_agents_label, hook_points_elsewhere, key_chip,
    key_placeholder, key_test_enabled, key_uses_replace, keyed_classifier, opencode_description,
    parse_agent_setup, parse_classify, parse_cli_model, parse_key_map, parse_key_status_value,
    parse_policy, parse_route, policy_options, priority_label, priority_value, prob_line,
    radio_value, role_answer_line, route_tier_label, route_tier_options, skill_folders,
    skills_need_install, token_delta, trimmed_draft, with_token, without_token,
};
use crate::motion::{Animator, Spring, SpringTier, SpringValue};
use crate::state::{AppState, Route, SettingsPage, ToastTone};
use crate::theme::{StatusTone, Theme};
use crate::ui::parts::settings::{
    radio_mark, settings_field, settings_row, settings_section, wash,
};
use crate::ui::{
    ButtonKind, Icon, Menu, MenuEntry, Popover, Switch, TextInput, button, chip, icon, icon_button,
    rail_group, toolbar, tooltip,
};

const RAIL_ROW: f32 = 32.0;
const RAIL_GAP: f32 = 2.0;
const EDITOR_HEIGHT: f32 = 120.0;

const RAIL: [(SettingsPage, &str, Icon); 6] = [
    (SettingsPage::Agents, "Agents", Icon::Settings),
    (SettingsPage::TryRoute, "Try a route", Icon::Route),
    (SettingsPage::Classifier, "Classifier", Icon::AiBrain01Icon),
    (SettingsPage::PickRole, "Pick a role", Icon::Route),
    (
        SettingsPage::ClassifyPrompt,
        "Classify a prompt",
        Icon::Search,
    ),
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
    project_dir: String,
    project_id: String,
    project_scope: bool,
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
    route_tier: String,
    route_agent: String,
    route_report: Option<RouteView>,
    classify_report: Option<crate::models::settings::ClassifyView>,
    route_prompt: Entity<TextInput>,
    classify_prompt: Entity<TextInput>,
    jev_key: Entity<TextInput>,
    openai_key: Entity<TextInput>,
    jev_placeholder: &'static str,
    openai_placeholder: &'static str,
    token_draft: Entity<TextInput>,
    hook: Entity<Switch>,
    opencode: Entity<Switch>,
    roles: Entity<Switch>,
    discover: Entity<Switch>,
    scope: Entity<Switch>,
    read_only: Entity<Switch>,
    priority_menu: Entity<Menu>,
    tier_menu: Entity<Menu>,
    agent_menu: Entity<Menu>,
    token_popover: Entity<Popover>,
    token_slot: Option<TokenSlot>,
    indicator: Spring,
    indicator_ready: bool,
    rail_hover: [SpringValue; 6],
    rail_press: [SpringValue; 6],
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
        let route_prompt = prompt_input(cx);
        let classify_prompt = prompt_input(cx);
        let token_draft = token_input(view.clone(), cx);
        let jev_key = key_input("jev", "Paste a key", view.clone(), cx);
        let openai_key = key_input("openai", "Paste a key", view.clone(), cx);
        let hook = switch(false, view.clone(), cx, |this, on, cx| this.on_hook(on, cx));
        let opencode = switch(true, view.clone(), cx, |this, on, cx| {
            this.on_opencode(on, cx)
        });
        let roles = switch(true, view.clone(), cx, |this, on, cx| this.on_roles(on, cx));
        let discover = switch(true, view.clone(), cx, |this, on, cx| {
            this.on_discover(on, cx)
        });
        let scope = switch(false, view.clone(), cx, |this, on, cx| {
            this.on_scope(on, cx)
        });
        let read_only = cx.new(|cx| Switch::new(false, cx));
        let priority_menu = cx.new(Menu::new);
        let tier_menu = cx.new(Menu::new);
        let agent_menu = cx.new(Menu::new);
        let token_popover = cx.new(Popover::new);
        let panel_view = view.clone();
        token_popover.update(cx, |popover, _| {
            popover.set_panel(Rc::new(move |_window, app| token_panel(&panel_view, app)));
        });
        cx.observe(&state, |this, _, cx| {
            this.on_app_state(cx);
            cx.notify();
        })
        .detach();
        watch_input(&route_prompt, cx);
        watch_input(&classify_prompt, cx);
        watch_input(&jev_key, cx);
        watch_input(&openai_key, cx);
        watch_input(&token_draft, cx);
        let mut this = Self {
            state,
            started: false,
            epoch: 0,
            project_dir: String::new(),
            project_id: String::new(),
            project_scope: false,
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
            route_tier: "normal".to_string(),
            route_agent: String::new(),
            route_report: None,
            classify_report: None,
            route_prompt,
            classify_prompt,
            jev_key,
            openai_key,
            jev_placeholder: "Paste a key",
            openai_placeholder: "Paste a key",
            token_draft,
            hook,
            opencode,
            roles,
            discover,
            scope,
            read_only,
            priority_menu,
            tier_menu,
            agent_menu,
            token_popover,
            token_slot: None,
            indicator: Spring::selection(0.0),
            indicator_ready: false,
            rail_hover: [SpringValue::new(0.0, SpringTier::Fast); 6],
            rail_press: [SpringValue::new(0.0, SpringTier::Fast); 6],
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
        let workspaces = self.state.read(cx).workspaces().to_vec();
        let (dir, id) = project_binding(&workspaces);
        let mut reload_policy = false;
        if dir != self.project_dir || id != self.project_id {
            self.project_dir = dir;
            self.project_id = id;
            if self.project_dir.is_empty() && self.project_scope {
                self.project_scope = false;
                self.scope.update(cx, |switch, _| switch.set_on(false));
            }
            reload_policy = self.started;
        }
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

    fn page(&self, cx: &App) -> SettingsPage {
        match self.state.read(cx).route() {
            Route::Settings(page) => page.clone(),
            _ => SettingsPage::Agents,
        }
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
            if !policy.classifier_on() && self.route_tier == "auto" {
                self.route_tier = "normal".to_string();
            }
        }
    }

    fn sync_setup_switches(&mut self, cx: &mut Context<Self>) {
        if let Some(setup) = self.setup.clone() {
            self.hook
                .update(cx, |switch, _| switch.set_on(setup.hook.on));
            self.opencode
                .update(cx, |switch, _| switch.set_on(setup.opencode.on));
        }
    }

    fn apply_policy(&mut self, policy: Policy, cx: &mut Context<Self>) {
        if !policy.classifier_on() && self.route_tier == "auto" {
            self.route_tier = "normal".to_string();
        }
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
                    let agents = client.list_agents(&ListAgentsRequest::default()).ok();
                    let models = client.list_models(&ListModelsRequest::default()).ok();
                    (agents, models)
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
        let dir = self.project_dir.clone();
        let project = self.project_scope && !dir.is_empty();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        dir,
                        project,
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
        let dir = self.project_dir.clone();
        let project = self.project_scope && !dir.is_empty();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let keys = client.config(&ConfigRequest {
                        action: "get".to_string(),
                        dir: dir.clone(),
                        project,
                        key: "classifier_keys".to_string(),
                        ..Default::default()
                    });
                    let cli = client.config(&ConfigRequest {
                        action: "get".to_string(),
                        dir,
                        project,
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
            if !policy.classifier_on() && self.route_tier == "auto" {
                self.route_tier = "normal".to_string();
            }
        }
        self.policy_ticket = self.policy_ticket.wrapping_add(1);
        let ticket = self.policy_ticket;
        let epoch = self.epoch;
        let dir = self.project_dir.clone();
        let project = self.project_scope && !dir.is_empty();
        let key = key.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    client.config(&ConfigRequest {
                        action: "set".to_string(),
                        dir: dir.clone(),
                        project,
                        key,
                        values,
                    })?;
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        dir,
                        project,
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
        let dir = self.project_dir.clone();
        let project = self.project_scope && !dir.is_empty();
        let key = dimension.key().to_string();
        cx.spawn(async move |this, cx| {
            let result: Result<ConfigResponse, CoreError> = cx
                .background_spawn(async move {
                    if !added.is_empty() {
                        client.config(&ConfigRequest {
                            action: side.action("add"),
                            dir: dir.clone(),
                            project,
                            key: key.clone(),
                            values: added,
                        })?;
                    }
                    if !removed.is_empty() {
                        client.config(&ConfigRequest {
                            action: side.action("rm"),
                            dir: dir.clone(),
                            project,
                            key: key.clone(),
                            values: removed,
                        })?;
                    }
                    client.config(&ConfigRequest {
                        action: "get".to_string(),
                        dir,
                        project,
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
                        ..Default::default()
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

    fn run_route(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            self.toast("This core does not support it yet.", ToastTone::Danger, cx);
            return;
        };
        let request = RouteRequest {
            tier: self.route_tier.clone(),
            agent: self.route_agent.clone(),
            read_only: self.read_only.read(cx).is_on(),
            dir: self.project_dir.clone(),
            prompt: self.route_prompt.read(cx).text().to_string(),
            project_id: self.project_id.clone(),
            ..Default::default()
        };
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.route(&request) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                match result {
                    Ok(response) => {
                        // `ok` is omitted when no candidate qualifies. The body is still the report.
                        view.route_report = Some(parse_route(&response.json));
                        view.result_enter.snap(0.0);
                        view.result_enter.set_target(1.0);
                    }
                    Err(err) => {
                        view.toast(
                            core_message(&err, "Could not route."),
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
            dir: self.project_dir.clone(),
            project_id: self.project_id.clone(),
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

    fn on_hook(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.busy() {
            self.hook.update(cx, |switch, _| switch.set_on(!on));
            cx.notify();
            return;
        }
        let (action, done) = if on {
            ("hook_on", "Hook on.")
        } else {
            ("hook_off", "Hook off.")
        };
        self.apply_setup("hook", action, "", done, cx);
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

    fn on_scope(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.project_dir.is_empty() {
            self.project_scope = false;
            self.scope.update(cx, |switch, _| switch.set_on(false));
            cx.notify();
            return;
        }
        self.project_scope = on;
        self.load_policy(cx);
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
                MenuEntry {
                    label: priority_label(value).into(),
                    icon: picked.then_some(Icon::Check),
                    danger: false,
                    on_select: Rc::new(move |_, app| {
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
                }
            })
            .collect();
        self.priority_menu.update(cx, |menu, cx| {
            menu.set_items(items);
            menu.set_open(true);
            cx.notify();
        });
    }

    fn open_tier(&mut self, cx: &mut Context<Self>) {
        let current = self.route_tier.clone();
        let view = cx.entity().downgrade();
        let items = route_tier_options(self.classifier_on())
            .into_iter()
            .map(|(value, label)| {
                let view = view.clone();
                let picked = current == value;
                MenuEntry {
                    label: label.into(),
                    icon: picked.then_some(Icon::Check),
                    danger: false,
                    on_select: Rc::new(move |_, app| {
                        let view = view.clone();
                        app.defer(move |app| {
                            if let Some(view) = view.upgrade() {
                                let _ = view.update(app, |this, cx| {
                                    this.route_tier = value.to_string();
                                    cx.notify();
                                });
                            }
                        });
                    }),
                }
            })
            .collect();
        self.tier_menu.update(cx, |menu, cx| {
            menu.set_items(items);
            menu.set_open(true);
            cx.notify();
        });
    }

    fn open_agent(&mut self, cx: &mut Context<Self>) {
        let current = self.route_agent.clone();
        let mut options = vec![("".to_string(), "Any agent".to_string())];
        options.extend(self.agents.iter().cloned().map(|name| (name.clone(), name)));
        let view = cx.entity().downgrade();
        let items = options
            .into_iter()
            .map(|(value, label)| {
                let view = view.clone();
                let picked = current == value;
                MenuEntry {
                    label: label.into(),
                    icon: picked.then_some(Icon::Check),
                    danger: false,
                    on_select: Rc::new(move |_, app| {
                        let view = view.clone();
                        let value = value.clone();
                        app.defer(move |app| {
                            if let Some(view) = view.upgrade() {
                                let _ = view.update(app, |this, cx| {
                                    this.route_agent = value;
                                    cx.notify();
                                });
                            }
                        });
                    }),
                }
            })
            .collect();
        self.agent_menu.update(cx, |menu, cx| {
            menu.set_items(items);
            menu.set_open(true);
            cx.notify();
        });
    }

    fn open_tokens(&mut self, side: RuleSide, dimension: Dimension, cx: &mut Context<Self>) {
        self.token_slot = Some(TokenSlot { side, dimension });
        self.token_draft
            .update(cx, |input, cx| input.set_text("", cx));
        self.token_popover.update(cx, |popover, cx| {
            popover.set_open(true);
            cx.notify();
        });
    }

    fn moving(&self) -> bool {
        !self.indicator.is_settled()
            || !self.editor.is_settled()
            || !self.enter.settled()
            || !self.result_enter.settled()
            || self.rail_hover.iter().any(|spring| !spring.settled())
            || self.rail_press.iter().any(|spring| !spring.settled())
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
        let stride = RAIL_ROW + RAIL_GAP;
        let y = page_index(&page) as f32 * stride;
        if !self.indicator_ready {
            self.indicator.snap(y);
            self.indicator_ready = true;
        } else {
            self.indicator.set_target(y);
        }
        let editor_open = matches!(page, SettingsPage::Classifier) && self.classifier_on();
        if editor_open && let Some(policy) = &self.policy {
            self.editor_slot = Some(picked_index(&policy.classifier));
        }
        self.editor
            .set_target(if editor_open { EDITOR_HEIGHT } else { 0.0 });
        let show_result = match page {
            SettingsPage::TryRoute => self.route_report.is_some(),
            SettingsPage::Classifier | SettingsPage::ClassifyPrompt => {
                self.classify_report.is_some()
            }
            _ => false,
        };
        self.result_enter
            .set_target(if show_result { 1.0 } else { 0.0 });
        let dt = self.animator.dt(window, self.moving());
        if dt > 0.0 {
            self.indicator.step(dt);
            self.editor.step(dt);
            self.enter.step(dt);
            self.result_enter.step(dt);
            for spring in &mut self.rail_hover {
                spring.step(dt);
            }
            for spring in &mut self.rail_press {
                spring.step(dt);
            }
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

    fn rail(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let state = self.state.clone();
        let mut rows = div().relative().w_full().flex().flex_col().child(
            div()
                .absolute()
                .top(px(self.indicator.value()))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(RAIL_ROW))
                .rounded(px(theme.radius_sm))
                .bg(theme.color_surface_selected),
        );
        for (index, (page, label, icon_name)) in RAIL.iter().enumerate() {
            let selected = &self.page(cx) == page;
            let hover = self.rail_hover[index].value();
            let press = self.rail_press[index].value();
            let view_hover = view.clone();
            let view_press = view.clone();
            let view_up = view.clone();
            let state_click = state.clone();
            let page = page.clone();
            rows = rows.child(
                div().h(px(RAIL_ROW + RAIL_GAP)).w_full().child(
                    div()
                        .id(("settings-rail", index))
                        .h(px(RAIL_ROW))
                        .w_full()
                        .px(px(theme.space_2))
                        .flex()
                        .items_center()
                        .gap(px(theme.space_2))
                        .rounded(px(theme.radius_sm))
                        .bg(row_wash(&theme, hover, press))
                        .cursor_pointer()
                        .on_hover(move |over, _, app| {
                            let over = *over;
                            let _ = view_hover.update(app, |this, cx| {
                                this.rail_hover[index].set_target(if over { 1.0 } else { 0.0 });
                                cx.notify();
                            });
                        })
                        .on_mouse_down(MouseButton::Left, move |_, _, app| {
                            let _ = view_press.update(app, |this, cx| {
                                this.rail_press[index].set_target(1.0);
                                cx.notify();
                            });
                        })
                        .on_mouse_up(MouseButton::Left, {
                            let view_up = view_up.clone();
                            move |_, _, app| {
                                let _ = view_up.update(app, |this, cx| {
                                    this.rail_press[index].set_target(0.0);
                                    cx.notify();
                                });
                            }
                        })
                        .on_mouse_up_out(MouseButton::Left, move |_, _, app| {
                            let _ = view_up.update(app, |this, cx| {
                                this.rail_press[index].set_target(0.0);
                                cx.notify();
                            });
                        })
                        .on_click(move |_, _, app| {
                            state_click.update(app, |state, cx| {
                                state.navigate(Route::Settings(page.clone()));
                                cx.notify();
                            });
                        })
                        .child(icon(*icon_name, 16.0, cx))
                        .child(
                            div()
                                .text_size(px(theme.font_size_sm))
                                .text_color(if selected {
                                    theme.color_text_primary
                                } else {
                                    theme.color_text_secondary
                                })
                                .child(*label),
                        ),
                ),
            );
        }
        div()
            .w(px(220.0))
            .h_full()
            .flex_none()
            .bg(theme.color_sidebar_background)
            .border_r_1()
            .border_color(theme.color_border_subtle)
            .p(px(theme.space_3))
            .child(rail_group("Dispatch", cx).child(rows))
    }

    fn page_body(
        &self,
        page: &SettingsPage,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match page {
            SettingsPage::Agents => self.agents_page(theme, cx),
            SettingsPage::TryRoute => self.route_page(theme, cx),
            SettingsPage::Classifier => self.classifier_page(theme, cx),
            SettingsPage::PickRole => self.roles_page(theme, cx),
            SettingsPage::ClassifyPrompt => self.classify_page(theme, cx),
            SettingsPage::RoutingPolicy => self.policy_page(theme, cx),
        }
    }

    fn agents_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.client(cx).is_none() {
            return note("This core does not report the agent setup yet.", theme);
        }
        let Some(setup) = self.setup.clone() else {
            return div().into_any_element();
        };
        let enabled = !self.busy();
        let view = cx.entity().downgrade();
        let mut column = div().w_full().flex().flex_col().gap(px(theme.space_6));
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
                        "Skill installed for every agent.",
                        cx,
                    );
                }),
                cx,
            )
        });
        let mut skills = div().flex().flex_col();
        for folder in skill_folders(&setup.skills) {
            let path = folder.path.clone();
            let agent = folder.agents.first().cloned().unwrap_or_default();
            let avatar_agent = agent.clone();
            let label = folder_agents_label(&folder.agents);
            let mut trailing = div()
                .flex()
                .items_center()
                .gap(px(theme.space_2))
                .child(chip(folder.state.label(), install_tone(folder.state), cx));
            if let Some(action) = folder.state.action_label() {
                let path_for_id = path.clone();
                trailing = trailing.child(dim_button(
                    SharedString::from(format!("settings-skill-{path_for_id}")),
                    action,
                    ButtonKind::Secondary,
                    enabled,
                    gated(enabled, view.clone(), move |this, cx| {
                        this.apply_setup(
                            &format!("skill:{path}"),
                            "install_skill",
                            &agent,
                            "Skill installed.",
                            cx,
                        );
                    }),
                    cx,
                ));
            }
            skills = skills.child(settings_row(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme.space_2))
                    .child(agent_avatar(&avatar_agent, "", 20.0))
                    .child(label),
                text_or_none(&folder.path),
                trailing,
                cx,
            ));
        }
        column = column.child(settings_section(
            "Skill",
            "Teaches each agent when to delegate with dispatch run and how to read the result.",
            install_all,
            skills,
            cx,
        ));

        let mut hook_body = div().flex().flex_col().child(settings_row(
            "Name dispatch when a Claude Code session starts",
            None,
            div()
                .when(!enabled, |row| row.opacity(0.45))
                .child(self.hook.clone()),
            cx,
        ));
        if hook_points_elsewhere(&setup.hook) {
            hook_body = hook_body.child(settings_row(
                "Runs another dispatch",
                text_or_none(&setup.hook.command),
                dim_button(
                    "settings-point-hook",
                    "Point to this app",
                    ButtonKind::Secondary,
                    enabled,
                    gated(enabled, view.clone(), |this, cx| {
                        this.apply_setup(
                            "hook",
                            "hook_on",
                            "",
                            "The hook runs this app's core.",
                            cx,
                        );
                    }),
                    cx,
                ),
                cx,
            ));
        }
        column = column.child(settings_section(
            "Session hook",
            "Claude Code reads one line at the start of each session saying dispatch is here and where its skill is.",
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
            "OpenCode plugin",
            "Replaces OpenCode's subagents with dispatch runs and shows each conversation's runs in OpenCode Desktop. It loads from OpenCode's global config.",
            None,
            plugin_body,
            cx,
        ));
        column.into_any_element()
    }

    fn route_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view = cx.entity().downgrade();
        let tier_label = route_tier_label(&self.route_tier);
        let agent_label = if self.route_agent.is_empty() {
            "Any agent".to_string()
        } else {
            self.route_agent.clone()
        };
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme.space_3))
            .child(settings_field("Prompt", self.route_prompt.clone(), cx))
            .child(
                div()
                    .flex()
                    .gap(px(theme.space_3))
                    .child(settings_field(
                        "Route",
                        menu_trigger(
                            "settings-tier",
                            tier_label.into(),
                            self.tier_menu.clone(),
                            gated(true, view.clone(), |this, cx| this.open_tier(cx)),
                            theme,
                            cx,
                        ),
                        cx,
                    ))
                    .child(settings_field(
                        "Agent",
                        menu_trigger(
                            "settings-agent",
                            agent_label.into(),
                            self.agent_menu.clone(),
                            gated(true, view.clone(), |this, cx| this.open_agent(cx)),
                            theme,
                            cx,
                        ),
                        cx,
                    )),
            )
            .child(settings_row("Read only", None, self.read_only.clone(), cx))
            .child(dim_button(
                "settings-route",
                "Route",
                ButtonKind::Primary,
                true,
                gated(true, view, |this, cx| this.run_route(cx)),
                cx,
            ));
        if let Some(report) = &self.route_report {
            column = column.child(self.route_result(report, theme, cx));
        }
        settings_section(
            "Try a route",
            "Which agent a run with these options would get, in order, without starting it. Auto asks the classifier, then prices each model for the prompt.",
            None,
            column,
            cx,
        )
    }

    fn route_result(&self, report: &RouteView, theme: Theme, cx: &App) -> gpui::AnyElement {
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(theme.space_2))
            .opacity(self.result_enter.value().clamp(0.0, 1.0))
            .text_size(px(theme.font_size_sm));
        if !report.reason.is_empty() {
            column = column.child(
                div()
                    .text_color(theme.color_text_secondary)
                    .child(report.reason.clone()),
            );
        }
        if let Some(difficulty) = report.difficulty {
            column = column.child(
                div()
                    .text_color(theme.color_text_muted)
                    .child(format!("Difficulty {}", fixed2(difficulty))),
            );
        }
        if let Some(role) = &report.role {
            column = column.child(div().text_color(theme.color_text_muted).child(role.clone()));
        }
        if report.candidates.is_empty() {
            column = column.child(
                div()
                    .text_color(theme.color_text_muted)
                    .child("No agent can take this run."),
            );
        }
        for (index, candidate) in report.candidates.iter().enumerate() {
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(theme.space_2))
                .child(
                    div()
                        .w(px(20.0))
                        .text_color(theme.color_text_muted)
                        .child((index + 1).to_string()),
                )
                .child(
                    div()
                        .font_family(theme.font_mono)
                        .child(candidate.agent.clone()),
                )
                .child(
                    div()
                        .font_family(theme.font_mono)
                        .text_color(theme.color_text_muted)
                        .child(candidate.model.clone()),
                )
                .child(div().flex_1());
            if let Some(auto) = &candidate.auto {
                let tone = if auto.qualified {
                    StatusTone::Ok
                } else {
                    StatusTone::Muted
                };
                let chip_label = if auto.qualified {
                    "Likely to pass"
                } else {
                    "Below the bar"
                };
                let mut metrics = div()
                    .id(("settings-auto", index))
                    .flex()
                    .items_center()
                    .gap(px(theme.space_2))
                    .text_size(px(theme.font_size_xs))
                    .text_color(theme.color_text_muted)
                    .child(format!("P {}", fixed2(auto.p_pass)))
                    .child(format!("${}", fixed2(auto.effective_cost)))
                    .child(format!("q {}", fixed2(auto.quota_multiplier)))
                    .child(chip(chip_label, tone, cx));
                if !candidate.reason.is_empty() {
                    metrics = metrics.tooltip(tooltip(candidate.reason.clone()));
                }
                row = row.child(metrics);
            } else if !candidate.reason.is_empty() {
                row = row.child(
                    div()
                        .text_color(theme.color_text_muted)
                        .child(candidate.reason.clone()),
                );
            }
            column = column.child(row);
        }
        for line in &report.eliminated {
            column = column.child(div().text_color(theme.color_text_muted).child(line.clone()));
        }
        column.into_any_element()
    }

    fn classifier_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let body = if self.policy.is_none() {
            note("No policy read yet.", theme)
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
                                    .pt(px(theme.space_2))
                                    .px(px(theme.space_2))
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
            "Classifier",
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
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(theme.space_2));
        if agent.is_empty() {
            row = row.child(
                div()
                    .text_size(px(theme.font_size_xs))
                    .text_color(theme.color_text_muted)
                    .child("No agent of the easy tier is installed."),
            );
        } else {
            row = row.child(
                div()
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
                                .font_family(theme.font_mono)
                                .text_size(px(theme.font_size_sm))
                                .text_color(theme.color_text_muted)
                                .child(model),
                        )
                    }),
            );
        }
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
            let _ = policy.roles;
            settings_row(
                "Pick a role",
                Some(
                    "A pick never makes a run read-only, and its tier is a floor. Edit the roles and their boards on the Roles page."
                        .into(),
                ),
                self.roles.clone(),
                cx,
            )
        } else {
            note("No policy read yet.", theme)
        };
        settings_section(
            "Roles",
            "With a classifier on, an Auto run that names no role gets the one whose When fits, asked in the same call. Jev and the agent CLI model pick one; OpenAI Decisions does not.",
            None,
            body,
            cx,
        )
    }

    fn classify_page(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let prompt = self.classify_prompt.read(cx).text().to_string();
        let enabled = draft_ready(&prompt) && self.classifier_on();
        let view = cx.entity().downgrade();
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(theme.space_3))
            .child(settings_field("Prompt", self.classify_prompt.clone(), cx))
            .child(dim_button(
                "settings-classify",
                "Classify",
                ButtonKind::Primary,
                enabled,
                gated(enabled, view, |this, cx| {
                    let prompt = trimmed_draft(this.classify_prompt.read(cx).text());
                    if this.classifier_on() {
                        this.classify_text(prompt, cx);
                    }
                }),
                cx,
            ));
        if self.classify_report.is_some() {
            column = column.child(self.classify_result(theme, cx));
        }
        settings_section(
            "Classify a prompt",
            "How hard the classifier judges a prompt, the tier Auto would route it to, and the role it would pick.",
            None,
            column,
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
        let description = if self.project_dir.is_empty() {
            "Open a project to give it its own policy."
        } else {
            "Read and write this project's .dispatch/config.json, over the global policy."
        };
        let view = cx.entity().downgrade();
        let mut body = div().flex().flex_col().child(settings_row(
            "This project only",
            Some(description.into()),
            div()
                .when(self.project_dir.is_empty(), |row| row.opacity(0.45))
                .child(self.scope.clone()),
            cx,
        ));
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
                        self.priority_menu.clone(),
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
                    self.discover.clone(),
                    cx,
                ));
        } else {
            body = body.child(note("No policy read yet.", theme));
        }
        let mut column = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme.space_6))
            .child(settings_section(
                "Routing policy",
                "How runs pick a model, for every repository or only this project.",
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
                .when(active, |row| row.child(self.token_popover.clone())),
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
        div()
            .size_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .font_family(theme.font_sans)
            .text_color(theme.color_text_primary)
            .child(toolbar(div(), div(), cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(self.rail(theme, cx))
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .p(px(theme.space_6))
                            .opacity(entered)
                            .child(self.page_body(&page, theme, cx)),
                    ),
            )
    }
}

fn project_binding(workspaces: &[Workspace]) -> (String, String) {
    workspaces
        .iter()
        .find(|workspace| !workspace.path.trim().is_empty())
        .map(|workspace| (workspace.path.clone(), workspace.id.clone()))
        .unwrap_or_else(|| (String::new(), String::new()))
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

fn text_or_none(text: &str) -> Option<SharedString> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_string().into())
    }
}

fn watch_input(input: &Entity<TextInput>, cx: &mut Context<SettingsView>) {
    cx.observe(input, |_, _, cx| cx.notify()).detach();
}

fn prompt_input(cx: &mut Context<SettingsView>) -> Entity<TextInput> {
    cx.new(TextInput::new)
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

fn menu_trigger(
    id: &'static str,
    label: SharedString,
    menu: Entity<Menu>,
    on_open: Rc<dyn Fn(&mut Window, &mut App)>,
    theme: Theme,
    cx: &App,
) -> gpui::AnyElement {
    div()
        .relative()
        .w(px(168.0))
        .child(
            div()
                .id(id)
                .h(px(theme.control_height_md))
                .w_full()
                .px(px(theme.space_3))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme.space_2))
                .rounded(px(theme.radius_sm))
                .bg(theme.color_surface_subtle)
                .border_1()
                .border_color(theme.color_border)
                .cursor_pointer()
                .on_click(move |_, window, cx| on_open(window, cx))
                .child(
                    div()
                        .text_size(px(theme.font_size_sm))
                        .text_color(theme.color_text_primary)
                        .child(label),
                )
                .child(icon(Icon::ChevronDown, 14.0, cx)),
        )
        .child(menu)
        .into_any_element()
}

fn token_panel(view: &WeakEntity<SettingsView>, cx: &mut App) -> gpui::AnyElement {
    let Some(view) = view.upgrade() else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let (options, draft) = {
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
        (options, settings.token_draft.clone())
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
        list = list.child(
            div()
                .id(("settings-token-option", index))
                .h(px(theme.control_height_md))
                .px(px(theme.space_2))
                .flex()
                .items_center()
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
                .child(label),
        );
    }
    div()
        .w(px(220.0))
        .flex()
        .flex_col()
        .gap(px(theme.space_2))
        .child(list)
        .child(draft)
        .into_any_element()
}
