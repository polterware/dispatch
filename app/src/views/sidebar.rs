//! Sidebar: Commander, pages, grouped runs, display options, and the transcript card.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::window_drag::WindowDragRegion;
use gpui::{
    App, Bounds, Context, Entity, Focusable, IntoElement, ListAlignment, ListOffset, ListState,
    MouseButton, Pixels, Render, SharedString, Window, canvas, deferred, div, prelude::*, px,
};

use super::TranscriptView;
use super::conversation_usage::ConversationUsageView;
use super::repository_actions;
use crate::avatar::agent_avatar_live;
use crate::client::{
    DeleteRunsRequest, DeleteSectionRequest, MoveToSectionRequest, RenameConversationRequest,
    SaveSectionRequest,
};
use crate::format::agent_display_name;
use crate::models::avatar::avatar_status;
use crate::models::sidebar::{
    COLLAPSED_PREFERENCE, DISPLAY_PREFERENCE, Direction, GroupBy, GroupKind, HOVER_CLOSE_MS,
    HOVER_OPEN_MS, REVEALED_PREFERENCE, RunsNotice, SEEN_PREFERENCE, SIDEBAR_PAGE_ROWS, Show,
    SidebarDisplay, SidebarGroup, SidebarRun, SortBy, conversation_ref, ended_run_ids,
    group_sidebar_runs, group_title_tooltip, has_unseen_update, holding_section, now_unix_ms,
    page_sidebar_rows, parse_sidebar_display, repository_ref, row_delete_ids, run_mark,
    runs_notice, section_ref, sidebar_run_title, sidebar_runs,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route, SettingsPage, observe};
use crate::theme::Theme;
use crate::ui::list_skeleton;
use crate::ui::parts::sidebar::{preview_surface, run_dot, run_none};
use crate::ui::patterns::{FadeEdges, composer_card, composer_footer, modal_layer};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Icon, MascotMood, Menu, MenuEntry, MenuItem, STACK_OVERDRAW,
    SidebarItem, SidebarSectionLabel, TextInput, ToastTone, button, ellipsis, icon_button, kbd,
    list_empty, sync_virtual_list, virtual_fade_list,
};

/// The Display menu's width (`min-w-48`, 12rem).
const DISPLAY_MENU_WIDTH: f32 = 12.0 * crate::theme::REM;

/// What the runs list says when the core did not answer its last refresh.
const REFRESH_FAILED: &str = "Could not refresh runs and sections.";

#[derive(Clone)]
struct SectionDraft {
    id: Option<String>,
}

enum SectionJob {
    Save(SaveSectionRequest),
    Delete(DeleteSectionRequest),
    Move(MoveToSectionRequest),
}

struct Snapshot {
    /// The core has not answered yet: the runs show as bones.
    connecting: bool,
    groups: Rc<Vec<SidebarGroup>>,
    route: Route,
    show_error: bool,
}

const NAV: &[(&str, Icon, Route)] = &[
    ("Roles", Icon::Route, Route::Roles),
    ("Intelligence", Icon::AiBrain01Icon, Route::Intelligence),
    ("Spend", Icon::CreditCard, Route::Spend),
    ("Stats", Icon::Activity, Route::Stats),
    ("Images", Icon::GridTableIcon, Route::Images),
];

pub struct SidebarView {
    state: Entity<AppState>,
    display: SidebarDisplay,
    collapsed: Vec<String>,
    /// How many rows each group shows, by group id, once a Show more revealed past the first
    /// page; absent groups show [`SIDEBAR_PAGE_ROWS`].
    revealed: HashMap<String, usize>,
    /// Each run's mark as its page last showed it (`run_mark`); `None` until the first list,
    /// which marks every run then listed as seen, so an update means one since this app ran.
    seen: Option<HashMap<String, String>>,
    /// The run rows' bounds as last laid out, which place the hover preview beside its row.
    row_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    hover_run: Option<String>,
    preview_run: Option<String>,
    preview_view: Option<Entity<TranscriptView>>,
    preview_open: bool,
    preview_card_hover: bool,
    preview_progress: SpringValue,
    hover_gen: u64,
    /// Whether the open context menu is the Display menu, so its button toggles it.
    display_open: bool,
    context_menu: Entity<Menu>,
    context_x: f32,
    context_y: f32,
    editor: Option<SectionDraft>,
    editor_focus: bool,
    section_name: Entity<TextInput>,
    editor_progress: SpringValue,
    /// The conversation the rename dialog is naming, while it is open.
    renaming: Option<String>,
    rename_focus: bool,
    conversation_name: Entity<TextInput>,
    rename_opacity: SpringValue,
    rename_y: SpringValue,
    /// The conversation Usage dialog, once a conversation menu opened it.
    usage: Option<Entity<ConversationUsageView>>,
    list: ListState,
    slots: Rc<Vec<SidebarSlot>>,
    /// The groups, collapsed ids, revealed limits, and open run the slots were built from.
    slot_groups: Rc<Vec<SidebarGroup>>,
    slot_collapsed: Vec<String>,
    slot_revealed: HashMap<String, usize>,
    slot_selected: Option<String>,
    slot_notice: bool,
    /// The structure of `slots`, so an input that moved without changing it keeps the list.
    slots_key: String,
    animator: Animator,
    /// Grouped runs for the last revision, display, and minute. A hover frame reuses them.
    groups: Rc<Vec<SidebarGroup>>,
    groups_key: Option<(u64, SidebarDisplay, i64)>,
}

/// One item of the sidebar's stack: a page row, the Runs header, the list's notice, or one
/// group's heading, a shown run, or its Show more. The stack is replaced only when `after` or
/// `item` changes for an index; a row reads its run from the groups at render time.
#[derive(Clone, Copy)]
struct SidebarSlot {
    item: StackRow,
    /// The gap under it.
    after: f32,
}

#[derive(Clone, Copy)]
enum StackRow {
    Nav(usize),
    Header,
    Notice,
    Group { group: usize, collapsed: bool },
    Run { group: usize, row: usize },
    More { group: usize, next: usize },
}

impl SidebarSlot {
    /// Names the item across a structure change, so the scroll stays on it.
    fn key(&self, groups: &[SidebarGroup]) -> String {
        match self.item {
            StackRow::Nav(index) => format!("n:{index}"),
            StackRow::Header => "h:runs".to_string(),
            StackRow::Notice => "h:notice".to_string(),
            StackRow::Group { group, .. } => match groups.get(group) {
                Some(group) => format!("g:{}", group.id),
                None => format!("g:{group}"),
            },
            StackRow::Run { group, row } => match groups.get(group).and_then(|g| g.runs.get(row)) {
                Some(run) => format!("r:{}", run.id),
                None => format!("r:{group}:{row}"),
            },
            StackRow::More { group, .. } => match groups.get(group) {
                Some(group) => format!("m:{}", group.id),
                None => format!("m:{group}"),
            },
        }
    }
}

