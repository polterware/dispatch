//! Roles: the list, each role's tier board, and the composer.
//!
//! General is the tier table. Every other role has the same board; an empty tier runs on General's.
//! The core is called only from `cx.background_spawn`. Subagents and Config get an empty `dir`
//! because this window has no project directory.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, DispatchPhase, Entity,
    FocusHandle, FontWeight, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Render, SharedString, Window, canvas, deferred, div, point,
    prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::client::{
    BuildTiersRequest, CheckTiersRequest, CoreClient, CoreError, DispatchAgent, DispatchTier,
    ListAgentsRequest, ListModelsRequest, ListTiersRequest, ReplaceTiersRequest, SubagentsRequest,
    at_once,
};
use crate::models::roles::{
    GENERAL_ROLE, PaletteModel, RoleDraft, RoleListRow, RoleProfile, RoleRow, RoleSource,
    RoleTemplate, RolesList, TIER_COLUMNS, TierBuild, TierCardInput, TierColumns, TierSlot,
    add_column, available_templates, board_row_count, build_board, compose_profile, is_build_empty,
    kept_columns, models_label, palette_card_at, palette_groups, parse_roles_list,
    parse_tier_build, place_tier_card, profile_error, profile_with_board, remove_column,
    remove_tier_card, rename_drops_previous, replace_tier_card, role_board_tiers, role_list_rows,
    role_models_prompt, role_name_error, stored_columns, tier_column_label, tier_columns,
    tier_columns_in, tier_matches, when_display,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, Route, ToastTone, observe};
use crate::theme::{REM, StatusTone, Theme, mix, tw};
use crate::ui::ellipsis;
use crate::ui::hang::hang;
use crate::ui::parts::roles::{
    build_sections, muted_note, order_mark, role_board_note, role_summary,
};
use crate::ui::patterns::{
    AnchorSlot, ConfirmDialog, Property, PropertyOption, PropertyPicker, PropertyPickerEvent,
    PropertyPills, PropertyPillsEvent, composer_card, composer_field, composer_footer, modal_layer,
    window_anchor, window_layer,
};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Column, DataTable, Edge, Icon, IconTile, MascotMood, Menu,
    MenuItem, PointerTiming, TextInput, TileTone, button, card, chip, control_drop, drop, edge,
    face, fade_text, floating_panel, icon_button, icon_colored, mono, page_content, page_empty,
    page_under, page_under_inset, primary_cell, sans, tabular, text_width, toolbar, toolbar_center,
    track, use_pointer_timed, wrap_text,
};
use crate::ui::{SkeletonLayout, list_skeleton, page_skeleton};

/// The list's `PageContent` scroll, and a board's.
const ROLES_PAGE: &str = "roles-page";
const ROLES_BOARD: &str = "roles-board";
/// `DispatchAgentAvatar`'s default size.
const AVATAR: f32 = 28.0;
/// `TierModelPalette`: its popover's width and its tallest.
const PALETTE_WIDTH: f32 = 400.0;
const PALETTE_MAX_H: f32 = 360.0;
/// The board grid's order column and its gap (`72px repeat(3, minmax(0, 1fr))`, `gap: 10px`).
const ORDER_W: f32 = 72.0;
const GAP: f32 = 10.0;
/// `DRAG_THRESHOLD`: a press becomes a drag after 4px.
const DRAG_DISTANCE_SQ: f32 = 16.0;
/// A one-line tier card's height: its padding around the name and the agent / provider line
/// (taller than the bot). Add model and an empty slot take it, so a slot is a card's size.
const CARD_MIN_H: f32 = 2.0 * (3.0 * REM / 4.0) + 13.0 * 1.35 + 0.5 * REM / 4.0 + 11.0 * 1.5;
/// `.pw-tier-ghost__card`, 12px below and right of the pointer.
const GHOST_W: f32 = 220.0;
const GHOST_OFFSET: f32 = 12.0;
/// `.pw-tier-card--lifted` (`brightness(0.6)`) and `--dimmed` (`grayscale(1) brightness(0.55)`),
/// as opacity over the page: GPUI has no filters.
const LIFTED_OPACITY: f32 = 0.6;
const DIMMED_OPACITY: f32 = 0.55;

/// A press on a card, a drag once it moves 4px. The board keeps the stored table while it
/// moves: the card it left steps back, the cell under the pointer is outlined, and a ghost
/// follows the pointer.
struct PointerDrag {
    card: TierCardInput,
    from: TierSlot,
    origin_x: f32,
    origin_y: f32,
    active: bool,
    over: Option<TierSlot>,
    /// The pointer, in page coordinates.
    pointer: (f32, f32),
    /// The press landed on the card whose swap palette was open: letting go leaves it closed.
    was_open: bool,
}

/// What the page draws: answered at once, so it waits on no agent CLI.
struct LoadBundle {
    tiers: Result<Vec<DispatchTier>, String>,
    roles: Result<RolesList, String>,
}

/// What can come after: the tier check asks the agent CLIs for their models, and the agents'
/// list probes their quota, either of which can take seconds when the core's copy is stale.
struct Extras {
    problems: Result<Vec<String>, String>,
    agents: Vec<String>,
}

pub struct RolesView {
    state: Entity<AppState>,
    roles: Vec<RoleRow>,
    templates: Vec<RoleTemplate>,
    tiers: Vec<DispatchTier>,
    tier_problems: Vec<String>,
    agents: Vec<String>,
    palette_models: Vec<PaletteModel>,
    palette_problems: Vec<String>,
    palette_loaded: bool,
    palette_loading: bool,
    tier_query: String,
    palette_query: String,
    tier_search: Entity<TextInput>,
    palette_search: Entity<TextInput>,
    custom_agent: Entity<TextInput>,
    custom_model: Entity<TextInput>,
    name_input: Entity<TextInput>,
    instructions_input: Entity<TextInput>,
    when_input: Entity<TextInput>,
    timeout_input: Entity<TextInput>,
    /// The Composer's Lowest tier, Access, Picked, Prefer, and Avoid pills.
    pills: Entity<PropertyPills>,
    /// `None` is closed, `Some("")` is a new role, otherwise the name being edited.
    composer: Option<String>,
    draft_tier: usize,
    draft_readonly: bool,
    draft_manual: bool,
    draft_prefer: Vec<String>,
    draft_avoid: Vec<String>,
    build_open: bool,
    build: Option<TierBuild>,
    build_loading: bool,
    build_applying: bool,
    build_gen: u64,
    /// The board the open proposal is for (General's name for the tier table), and who ranked it.
    build_role: String,
    build_judge: String,
    build_menu: Entity<Menu>,
    /// New role's choice of a blank role or a template not in use.
    new_menu: Entity<Menu>,
    /// The role the Remove dialog asks about, and the handle that holds Enter and Escape.
    removing: Option<String>,
    confirm_focus: FocusHandle,
    /// Add column: the picker of the missing tiers, and the button it hangs from.
    column_picker: Entity<PropertyPicker>,
    column_anchor: AnchorSlot,
    drag: Option<PointerDrag>,
    motion_for: Option<String>,
    row_hover: HashMap<String, SpringValue>,
    cell_bounds: Rc<RefCell<HashMap<(String, usize), Bounds<gpui::Pixels>>>>,
    page_origin: Rc<RefCell<(f32, f32)>>,
    animator: Animator,
    composer_opacity: SpringValue,
    composer_y: SpringValue,
    build_opacity: SpringValue,
    build_y: SpringValue,
    palette_opacity: SpringValue,
    palette_y: SpringValue,
    palette_slot: Option<TierSlot>,
    palette_anchor: Option<TierSlot>,
    /// The palette swaps the card at `palette_slot` instead of adding one there.
    palette_replace: bool,
    fetch_gen: u64,
    tier_write: u64,
    role_write: u64,
    loading: bool,
    visit: bool,
    loaded: bool,
    /// A save is on its way: a second press waits for it.
    saving: bool,
    announced_error: bool,
}

impl RolesView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        observe(&state, cx);
        let view = cx.entity().clone();
        cx.observe(&state, |this, _, cx| this.ensure_loaded(cx))
            .detach();

        let tier_search = cx.new(|cx| TextInput::new(cx).placeholder("Search tiers…"));
        let palette_search = cx.new(|cx| TextInput::new(cx).pill().placeholder("Search models…"));
        let custom_agent = cx.new(|cx| TextInput::new(cx).bare().placeholder("Agent"));
        let custom_model = {
            let view = view.clone();
            cx.new(move |cx| {
                TextInput::new(cx)
                    .bare()
                    .placeholder("Model")
                    .on_submit(move |_, _, cx| {
                        view.update(cx, |this, cx| this.add_custom(cx));
                    })
            })
        };
        let name_input = {
            let submit = view.clone();
            let cancel = view.clone();
            cx.new(move |cx| {
                TextInput::new(cx)
                    .bare()
                    .title()
                    .wrap()
                    .placeholder("Name, such as reviewer")
                    .on_submit(move |_, _, cx| {
                        submit.update(cx, |this, cx| this.submit_composer(cx));
                    })
                    .on_cancel(move |_, cx| {
                        cancel.update(cx, |this, cx| this.close_composer(cx));
                    })
            })
        };
        let instructions_input = {
            let submit = view.clone();
            let cancel = view.clone();
            cx.new(move |cx| {
                TextInput::new(cx)
                    .description()
                    .wrap()
                    .placeholder("Instructions put before every task it takes")
                    .on_submit(move |_, _, cx| {
                        submit.update(cx, |this, cx| this.submit_composer(cx));
                    })
                    .on_cancel(move |_, cx| {
                        cancel.update(cx, |this, cx| this.close_composer(cx));
                    })
            })
        };
        let when_input = {
            let cancel = view.clone();
            cx.new(move |cx| {
                TextInput::new(cx)
                    .inline()
                    .wrap()
                    .placeholder("The tasks it fits, for the classifier that picks it")
                    .on_cancel(move |_, cx| {
                        cancel.update(cx, |this, cx| this.close_composer(cx));
                    })
            })
        };
        let timeout_input = {
            let cancel = view.clone();
            cx.new(move |cx| {
                TextInput::new(cx)
                    .inline()
                    .wrap()
                    .placeholder("Such as 15m; empty uses the default")
                    .on_cancel(move |_, cx| {
                        cancel.update(cx, |this, cx| this.close_composer(cx));
                    })
            })
        };
        let pills = cx.new(PropertyPills::new);
        cx.subscribe(&pills, |this, _, event: &PropertyPillsEvent, cx| {
            this.on_property(event, cx);
        })
        .detach();

        let column_picker = cx.new(PropertyPicker::new);
        cx.subscribe(
            &column_picker,
            |this, _, event: &PropertyPickerEvent, cx| {
                if let PropertyPickerEvent::Pick(tier) = event {
                    this.add_column(tier.to_string(), cx);
                }
            },
        )
        .detach();

        let tier_watch = tier_search.clone();
        cx.observe(&tier_watch, |this, input, cx| {
            let next = input.read(cx).text().to_string();
            if this.tier_query != next {
                this.tier_query = next;
                cx.notify();
            }
        })
        .detach();
        let palette_watch = palette_search.clone();
        cx.observe(&palette_watch, |this, input, cx| {
            let next = input.read(cx).text().to_string();
            if this.palette_query != next {
                this.palette_query = next;
                cx.notify();
            }
        })
        .detach();
        let deferred = view.clone();
        cx.defer(move |app| {
            deferred.update(app, |this, cx| this.ensure_loaded(cx));
        });

        Self {
            state,
            roles: Vec::new(),
            templates: Vec::new(),
            tiers: Vec::new(),
            tier_problems: Vec::new(),
            agents: Vec::new(),
            palette_models: Vec::new(),
            palette_problems: Vec::new(),
            palette_loaded: false,
            palette_loading: false,
            tier_query: String::new(),
            palette_query: String::new(),
            tier_search,
            palette_search,
            custom_agent,
            custom_model,
            name_input,
            instructions_input,
            when_input,
            timeout_input,
            pills,
            composer: None,
            draft_tier: 0,
            draft_readonly: false,
            draft_manual: false,
            draft_prefer: Vec::new(),
            draft_avoid: Vec::new(),
            build_open: false,
            build: None,
            build_loading: false,
            build_applying: false,
            build_gen: 0,
            build_role: GENERAL_ROLE.to_string(),
            build_judge: String::new(),
            build_menu: cx.new(Menu::new),
            new_menu: cx.new(Menu::new),
            removing: None,
            confirm_focus: cx.focus_handle(),
            column_picker,
            column_anchor: AnchorSlot::default(),
            drag: None,
            motion_for: None,
            row_hover: HashMap::new(),
            cell_bounds: Rc::new(RefCell::new(HashMap::new())),
            page_origin: Rc::new(RefCell::new((0.0, 0.0))),
            animator: Animator::new(),
            composer_opacity: SpringValue::new(0.0, SpringTier::Fast),
            composer_y: SpringValue::new(8.0, SpringTier::Fast),
            build_opacity: SpringValue::new(0.0, SpringTier::Fast),
            build_y: SpringValue::new(8.0, SpringTier::Fast),
            palette_opacity: SpringValue::new(0.0, SpringTier::Fast),
            palette_y: SpringValue::new(6.0, SpringTier::Fast),
            palette_slot: None,
            palette_anchor: None,
            palette_replace: false,
            fetch_gen: 0,
            tier_write: 0,
            role_write: 0,
            loading: false,
            visit: false,
            loaded: false,
            saving: false,
            announced_error: false,
        }
    }

    fn on_roles(route: &Route) -> bool {
        matches!(route, Route::Roles | Route::RoleBoard(_))
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let on_page = Self::on_roles(self.state.read(cx).route());
        if !on_page {
            self.visit = false;
            self.announced_error = false;
            return;
        }
        if self.visit || self.loading {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.loading = true;
        let generation = self.bump_fetch();
        cx.spawn(async move |this, cx| {
            // Both loads start now; the extras, which wait on the agent CLIs, land after the
            // roles and the tier table are drawn.
            let first = client.clone();
            let bundle = cx.background_spawn(async move { load_all(&first) });
            let extras = cx.background_spawn(async move { load_extras(&client) });
            let bundle = bundle.await;
            let current = this
                .update(cx, |view, cx| {
                    if view.fetch_gen != generation {
                        view.loading = false;
                        return false;
                    }
                    view.apply_load(bundle, cx);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let extras = extras.await;
            let _ = this.update(cx, |view, cx| {
                if view.fetch_gen == generation {
                    view.apply_extras(extras);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn bump_fetch(&mut self) -> u64 {
        self.fetch_gen = self.fetch_gen.wrapping_add(1);
        self.fetch_gen
    }

    fn apply_load(&mut self, bundle: LoadBundle, cx: &mut Context<Self>) {
        self.loading = false;
        let mut roles_ok = false;
        let mut tiers_ok = false;
        if let Ok(tiers) = bundle.tiers {
            self.tiers = tiers;
            tiers_ok = true;
        } else if let Err(message) = bundle.tiers {
            self.note_error(message, cx);
        }
        if let Ok(list) = bundle.roles {
            self.roles = list.rows;
            self.templates = list.templates;
            roles_ok = true;
            self.loaded = true;
            self.bounce_missing(cx);
        } else if let Err(message) = bundle.roles {
            self.note_error(message, cx);
        }
        if roles_ok && tiers_ok {
            self.visit = true;
            self.announced_error = false;
        } else {
            self.visit = false;
        }
    }

    fn apply_extras(&mut self, extras: Extras) {
        if let Ok(problems) = extras.problems {
            self.tier_problems = problems;
        }
        if !extras.agents.is_empty() {
            self.agents = extras.agents;
        }
    }

    fn note_error(&mut self, message: String, cx: &mut Context<Self>) {
        if self.announced_error {
            return;
        }
        self.announced_error = true;
        self.toast(message, ToastTone::Danger, cx);
    }

    fn bounce_missing(&mut self, cx: &mut Context<Self>) {
        let Route::RoleBoard(name) = self.state.read(cx).route().clone() else {
            return;
        };
        if name == GENERAL_ROLE || self.roles.iter().any(|row| row.name == name) {
            return;
        }
        self.go(Route::Roles, cx);
    }

    fn toast(&self, message: impl Into<SharedString>, tone: ToastTone, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.push_toast(message, tone);
            cx.notify();
        });
    }

    fn go(&self, route: Route, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.navigate(route);
            cx.notify();
        });
    }

    fn board_name(&self, cx: &Context<Self>) -> Option<String> {
        match self.state.read(cx).route() {
            Route::RoleBoard(name) => Some(name.clone()),
            _ => None,
        }
    }

    fn role_row(&self, name: &str) -> Option<&RoleRow> {
        self.roles.iter().find(|row| row.name == name)
    }

    fn editable(&self, cx: &Context<Self>) -> bool {
        match self.board_name(cx).as_deref() {
            Some(name) if name == GENERAL_ROLE => true,
            Some(name) => self
                .role_row(name)
                .is_some_and(|row| row.source != RoleSource::Repository),
            None => false,
        }
    }

    fn committed(&self, cx: &Context<Self>) -> Vec<DispatchTier> {
        match self.board_name(cx).as_deref() {
            Some(name) if name == GENERAL_ROLE => self.tiers.clone(),
            Some(name) => self
                .role_row(name)
                .map(|row| role_board_tiers(&row.profile))
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// The tier columns the open board draws: General's all three, or the role's kept ones.
    fn open_columns(&self, cx: &Context<Self>) -> Vec<String> {
        match self.board_name(cx).as_deref() {
            Some(name) if name == GENERAL_ROLE => TIER_COLUMNS
                .iter()
                .map(|(id, _)| (*id).to_string())
                .collect(),
            Some(name) => self
                .role_row(name)
                .map(|row| kept_columns(&row.profile))
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn commit_board(&mut self, next: Vec<DispatchTier>, cx: &mut Context<Self>) {
        if self.state.read(cx).client().is_none() {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            cx.notify();
            return;
        }
        match self.board_name(cx).as_deref() {
            Some(name) if name == GENERAL_ROLE => self.save_tiers(next, cx),
            Some(name) => {
                let name = name.to_string();
                self.save_role_board(name, next, cx);
            }
            None => {}
        }
    }

    fn save_tiers(&mut self, next: Vec<DispatchTier>, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            return;
        };
        let previous = self.tiers.clone();
        self.tiers = next.clone();
        self.tier_write = self.tier_write.wrapping_add(1);
        let token = self.tier_write;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let saved = client.replace_tiers(&ReplaceTiersRequest { tiers: next })?;
                    let problems = client.check_tiers(&CheckTiersRequest {})?;
                    Ok::<_, CoreError>((saved.tiers, problems.problems))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.tier_write != token {
                    return;
                }
                match result {
                    Ok((tiers, problems)) => {
                        view.tiers = tiers;
                        view.tier_problems = problems;
                    }
                    Err(err) => {
                        view.tiers = previous;
                        view.toast(
                            core_message(err, "Could not save the tiers."),
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

    fn save_role_board(&mut self, name: String, next: Vec<DispatchTier>, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            return;
        };
        let Some(previous) = self.role_row(&name).map(|row| row.profile.clone()) else {
            return;
        };
        let updated = profile_with_board(&previous, &next);
        let profile = serde_json::to_string(&updated).unwrap_or_else(|_| "{}".to_string());
        let saved_name = name.clone();
        if let Some(row) = self.roles.iter_mut().find(|row| row.name == name) {
            row.profile = updated;
        }
        self.role_write = self.role_write.wrapping_add(1);
        let token = self.role_write;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let response = client
                        .subagents(&SubagentsRequest {
                            action: "save".into(),
                            dir: String::new(),
                            name,
                            profile,
                        })
                        .map_err(|err| core_message(err, "Could not save the role."))?;
                    if response.ok {
                        Ok(())
                    } else {
                        Err(if response.json.is_empty() {
                            "Could not save the role.".to_string()
                        } else {
                            response.json
                        })
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.role_write != token {
                    return;
                }
                if let Err(message) = result {
                    if let Some(row) = view.roles.iter_mut().find(|row| row.name == saved_name) {
                        row.profile = previous;
                    }
                    view.toast(message, ToastTone::Danger, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn reload_tiers(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.tier_write = self.tier_write.wrapping_add(1);
        let token = self.tier_write;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let (tiers, problems) = at_once(
                        || client.list_tiers(&ListTiersRequest {}),
                        || client.check_tiers(&CheckTiersRequest {}),
                    );
                    Ok::<_, CoreError>((tiers?.tiers, problems?.problems))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.tier_write != token {
                    return;
                }
                if let Ok((tiers, problems)) = result {
                    view.tiers = tiers;
                    view.tier_problems = problems;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_composer(&mut self, name: Option<&str>, cx: &mut Context<Self>) {
        let row = name
            .and_then(|name| self.roles.iter().find(|row| row.name == name))
            .cloned();
        let profile = row
            .as_ref()
            .map(|row| row.profile.clone())
            .unwrap_or_default();
        self.composer = Some(name.unwrap_or("").to_string());
        let title = name.unwrap_or("").to_string();
        self.name_input
            .update(cx, |input, cx| input.set_text(title, cx));
        self.fill_draft(&profile, cx);
    }

    /// A new role from a built-in template: the composer opens with its words, tier,
    /// access, time limit, and preferences, and the name is the user's to give.
    fn open_composer_from_template(&mut self, template: &RoleTemplate, cx: &mut Context<Self>) {
        self.open_composer(None, cx);
        self.fill_draft(&template.profile, cx);
    }

    fn fill_draft(&mut self, profile: &RoleProfile, cx: &mut Context<Self>) {
        self.draft_tier = match profile.tier.as_str() {
            "easy" => 1,
            "normal" => 2,
            "hard" => 3,
            _ => 0,
        };
        self.draft_readonly = profile.readonly;
        self.draft_manual = profile.manual;
        self.draft_prefer = profile.prefer.clone();
        self.draft_avoid = profile.avoid.clone();
        let instructions = profile.instructions.clone();
        let when = profile.when.clone();
        let timeout = profile.timeout.clone();
        self.instructions_input
            .update(cx, |input, cx| input.set_text(instructions, cx));
        self.when_input
            .update(cx, |input, cx| input.set_text(when, cx));
        self.timeout_input
            .update(cx, |input, cx| input.set_text(timeout, cx));
        let properties = self.role_properties();
        self.pills
            .update(cx, |pills, cx| pills.set_properties(properties, cx));
        cx.notify();
    }

    /// The pills again from the draft, after a change one pill made to another.
    fn refresh_pills(&mut self, cx: &mut Context<Self>) {
        let properties = self.role_properties();
        self.pills
            .update(cx, |pills, cx| pills.set_properties(properties, cx));
    }

    /// Fills the draft's Prefer and Avoid (the screenshot tour).
    pub(crate) fn draft_tokens(&mut self, prefer: &[&str], avoid: &[&str], cx: &mut Context<Self>) {
        let owned = |list: &[&str]| list.iter().map(|name| name.to_string()).collect();
        self.draft_prefer = owned(prefer);
        self.draft_avoid = owned(avoid);
        let properties = self.role_properties();
        self.pills
            .update(cx, |pills, cx| pills.set_properties(properties, cx));
        cx.notify();
    }

    /// Opens the picker of the Composer's pill `index` (the screenshot tour).
    pub(crate) fn open_pill(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.pills
            .update(cx, |pills, cx| pills.toggle(index, window, cx));
        cx.notify();
    }

    /// The Composer's pills from the draft, as the Solid roles page lists them.
    fn role_properties(&self) -> Vec<Property> {
        let options = |pairs: &[(&'static str, &'static str)]| -> Vec<PropertyOption> {
            pairs
                .iter()
                .map(|(value, label)| PropertyOption::new(*value, *label))
                .collect()
        };
        let agents: Vec<PropertyOption> = self
            .agents
            .iter()
            .map(|name| {
                let agent = name.clone();
                PropertyOption::new(name.clone(), name.clone())
                    .mark(move |size, _, _| agent_avatar(&agent, "", size).into_any_element())
            })
            .collect();
        let names = |list: &[String]| -> Vec<SharedString> {
            list.iter()
                .map(|name| SharedString::from(name.clone()))
                .collect()
        };
        vec![
            Property::single(
                "tier",
                "Lowest tier",
                options(&[
                    ("", "Any tier"),
                    ("easy", "Easy"),
                    ("normal", "Normal"),
                    ("hard", "Hard"),
                ]),
                ["", "easy", "normal", "hard"][self.draft_tier.min(3)],
            )
            .icon(Icon::Layers),
            Property::single(
                "access",
                "Access",
                vec![
                    PropertyOption::new("edit", "Can change files").icon(Icon::Pencil),
                    PropertyOption::new("readonly", "Read only").icon(Icon::Eye),
                ],
                if self.draft_readonly {
                    "readonly"
                } else {
                    "edit"
                },
            ),
            Property::single(
                "pick",
                "Picked",
                vec![
                    PropertyOption::new("auto", "Picked automatically").icon(Icon::Zap),
                    PropertyOption::new("manual", "Only with --role").icon(Icon::Terminal),
                ],
                if self.draft_manual { "manual" } else { "auto" },
            ),
            Property::tokens(
                "prefer",
                "Prefer",
                agents.clone(),
                names(&self.draft_prefer),
                "None",
            )
            .icon(Icon::ThumbsUpIcon),
            Property::tokens("avoid", "Avoid", agents, names(&self.draft_avoid), "None")
                .icon(Icon::ThumbsDownIcon),
        ]
    }

    /// A pill changed: the draft takes its values.
    fn on_property(&mut self, event: &PropertyPillsEvent, cx: &mut Context<Self>) {
        let PropertyPillsEvent::Changed { id, values } = event;
        let first = values.first().map(|value| value.as_ref()).unwrap_or("");
        let names = || values.iter().map(|value| value.to_string()).collect();
        match id.as_ref() {
            "tier" => {
                self.draft_tier = match first {
                    "easy" => 1,
                    "normal" => 2,
                    "hard" => 3,
                    _ => 0,
                }
            }
            "access" => self.draft_readonly = first == "readonly",
            "pick" => self.draft_manual = first == "manual",
            // An agent is preferred or avoided, never both: picking it in one list takes it
            // out of the other.
            "prefer" => {
                self.draft_prefer = names();
                let prefer = self.draft_prefer.clone();
                self.draft_avoid.retain(|agent| !prefer.contains(agent));
                self.refresh_pills(cx);
            }
            "avoid" => {
                self.draft_avoid = names();
                let avoid = self.draft_avoid.clone();
                self.draft_prefer.retain(|agent| !avoid.contains(agent));
                self.refresh_pills(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn close_composer(&mut self, cx: &mut Context<Self>) {
        self.composer = None;
        cx.notify();
    }

    fn submit_composer(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.composer.clone() else {
            return;
        };
        let name = self.name_input.read(cx).text().trim().to_string();
        if let Some(error) = role_name_error(&name) {
            self.toast(error, ToastTone::Danger, cx);
            return;
        }
        let draft = RoleDraft {
            tier: match self.draft_tier {
                1 => "easy".to_string(),
                2 => "normal".to_string(),
                3 => "hard".to_string(),
                _ => String::new(),
            },
            readonly: self.draft_readonly,
            manual: self.draft_manual,
            prefer: self.draft_prefer.clone(),
            avoid: self.draft_avoid.clone(),
            instructions: self.instructions_input.read(cx).text().to_string(),
            when: self.when_input.read(cx).text().to_string(),
            timeout: self.timeout_input.read(cx).text().to_string(),
        };
        let existing = if previous.is_empty() {
            None
        } else {
            self.roles.iter().find(|row| row.name == previous).cloned()
        };
        let source = existing
            .as_ref()
            .map(|row| row.source)
            .unwrap_or(RoleSource::User);
        let profile = compose_profile(existing.as_ref().map(|row| &row.profile), &draft);
        if let Some(error) = profile_error(&profile) {
            self.toast(error, ToastTone::Danger, cx);
            return;
        }
        if self.saving {
            return;
        }
        let profile_json = serde_json::to_string(&profile).unwrap_or_else(|_| "{}".to_string());
        let drops = rename_drops_previous(&previous, &name, source);
        let Some(client) = self.state.read(cx).client() else {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            return;
        };
        let snapshot = self.roles.clone();
        self.upsert_role(name.clone(), profile, RoleSource::User);
        if drops {
            let old = previous.clone();
            self.roles.retain(|row| row.name != old);
        }
        self.saving = true;
        let generation = self.bump_fetch();
        let saved_name = name.clone();
        let drop_name = previous.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    save_role_ops(
                        &client,
                        &saved_name,
                        &profile_json,
                        drops.then_some(drop_name.as_str()),
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(list) => {
                        if view.fetch_gen == generation
                            && let Some(list) = list
                        {
                            view.roles = list.rows;
                            view.templates = list.templates;
                        }
                        view.composer = None;
                        view.toast(
                            format!("Saved {name}. Runs take it with --role {name}."),
                            ToastTone::Success,
                            cx,
                        );
                        view.go(Route::RoleBoard(name), cx);
                    }
                    Err(message) => {
                        view.roles = snapshot;
                        view.toast(message, ToastTone::Danger, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn upsert_role(&mut self, name: String, profile: RoleProfile, source: RoleSource) {
        if let Some(row) = self.roles.iter_mut().find(|row| row.name == name) {
            row.profile = profile;
            row.source = source;
        } else {
            self.roles.push(RoleRow {
                name,
                profile,
                source,
            });
        }
        self.roles.sort_by(|left, right| left.name.cmp(&right.name));
    }

    /// Deletes a role for good, built-in or the user's own. A built-in stays gone through
    /// `roles.json`'s `removed` list and remains a template.
    fn remove_role(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            return;
        };
        let generation = self.bump_fetch();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let response = client
                        .subagents(&SubagentsRequest {
                            action: "remove".into(),
                            dir: String::new(),
                            name: name.clone(),
                            profile: String::new(),
                        })
                        .map_err(|err| core_message(err, "Could not remove the role."))?;
                    if !response.ok {
                        return Err(if response.json.is_empty() {
                            "Could not remove the role.".to_string()
                        } else {
                            response.json
                        });
                    }
                    let listed = client
                        .subagents(&SubagentsRequest {
                            action: "list".into(),
                            dir: String::new(),
                            name: String::new(),
                            profile: String::new(),
                        })
                        .ok()
                        .filter(|response| response.ok)
                        .and_then(|response| parse_roles_list(&response.json).ok());
                    Ok((name, listed))
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok((name, listed)) => {
                        if view.fetch_gen == generation {
                            if let Some(list) = listed {
                                view.roles = list.rows;
                                view.templates = list.templates;
                            } else {
                                view.roles.retain(|row| row.name != name);
                            }
                        }
                        view.toast(format!("Removed {name}."), ToastTone::Success, cx);
                        view.composer = None;
                        view.go(Route::Roles, cx);
                    }
                    Err(message) => view.toast(message, ToastTone::Danger, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Proposes `role`'s board (General's name for the tier table), ranked by the catalog or,
    /// with `judge` "jev", by the Decision API (Jev), or with "local", by the local model, the
    /// agent CLI model Settings › Classifier names.
    fn open_build(&mut self, role: &str, judge: &str, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            return;
        };
        self.build_role = role.to_string();
        self.build_judge = judge.to_string();
        let (role, judge) = (
            if role == GENERAL_ROLE {
                String::new()
            } else {
                role.to_string()
            },
            judge.to_string(),
        );
        self.build_open = true;
        self.build = None;
        self.build_loading = true;
        self.build_applying = false;
        self.build_gen = self.build_gen.wrapping_add(1);
        let token = self.build_gen;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { run_build(&client, false, &role, &judge) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.build_gen != token {
                    return;
                }
                view.build_loading = false;
                match result {
                    Ok(build) => view.build = Some(build),
                    Err(message) => view.toast(message, ToastTone::Danger, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_build(&mut self, cx: &mut Context<Self>) {
        if self.build_loading || self.build_applying {
            return;
        }
        if self.build.as_ref().is_none_or(is_build_empty) {
            return;
        }
        // A role's board, or a table Jev ranked, is saved as shown: asking again could answer
        // otherwise.
        if self.build_role != GENERAL_ROLE || !self.build_judge.is_empty() {
            let Some(next) = self.build.as_ref().map(build_board) else {
                return;
            };
            let role = self.build_role.clone();
            self.build_open = false;
            if role == GENERAL_ROLE {
                self.save_tiers(next, cx);
            } else {
                self.save_role_board(role, next, cx);
            }
            cx.notify();
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            self.toast("The local core is not running.", ToastTone::Danger, cx);
            return;
        };
        self.build_applying = true;
        self.build_gen = self.build_gen.wrapping_add(1);
        let token = self.build_gen;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { run_build(&client, true, "", "") })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.build_gen != token {
                    return;
                }
                view.build_applying = false;
                match result {
                    Ok(build) if build.applied => {
                        view.build_open = false;
                        view.toast(
                            "The tiers now follow the router model.",
                            ToastTone::Success,
                            cx,
                        );
                        view.reload_tiers(cx);
                    }
                    Ok(build) => view.build = Some(build),
                    Err(message) => view.toast(message, ToastTone::Danger, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_loaded || self.palette_loading {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            self.palette_problems = vec!["The local core is not running.".to_string()];
            return;
        };
        self.palette_loading = true;
        self.palette_loaded = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.list_models(&ListModelsRequest {}) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(response) => {
                        view.palette_models = response
                            .models
                            .into_iter()
                            .map(|model| PaletteModel {
                                agent: model.agent,
                                model: model.model,
                                provider: model.provider,
                                tier: model.tier,
                                efforts: model.efforts,
                            })
                            .collect();
                        view.palette_problems = response.problems;
                        view.palette_loading = false;
                    }
                    Err(err) => {
                        view.palette_loaded = false;
                        view.palette_loading = false;
                        view.toast(
                            core_message(err, "Could not list models."),
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

    /// Opens the model palette from tier `tier`'s cell `index` (the screenshot tour).
    pub(crate) fn open_palette(&mut self, tier: &str, index: usize, cx: &mut Context<Self>) {
        self.palette_slot = None;
        self.toggle_palette(
            TierSlot {
                tier: tier.to_string(),
                index,
            },
            cx,
        );
    }

    fn toggle_palette(&mut self, slot: TierSlot, cx: &mut Context<Self>) {
        if self.palette_slot.as_ref() == Some(&slot) && !self.palette_replace {
            self.palette_slot = None;
        } else {
            self.palette_slot = Some(slot);
            self.palette_replace = false;
            self.load_palette(cx);
        }
        cx.notify();
    }

    /// Opens the palette on the card at `slot`, to swap it for another model.
    fn open_replace(&mut self, slot: TierSlot, cx: &mut Context<Self>) {
        self.palette_slot = Some(slot);
        self.palette_replace = true;
        self.load_palette(cx);
        cx.notify();
    }

    /// The model of the card the palette swaps, if it swaps one.
    fn replacing_model(&self, cx: &Context<Self>) -> Option<(String, String)> {
        let slot = self
            .palette_slot
            .as_ref()
            .filter(|_| self.palette_replace)?;
        tier_columns(&self.committed(cx))
            .get(&slot.tier)
            .get(slot.index)
            .map(|tier| (tier.agent.clone(), tier.model.clone()))
    }

    fn place_new(&mut self, card: TierCardInput, cx: &mut Context<Self>) {
        let Some(slot) = self.palette_slot.clone() else {
            return;
        };
        let committed = self.committed(cx);
        let next = if self.palette_replace {
            replace_tier_card(&committed, &card, &slot)
        } else {
            place_tier_card(&committed, &card, &slot, None)
        };
        self.palette_slot = None;
        self.palette_replace = false;
        if next != committed {
            self.commit_board(next, cx);
        } else {
            cx.notify();
        }
    }

    fn add_custom(&mut self, cx: &mut Context<Self>) {
        let agent = self.custom_agent.read(cx).text().trim().to_string();
        let model = self.custom_model.read(cx).text().trim().to_string();
        if agent.is_empty() || model.is_empty() {
            return;
        }
        self.custom_model
            .update(cx, |input, cx| input.set_text("", cx));
        self.place_new(
            TierCardInput {
                agent,
                model,
                provider: String::new(),
            },
            cx,
        );
    }

    fn remove_at(&mut self, slot: TierSlot, cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let next = remove_tier_card(&self.committed(cx), &slot);
        self.commit_board(next, cx);
    }

    /// Removes a non-General role's tier column and the models in it. The last one stays.
    fn delete_column(&mut self, tier: String, cx: &mut Context<Self>) {
        let Some(name) = self.board_name(cx) else {
            return;
        };
        if name == GENERAL_ROLE || !self.editable(cx) {
            return;
        }
        let Some(row) = self.role_row(&name) else {
            return;
        };
        let kept = kept_columns(&row.profile);
        if !kept.iter().any(|column| column == &tier) {
            return;
        }
        let next_columns = remove_column(&kept, &tier);
        if next_columns == kept {
            return;
        }
        let next: Vec<DispatchTier> = self
            .committed(cx)
            .into_iter()
            .filter(|card| card.name != tier)
            .collect();
        if let Some(row) = self.roles.iter_mut().find(|row| row.name == name) {
            row.profile.columns = stored_columns(&next_columns);
        }
        self.save_role_board(name, next, cx);
    }

    /// Adds a tier column back to a non-General role, empty.
    fn add_column(&mut self, tier: String, cx: &mut Context<Self>) {
        let Some(name) = self.board_name(cx) else {
            return;
        };
        if name == GENERAL_ROLE || !self.editable(cx) {
            return;
        }
        let Some(row) = self.role_row(&name) else {
            return;
        };
        let kept = kept_columns(&row.profile);
        let next_columns = add_column(&kept, &tier);
        if next_columns == kept {
            return;
        }
        if let Some(row) = self.roles.iter_mut().find(|row| row.name == name) {
            row.profile.columns = stored_columns(&next_columns);
        }
        let next = self.committed(cx);
        self.save_role_board(name, next, cx);
    }

    /// Opens the Add column picker under its button, or closes it.
    fn toggle_column_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.board_name(cx) else {
            return;
        };
        if name == GENERAL_ROLE || !self.editable(cx) {
            return;
        }
        if self.column_picker.read(cx).is_open() {
            self.column_picker.update(cx, |picker, cx| picker.close(cx));
            return;
        }
        let kept = self
            .role_row(&name)
            .map(|row| kept_columns(&row.profile))
            .unwrap_or_default();
        let options: Vec<PropertyOption> = TIER_COLUMNS
            .iter()
            .filter(|(id, _)| !kept.iter().any(|column| column == id))
            .map(|(id, label)| PropertyOption::new(*id, *label).hint("Adds an empty column"))
            .collect();
        if options.is_empty() {
            return;
        }
        let height = self
            .column_anchor
            .get()
            .map_or(28.0, |bounds| f32::from(bounds.size.height));
        self.column_picker.update(cx, |picker, cx| {
            picker.set_options("Column", options, None, 200.0, cx);
            picker.set_trigger_height(height);
            picker.open(window, cx);
        });
        cx.notify();
    }

    fn begin_drag(&mut self, slot: TierSlot, x: f32, y: f32, cx: &mut Context<Self>) {
        if !self.editable(cx) || self.composer.is_some() || self.build_open {
            return;
        }
        let was_open = self.palette_replace && self.palette_slot.as_ref() == Some(&slot);
        let committed = self.committed(cx);
        let Some(tier) = tier_columns(&committed)
            .get(&slot.tier)
            .get(slot.index)
            .cloned()
        else {
            return;
        };
        self.drag = Some(PointerDrag {
            card: TierCardInput::from_tier(&tier),
            from: slot,
            origin_x: x,
            origin_y: y,
            active: false,
            over: None,
            pointer: (0.0, 0.0),
            was_open,
        });
        cx.notify();
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        let x = f32::from(event.position.x);
        let y = f32::from(event.position.y);
        let (origin_x, origin_y) = *self.page_origin.borrow();
        let over = self.hit_slot(x, y);
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if !drag.active {
            let dx = x - drag.origin_x;
            let dy = y - drag.origin_y;
            if dx * dx + dy * dy < DRAG_DISTANCE_SQ {
                return;
            }
            drag.active = true;
        }
        drag.pointer = (x - origin_x, y - origin_y);
        drag.over = over;
        cx.notify();
    }

    fn hit_slot(&self, x: f32, y: f32) -> Option<TierSlot> {
        let cursor = point(px(x), px(y));
        self.cell_bounds
            .borrow()
            .iter()
            .find_map(|((tier, index), bounds)| {
                bounds.contains(&cursor).then(|| TierSlot {
                    tier: tier.clone(),
                    index: *index,
                })
            })
    }

    fn pointer_up(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !drag.active {
            // A press that never moved is a click: it opens the swap palette on that card.
            if !drag.was_open {
                self.open_replace(drag.from, cx);
            }
            return;
        }
        if drag.active {
            if let Some(over) = drag.over.clone() {
                if over != drag.from {
                    let committed = self.committed(cx);
                    let next = place_tier_card(&committed, &drag.card, &over, Some(&drag.from));
                    if next != committed {
                        self.commit_board(next, cx);
                        return;
                    }
                }
            }
        }
        cx.notify();
    }

    fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    fn on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.drag.is_some() {
            self.cancel_drag(cx);
            return true;
        }
        if self.palette_slot.is_some() {
            self.palette_slot = None;
            cx.notify();
            return true;
        }
        if self.composer.is_some() {
            self.composer = None;
            cx.notify();
            return true;
        }
        if self.build_open {
            self.build_open = false;
            cx.notify();
            return true;
        }
        false
    }

    fn on_window_down(&mut self, _event: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.palette_slot.is_some() {
            self.palette_slot = None;
            cx.notify();
        }
    }

    fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let board = self.board_name(cx);
        if self.motion_for != board {
            self.motion_for = board;
            self.drag = None;
            self.palette_slot = None;
            self.column_picker.update(cx, |picker, cx| picker.close(cx));
        }
        let composer_on = self.composer.is_some();
        self.composer_opacity
            .set_target(if composer_on { 1.0 } else { 0.0 });
        self.composer_y
            .set_target(if composer_on { 0.0 } else { 8.0 });
        let build_on = self.build_open;
        self.build_opacity
            .set_target(if build_on { 1.0 } else { 0.0 });
        self.build_y.set_target(if build_on { 0.0 } else { 8.0 });
        let palette_on = self.palette_slot.is_some();
        self.palette_opacity
            .set_target(if palette_on { 1.0 } else { 0.0 });
        self.palette_y
            .set_target(if palette_on { 0.0 } else { 6.0 });
        if let Some(slot) = &self.palette_slot {
            self.palette_anchor = Some(slot.clone());
        } else if self.palette_opacity.settled() && self.palette_opacity.value() < 0.05 {
            self.palette_anchor = None;
        }
        let active = self.any_moving();
        let dt = self.animator.dt(window, active);
        if dt > 0.0 {
            self.step_all(dt);
        }
    }

    fn any_moving(&self) -> bool {
        self.row_hover.values().any(|spring| !spring.settled())
            || !self.composer_opacity.settled()
            || !self.composer_y.settled()
            || !self.build_opacity.settled()
            || !self.build_y.settled()
            || !self.palette_opacity.settled()
            || !self.palette_y.settled()
    }

    fn step_all(&mut self, dt: f32) {
        for spring in self.row_hover.values_mut() {
            spring.step(dt);
        }
        self.composer_opacity.step(dt);
        self.composer_y.step(dt);
        self.build_opacity.step(dt);
        self.build_y.step(dt);
        self.palette_opacity.step(dt);
        self.palette_y.step(dt);
    }

    fn set_hover(&mut self, key: String, on: bool, cx: &mut Context<Self>) {
        let spring = self
            .row_hover
            .entry(key)
            .or_insert_with(|| SpringValue::new(0.0, SpringTier::Fast));
        spring.set_target(if on { 1.0 } else { 0.0 });
        cx.notify();
    }

    /// The card a drag left, which stays in its slot as a quiet placeholder.
    fn lifted(&self, tier: &str, row: usize) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|drag| drag.active && drag.from.tier == tier && drag.from.index == row)
    }

    /// The cell a drag is over, outlined in the accent.
    fn drop_over(&self, tier: &str, row: usize) -> bool {
        self.drag
            .as_ref()
            .filter(|drag| drag.active)
            .and_then(|drag| drag.over.as_ref())
            .is_some_and(|slot| slot.tier == tier && slot.index == row)
    }

    fn origin_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let origin = self.page_origin.clone();
        let view_move = cx.entity().clone();
        let view_up = cx.entity().clone();
        let view_key = cx.entity().clone();
        let view_down = cx.entity().clone();
        canvas(
            move |bounds, _, _| {
                *origin.borrow_mut() = (f32::from(bounds.left()), f32::from(bounds.top()));
            },
            move |_, _, window, _cx| {
                // Listeners live one frame. The canvas is painted whenever this page is, so a
                // mouse-up from a press that started last frame still reaches the drag.
                window.on_mouse_event({
                    let view = view_move.clone();
                    move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            view.update(cx, |this, cx| this.pointer_move(event, cx));
                        }
                    }
                });
                window.on_mouse_event({
                    let view = view_up.clone();
                    move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                            view.update(cx, |this, cx| this.pointer_up(cx));
                        }
                    }
                });
                window.on_mouse_event({
                    let view = view_down.clone();
                    move |event: &MouseDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                            view.update(cx, |this, cx| this.on_window_down(event, cx));
                        }
                    }
                });
                window.on_key_event({
                    let view = view_key.clone();
                    move |event: &KeyDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                            view.update(cx, |this, cx| {
                                this.on_escape(cx);
                            });
                        }
                    }
                });
            },
        )
        .absolute()
        .w(px(0.0))
        .h(px(0.0))
    }

    fn toolbar_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *Theme::global(cx);
        let route = self.state.read(cx).route().clone();
        let view = cx.entity().clone();
        let board = match &route {
            Route::RoleBoard(name) => Some(name.clone()),
            _ => None,
        };
        let start = match &board {
            Some(_) => {
                let back_state = self.state.clone();
                Button::new("roles-back", ButtonKind::Secondary)
                    .size(ButtonSize::Icon)
                    .icon(Icon::Back)
                    .tooltip("Back")
                    .on_click(move |_, cx| {
                        back_state.update(cx, |state, cx| {
                            state.navigate(Route::Roles);
                            cx.notify();
                        });
                    })
                    .into_any_element()
            }
            None => div().into_any_element(),
        };
        let end = match &board {
            Some(name) => {
                // The tier search, then the button that proposes this board's models, at the
                // toolbar's right edge. A repository's role is edited in its file.
                let editable = name == GENERAL_ROLE || self.editable(cx);
                let menu_view = view.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme.space_2))
                    .child(div().w(px(220.0)).child(self.tier_search.clone()))
                    .when(editable, |row| {
                        row.child(
                            Button::new("roles-build", ButtonKind::Secondary)
                                .size(ButtonSize::Icon)
                                .icon(Icon::AiBrain01Icon)
                                .tooltip("Pick models")
                                .selected(self.build_menu.read(cx).is_open())
                                .on_click(move |_, cx| {
                                    menu_view.update(cx, |this, cx| this.toggle_build_menu(cx));
                                }),
                        )
                    })
                    .into_any_element()
            }
            None => Button::new("roles-new", ButtonKind::Secondary)
                .size(ButtonSize::Icon)
                .icon(Icon::Plus)
                .tooltip("New role")
                .selected(self.new_menu.read(cx).is_open())
                .on_click(move |_, cx| {
                    view.update(cx, |this, cx| this.toggle_new_menu(cx));
                })
                .into_any_element(),
        };
        let Some(name) = board else {
            return toolbar(start, end, cx).into_any_element();
        };
        toolbar_center(start, self.role_button(&name, cx), end, cx).into_any_element()
    }

    /// New role's choice: a blank role, or a built-in template no role uses now. With every
    /// template in use it opens the blank composer at once.
    fn toggle_new_menu(&mut self, cx: &mut Context<Self>) {
        let templates = available_templates(&self.templates);
        if templates.is_empty() {
            self.open_composer(None, cx);
            return;
        }
        let weak = cx.weak_entity();
        let blank = weak.clone();
        let mut items = vec![MenuItem::action(
            "Blank role".to_string(),
            Some(Icon::Plus),
            Rc::new(move |_, cx| {
                let _ = blank.update(cx, |this, cx| this.open_composer(None, cx));
            }),
        )];
        for template in templates {
            let label = format!("From {}", template.name);
            let weak = weak.clone();
            items.push(MenuItem::action(
                label,
                Some(Icon::FileText),
                Rc::new(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.open_composer_from_template(&template, cx);
                    });
                }),
            ));
        }
        let open = !self.new_menu.read(cx).is_open();
        self.new_menu.update(cx, |menu, cx| {
            menu.set_menu(items);
            menu.set_width(232.0);
            menu.set_open(open);
            cx.notify();
        });
        cx.notify();
    }

    /// The models button's menu: the catalog's pick, Jev's, or a prompt for an agent.
    fn toggle_build_menu(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.board_name(cx) else {
            return;
        };
        let weak = cx.weak_entity();
        let item =
            |label: &str, icon: Icon, run: Rc<dyn Fn(&mut RolesView, &mut Context<RolesView>)>| {
                let weak = weak.clone();
                MenuItem::action(
                    label.to_string(),
                    Some(icon),
                    Rc::new(move |_: &mut Window, cx: &mut App| {
                        let run = run.clone();
                        let _ = weak.update(cx, |this, cx| run(this, cx));
                    }),
                )
            };
        let (catalog, judged, jev, prompt) = (name.clone(), name.clone(), name.clone(), name);
        let items = vec![
            item(
                "From the catalog",
                Icon::LibraryBig,
                Rc::new(move |this, cx| this.open_build(&catalog, "", cx)),
            ),
            item(
                "Ranked by a local model",
                Icon::Laptop,
                Rc::new(move |this, cx| this.open_build(&judged, "local", cx)),
            ),
            item(
                "Ranked by the Decision API",
                Icon::Cloud,
                Rc::new(move |this, cx| this.open_build(&jev, "jev", cx)),
            ),
            MenuItem::Separator,
            item(
                "Copy a prompt for an agent",
                Icon::Copy,
                Rc::new(move |this, cx| this.copy_models_prompt(&prompt, cx)),
            ),
        ];
        let open = !self.build_menu.read(cx).is_open();
        self.build_menu.update(cx, |menu, cx| {
            menu.set_menu(items);
            menu.set_width(232.0);
            menu.set_open(open);
            cx.notify();
        });
        cx.notify();
    }

    /// Puts a prompt on the clipboard that has a coding agent pick `name`'s models and save
    /// them with the CLI.
    fn copy_models_prompt(&mut self, name: &str, cx: &mut Context<Self>) {
        let profile = self
            .role_row(name)
            .map(|row| row.profile.clone())
            .unwrap_or_default();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(role_models_prompt(
            name, &profile,
        )));
        let whose = if name == GENERAL_ROLE {
            "the tier table".to_string()
        } else {
            format!("{name}'s board")
        };
        self.toast(
            format!("Copied a prompt that has an agent pick {whose}."),
            ToastTone::Success,
            cx,
        );
    }

    /// The proposal's title: whose board, and who ranked it.
    fn build_title(&self) -> String {
        let general = self.build_role == GENERAL_ROLE;
        match (general, self.build_judge.as_str()) {
            (true, "jev") => "Tiers ranked by the Decision API".to_string(),
            (true, "local") => "Tiers ranked by your local model".to_string(),
            (false, "local") => format!("{}'s board, ranked by your local model", self.build_role),
            (true, _) => "Build tiers from the model".to_string(),
            (false, "jev") => format!("{}'s board, ranked by the Decision API", self.build_role),
            (false, _) => format!("{}'s board from the catalog", self.build_role),
        }
    }

    /// The board's role at the toolbar's middle: its tile and name, opening its Composer, the
    /// way the role is edited. General, the tier table, and a repository's role are shown,
    /// not opened.
    fn role_button(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let general = name == GENERAL_ROLE;
        let editable = !general && self.editable(cx);
        let label = if general {
            "General".to_string()
        } else {
            name.to_string()
        };
        let tile = IconTile::new(Icon::Bot)
            .size(20.0, 12.0)
            .round()
            .tone(if general {
                TileTone::Neutral
            } else {
                TileTone::Execution
            });
        let tip = if general {
            "The tier table every role falls back on"
        } else if editable {
            "Edit role"
        } else {
            "In the repository's .dispatch/roles.json"
        };
        let view = cx.entity().clone();
        let target = name.to_string();
        let mut button = Button::new("roles-role", ButtonKind::Secondary)
            .size(ButtonSize::Sm)
            .pill()
            .pad_x(5.0)
            .leading(tile)
            .label(label)
            .tooltip(tip);
        if editable {
            button = button
                .trailing(icon_colored(
                    Icon::ChevronDown,
                    12.0,
                    Theme::global(cx).color_text_muted,
                ))
                .on_click(move |_, cx| {
                    view.update(cx, |this, cx| this.open_composer(Some(&target), cx));
                });
        }
        button.into_any_element()
    }

    /// The roles table, or a role's board, in bones until the roles load.
    fn loading_body(board: bool, theme: &Theme) -> AnyElement {
        let layout = if board {
            SkeletonLayout::Canvas
        } else {
            SkeletonLayout::Table
        };
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .px(px(theme.space_2))
            .child(page_skeleton(layout).without_toolbar())
            .into_any_element()
    }

    /// The open board's page, `PageContent`'s `flex-col gap-4` column: General's check and
    /// board, or a role's summary, note, and board. `None` while the role is not loaded.
    fn board_content(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.loaded {
            return None;
        }
        let theme = *Theme::global(cx);
        let column = div().w_full().flex().flex_col().gap(px(tw(4.0)));
        match self.state.read(cx).route().clone() {
            Route::RoleBoard(name) if name == GENERAL_ROLE => {
                let column = column.when(!self.tier_problems.is_empty(), |column| {
                    column.child(self.problems_card(&theme, cx))
                });
                let query = self.tier_query.clone();
                Some(column.child(self.board(true, &query, window, cx)))
            }
            Route::RoleBoard(name) => {
                let row = self.roles.iter().find(|row| row.name == name).cloned()?;
                let repository = row.source == RoleSource::Repository;
                // The role's own details open from its name in the toolbar; a repository's
                // role, which is not edited here, keeps them on the page.
                let query = self.tier_query.clone();
                Some(
                    column
                        .when(repository, |column| {
                            column
                                .child(role_summary(&row.profile, row.source, &theme, cx))
                                .child(role_board_note(&theme))
                        })
                        .child(self.board(!repository, &query, window, cx)),
                )
            }
            _ => None,
        }
    }

    fn list_body(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let theme = *Theme::global(cx);
        let rows = Rc::new(role_list_rows(&self.roles, self.tiers.len()));
        let measured = rows.clone();
        let activated = rows.clone();
        let state = self.state.clone();
        let table = DataTable::new(
            "roles-table",
            vec![
                Column::new("Role").fixed(),
                Column::new("When to use").fixed().shrinks(),
                Column::new("Tier").shrinks(),
                Column::new("Access").shrinks(),
                Column::new("Models").end(),
                Column::new("Source"),
            ],
            rows.len(),
            move |row, _window, cx| {
                let theme = *Theme::global(cx);
                let item = &rows[row.index];
                let general = item.name == GENERAL_ROLE;
                let body = sans(&theme, FontWeight::NORMAL);
                let tile = IconTile::new(Icon::Bot).tone(if general {
                    TileTone::Neutral
                } else {
                    TileTone::Execution
                });
                vec![
                    primary_cell(
                        tile,
                        item.label.clone(),
                        role_title_font(general, &theme),
                        None,
                        cx,
                    ),
                    fade_text(
                        when_display(&item.when).to_string(),
                        body.clone(),
                        12.0,
                        theme.color_text_secondary,
                        row.widths[1],
                    ),
                    fade_text(
                        item.tier.clone(),
                        body.clone(),
                        12.0,
                        row.ink,
                        row.widths[2],
                    ),
                    fade_text(item.access.clone(), body, 12.0, row.ink, row.widths[3]),
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.color_text_secondary)
                        .child(models_label(&item.name, item.models))
                        .into_any_element(),
                    source_chips(item.source, item.manual, cx).into_any_element(),
                ]
            },
        )
        .measure(move |range, window, cx| {
            let theme = *Theme::global(cx);
            let body = sans(&theme, FontWeight::NORMAL);
            let measured =
                &measured[range.start.min(measured.len())..range.end.min(measured.len())];
            let widest = |text: &dyn Fn(&RoleListRow) -> String, font: &gpui::Font| {
                measured
                    .iter()
                    .map(|row| text_width(&text(row), font.clone(), 12.0, window))
                    .fold(0.0, f32::max)
            };
            let role = measured
                .iter()
                .map(|row| {
                    let font = role_title_font(row.name == GENERAL_ROLE, &theme);
                    24.0 + 10.0 + text_width(&row.label, font, 12.5, window)
                })
                .fold(0.0, f32::max);
            let source = measured
                .iter()
                .map(|row| source_chips_width(row.source, row.manual, &theme, window))
                .fold(0.0, f32::max);
            vec![
                role,
                widest(&|row| when_display(&row.when).to_string(), &body),
                widest(&|row| row.tier.clone(), &body),
                widest(&|row| row.access.clone(), &body),
                widest(&|row| models_label(&row.name, row.models), &body),
                source,
            ]
        })
        .on_activate(move |index, _, cx| {
            let Some(name) = activated.get(index).map(|row| row.name.clone()) else {
                return;
            };
            state.update(cx, |state, cx| {
                state.navigate(Route::RoleBoard(name));
                cx.notify();
            });
        })
        .empty("No roles");
        // The table runs up under the toolbar, which fades it out on its trail.
        let top = page_under_inset(ROLES_PAGE, false, theme.space_2, window, cx);
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(table.inset_top(top).fades(theme.color_surface))
            .text_color(theme.color_text_primary)
    }

    fn problems_card(&self, theme: &Theme, cx: &Context<Self>) -> gpui::Div {
        let count = self.tier_problems.len();
        let mut list = div().w_full().flex().flex_col().gap(px(theme.space_1));
        for problem in &self.tier_problems {
            list = list.child(
                div()
                    .text_size(px(theme.font_size_sm))
                    .text_color(theme.color_text_secondary)
                    .child(format!("• {problem}")),
            );
        }
        div()
            .p(px(theme.space_4))
            .rounded(px(theme.radius_card))
            .bg(theme.color_surface_raised)
            .border_1()
            .border_color(theme.color_border_subtle)
            .flex()
            .flex_col()
            .gap(px(theme.space_2))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(theme.font_size_md))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Tier configuration check"),
                    )
                    .child(chip(format!("{count} problem(s)"), StatusTone::Warn, cx)),
            )
            .child(list)
    }

    /// `TierBoard`: the tier heads, then one row per order position, each the order mark
    /// and a cell per tier. Rows take the height of their tallest card.
    fn board(
        &self,
        editable: bool,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = *Theme::global(cx);
        let committed = self.committed(cx);
        let kept = self.open_columns(cx);
        let columns = tier_columns_in(&committed, &kept);
        let rows = board_row_count(&columns, editable);
        let removable =
            editable && self.board_name(cx).as_deref() != Some(GENERAL_ROLE) && kept.len() > 1;
        let can_add = editable
            && self.board_name(cx).as_deref() != Some(GENERAL_ROLE)
            && kept.len() < TIER_COLUMNS.len();
        let clear_bounds = self.cell_bounds.clone();
        let mut board = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(
                canvas(
                    move |_, _, _| clear_bounds.borrow_mut().clear(),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.board_header(&columns, removable, cx));
        for row in 0..rows {
            board = board.child(self.board_row(row, &columns, editable, query, &theme, window, cx));
        }
        if can_add {
            board = board.child(self.add_column_button(&theme, cx));
        }
        board
    }

    /// `.pw-tier-board__head`: the tier's name in 13px semibold, its count in 11px figures,
    /// and, on a role's board, the control that removes the column.
    fn board_header(
        &self,
        columns: &TierColumns,
        removable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = *Theme::global(cx);
        let mut header = div()
            .w_full()
            .flex()
            .gap(px(GAP))
            .child(div().w(px(ORDER_W)).flex_none());
        for (id, list) in &columns.columns {
            let tier = id.clone();
            let view = cx.entity().clone();
            let mut head = div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(4.0))
                .pb(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .line_height(px(13.0 * 1.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.color_text_primary)
                        .child(tier_column_label(id)),
                )
                .child(
                    div()
                        .font(tabular(sans(&theme, FontWeight::NORMAL)))
                        .text_size(px(11.0))
                        .line_height(px(11.0 * 1.5))
                        .text_color(theme.color_text_muted)
                        .child(list.len().to_string()),
                );
            if removable {
                head = head.child(
                    Button::new(
                        SharedString::from(format!("column-remove-{tier}")),
                        ButtonKind::Ghost,
                    )
                    .size(ButtonSize::Icon)
                    .icon(Icon::Close)
                    .tooltip(format!("Remove the {} column", tier_column_label(&tier)))
                    .on_click(move |_, cx| {
                        let tier = tier.clone();
                        view.update(cx, |this, cx| this.delete_column(tier.clone(), cx));
                    }),
                );
            }
            header = header.child(head);
        }
        header
    }

    /// Add column: a ghost control under a role's board, with the picker it opens.
    fn add_column_button(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let view = cx.entity().clone();
        let button = Button::new("role-add-column", ButtonKind::Ghost)
            .size(ButtonSize::Sm)
            .icon(Icon::Plus)
            .label("Add column")
            .on_click(move |window, cx| {
                view.update(cx, |this, cx| this.toggle_column_picker(window, cx));
            });
        div()
            .relative()
            .w_full()
            .flex()
            .justify_end()
            .pt(px(theme.space_1))
            .child(self.column_anchor.probe())
            .child(button)
    }

    #[allow(clippy::too_many_arguments)]
    fn board_row(
        &self,
        row: usize,
        columns: &TierColumns,
        editable: bool,
        query: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut line = div()
            .w_full()
            .flex()
            .gap(px(GAP))
            .child(order_mark(row, theme, cx));
        for (id, _) in &columns.columns {
            let cards = columns.get(id);
            let tier = cards.get(row).cloned();
            let is_next = row == cards.len();
            line =
                line.child(self.cell(id, row, tier, is_next, editable, query, theme, window, cx));
        }
        line
    }

    /// `.pw-tier-board__cell`: the card in this slot, else Add model in a tier's next cell,
    /// else an empty slot to drop on. Only an editable board registers its slots for drops.
    #[allow(clippy::too_many_arguments)]
    fn cell(
        &self,
        tier: &str,
        row: usize,
        card: Option<DispatchTier>,
        is_next: bool,
        editable: bool,
        query: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut cell = div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .rounded(px(theme.radius_card));
        if editable {
            let bounds = self.cell_bounds.clone();
            let key = (tier.to_string(), row);
            cell = cell.child(
                canvas(
                    move |rect, _, _| {
                        bounds.borrow_mut().insert(key, rect);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        }
        cell = match card {
            Some(card) => {
                let dimmed = !query.is_empty() && !tier_matches(query, &card);
                cell.child(self.tier_card(&card, row, editable, dimmed, theme, cx))
            }
            None if editable && is_next => {
                cell.child(self.add_button(tier, row, theme, window, cx))
            }
            None if editable => cell.child(div().flex_1().min_h(px(CARD_MIN_H))),
            None => cell,
        };
        let swapping = self.palette_replace
            && self
                .palette_slot
                .as_ref()
                .is_some_and(|slot| slot.tier == tier && slot.index == row);
        if self.drop_over(tier, row) || swapping {
            // `outline: 2px solid var(--color-accent); outline-offset: 2px`.
            cell = cell.child(
                div()
                    .absolute()
                    .top(px(-4.0))
                    .left(px(-4.0))
                    .right(px(-4.0))
                    .bottom(px(-4.0))
                    .rounded(px(theme.radius_card + 4.0))
                    .border_2()
                    .border_color(theme.color_accent),
            );
        }
        cell
    }

    /// `.pw-tier-board__add`: a dashed slot that opens the model palette. Hovered or open, its
    /// border, fill, and text brighten over `--duration-fast`.
    fn add_button(
        &self,
        tier: &str,
        row: usize,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = SharedString::from(format!("add-model-{tier}-{row}"));
        let pointer = use_pointer_timed(id.clone(), PointerTiming::item(theme), window, cx);
        let open = self
            .palette_slot
            .as_ref()
            .is_some_and(|slot| slot.tier == tier && !self.palette_replace);
        let (fill, ink) = if open {
            (1.0, 1.0)
        } else {
            let state = pointer.read(cx);
            (state.hover(), state.hover_color())
        };
        let wash = theme.color_surface_hover;
        let ink = mix(theme.color_text_muted, theme.color_text_primary, ink);
        let view = cx.entity().clone();
        let slot = TierSlot {
            tier: tier.to_string(),
            index: row,
        };
        let button = div()
            .id(id)
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(CARD_MIN_H))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .rounded(px(theme.radius_card))
            .border_1()
            .border_dashed()
            .border_color(mix(
                theme.color_border_strong,
                theme.color_border_default,
                fill,
            ))
            .bg(gpui::Rgba {
                a: wash.a * fill,
                ..wash
            })
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(ink)
            .child(icon_colored(Icon::Plus, 14.0, ink))
            .child("Add model");
        track(button, &pointer, true).on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            view.update(cx, |this, cx| this.toggle_palette(slot.clone(), cx));
        })
    }

    /// `TierCard`: the shared `Card` with the model leading (two lines at most), the agent and
    /// provider under it, and Remove when editable.
    fn tier_card(
        &self,
        tier: &DispatchTier,
        row: usize,
        editable: bool,
        dimmed: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let opacity = if self.lifted(&tier.name, row) {
            LIFTED_OPACITY
        } else if dimmed {
            DIMMED_OPACITY
        } else {
            1.0
        };
        let subtitle = [tier.agent.as_str(), tier.provider.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        let slot = TierSlot {
            tier: tier.name.clone(),
            index: row,
        };
        let remove = editable.then(|| {
            let view = cx.entity().clone();
            let slot = slot.clone();
            let button = Button::new(
                SharedString::from(format!("tier-remove-{}-{row}", tier.name)),
                ButtonKind::Ghost,
            )
            .size(ButtonSize::Icon)
            .icon(Icon::Close)
            .tooltip(format!(
                "Remove {} {} from {}",
                tier.agent, tier.model, tier.name
            ))
            .on_click(move |_, cx| {
                view.update(cx, |this, cx| this.remove_at(slot.clone(), cx));
            });
            // Its press is not the card's: it neither drags nor opens the swap palette.
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(button)
        });
        let head = div()
            .flex()
            .min_w(px(0.0))
            .items_start()
            .gap(px(8.0))
            .child(agent_avatar(&tier.agent, "", AVATAR))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(tw(0.5)))
                    .child(
                        div()
                            .font(mono(theme, FontWeight::MEDIUM))
                            .text_size(px(13.0))
                            .line_height(px(13.0 * 1.35))
                            .text_color(theme.color_text_primary)
                            .line_clamp(2)
                            .child(tier.model.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(11.0 * 1.5))
                            .text_color(theme.color_text_muted)
                            .child(ellipsis(subtitle)),
                    ),
            )
            .children(remove);
        let mut body = div().flex_1().min_w(px(0.0)).flex().flex_col().child(head);
        if editable {
            let view = cx.entity().clone();
            body = body.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, _, cx| {
                    view.update(cx, |this, cx| {
                        this.begin_drag(
                            slot.clone(),
                            f32::from(event.position.x),
                            f32::from(event.position.y),
                            cx,
                        );
                    });
                },
            );
        }
        card(cx)
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(CARD_MIN_H))
            .p(px(tw(3.0)))
            .opacity(opacity)
            .child(body)
    }

    /// `TierModelPalette` in its 360px popover from the Add model cell: the search pill, the
    /// models grouped by agent (and the agents' problems after them) in one scroll at most
    /// 360px tall, and the custom Agent / Model pair under a rule. It hangs under its cell,
    /// or over it when the window has no room below, and draws above the page's clip.
    fn palette_layer(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.palette_opacity.value();
        let Some(bounds) = self.palette_anchor.as_ref().and_then(|slot| {
            self.cell_bounds
                .borrow()
                .get(&(slot.tier.clone(), slot.index))
                .copied()
        }) else {
            return div().into_any_element();
        };
        let groups = palette_groups(&self.palette_models, &self.palette_query);
        let replacing = self.replacing_model(cx);
        let view = cx.entity().clone();
        let mut list = div()
            .id("palette-scroll")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(theme.space_2));
        if self.palette_loading {
            // The installed agents are asked for their models: their rows, in bones.
            list = list.child(
                div()
                    .p(px(theme.space_2))
                    .child(list_skeleton(5, 18.0, window, cx)),
            );
        } else if groups.is_empty() {
            let (empty, mood) = if self.palette_query.trim().is_empty() {
                ("No models listed", MascotMood::Sleeping)
            } else {
                ("No matches", MascotMood::Lost)
            };
            list = list.child(page_empty(empty, mood, cx));
        } else {
            for (agent, models) in groups {
                let mut group = div().flex().flex_col().gap(px(theme.space_1)).child(
                    div()
                        .px(px(theme.space_1))
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.color_text_muted)
                        .child(agent),
                );
                for model in models {
                    let card = palette_card_at(&model, "");
                    let view = view.clone();
                    // The board names a model at an effort (`claude-opus-5-5-high`): each effort
                    // the agent takes is a chip that places the model at it; the row itself
                    // places it at the agent's default.
                    let mut efforts = div().flex_none().flex().items_center().gap(px(2.0));
                    for effort in &model.efforts {
                        let at = palette_card_at(&model, effort);
                        let current = replacing
                            .as_ref()
                            .is_some_and(|(agent, name)| *agent == at.agent && *name == at.model);
                        let view = view.clone();
                        efforts = efforts.child(
                            div()
                                .id(SharedString::from(format!(
                                    "palette-{}-{}-{effort}",
                                    model.agent, model.model
                                )))
                                .h(px(20.0))
                                .px(px(6.0))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .font_family(theme.font_mono)
                                .text_size(px(10.5))
                                .text_color(if current {
                                    theme.color_text_primary
                                } else {
                                    theme.color_text_muted
                                })
                                .when(current, |chip| chip.bg(theme.color_surface_hover))
                                .hover(|chip| {
                                    chip.bg(theme.color_border_subtle)
                                        .text_color(theme.color_text_primary)
                                })
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    cx.stop_propagation();
                                    view.update(cx, |this, cx| this.place_new(at.clone(), cx));
                                })
                                .child(effort.to_string()),
                        );
                    }
                    let subtitle = [model.provider.as_str(), model.tier.as_str()]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    group = group.child(
                        div()
                            .id(SharedString::from(format!(
                                "palette-{}-{}",
                                model.agent, model.model
                            )))
                            .px(px(theme.space_2))
                            .py(px(6.0))
                            .flex()
                            .items_center()
                            .gap(px(theme.space_2))
                            .rounded(px(theme.radius_item))
                            .cursor_pointer()
                            .hover(|row| row.bg(theme.color_surface_hover))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                view.update(cx, |this, cx| this.place_new(card.clone(), cx));
                            })
                            .child(agent_avatar(&model.agent, "", 18.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .font_family(theme.font_mono)
                                            .text_size(px(12.0))
                                            .line_height(px(18.0))
                                            .text_color(theme.color_text_primary)
                                            .child(ellipsis(model.model.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .line_height(px(16.5))
                                            .text_color(theme.color_text_muted)
                                            .child(ellipsis(subtitle)),
                                    ),
                            )
                            .child(efforts),
                    );
                }
                list = list.child(group);
            }
        }
        if !self.palette_problems.is_empty() {
            let mut problems = div()
                .flex()
                .flex_col()
                .gap(px(theme.space_1))
                .px(px(theme.space_1))
                .text_size(px(11.0))
                .line_height(px(16.5))
                .text_color(theme.color_text_muted);
            for problem in &self.palette_problems {
                problems = problems.child(wrap_text(format!("• {problem}")));
            }
            list = list.child(problems);
        }
        let agent_text = self.custom_agent.read(cx).text().trim().to_string();
        let model_text = self.custom_model.read(cx).text().trim().to_string();
        let can_custom = !agent_text.is_empty() && !model_text.is_empty();
        let custom_view = view.clone();
        let pair = div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h(px(theme.control_height_sm))
            .flex()
            .items_center()
            .rounded_full()
            .bg(face(theme.field_fill_top, theme.field_fill_bottom))
            .shadow(control_drop(theme))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(12.0))
                    .child(self.custom_agent.clone()),
            )
            .child(div().w(px(1.0)).h(px(14.0)).bg(theme.color_border_subtle))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .px(px(12.0))
                    .child(self.custom_model.clone()),
            )
            .child(edge(Edge::Well, theme.control_height_sm / 2.0, theme));
        let body =
            div()
                .relative()
                .w(px(PALETTE_WIDTH))
                .max_h(px(PALETTE_MAX_H))
                .flex()
                .flex_col()
                .gap(px(theme.space_2))
                .px(px(theme.space_3))
                .pt(px(theme.space_2))
                .pb(px(theme.space_3))
                .bg(theme.color_surface_subtle)
                // `floating_panel` occludes, so the wrapper under it never sees a press: the
                // body keeps its own presses (the search, the custom pair) from the window's
                // listener that closes the palette.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().flex_none().child(self.palette_search.clone()))
                .child(list)
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .pt(px(theme.space_2))
                        .border_t_1()
                        .border_color(theme.color_border_subtle)
                        .child(pair)
                        .child(div().opacity(if can_custom { 1.0 } else { 0.45 }).child(
                            icon_button(
                                "palette-custom",
                                Icon::Plus,
                                "Add custom model",
                                Rc::new(move |_, cx| {
                                    custom_view.update(cx, |this, cx| this.add_custom(cx));
                                }),
                                cx,
                            ),
                        )),
                );
        let panel = div()
            .opacity(t)
            .when(self.palette_slot.is_some(), |panel| panel.occlude())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(floating_panel(body, theme.radius_menu, theme));
        let height = f32::from(bounds.size.height);
        deferred(window_layer(
            div().size_full().child(
                div()
                    .absolute()
                    .left(bounds.origin.x)
                    .top(bounds.origin.y)
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .child(hang(height + 4.0, self.palette_y.value(), panel)),
            ),
        ))
        .with_priority(1)
        .into_any_element()
    }

    /// Asks before deleting a role, the way the app asks before removing a repository.
    fn remove_dialog(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let name = self.removing.clone()?;
        if !self.confirm_focus.is_focused(window) {
            self.confirm_focus.focus(window);
        }
        let theme = *Theme::global(cx);
        let template = self.templates.iter().any(|item| item.name == name);
        let cancel_view = cx.entity().clone();
        let cancel: crate::ui::Handler = Rc::new(move |_, cx: &mut App| {
            cancel_view.update(cx, |this, cx| {
                this.removing = None;
                cx.notify();
            });
        });
        let confirm_view = cx.entity().clone();
        let confirm_name = name.clone();
        let confirm: crate::ui::Handler = Rc::new(move |_, cx: &mut App| {
            let name = confirm_name.clone();
            confirm_view.update(cx, |this, cx| {
                this.removing = None;
                this.remove_role(name, cx);
            });
        });
        let description = if template {
            "The role stops being offered to runs. Its built-in template stays under New role."
        } else {
            "Dispatch deletes the role, its instructions, and its board."
        };
        let dialog = ConfirmDialog::new(
            "remove-role",
            format!("Remove \u{201c}{name}\u{201d}?"),
            description,
        )
        .recovery(format!(
            "Runs started with --role {name} are refused until a role of that name exists again."
        ))
        .confirm_label("Remove")
        .danger()
        .focus(&self.confirm_focus)
        .on_cancel(cancel.clone())
        .on_confirm(confirm);
        Some(modal_layer(1.0, true, cancel, dialog, &theme))
    }

    fn composer_layer(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.composer_opacity.value();
        let title = match self.composer.as_deref() {
            Some(name) if !name.is_empty() => format!("Edit {name}"),
            _ => "New role".to_string(),
        };
        let editing = self.composer.as_ref().is_some_and(|name| !name.is_empty());
        let source = self
            .composer
            .as_ref()
            .filter(|name| !name.is_empty())
            .and_then(|name| self.role_row(name).map(|row| row.source));
        // Every role but General can go; a repository's role is edited in its file, not here.
        let show_footer = self
            .composer
            .as_ref()
            .is_some_and(|name| !name.is_empty() && name != GENERAL_ROLE)
            && source != Some(RoleSource::Repository);
        let view = cx.entity().clone();
        let closer = view.clone();
        let mut start = Vec::new();
        if show_footer {
            let footer_view = view.clone();
            let removing = self.composer.clone().unwrap_or_default();
            start.push(
                button(
                    "role-remove",
                    "Remove role",
                    ButtonKind::Ghost,
                    Rc::new(move |window, cx| {
                        let name = removing.clone();
                        footer_view.update(cx, |this, cx| {
                            this.removing = Some(name);
                            this.confirm_focus.focus(window);
                            cx.notify();
                        });
                    }),
                    cx,
                )
                .size(ButtonSize::Sm)
                .into_any_element(),
            );
        }
        let submit = view.clone();
        let submit_label: SharedString = if editing {
            "Save".into()
        } else {
            "Create role".into()
        };
        let card = composer_card(title, t, self.composer_y.value(), theme)
            .on_close(Rc::new(move |_, cx| {
                closer.update(cx, |this, cx| this.close_composer(cx));
            }))
            .child(self.name_input.clone())
            .child(self.instructions_input.clone())
            .child(composer_field(
                "When to use",
                self.when_input.clone(),
                theme,
            ))
            .child(composer_field(
                "Time limit",
                self.timeout_input.clone(),
                theme,
            ))
            .child(div().pt(px(4.0)).child(self.pills.clone()))
            .footer(composer_footer(
                start,
                [button(
                    "role-save",
                    submit_label,
                    ButtonKind::Primary,
                    Rc::new(move |_, cx| {
                        submit.update(cx, |this, cx| this.submit_composer(cx));
                    }),
                    cx,
                )
                .size(ButtonSize::Sm)
                .into_any_element()],
                theme,
            ))
            .overlay(self.pills.read(cx).overlay(cx));
        let close = cx.entity().clone();
        modal_layer(
            t,
            self.composer.is_some(),
            Rc::new(move |_, cx| {
                close.update(cx, |this, cx| this.close_composer(cx));
            }),
            card,
            theme,
        )
    }

    fn build_layer(&self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let t = self.build_opacity.value();
        let view = cx.entity().clone();
        let empty = self.build.as_ref().is_none_or(is_build_empty);
        let can_apply = !self.build_loading && !self.build_applying && !empty;
        let mut body = div()
            .id("build-scroll")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(theme.space_3));
        if self.build_loading {
            // Every model is priced: the proposal's rows, in bones.
            body = body.child(list_skeleton(6, 20.0, window, cx));
        } else if empty {
            body = body.child(muted_note("No models to propose.", theme));
        } else if let Some(build) = &self.build {
            body = body.child(build_sections(build, theme, cx));
            for problem in &build.problems {
                body = body.child(muted_note(problem.clone(), theme));
            }
        }
        let apply_label: SharedString = if self.build_applying {
            "Applying…".into()
        } else {
            "Apply".into()
        };
        let cancel = view.clone();
        let apply = view.clone();
        let priority = self
            .build
            .as_ref()
            .map(|build| {
                if build.priority.is_empty() {
                    "balanced".to_string()
                } else {
                    build.priority.clone()
                }
            })
            .unwrap_or_else(|| "balanced".to_string());
        let card = div()
            .w(px(560.0))
            .max_h(px(640.0))
            .flex()
            .flex_col()
            .gap(px(theme.space_3))
            .p(px(theme.space_5))
            .rounded(px(theme.radius_dialog))
            .bg(theme.color_surface_raised)
            .border_1()
            .border_color(theme.color_border)
            .mt(px(self.build_y.value()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(theme.font_size_lg))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.build_title()),
                    )
                    .child(chip("Proposal", StatusTone::Muted, cx)),
            )
            .when(self.build.is_some(), |card| {
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme.space_2))
                        .child(chip(format!("Priority {priority}"), StatusTone::Muted, cx))
                        .child(muted_note(
                            "The pass bar comes from the routing priority",
                            theme,
                        )),
                )
            })
            .child(body)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(theme.space_2))
                    .child(button(
                        "build-cancel",
                        "Cancel",
                        ButtonKind::Ghost,
                        Rc::new(move |_, cx| {
                            cancel.update(cx, |this, cx| {
                                this.build_open = false;
                                cx.notify();
                            });
                        }),
                        cx,
                    ))
                    .child(
                        div()
                            .opacity(if can_apply { 1.0 } else { 0.45 })
                            .child(button(
                                "build-apply",
                                apply_label,
                                if can_apply {
                                    ButtonKind::Primary
                                } else {
                                    ButtonKind::Ghost
                                },
                                Rc::new(move |_, cx| {
                                    apply.update(cx, |this, cx| this.apply_build(cx));
                                }),
                                cx,
                            )),
                    ),
            );
        let close = cx.entity().clone();
        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .bg(theme.color_overlay)
                    .opacity(t)
                    .when(self.build_open, |backdrop| {
                        backdrop
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                close.update(cx, |this, cx| {
                                    this.build_open = false;
                                    cx.notify();
                                });
                            })
                    }),
            )
            .child(div().opacity(t).child(card))
    }

    /// `TierDragGhost`: an opaque card under the pointer, never a hit target.
    fn ghost_layer(&self, theme: &Theme, _cx: &Context<Self>) -> gpui::Div {
        let Some(drag) = self.drag.as_ref().filter(|drag| drag.active) else {
            return div();
        };
        let (x, y) = drag.pointer;
        div()
            .absolute()
            .left(px(x + GHOST_OFFSET))
            .top(px(y + GHOST_OFFSET))
            .w(px(GHOST_W))
            .p(px(tw(3.0)))
            .rounded(px(theme.radius_card))
            .bg(theme.color_surface_subtle)
            .shadow(vec![drop(8.0, 24.0, theme.color_shadow)])
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(agent_avatar(&drag.card.agent, "", AVATAR))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font(mono(theme, FontWeight::MEDIUM))
                            .text_size(px(13.0))
                            .line_height(px(13.0 * 1.5))
                            .text_color(theme.color_text_primary)
                            .child(ellipsis(drag.card.model.clone())),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(11.0 * 1.5))
                            .text_color(theme.color_text_muted)
                            .child(ellipsis(drag.card.agent.clone())),
                    ),
            )
    }

    fn layer_open(open: bool, opacity: &SpringValue) -> bool {
        open || !opacity.settled() || opacity.value() > 0.02
    }
}