impl SidebarView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        observe(&state, cx);
        let display = match state
            .read(cx)
            .read_preference::<serde_json::Value>(DISPLAY_PREFERENCE)
        {
            Ok(Some(value)) => parse_sidebar_display(&value.to_string()),
            _ => SidebarDisplay::default(),
        };
        let collapsed = state
            .read(cx)
            .read_preference::<Vec<String>>(COLLAPSED_PREFERENCE)
            .ok()
            .flatten()
            .unwrap_or_default();
        let revealed = state
            .read(cx)
            .read_preference::<HashMap<String, usize>>(REVEALED_PREFERENCE)
            .ok()
            .flatten()
            .unwrap_or_default();
        let seen = state
            .read(cx)
            .read_preference::<HashMap<String, String>>(SEEN_PREFERENCE)
            .ok()
            .flatten();
        let view = cx.entity();
        let section_name = cx.new({
            let view = view.clone();
            move |cx| {
                TextInput::new(cx)
                    .placeholder("Section name")
                    .on_submit({
                        let view = view.clone();
                        move |text, window, cx| {
                            let name = text.to_string();
                            let view = view.clone();
                            window.defer(cx, move |_, cx| {
                                let _ = view.update(cx, |this, cx| this.commit_named(name, cx));
                            });
                        }
                    })
                    .on_cancel({
                        let view = view.clone();
                        move |window, cx| {
                            let view = view.clone();
                            window.defer(cx, move |_, cx| {
                                let _ = view.update(cx, |this, cx| this.close_editor(cx));
                            });
                        }
                    })
            }
        });
        let conversation_name = cx.new({
            let view = view.clone();
            move |cx| {
                TextInput::new(cx)
                    .bare()
                    .title()
                    .wrap()
                    .placeholder("Conversation name")
                    .on_submit({
                        let view = view.clone();
                        move |_, window, cx| {
                            let view = view.clone();
                            window.defer(cx, move |_, cx| {
                                let _ = view.update(cx, |this, cx| this.commit_rename(cx));
                            });
                        }
                    })
                    .on_cancel({
                        let view = view.clone();
                        move |window, cx| {
                            let view = view.clone();
                            window.defer(cx, move |_, cx| {
                                let _ = view.update(cx, |this, cx| this.close_rename(cx));
                            });
                        }
                    })
            }
        });
        Self {
            state,
            display,
            collapsed,
            revealed,
            seen,
            row_bounds: Rc::new(RefCell::new(HashMap::new())),
            hover_run: None,
            preview_run: None,
            preview_view: None,
            preview_open: false,
            preview_card_hover: false,
            preview_progress: SpringValue::new(0.0, SpringTier::Fast),
            hover_gen: 0,
            display_open: false,
            context_menu: cx.new(Menu::new),
            context_x: 0.0,
            context_y: 0.0,
            editor: None,
            editor_focus: false,
            section_name,
            editor_progress: SpringValue::new(0.0, SpringTier::Fast),
            renaming: None,
            rename_focus: false,
            conversation_name,
            rename_opacity: SpringValue::new(0.0, SpringTier::Fast),
            rename_y: SpringValue::new(8.0, SpringTier::Fast),
            usage: None,
            list: ListState::new(0, ListAlignment::Top, px(STACK_OVERDRAW)),
            slots: Rc::new(Vec::new()),
            slot_groups: Rc::new(Vec::new()),
            slot_collapsed: Vec::new(),
            slot_revealed: HashMap::new(),
            slot_selected: None,
            slot_notice: false,
            slots_key: String::new(),
            animator: Animator::new(),
            groups: Rc::new(Vec::new()),
            groups_key: None,
        }
    }

    fn read_snapshot(&mut self, cx: &gpui::App) -> Snapshot {
        let now = now_unix_ms();
        let minute = now / 60_000;
        let revision = self.state.read(cx).runs_revision();
        let key = (revision, self.display, minute);
        if self.groups_key != Some(key) {
            let grouped = {
                let app = self.state.read(cx);
                let runs = sidebar_runs(app.workspaces(), app.runs());
                let mut groups = group_sidebar_runs(&runs, app.sections(), &self.display, now);
                // A conversation someone named shows that name over its first message.
                for group in &mut groups {
                    if let Some(name) =
                        conversation_ref(&group.id).and_then(|id| app.conversation_name(id))
                    {
                        group.title = name.to_string();
                    }
                }
                groups
            };
            self.groups = Rc::new(grouped);
            self.groups_key = Some(key);
        }
        let app = self.state.read(cx);
        let show_error = matches!(app.connection(), ConnectionState::Offline { .. })
            || app.last_error().is_some();
        Snapshot {
            groups: Rc::clone(&self.groups),
            route: app.route().clone(),
            show_error,
            connecting: matches!(app.connection(), ConnectionState::Connecting),
        }
    }

    fn sync(&mut self, snapshot: &Snapshot) {
        let live = |id: &String| {
            snapshot
                .groups
                .iter()
                .any(|group| group.runs.iter().any(|run| &run.id == id))
        };
        if self.preview_run.as_ref().is_some_and(|id| !live(id)) {
            self.preview_open = false;
        }
        self.row_bounds.borrow_mut().retain(|id, _| live(id));
        self.editor_progress
            .set_target(if self.editor.is_some() { 1.0 } else { 0.0 });
        self.preview_progress
            .set_target(if self.preview_open { 1.0 } else { 0.0 });
        let renaming = self.renaming.is_some();
        self.rename_opacity
            .set_target(if renaming { 1.0 } else { 0.0 });
        self.rename_y.set_target(if renaming { 0.0 } else { 8.0 });
    }

    fn advance(&mut self, window: &mut Window) {
        let dt = self.animator.dt(window, self.moving());
        if dt > 0.0 {
            self.preview_progress.step(dt);
            self.editor_progress.step(dt);
            self.rename_opacity.step(dt);
            self.rename_y.step(dt);
        }
        if self.moving() {
            let _ = self.animator.dt(window, true);
        }
        if !self.preview_open && self.preview_progress.settled() {
            self.preview_view = None;
            self.preview_run = None;
        }
    }

    fn moving(&self) -> bool {
        !self.preview_progress.settled()
            || !self.editor_progress.settled()
            || !self.rename_opacity.settled()
            || !self.rename_y.settled()
    }

    fn selected_id(&self, route: &Route) -> Option<String> {
        match route {
            Route::Run(id) => Some(id.clone()),
            _ => None,
        }
    }

    fn persist_display(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(DISPLAY_PREFERENCE, &self.display);
    }

    /// Marks the open run as seen, and on the first list every run then listed.
    fn sync_seen(&mut self, snapshot: &Snapshot, cx: &gpui::App) {
        if snapshot.connecting || snapshot.show_error {
            return;
        }
        let runs = || snapshot.groups.iter().flat_map(|group| group.runs.iter());
        let mut changed = false;
        let seen = self.seen.get_or_insert_with(|| {
            changed = true;
            runs().map(|run| (run.id.clone(), run_mark(run))).collect()
        });
        if let Route::Run(id) = &snapshot.route
            && let Some(run) = runs().find(|run| &run.id == id)
            && has_unseen_update(run, seen)
        {
            seen.insert(run.id.clone(), run_mark(run));
            changed = true;
        }
        if changed {
            let _ = self
                .state
                .read(cx)
                .write_preference(SEEN_PREFERENCE, &*seen);
        }
    }

    fn persist_collapsed(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(COLLAPSED_PREFERENCE, &self.collapsed);
    }

    fn persist_revealed(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(REVEALED_PREFERENCE, &self.revealed);
    }

    fn toast(&self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.state.update(cx, |app, cx| {
            app.push_toast(message, ToastTone::Danger);
            cx.notify();
        });
    }

    fn go(&self, route: Route, refresh: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.navigate(route);
            if refresh {
                app.request_refresh(cx);
            }
            cx.notify();
        });
    }

    fn open_commander(&self, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.set_commander_open(true);
            cx.notify();
        });
    }

    fn dismiss_panels(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if self.display_open {
            self.display_open = false;
            changed = true;
        }
        if self.editor.is_some() {
            self.editor = None;
            self.editor_focus = false;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The Display menu, as the Solid app's `DropdownMenu`: Group by, Sort by, and Show as
    /// submenus of radio items, then Reset to defaults (View all is the Runs title). It hangs
    /// under its button with their right edges lined up.
    pub(crate) fn toggle_display(&mut self, cx: &mut Context<Self>) {
        if self.display_open && self.context_menu.read(cx).is_open() {
            self.close_menu(cx);
            self.display_open = false;
            cx.notify();
            return;
        }
        self.editor = None;
        self.editor_focus = false;
        let theme = *Theme::global(cx);
        let items = self.display_items(cx);
        self.context_x =
            (theme.sidebar_width - theme.space_2 - DISPLAY_MENU_WIDTH).max(theme.space_2);
        self.context_y = self.overlay_top(theme) - theme.control_height_md;
        self.context_menu.update(cx, |menu, cx| {
            menu.set_menu(items);
            menu.set_width(DISPLAY_MENU_WIDTH);
            menu.set_open(true);
            cx.notify();
        });
        self.display_open = true;
        cx.notify();
    }

    fn display_items(&self, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let view = cx.entity();
        let radios = |labels: &[&str],
                      picked: usize,
                      pick: fn(&mut Self, usize, &mut Context<Self>)|
         -> Vec<MenuItem> {
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    let view = view.clone();
                    MenuItem::radio(
                        label.to_string(),
                        index == picked,
                        Rc::new(move |_, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.display_open = false;
                                pick(this, index, cx);
                            });
                        }),
                    )
                })
                .collect()
        };
        let group_index = match self.display.group_by {
            GroupBy::Conversation => 0,
            GroupBy::Repository => 1,
            GroupBy::Date => 2,
            GroupBy::Status => 3,
            GroupBy::Agent => 4,
            GroupBy::None => 5,
        };
        let sort_index = match self.display.sort_by {
            SortBy::Updated => 0,
            SortBy::Created => 1,
            SortBy::Name => 2,
        };
        let direction_index = match self.display.direction {
            Direction::Desc => 0,
            Direction::Asc => 1,
        };
        let show_index = match self.display.show {
            Show::All => 0,
            Show::Unmerged => 1,
            Show::Active => 2,
        };
        let mut sort = radios(
            &["Last updated", "Date created", "Name"],
            sort_index,
            Self::pick_sort,
        );
        sort.push(MenuItem::Separator);
        sort.extend(radios(
            &["Descending", "Ascending"],
            direction_index,
            Self::pick_direction,
        ));
        let reset = view.clone();
        vec![
            MenuItem::submenu(
                "Group by",
                Some(Icon::Layers),
                radios(
                    &[
                        "Conversation",
                        "Repository",
                        "Date",
                        "Status",
                        "Agent",
                        "None",
                    ],
                    group_index,
                    Self::pick_group,
                ),
            ),
            MenuItem::submenu("Sort by", Some(Icon::ListOrdered), sort),
            MenuItem::submenu(
                "Show",
                Some(Icon::ListFilter),
                radios(
                    &["All runs", "Hide merged", "Running and needs merge"],
                    show_index,
                    Self::pick_show,
                ),
            ),
            MenuItem::Separator,
            MenuItem::action(
                "Reset to defaults",
                Some(Icon::RefreshCw),
                Rc::new(move |_, cx| {
                    let _ = reset.update(cx, |this, cx| this.reset_display(cx));
                }),
            ),
        ]
    }

    fn reset_display(&mut self, cx: &mut Context<Self>) {
        self.display = SidebarDisplay::default();
        self.persist_display(cx);
        self.revealed.clear();
        self.persist_revealed(cx);
        self.display_open = false;
        cx.notify();
    }

    fn view_all(&mut self, cx: &mut Context<Self>) {
        self.display_open = false;
        self.go(Route::Runs, false, cx);
    }

    fn pick_group(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = match index {
            0 => GroupBy::Conversation,
            1 => GroupBy::Repository,
            2 => GroupBy::Date,
            3 => GroupBy::Status,
            4 => GroupBy::Agent,
            _ => GroupBy::None,
        };
        if self.display.group_by != value {
            self.display.group_by = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn pick_sort(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = match index {
            1 => SortBy::Created,
            2 => SortBy::Name,
            _ => SortBy::Updated,
        };
        if self.display.sort_by != value {
            self.display.sort_by = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn pick_direction(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = if index == 0 {
            Direction::Desc
        } else {
            Direction::Asc
        };
        if self.display.direction != value {
            self.display.direction = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn pick_show(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = match index {
            1 => Show::Unmerged,
            2 => Show::Active,
            _ => Show::All,
        };
        if self.display.show != value {
            self.display.show = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    /// Collapsing a group hides its runs at once, as `<Show>` unmounts them, and forgets the
    /// pages its Show more opened: opened again, it shows its first rows only.
    fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(pos) = self.collapsed.iter().position(|item| item == &id) {
            self.collapsed.remove(pos);
        } else {
            if self.revealed.remove(&id).is_some() {
                self.persist_revealed(cx);
            }
            self.collapsed.push(id);
        }
        self.persist_collapsed(cx);
        cx.notify();
    }

    /// A Show more adds a page of rows to its group, kept with the collapsed groups.
    fn reveal_more(&mut self, id: String, limit: usize, cx: &mut Context<Self>) {
        self.revealed.insert(id, limit);
        self.persist_revealed(cx);
        cx.notify();
    }

    /// Rests the pointer on run `id`'s row, as the hover does (the screenshot tour).
    pub(crate) fn hover_run(&mut self, id: String, cx: &mut Context<Self>) {
        self.hover_run_row(id, true, cx);
    }

    fn hover_run_row(&mut self, id: String, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            if self.hover_run.as_deref() == Some(id.as_str()) {
                return;
            }
            self.hover_run = Some(id.clone());
            self.arm_preview(id, cx);
        } else if self.hover_run.as_deref() == Some(id.as_str()) {
            self.hover_run = None;
            self.disarm_preview(cx);
        } else {
            return;
        }
        cx.notify();
    }

    fn arm_preview(&mut self, run_id: String, cx: &mut Context<Self>) {
        self.hover_gen = self.hover_gen.wrapping_add(1);
        let generation = self.hover_gen;
        let state = self.state.clone();
        cx.spawn(async move |this, async_cx| {
            async_cx
                .background_executor()
                .timer(Duration::from_millis(HOVER_OPEN_MS))
                .await;
            let _ = this.update(async_cx, |this, cx| {
                if this.hover_gen != generation
                    || this.hover_run.as_deref() != Some(run_id.as_str())
                {
                    return;
                }
                if this.preview_run.as_deref() != Some(run_id.as_str()) {
                    let view = cx.new(|cx| TranscriptView::new(state, run_id.clone(), true, cx));
                    this.preview_view = Some(view);
                    this.preview_run = Some(run_id);
                }
                this.preview_open = true;
                cx.notify();
            });
        })
        .detach();
    }

    fn disarm_preview(&mut self, cx: &mut Context<Self>) {
        self.hover_gen = self.hover_gen.wrapping_add(1);
        let generation = self.hover_gen;
        cx.spawn(async move |this, async_cx| {
            async_cx
                .background_executor()
                .timer(Duration::from_millis(HOVER_CLOSE_MS))
                .await;
            let _ = this.update(async_cx, |this, cx| {
                if this.hover_gen != generation
                    || this.hover_run.is_some()
                    || this.preview_card_hover
                {
                    return;
                }
                this.preview_open = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn hover_card(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.preview_card_hover = hovered;
        if hovered {
            self.hover_gen = self.hover_gen.wrapping_add(1);
        } else if self.hover_run.is_none() {
            self.disarm_preview(cx);
        }
    }

    /// The screenshot tour's way in: lights row `index` of the open menu.
    pub(crate) fn highlight_menu_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.context_menu.update(cx, |menu, cx| {
            menu.highlight(index);
            cx.notify();
        });
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu.update(cx, |menu, cx| {
            if menu.is_open() {
                menu.set_open(false);
                cx.notify();
            }
        });
    }

    fn place_menu(
        &mut self,
        x: f32,
        y: f32,
        items: Vec<MenuEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.place_menu_items(x, y, actions_of(items), window, cx);
    }

    /// Opens the context menu at `x`, `y` with any rows, separators among them.
    fn place_menu_items(
        &mut self,
        x: f32,
        y: f32,
        items: Vec<MenuItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = *Theme::global(cx);
        let viewport = window.viewport_size();
        let width = theme.sidebar_width * 0.8;
        let max_x = (px_of(viewport.width) - width).max(0.0);
        let max_y = (px_of(viewport.height) - theme.control_height_md).max(0.0);
        self.context_x = x.clamp(0.0, max_x);
        self.context_y = (y - theme.control_height_md).clamp(0.0, max_y);
        self.display_open = false;
        self.context_menu.update(cx, |menu, cx| {
            menu.set_menu(items);
            menu.set_width(0.0);
            menu.set_open(true);
            cx.notify();
        });
        cx.notify();
    }

    fn section_names(&self, cx: &gpui::App) -> Vec<(String, String)> {
        let mut items: Vec<_> = self
            .state
            .read(cx)
            .sections()
            .iter()
            .map(|section| (section.id.clone(), section.name.clone(), section.position))
            .collect();
        items.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
        items.into_iter().map(|(id, name, _)| (id, name)).collect()
    }

    /// Move to each section the item is not in, and Remove from section only when the item
    /// is in one itself.
    fn move_entries(&self, kind: &str, item_ref: &str, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let view = cx.entity();
        let holder = {
            let app = self.state.read(cx);
            let mut refs = vec![item_ref.to_string()];
            if kind == "run"
                && let Some(run) = sidebar_runs(app.workspaces(), app.runs())
                    .into_iter()
                    .find(|run| run.id == item_ref)
            {
                refs.extend(run.earlier);
            }
            let refs: Vec<&str> = refs.iter().map(String::as_str).collect();
            holding_section(app.sections(), kind, &refs).map(str::to_string)
        };
        let mut items = Vec::new();
        for (id, name) in self.section_names(cx) {
            if holder.as_deref() == Some(id.as_str()) {
                continue;
            }
            let request = MoveToSectionRequest {
                kind: kind.to_string(),
                item_ref: item_ref.to_string(),
                section_id: id,
            };
            let view = view.clone();
            items.push(MenuEntry {
                label: format!("Move to {name}").into(),
                icon: None,
                danger: false,
                on_select: Rc::new(move |_, cx| {
                    let request = request.clone();
                    let _ =
                        view.update(cx, |this, cx| this.spawn_job(SectionJob::Move(request), cx));
                }),
            });
        }
        if holder.is_none() {
            return items;
        }
        let request = MoveToSectionRequest {
            kind: kind.to_string(),
            item_ref: item_ref.to_string(),
            section_id: String::new(),
        };
        let view = view.clone();
        items.push(MenuEntry {
            label: "Remove from section".into(),
            icon: None,
            danger: false,
            on_select: Rc::new(move |_, cx| {
                let request = request.clone();
                let _ = view.update(cx, |this, cx| this.spawn_job(SectionJob::Move(request), cx));
            }),
        });
        items
    }

    /// A custom section's "…": its Rename and Delete entries, at `x`, `y`.
    fn open_section_menu(
        &mut self,
        id: String,
        name: String,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(raw) = section_ref(&id) else {
            return;
        };
        let items = self.section_entries(raw.to_string(), name, cx);
        self.place_menu(x, y, items, window, cx);
    }

    fn section_entries(&self, id: String, title: String, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let view = cx.entity();
        let rename_view = view.clone();
        let rename_id = id.clone();
        let delete_view = view;
        let delete_id = id;
        vec![
            MenuEntry {
                label: "Rename".into(),
                icon: None,
                danger: false,
                on_select: Rc::new(move |window, cx| {
                    let id = rename_id.clone();
                    let title = title.clone();
                    let _ = rename_view.update(cx, |this, cx| {
                        this.open_editor(Some(id), title, window, cx);
                    });
                }),
            },
            MenuEntry {
                label: "Delete section".into(),
                icon: None,
                danger: true,
                on_select: Rc::new(move |_, cx| {
                    let id = delete_id.clone();
                    let _ = delete_view.update(cx, |this, cx| {
                        this.spawn_job(SectionJob::Delete(DeleteSectionRequest { id }), cx);
                    });
                }),
            },
        ]
    }

    /// A danger entry that deletes runs for good: their records, transcripts, and worktrees.
    fn delete_entry(
        &self,
        label: &'static str,
        request: DeleteRunsRequest,
        cx: &mut Context<Self>,
    ) -> MenuEntry {
        let view = cx.entity();
        MenuEntry {
            label: label.into(),
            icon: None,
            danger: true,
            on_select: Rc::new(move |_, cx| {
                let request = request.clone();
                let _ = view.update(cx, |this, cx| this.delete_runs(request, cx));
            }),
        }
    }

    fn delete_runs(&mut self, request: DeleteRunsRequest, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("Dispatch core is not running", cx);
            return;
        };
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move { client.delete_runs(&request) })
                .await;
            let _ = this.update(async_cx, |this, cx| match result {
                Ok(response) => {
                    let ids = response.ids();
                    this.state.update(cx, |app, cx| {
                        app.forget_runs(&ids);
                        app.request_refresh(cx);
                        cx.notify();
                    });
                    cx.notify();
                }
                Err(error) => this.toast(core_message(error), cx),
            });
        })
        .detach();
    }

    /// A row's menu: `delete` is every run the row stands for (a folded chat's earlier ones
    /// too), which Delete run takes together.
    fn open_run_menu(
        &mut self,
        run_id: &str,
        delete: Vec<String>,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items = self.move_entries("run", run_id, cx);
        items.push(self.delete_entry(
            "Delete run",
            DeleteRunsRequest {
                ids: delete,
                ..Default::default()
            },
            cx,
        ));
        self.place_menu(x, y, items, window, cx);
    }

    fn open_group_menu(
        &mut self,
        group: &SidebarGroup,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = if group.kind == GroupKind::Section {
            let Some(id) = section_ref(&group.id).map(str::to_string) else {
                return;
            };
            actions_of(self.section_entries(id, group.title.clone(), cx))
        } else if let Some(repository) = repository_ref(&group.id) {
            let target =
                repository_actions::Repository::at(repository, &group.title, self.state.read(cx));
            let removable = target.workspace_id.clone();
            let mut items = repository_actions::menu_items(&target, &self.state);
            // Removing a repository takes its runs with it. One no workspace was added as
            // (a worktree's, one that is gone) can still be cleared: its ended runs go, a
            // working one stays.
            let ids = ended_run_ids(&group.runs);
            if removable.is_none() && !ids.is_empty() {
                items.push(MenuItem::Separator);
                items.push(MenuItem::Action(self.delete_entry(
                    "Delete runs",
                    DeleteRunsRequest {
                        ids,
                        ..Default::default()
                    },
                    cx,
                )));
            }
            if items.is_empty() {
                return;
            }
            items
        } else if let Some(conversation) = conversation_ref(&group.id) {
            let mut items = vec![
                self.rename_entry(conversation, &group.title, cx),
                self.usage_entry(conversation, cx),
            ];
            items.extend(self.move_entries("conversation", conversation, cx));
            items.push(self.delete_entry(
                "Delete conversation",
                DeleteRunsRequest {
                    conversation_id: conversation.to_string(),
                    ..Default::default()
                },
                cx,
            ));
            actions_of(items)
        } else {
            return;
        };
        self.place_menu_items(x, y, items, window, cx);
    }

    /// Rename…: opens the rename dialog on the conversation's name as it shows.
    fn rename_entry(&self, conversation: &str, title: &str, cx: &mut Context<Self>) -> MenuEntry {
        let view = cx.entity();
        let conversation = conversation.to_string();
        let title = title.to_string();
        MenuEntry {
            label: "Rename…".into(),
            icon: None,
            danger: false,
            on_select: Rc::new(move |_, cx| {
                let conversation = conversation.clone();
                let title = title.clone();
                let _ = view.update(cx, |this, cx| this.open_rename(conversation, title, cx));
            }),
        }
    }

    /// Opens the conversation's Usage dialog: its Dispatch runs and its calling session.
    fn usage_entry(&self, conversation: &str, cx: &mut Context<Self>) -> MenuEntry {
        let view = cx.entity();
        let conversation = conversation.to_string();
        MenuEntry {
            label: "Usage".into(),
            icon: None,
            danger: false,
            on_select: Rc::new(move |_, cx| {
                let conversation = conversation.clone();
                let _ = view.update(cx, |this, cx| this.open_usage(conversation, cx));
            }),
        }
    }

    fn open_usage(&mut self, conversation: String, cx: &mut Context<Self>) {
        self.close_menu(cx);
        let state = self.state.clone();
        let dialog = cx.new(|cx| ConversationUsageView::new(state, conversation, cx));
        dialog.update(cx, |dialog, cx| dialog.open(cx));
        self.usage = Some(dialog);
        cx.notify();
    }

    fn open_rename(&mut self, conversation: String, title: String, cx: &mut Context<Self>) {
        self.renaming = Some(conversation);
        self.rename_focus = true;
        self.display_open = false;
        self.close_editor(cx);
        self.conversation_name
            .update(cx, |input, cx| input.set_text(title, cx));
        self.close_menu(cx);
        cx.notify();
    }

    fn close_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.rename_focus = false;
            cx.notify();
        }
    }

    /// Saves the name; an empty one gives the conversation back its first message's title.
    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(conversation_id) = self.renaming.clone() else {
            return;
        };
        let Some(client) = self.state.read(cx).client() else {
            self.toast("Dispatch core is not running", cx);
            return;
        };
        let name = self.conversation_name.read(cx).text().trim().to_string();
        let request = RenameConversationRequest {
            conversation_id: conversation_id.clone(),
            name: name.clone(),
        };
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move { client.rename_conversation(&request) })
                .await;
            let _ = this.update(async_cx, |this, cx| match result {
                Ok(_) => {
                    if this.renaming.as_deref() == Some(conversation_id.as_str()) {
                        this.close_rename(cx);
                    }
                    this.state.update(cx, |app, cx| {
                        app.note_conversation_name(&conversation_id, &name);
                        app.request_refresh(cx);
                        cx.notify();
                    });
                    cx.notify();
                }
                Err(error) => this.toast(core_message(error), cx),
            });
        })
        .detach();
    }

    /// The rename dialog: the Composer card with the name as its title field, and Save.
    fn rename_layer(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = self.rename_opacity.value();
        if self.renaming.is_none() && t < 0.01 {
            return None;
        }
        let view = cx.entity();
        let closer = view.clone();
        let saver = view.clone();
        let card = composer_card("Rename conversation", t, self.rename_y.value(), theme)
            .on_close(Rc::new(move |_, cx| {
                let _ = closer.update(cx, |this, cx| this.close_rename(cx));
            }))
            .child(self.conversation_name.clone())
            .footer(composer_footer(
                Vec::new(),
                [button(
                    "sidebar-rename-save",
                    "Save",
                    ButtonKind::Primary,
                    Rc::new(move |_, cx| {
                        let _ = saver.update(cx, |this, cx| this.commit_rename(cx));
                    }),
                    cx,
                )
                .size(ButtonSize::Sm)
                .into_any_element()],
                theme,
            ));
        Some(modal_layer(
            t,
            self.renaming.is_some(),
            Rc::new(move |_, cx| {
                let _ = view.update(cx, |this, cx| this.close_rename(cx));
            }),
            card,
            theme,
        ))
    }

    fn open_editor(
        &mut self,
        id: Option<String>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = window;
        self.editor = Some(SectionDraft { id });
        self.editor_focus = true;
        self.display_open = false;
        self.section_name
            .update(cx, |input, cx| input.set_text(name, cx));
        self.close_menu(cx);
        cx.notify();
    }

    fn close_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_none() {
            return;
        }
        self.editor = None;
        self.editor_focus = false;
        cx.notify();
    }

    fn commit_named(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(draft) = self.editor.clone() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.save_section(draft, name, cx);
    }

    fn commit_section(&mut self, cx: &mut Context<Self>) {
        let name = self.section_name.read(cx).text().to_string();
        self.commit_named(name, cx);
    }

    fn save_section(&mut self, draft: SectionDraft, name: String, cx: &mut Context<Self>) {
        let position = if let Some(id) = &draft.id {
            self.state
                .read(cx)
                .sections()
                .iter()
                .find(|section| &section.id == id)
                .map(|section| section.position)
                .unwrap_or(0)
        } else {
            self.state
                .read(cx)
                .sections()
                .iter()
                .map(|section| section.position)
                .max()
                .unwrap_or(-1)
                .saturating_add(1)
        };
        self.spawn_job(
            SectionJob::Save(SaveSectionRequest {
                id: draft.id.unwrap_or_default(),
                name,
                position,
            }),
            cx,
        );
    }

    fn spawn_job(&mut self, job: SectionJob, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.toast("Dispatch core is not running", cx);
            return;
        };
        let close_editor = matches!(job, SectionJob::Save(_));
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move {
                    match job {
                        SectionJob::Save(request) => client.save_section(&request).map(|_| ()),
                        SectionJob::Delete(request) => client.delete_section(&request).map(|_| ()),
                        SectionJob::Move(request) => client.move_to_section(&request).map(|_| ()),
                    }
                })
                .await;
            let _ = this.update(async_cx, |this, cx| match result {
                Ok(()) => {
                    if close_editor {
                        this.editor = None;
                        this.editor_focus = false;
                    }
                    this.state.update(cx, |app, cx| {
                        app.request_refresh(cx);
                        cx.notify();
                    });
                    cx.notify();
                }
                Err(error) => this.toast(core_message(error), cx),
            });
        })
        .detach();
    }

    /// The pages as rows of the stack: `<nav class="pw-sidebar-group">`, New (+), Commander
    /// (their ⌘N and ⌘K show under the pointer), and then each page, 2px apart.
    fn nav_item(
        index: usize,
        view: &Entity<SidebarView>,
        theme: Theme,
        route: &Route,
    ) -> gpui::AnyElement {
        match index {
            0 => {
                let view = view.clone();
                SidebarItem::new("sidebar-nav-New", "New")
                    .icon(Icon::Plus)
                    .trailing(kbd("⌘N", &theme))
                    .trailing_on_hover()
                    .active(matches!(route, Route::NewRun))
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.go(Route::NewRun, false, cx));
                    })
                    .into_any_element()
            }
            1 => {
                let view = view.clone();
                SidebarItem::new("sidebar-nav-Commander", "Commander")
                    .icon(Icon::Search)
                    .trailing(kbd("⌘K", &theme))
                    .trailing_on_hover()
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.open_commander(cx));
                    })
                    .into_any_element()
            }
            index => {
                let Some((label, icon, target)) = NAV.get(index - 2) else {
                    return div().into_any_element();
                };
                let view = view.clone();
                let target = target.clone();
                let selector = format!("sidebar-nav-{label}");
                let item = SidebarItem::new(SharedString::from(selector.clone()), *label)
                    .icon(*icon)
                    .active(page_selected(route, label))
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.go(target.clone(), false, cx));
                    });
                div()
                    .w_full()
                    .debug_selector(move || selector.clone())
                    .child(item)
                    .into_any_element()
            }
        }
    }

    /// The list's notice, when it has one: the runs in bones while the core starts, or the
    /// mascot asleep or sad.
    fn notice_item(
        notice: RunsNotice,
        theme: Theme,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui::AnyElement> {
        match notice {
            RunsNotice::None => None,
            // The core is starting: the runs, in bones, until it answers.
            RunsNotice::Loading => Some(
                div()
                    .px(px(theme.space_2))
                    .child(list_skeleton(6, 32.0, window, cx))
                    .into_any_element(),
            ),
            RunsNotice::Empty => {
                Some(list_empty("No runs to show.", MascotMood::Sleeping, cx).into_any_element())
            }
            RunsNotice::Unreachable => {
                Some(list_empty(REFRESH_FAILED, MascotMood::Sad, cx).into_any_element())
            }
            RunsNotice::Stale => Some(run_none(REFRESH_FAILED, &theme).into_any_element()),
        }
    }

    /// The pages and the runs as one virtual stack between the titlebar and the footer: only
    /// the items in view are built, and each fades itself as it nears an end of the scroll.
    fn stack(
        &self,
        theme: Theme,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view = cx.entity();
        let state = self.state.clone();
        let slots = self.slots.clone();
        let groups = snapshot.groups.clone();
        let route = snapshot.route.clone();
        let selected = self.selected_id(&snapshot.route);
        let row_bounds = self.row_bounds.clone();
        let notice = runs_notice(!groups.is_empty(), snapshot.connecting, snapshot.show_error);
        // The shown rows whose page has not shown them since their last update.
        let unseen: Rc<std::collections::HashSet<String>> = Rc::new(match &self.seen {
            Some(seen) => slots
                .iter()
                .filter_map(|slot| match slot.item {
                    StackRow::Run { group, row } => groups
                        .get(group)
                        .and_then(|group| group.runs.get(row))
                        .filter(|run| has_unseen_update(run, seen))
                        .map(|run| run.id.clone()),
                    _ => None,
                })
                .collect(),
            None => std::collections::HashSet::new(),
        });
        let edges = fade_edges(theme);
        virtual_fade_list(
            "sidebar-scroll",
            &self.list,
            theme.titlebar_height + theme.space_3,
            footer_height(theme) + theme.space_2,
            edges,
            move |index, window, cx| {
                let Some(slot) = slots.get(index) else {
                    return div().into_any_element();
                };
                let body = match slot.item {
                    StackRow::Nav(index) => SidebarView::nav_item(index, &view, theme, &route),
                    StackRow::Header => SidebarView::runs_header(&view, state.clone(), theme, cx),
                    StackRow::Notice => {
                        let Some(item) = SidebarView::notice_item(notice, theme, window, cx) else {
                            return div().into_any_element();
                        };
                        item
                    }
                    StackRow::Group { group, collapsed } => {
                        let Some(group) = groups.get(group) else {
                            return div().into_any_element();
                        };
                        SidebarView::group_heading(group, collapsed, &view, theme)
                    }
                    StackRow::Run { group, row } => {
                        let Some(run) = groups.get(group).and_then(|group| group.runs.get(row))
                        else {
                            return div().into_any_element();
                        };
                        SidebarView::run_row(
                            run,
                            selected.as_deref() == Some(run.id.as_str()),
                            unseen.contains(&run.id),
                            &view,
                            row_bounds.clone(),
                            theme,
                        )
                    }
                    StackRow::More { group, next } => {
                        let Some(group) = groups.get(group) else {
                            return div().into_any_element();
                        };
                        SidebarView::show_more_row(group, next, &view)
                    }
                };
                div()
                    .w_full()
                    .when(slot.after > 0.0, |item| item.mb(px(slot.after)))
                    .child(body)
                    .into_any_element()
            },
            window,
            cx,
        )
    }

    /// The items of the stack: the pages, the Runs header, the list's notice, and each
    /// group's heading, shown rows, and Show more. Replaced only when one of them moves, so a
    /// hover or a bot's frame reuses the stack.
    fn sync_slots(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let theme = *Theme::global(cx);
        let selected = self.selected_id(&snapshot.route);
        let notice = runs_notice(
            !snapshot.groups.is_empty(),
            snapshot.connecting,
            snapshot.show_error,
        );
        let notice_shown = notice != RunsNotice::None;
        let changed = !Rc::ptr_eq(&self.slot_groups, &snapshot.groups)
            || self.slot_collapsed != self.collapsed
            || self.slot_revealed != self.revealed
            || self.slot_selected != selected
            || self.slot_notice != notice_shown;
        if !changed {
            return;
        }
        let (slots, key) = sidebar_slots(
            &snapshot.groups,
            &self.collapsed,
            &self.revealed,
            selected.as_deref(),
            notice_shown,
            theme,
        );
        let anchor = (key != self.slots_key).then(|| self.scroll_anchor());
        self.slot_groups = snapshot.groups.clone();
        self.slot_collapsed = self.collapsed.clone();
        self.slot_revealed = self.revealed.clone();
        self.slot_selected = selected;
        self.slot_notice = notice_shown;
        let Some(anchor) = anchor else {
            return;
        };
        self.slots = slots;
        self.slots_key = key;
        sync_virtual_list(&self.list, self.slots.len(), true);
        self.restore_scroll(anchor);
    }

    /// The item under the scroll, so a fold above it does not jump to another row.
    fn scroll_anchor(&self) -> Option<(String, Pixels)> {
        if self.slots.is_empty() {
            return None;
        }
        let top = self.list.logical_scroll_top();
        let index = top.item_ix.min(self.slots.len() - 1);
        Some((self.slots[index].key(&self.slot_groups), top.offset_in_item))
    }

    fn restore_scroll(&self, anchor: Option<(String, Pixels)>) {
        let Some((key, offset)) = anchor else {
            return;
        };
        let Some(item_ix) = self
            .slots
            .iter()
            .position(|slot| slot.key(&self.slot_groups) == key)
        else {
            return;
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: offset,
        });
    }

    /// `.dispatch-runs-header`: "Runs" as a ghost button that opens View all ([`Route::Runs`]),
    /// then the Display menu's ghost icon button and Add repository's `+` at the end. A
    /// repository added there opens New on it.
    fn runs_header(
        view: &Entity<SidebarView>,
        state: Entity<AppState>,
        _theme: Theme,
        cx: &App,
    ) -> gpui::AnyElement {
        let view = view.clone();
        let all = view.clone();
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("sidebar-view-all", ButtonKind::Ghost)
                            .size(ButtonSize::Sm)
                            .label("Runs")
                            .tooltip("View all runs")
                            .on_click(move |_, cx| {
                                let _ = all.update(cx, |this, cx| this.view_all(cx));
                            }),
                    ),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .items_center()
                    .child(icon_button(
                        "sidebar-display",
                        Icon::ListFilter,
                        "Display",
                        Rc::new(move |_, cx| {
                            let _ = view.update(cx, |this, cx| this.toggle_display(cx));
                        }),
                        cx,
                    ))
                    .child(icon_button(
                        "sidebar-add-repository",
                        Icon::Plus,
                        "Add repository",
                        Rc::new(move |_, cx| {
                            let _ = state.update(cx, |state, cx| {
                                state.add_repository(cx, |state, _, _, _| {
                                    state.navigate(Route::NewRun);
                                });
                            });
                        }),
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// `.dispatch-run-section`'s heading: the group's toggle label (after a 20px bot for a
    /// conversation, before the "…" of a custom section). Its runs follow it as their own
    /// items.
    fn group_heading(
        group: &SidebarGroup,
        collapsed: bool,
        view: &Entity<SidebarView>,
        theme: Theme,
    ) -> gpui::AnyElement {
        let custom = group.kind == GroupKind::Section;
        let label = {
            let toggle_id = group.id.clone();
            let menu_group = group.clone();
            let toggle_view = view.clone();
            let menu_view = view.clone();
            SidebarSectionLabel::new(
                SharedString::from(format!("sidebar-header-{}", group.id)),
                group.title.clone(),
                collapsed,
            )
            .title_tooltip(group_title_tooltip(&group.title))
            .padding(theme.space_2, if custom { 32.0 } else { theme.space_2 })
            .on_toggle(move |_, cx| {
                let id = toggle_id.clone();
                let _ = toggle_view.update(cx, |this, cx| this.toggle_group(id, cx));
            })
            .on_right_click(move |event, window, cx| {
                let group = menu_group.clone();
                let (x, y) = (px_of(event.position.x), px_of(event.position.y));
                let _ = menu_view.update(cx, |this, cx| {
                    this.open_group_menu(&group, x, y, window, cx)
                });
            })
        };

        // The heading row: its label, and a section's "…" over it. A conversation reads as
        // its title alone, cut short, with the chevron after it.
        let mut heading = div()
            .relative()
            .w_full()
            .debug_selector({
                let id = group.id.clone();
                move || format!("sidebar-group-{id}")
            })
            .child(label);
        if custom {
            let section_id = group.id.clone();
            let section_name = group.title.clone();
            let view = view.clone();
            heading = heading.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new(
                            SharedString::from(format!("sidebar-more-{}", group.id)),
                            ButtonKind::Ghost,
                        )
                        .size(ButtonSize::Icon)
                        .icon(Icon::More)
                        .tooltip(format!("Actions for {}", group.title))
                        .on_click(move |window, cx| {
                            let id = section_id.clone();
                            let name = section_name.clone();
                            let position = window.mouse_position();
                            let (x, y) = (px_of(position.x), px_of(position.y));
                            let _ = view.update(cx, |this, cx| {
                                this.open_section_menu(id, name, x, y, window, cx);
                            });
                        }),
                    ),
            );
        }
        heading.into_any_element()
    }

    /// The Show more row under a group's shown runs: it asks for `next` rows.
    fn show_more_row(
        group: &SidebarGroup,
        next: usize,
        view: &Entity<SidebarView>,
    ) -> gpui::AnyElement {
        let more_id = format!("sidebar-show-more-{}", group.id);
        let reveal_id = group.id.clone();
        let reveal_view = view.clone();
        let more = SidebarItem::new(SharedString::from(more_id.clone()), "Show more").on_click(
            move |_, cx| {
                let id = reveal_id.clone();
                let _ = reveal_view.update(cx, |this, cx| this.reveal_more(id, next, cx));
            },
        );
        div()
            .w_full()
            .debug_selector(move || more_id.clone())
            .child(more)
            .into_any_element()
    }

    /// A run row (`SidebarItem.dispatch-run-item`): the 40px bot in a 36px box, the agent in
    /// 14px/500 primary with the model in 11px muted 6px after it, over the prompt's first
    /// line in 12px muted, and the 6px status dot. A long hover opens its transcript.
    fn run_row(
        run: &SidebarRun,
        active: bool,
        unseen: bool,
        view: &Entity<SidebarView>,
        row_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
        theme: Theme,
    ) -> gpui::AnyElement {
        let view = view.clone();
        let run_id = run.id.clone();
        let model = run.model.trim().to_string();
        let mut agent_line = div()
            .flex()
            .items_baseline()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(14.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.color_text_primary)
            .child(div().flex_none().child(agent_display_name(&run.agent)));
        if !model.is_empty() {
            agent_line = agent_line.child(
                div()
                    .flex_shrink()
                    .min_w(px(0.0))
                    .pl(px(6.0))
                    .text_size(px(theme.font_size_xs))
                    .font_weight(gpui::FontWeight::NORMAL)
                    .text_color(theme.color_text_muted)
                    .child(ellipsis(model)),
            );
        }
        let content = div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .min_w(px(0.0))
            .child(agent_line)
            .child(
                div()
                    .text_size(px(theme.font_size_sm))
                    .text_color(theme.color_text_muted)
                    .child(ellipsis(sidebar_run_title(run))),
            );
        let item = SidebarItem::new(SharedString::from(format!("run-row-{}", run.id)), "")
            .run_row()
            .active(active)
            .leading(
                agent_avatar_live(&run.agent, avatar_status(&run.status, &run.merged_at), 40.0),
                36.0,
            )
            .content(content)
            .when(unseen, |item| item.trailing(run_dot(&run.status, &theme)))
            .on_hover({
                let view = view.clone();
                let run_id = run_id.clone();
                move |hovered, _, cx| {
                    let id = run_id.clone();
                    let _ = view.update(cx, |this, cx| this.hover_run_row(id, hovered, cx));
                }
            })
            .on_right_click({
                let view = view.clone();
                let run_id = run_id.clone();
                let delete = row_delete_ids(run);
                move |event, window, cx| {
                    let id = run_id.clone();
                    let delete = delete.clone();
                    let (x, y) = (px_of(event.position.x), px_of(event.position.y));
                    let _ = view.update(cx, |this, cx| {
                        this.open_run_menu(&id, delete, x, y, window, cx)
                    });
                }
            })
            .on_click(move |_, cx| {
                let id = run_id.clone();
                let _ = view.update(cx, |this, cx| this.go(Route::Run(id), true, cx));
            });
        let bounds = row_bounds;
        let key = run.id.clone();
        let row_key = run.id.clone();
        div()
            .relative()
            .w_full()
            .debug_selector(move || format!("run-row-{row_key}"))
            .child(item)
            .child(
                canvas(
                    move |row, _, _| {
                        bounds.borrow_mut().insert(key.clone(), row);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    /// `.dispatch-sidebar-footer`: the Settings item, which opens the settings over the app,
    /// so it is never the open page.
    fn footer_block(
        &self,
        theme: Theme,
        _route: &Route,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view = cx.entity();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme.space_1))
            .pt(px(theme.space_2))
            .child(
                SidebarItem::new("sidebar-settings", "Settings")
                    .icon(Icon::Settings)
                    .footer()
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.go(Route::Settings(SettingsPage::Agents), false, cx);
                        });
                    }),
            )
            .into_any_element()
    }

    /// Scrolls the list `y` down from its top (the screenshot tour).
    pub(crate) fn scroll_to(&mut self, y: f32, cx: &mut Context<Self>) {
        let offset = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        self.list.scroll_by(px(y - offset));
        cx.notify();
    }

    fn overlay_top(&self, theme: Theme) -> f32 {
        let bounds = self.list.viewport_bounds();
        if px_of(bounds.size.height) > 1.0 {
            // The runs header sits under the pages, inside the same scroll.
            px_of(bounds.origin.y)
                + (-f32::from(self.list.scroll_px_offset_for_scrollbar().y)).max(0.0)
                + theme.titlebar_height
                + theme.space_3
                + nav_height(theme)
                + theme.space_2 * 2.0
                + theme.control_height_md
        } else {
            theme.titlebar_height
                + theme.space_3
                + nav_height(theme)
                + theme.space_2 * 3.0
                + theme.control_height_md
        }
    }

    fn editor_panel(
        &self,
        theme: Theme,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let t = self.editor_progress.value().clamp(0.0, 1.0);
        if self.editor.is_none() && t < 0.01 {
            return None;
        }
        let renaming = self.editor.as_ref().is_some_and(|draft| draft.id.is_some());
        let title = if renaming {
            "Rename section"
        } else {
            "New section"
        };
        let submit = if renaming {
            "Save section"
        } else {
            "Create section"
        };
        let view = cx.entity();
        let top = self.overlay_top(theme) + theme.space_4 + (1.0 - t) * theme.space_2;
        Some(
            deferred(
                div()
                    .absolute()
                    .left(px(theme.space_2))
                    .top(px(top))
                    .w(px(theme.sidebar_width - theme.space_4))
                    .occlude()
                    .opacity(t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .p(px(theme.space_3))
                            .flex()
                            .flex_col()
                            .gap(px(theme.space_3))
                            .rounded(px(theme.radius_lg))
                            .bg(theme.color_surface_raised)
                            .border_1()
                            .border_color(theme.color_border)
                            .child(
                                div()
                                    .text_size(px(theme.font_size_sm))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.color_text_primary)
                                    .child(title),
                            )
                            .child(self.section_name.clone())
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(theme.space_2))
                                    .child(button(
                                        "sidebar-cancel-section",
                                        "Cancel",
                                        ButtonKind::Ghost,
                                        Rc::new({
                                            let view = view.clone();
                                            move |_, cx| {
                                                let _ = view
                                                    .update(cx, |this, cx| this.close_editor(cx));
                                            }
                                        }),
                                        cx,
                                    ))
                                    .child(button(
                                        "sidebar-save-section",
                                        submit,
                                        ButtonKind::Primary,
                                        Rc::new(move |_, cx| {
                                            let _ =
                                                view.update(cx, |this, cx| this.commit_section(cx));
                                        }),
                                        cx,
                                    )),
                            ),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    fn preview_card(
        &self,
        theme: Theme,
        _snapshot: &Snapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let t = self.preview_progress.value().clamp(0.0, 1.0);
        let view = self.preview_view.clone()?;
        if t < 0.01 && !self.preview_open {
            return None;
        }
        let run_id = self.preview_run.clone()?;
        let row = self.row_bounds.borrow().get(&run_id).copied()?;
        // `HoverCard` at `right-start` with a 12px gutter, slid back inside the window's 8px
        // overflow padding; `.pw-run-preview` is 340 wide and at most 260 high.
        let card_w = 340.0;
        let card_h = 260.0;
        let viewport_h = px_of(window.viewport_size().height);
        let top = px_of(row.origin.y).clamp(
            theme.space_2,
            (viewport_h - card_h - theme.space_2).max(theme.space_2),
        );
        let left = px_of(row.origin.x + row.size.width) + 12.0;
        let slide = (1.0 - t) * theme.space_2;
        let sidebar = cx.entity();
        Some(
            deferred(
                div()
                    .id("sidebar-hover-card")
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .on_hover({
                        move |hovered, _, cx| {
                            let _ = sidebar.update(cx, |this, cx| this.hover_card(*hovered, cx));
                        }
                    })
                    .child(preview_surface(card_w, card_h, t, slide, &theme, view)),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    fn menu_anchor(&self, theme: Theme) -> gpui::AnyElement {
        let _ = theme;
        deferred(
            div()
                .absolute()
                .left(px(self.context_x))
                .top(px(self.context_y))
                .child(div().relative().child(self.context_menu.clone())),
        )
        .with_priority(4)
        .into_any_element()
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        let snapshot = self.read_snapshot(cx);
        self.sync(&snapshot);
        self.sync_seen(&snapshot, cx);
        self.advance(window);
        if self.editor_focus {
            self.section_name.read(cx).focus_handle(cx).focus(window);
            self.editor_focus = false;
        }
        if self.rename_focus {
            self.conversation_name
                .read(cx)
                .focus_handle(cx)
                .focus(window);
            self.rename_focus = false;
        }
        self.sync_slots(&snapshot, cx);
        let stack = self.stack(theme, &snapshot, window, cx);
        let footer = self.footer_block(theme, &snapshot.route, cx);
        let editor = self.editor_panel(theme, window, cx);
        let preview = self.preview_card(theme, &snapshot, window, cx);
        let menu = self.menu_anchor(theme);
        let rename = self.rename_layer(&theme, cx);

        // The whole sidebar is the drag region. On macOS, gpui 0.2.2 ignores
        // window-control hit testing, so the native titlebar strip is what moves the window.
        let mut root = div()
            .id("sidebar")
            .relative()
            .w(px(theme.sidebar_width))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .window_drag_region()
            .px(px(theme.space_2))
            .flex()
            .flex_col()
            .font_family(theme.font_sans)
            .text_color(theme.color_text_primary)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss_panels(cx)),
            )
            // The pages and the runs are one virtual stack that runs up under the titlebar and
            // down under Settings, which stays put; each row fades out before either covers
            // it, and only the rows in view are built.
            .child(stack)
            .child(history_buttons(&self.state, theme, cx))
            .child(
                div()
                    .absolute()
                    .left(px(theme.space_2))
                    .right(px(theme.space_2))
                    .bottom(px(0.0))
                    .pb(px(theme.space_2))
                    // The list runs on under Settings: a press here is Settings', never the
                    // faded row beneath it.
                    .occlude()
                    .child(footer),
            )
            .child(menu);
        if let Some(editor) = editor {
            root = root.child(editor);
        }
        if let Some(rename) = rename {
            root = root.child(rename);
        }
        if let Some(usage) = self.usage.clone() {
            root = root.child(usage);
        }
        if let Some(preview) = preview {
            root = root.child(preview);
        }
        root
    }
}

/// Go back and Go forward at the top right, on the traffic lights' row.
fn history_buttons(state: &Entity<AppState>, theme: Theme, cx: &mut App) -> impl IntoElement {
    let app = state.read(cx);
    let (can_back, can_forward) = (app.can_go_back(), app.can_go_forward());
    let back_state = state.clone();
    let forward_state = state.clone();
    div()
        .absolute()
        .top(px(0.0))
        .right(px(theme.space_2))
        .h(px(theme.titlebar_height))
        .flex()
        .items_center()
        // A press on a button is the button's, never the window drag's.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            Button::new("sidebar-back", ButtonKind::Ghost)
                .size(ButtonSize::Icon)
                .icon(Icon::ChevronLeft)
                .tooltip("Go back")
                .disabled(!can_back)
                .on_click(move |_, cx| {
                    back_state.update(cx, |state, cx| {
                        if state.go_back() {
                            state.request_refresh(cx);
                        }
                        cx.notify();
                    });
                }),
        )
        .child(
            Button::new("sidebar-forward", ButtonKind::Ghost)
                .size(ButtonSize::Icon)
                .icon(Icon::ChevronRight)
                .tooltip("Go forward")
                .disabled(!can_forward)
                .on_click(move |_, cx| {
                    forward_state.update(cx, |state, cx| {
                        if state.go_forward() {
                            state.request_refresh(cx);
                        }
                        cx.notify();
                    });
                }),
        )
}