impl Render for RolesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare(window, cx);
        let theme = *Theme::global(cx);
        let removal = self.remove_dialog(window, cx);
        let show_composer = Self::layer_open(self.composer.is_some(), &self.composer_opacity);
        let show_build = Self::layer_open(self.build_open, &self.build_opacity);
        let show_palette = self.palette_anchor.is_some();
        let show_ghost = self.drag.as_ref().is_some_and(|drag| drag.active);
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(self.origin_canvas(cx))
            .map(|page| {
                let list =
                    self.loaded && !matches!(self.state.read(cx).route(), Route::RoleBoard(_));
                if list {
                    let content = self.list_body(window, cx);
                    let toolbar = self.toolbar_view(cx);
                    page.child(page_under(
                        ROLES_PAGE,
                        toolbar,
                        None,
                        // The toolbar's inset, so the table's edges line up with its buttons.
                        theme.space_2,
                        content,
                        window,
                        cx,
                    ))
                } else if let Some(content) = self.board_content(window, cx) {
                    let toolbar = self.toolbar_view(cx);
                    page.child(page_content(
                        ROLES_BOARD,
                        toolbar,
                        tw(4.0),
                        content,
                        window,
                        cx,
                    ))
                } else {
                    let board = matches!(self.state.read(cx).route(), Route::RoleBoard(_));
                    page.child(self.toolbar_view(cx))
                        .child(Self::loading_body(board, &theme))
                }
            })
            .when(show_composer, |page| {
                page.child(self.composer_layer(&theme, cx))
            })
            .when(show_build, |page| {
                page.child(self.build_layer(&theme, window, cx))
            })
            .when(show_palette, |page| {
                page.child(self.palette_layer(&theme, window, cx))
            })
            .when(show_ghost, |page| page.child(self.ghost_layer(&theme, cx)))
            // Under the New role and models buttons at the toolbar's right edge; neither
            // defers anything of its own.
            .child(
                div()
                    .absolute()
                    .top(px(theme.space_2))
                    .right(px(theme.space_2))
                    .w(px(232.0))
                    .child(deferred(self.new_menu.clone()).with_priority(2)),
            )
            .child(
                div()
                    .absolute()
                    .top(px(theme.space_2))
                    .right(px(theme.space_2))
                    .w(px(232.0))
                    .child(deferred(self.build_menu.clone()).with_priority(2)),
            )
            // Under a role board's Add column button, over the rows after it.
            .when(self.column_picker.read(cx).is_presented(), |page| {
                let Some(anchor) = self.column_anchor.get() else {
                    return page;
                };
                page.child(window_anchor(
                    anchor,
                    self.column_picker.clone().into_any_element(),
                ))
            })
            // Last, over the composer it was opened from.
            .when_some(removal, |page, dialog| page.child(dialog))
    }
}

fn core_message(err: CoreError, fallback: &str) -> String {
    match err {
        CoreError::NotRunning => "The local core is not running.".to_string(),
        CoreError::Rpc(message) if !message.is_empty() => message,
        CoreError::Rpc(_) => fallback.to_string(),
    }
}

fn load_all(client: &CoreClient) -> LoadBundle {
    let (tiers, roles) = at_once(
        || {
            client
                .list_tiers(&ListTiersRequest {})
                .map(|response| response.tiers)
                .map_err(|err| core_message(err, "Could not list the tiers."))
        },
        || load_roles(client),
    );
    LoadBundle { tiers, roles }
}

fn load_extras(client: &CoreClient) -> Extras {
    let (problems, agents) = at_once(
        || {
            client
                .check_tiers(&CheckTiersRequest {})
                .map(|response| response.problems)
                .map_err(|err| core_message(err, "Could not check the tiers."))
        },
        || {
            client
                .list_agents(&ListAgentsRequest {})
                .map(|response| agent_names(response.agents))
                .unwrap_or_default()
        },
    );
    Extras { problems, agents }
}

fn load_roles(client: &CoreClient) -> Result<RolesList, String> {
    let response = client
        .subagents(&SubagentsRequest {
            action: "list".into(),
            dir: String::new(),
            name: String::new(),
            profile: String::new(),
        })
        .map_err(|err| core_message(err, "Could not list the roles."))?;
    if !response.ok {
        return Err(if response.json.is_empty() {
            "Could not list the roles.".to_string()
        } else {
            response.json
        });
    }
    parse_roles_list(&response.json)
}