fn page_selected(route: &Route, label: &str) -> bool {
    match label {
        "Roles" => matches!(route, Route::Roles | Route::RoleBoard(_)),
        "Intelligence" => matches!(route, Route::Intelligence),
        "Spend" => matches!(route, Route::Spend | Route::SpendAgent(_)),
        "Stats" => matches!(route, Route::Stats),
        "Images" => matches!(route, Route::Images | Route::Image(_)),
        _ => false,
    }
}

/// Settings at the foot, over the list: its 8px above, its row, and the sidebar's 8px under.
fn footer_height(theme: Theme) -> f32 {
    theme.space_2 * 2.0 + theme.sidebar_row_height
}

/// How many rows the pages take: New, Commander, then each page in [`NAV`].
fn nav_items() -> usize {
    2 + NAV.len()
}

/// The sidebar's items in order: the pages, the Runs header (8px more below them), the
/// list's notice, and each group's heading with its first page of runs and its Show more.
/// `key` names the structure, so a change that leaves it whole (a new title, a status) does
/// not measure the stack again.
fn sidebar_slots(
    groups: &[SidebarGroup],
    collapsed: &[String],
    revealed: &HashMap<String, usize>,
    selected: Option<&str>,
    notice_shown: bool,
    theme: Theme,
) -> (Rc<Vec<SidebarSlot>>, String) {
    let gap = theme.space_2;
    let row_gap = theme.sidebar_row_gap;
    let mut slots = Vec::new();
    let mut key = String::new();

    let nav = nav_items();
    for index in 0..nav {
        key.push('n');
        slots.push(SidebarSlot {
            item: StackRow::Nav(index),
            after: if index + 1 == nav { gap * 2.0 } else { row_gap },
        });
    }

    // The Runs header, then its notice, then the groups, 8px apart.
    let anything_below = notice_shown || !groups.is_empty();
    slots.push(SidebarSlot {
        item: StackRow::Header,
        after: if anything_below { gap } else { 0.0 },
    });
    if notice_shown {
        key.push('t');
        slots.push(SidebarSlot {
            item: StackRow::Notice,
            after: if groups.is_empty() { 0.0 } else { gap },
        });
    }

    let last_group = groups.len().saturating_sub(1);
    for (group_index, group) in groups.iter().enumerate() {
        let after_group = if group_index == last_group { 0.0 } else { gap };
        let folded = collapsed.iter().any(|id| id == &group.id);
        key.push('\u{1e}');
        key.push_str(&group.id);
        key.push(if folded { 'c' } else { 'o' });
        // Five rows, strictly: only the open run stays past the page, in the Display order.
        let limit = revealed
            .get(&group.id)
            .copied()
            .unwrap_or(SIDEBAR_PAGE_ROWS)
            .max(1);
        let page = (!folded).then(|| {
            page_sidebar_rows(&group.runs, limit, |run| selected == Some(run.id.as_str()))
        });
        let shown: Vec<usize> = page
            .as_ref()
            .map(|page| {
                page.rows
                    .iter()
                    .filter_map(|run| group.runs.iter().position(|row| row.id == run.id))
                    .collect()
            })
            .unwrap_or_default();
        let hidden = page.as_ref().map_or(0, |page| page.hidden);
        let next = page.as_ref().map_or(limit, |page| page.next_limit);
        let has_rows = !shown.is_empty() || hidden > 0;
        slots.push(SidebarSlot {
            item: StackRow::Group {
                group: group_index,
                collapsed: folded,
            },
            after: if has_rows { row_gap } else { after_group },
        });
        for (position, row) in shown.iter().copied().enumerate() {
            key.push('\u{1f}');
            key.push_str(&group.runs[row].id);
            let last_row = position + 1 == shown.len();
            slots.push(SidebarSlot {
                item: StackRow::Run {
                    group: group_index,
                    row,
                },
                after: if !last_row || hidden > 0 {
                    row_gap
                } else {
                    after_group
                },
            });
        }
        if hidden > 0 {
            key.push('m');
            slots.push(SidebarSlot {
                item: StackRow::More {
                    group: group_index,
                    next,
                },
                after: after_group,
            });
        }
    }

    (Rc::new(slots), key)
}