fn agent_names(agents: Vec<DispatchAgent>) -> Vec<String> {
    let mut names: Vec<String> = agents
        .into_iter()
        .map(|agent| agent.name)
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn save_role_ops(
    client: &CoreClient,
    name: &str,
    profile: &str,
    drop_previous: Option<&str>,
) -> Result<Option<RolesList>, String> {
    let saved = client
        .subagents(&SubagentsRequest {
            action: "save".into(),
            dir: String::new(),
            name: name.to_string(),
            profile: profile.to_string(),
        })
        .map_err(|err| core_message(err, "Could not save the role."))?;
    if !saved.ok {
        return Err(if saved.json.is_empty() {
            "Could not save the role.".to_string()
        } else {
            saved.json
        });
    }
    if let Some(previous) = drop_previous {
        let _ = client.subagents(&SubagentsRequest {
            action: "remove".into(),
            dir: String::new(),
            name: previous.to_string(),
            profile: String::new(),
        });
    }
    Ok(client
        .subagents(&SubagentsRequest {
            action: "list".into(),
            dir: String::new(),
            name: String::new(),
            profile: String::new(),
        })
        .ok()
        .filter(|response| response.ok)
        .and_then(|response| parse_roles_list(&response.json).ok()))
}

fn run_build(
    client: &CoreClient,
    apply: bool,
    role: &str,
    judge: &str,
) -> Result<TierBuild, String> {
    let response = client
        .build_tiers(&BuildTiersRequest {
            apply,
            agent: String::new(),
            role: role.to_string(),
            judge: judge.to_string(),
        })
        .map_err(|err| core_message(err, "Could not build the tiers."))?;
    if !response.ok {
        return Err(if response.json.is_empty() {
            "Could not build the tiers.".to_string()
        } else {
            response.json
        });
    }
    parse_tier_build(&response.json)
}

/// A role's title: General in the sans face, the others as `<code>` in the mono face.
fn role_title_font(general: bool, theme: &Theme) -> gpui::Font {
    if general {
        sans(theme, FontWeight::MEDIUM)
    } else {
        mono(theme, FontWeight::MEDIUM)
    }
}

/// The Source cell: the source's `StatusChip` (Tier table for General), then "Only with
/// --role" for a manual role, 6px apart.
fn source_chips(source: Option<RoleSource>, manual: bool, cx: &gpui::App) -> gpui::Div {
    let (label, tone) = match source {
        None => ("Tier table", StatusTone::Muted),
        Some(RoleSource::Builtin) => (RoleSource::Builtin.label(), StatusTone::Muted),
        Some(other) => (other.label(), StatusTone::Ok),
    };
    div()
        .flex()
        .items_center()
        .gap(px(tw(1.5)))
        .child(chip(label, tone, cx))
        .when(manual, |row| {
            row.child(chip("Only with --role", StatusTone::Muted, cx))
        })
}

fn source_chips_width(
    source: Option<RoleSource>,
    manual: bool,
    theme: &Theme,
    window: &Window,
) -> f32 {
    let font = sans(theme, FontWeight::MEDIUM);
    let chip_width = |label: &str| text_width(label, font.clone(), 11.0, window) + 18.0 + 2.0;
    let label = source.map(RoleSource::label).unwrap_or("Tier table");
    let mut width = chip_width(label);
    if manual {
        width += tw(1.5) + chip_width("Only with --role");
    }
    width
}