/// Where the list's rows fade: out under the traffic lights by the titlebar's middle, and
/// out under Settings by the middle of its row, each over a 40-odd px stretch.
fn fade_edges(theme: Theme) -> FadeEdges {
    let top_inset = theme.titlebar_height * 0.5;
    let bottom_inset = footer_height(theme) - theme.space_2;
    FadeEdges::new(
        theme.titlebar_height + theme.space_3 - top_inset + theme.space_4,
        theme.space_4 * 2.5,
    )
    .insets(top_inset, bottom_inset)
}

fn nav_height(theme: Theme) -> f32 {
    6.0 * theme.sidebar_row_height + 5.0 * theme.sidebar_row_gap
}

use super::core_message;

/// Plain entries as menu rows.
fn actions_of(entries: Vec<MenuEntry>) -> Vec<MenuItem> {
    entries.into_iter().map(MenuItem::Action).collect()
}

fn px_of(value: impl Into<f32>) -> f32 {
    value.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DispatchRun;

    fn group(id: &str, runs: usize) -> SidebarGroup {
        SidebarGroup {
            id: id.to_string(),
            kind: GroupKind::Conversation,
            title: id.to_string(),
            agent: None,
            runs: (0..runs)
                .map(|index| {
                    let run = DispatchRun {
                        id: format!("{id}-{index}"),
                        prompt: format!("say hi {index}"),
                        ..Default::default()
                    };
                    SidebarRun::from_dispatch(&run, "repo", "/tmp/repo")
                })
                .collect(),
        }
    }

    fn rows(slots: &[SidebarSlot], group: usize) -> Vec<&SidebarSlot> {
        slots
            .iter()
            .filter(|slot| matches!(slot.item, StackRow::Run { group: g, .. } if g == group))
            .collect()
    }

    /// A group is its heading, its first page of rows, and Show more: the rows past the page
    /// are never items, so the list never builds them.
    #[test]
    fn the_stack_holds_a_groups_page_and_its_show_more() {
        let theme = Theme::dark();
        let groups = vec![group("a", 17)];
        let (slots, key) = sidebar_slots(&groups, &[], &HashMap::new(), None, false, theme);
        assert_eq!(
            slots.len(),
            nav_items() + 1 + 1 + SIDEBAR_PAGE_ROWS + 1,
            "the pages, the header, the heading, a page of rows, and Show more"
        );
        assert_eq!(rows(&slots, 0).len(), SIDEBAR_PAGE_ROWS);
        assert!(matches!(
            slots.last().map(|slot| slot.item),
            Some(StackRow::More { group: 0, next: 10 })
        ));
        assert!(key.contains("a-4") && !key.contains("a-5"));
    }

    /// Show more adds a page; collapsed, a group is its heading alone; and the open run stays
    /// past the page.
    #[test]
    fn the_stack_follows_revealed_collapsed_and_the_open_run() {
        let theme = Theme::dark();
        let groups = vec![group("a", 17), group("b", 2)];
        let revealed = HashMap::from([("a".to_string(), 10)]);
        let (slots, _) = sidebar_slots(&groups, &[], &revealed, None, false, theme);
        assert_eq!(rows(&slots, 0).len(), 10);
        assert!(
            slots
                .iter()
                .any(|slot| matches!(slot.item, StackRow::More { group: 0, next: 15 }))
        );

        let selected = "a-12".to_string();
        let (slots, _) = sidebar_slots(
            &groups,
            &["a".into()],
            &HashMap::new(),
            Some(&selected),
            false,
            theme,
        );
        assert!(rows(&slots, 0).is_empty(), "a folded group shows no rows");

        let (slots, _) =
            sidebar_slots(&groups, &[], &HashMap::new(), Some(&selected), false, theme);
        let shown = rows(&slots, 0);
        assert_eq!(
            shown.len(),
            SIDEBAR_PAGE_ROWS + 1,
            "the open run joins the page"
        );
        assert!(
            shown
                .iter()
                .any(|slot| matches!(slot.item, StackRow::Run { row, .. } if row == 12))
        );
    }
}
