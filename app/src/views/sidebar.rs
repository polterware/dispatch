//! Sidebar: Commander, pages, grouped runs, display options, and the transcript card.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::window_drag::WindowDragRegion;
use gpui::{
    Bounds, Context, Entity, Focusable, IntoElement, MouseButton, Pixels, Render, ScrollHandle,
    SharedString, Window, canvas, deferred, div, prelude::*, px,
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
    page_sidebar_rows, parse_sidebar_display, repository_ref, run_mark, runs_notice, section_ref,
    sidebar_run_title, sidebar_runs,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route, SettingsPage, observe};
use crate::theme::Theme;
use crate::ui::list_skeleton;
use crate::ui::parts::sidebar::{preview_surface, run_dot, run_none};
use crate::ui::patterns::{
    FadeEdges, composer_card, composer_footer, fade_item, fade_scroll, modal_layer,
};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Icon, MascotMood, Menu, MenuEntry, MenuItem, SidebarItem,
    SidebarSectionLabel, TextInput, ToastTone, button, ellipsis, icon_button, kbd, list_empty,
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
    groups: Vec<SidebarGroup>,
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
    scroll: ScrollHandle,
    animator: Animator,
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
            scroll: ScrollHandle::new(),
            animator: Animator::new(),
        }
    }

    fn read_snapshot(&self, cx: &gpui::App) -> Snapshot {
        let app = self.state.read(cx);
        let runs = sidebar_runs(app.workspaces(), app.runs());
        let mut groups = group_sidebar_runs(&runs, app.sections(), &self.display, now_unix_ms());
        // A conversation someone named shows that name over its first message.
        for group in &mut groups {
            if let Some(name) = conversation_ref(&group.id).and_then(|id| app.conversation_name(id))
            {
                group.title = name.to_string();
            }
        }
        let show_error = matches!(app.connection(), ConnectionState::Offline { .. })
            || app.last_error().is_some();
        Snapshot {
            groups,
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

    fn is_collapsed(&self, id: &str) -> bool {
        self.collapsed.iter().any(|item| item == id)
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

    /// The rows a group shows: its first page, or as many as its Show more revealed.
    fn revealed_limit(&self, id: &str) -> usize {
        self.revealed
            .get(id)
            .copied()
            .unwrap_or(SIDEBAR_PAGE_ROWS)
            .max(1)
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
    /// submenus of radio items, then Reset to defaults and View all. It hangs
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
        let all = view.clone();
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
            MenuItem::action(
                "View all",
                Some(Icon::ListOrdered),
                Rc::new(move |_, cx| {
                    let _ = all.update(cx, |this, cx| this.view_all(cx));
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

    /// Collapsing a group hides its runs at once, as `<Show>` unmounts them.
    fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(pos) = self.collapsed.iter().position(|item| item == &id) {
            self.collapsed.remove(pos);
        } else {
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

    fn open_run_menu(
        &mut self,
        run_id: &str,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items = self.move_entries("run", run_id, cx);
        items.push(self.delete_entry(
            "Delete run",
            DeleteRunsRequest {
                ids: vec![run_id.to_string()],
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

    /// The pages: `<nav class="pw-sidebar-group">`, New (+), Commander (their ⌘N and ⌘K show
    /// under the pointer), and then each
    /// page, 2px apart.
    fn nav_block(
        &self,
        theme: Theme,
        route: &Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view = cx.entity();
        let mut column = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme.sidebar_row_gap))
            .child({
                let view = view.clone();
                let item = SidebarItem::new("sidebar-nav-New", "New")
                    .icon(Icon::Plus)
                    .trailing(kbd("⌘N", &theme))
                    .trailing_on_hover()
                    .active(matches!(route, Route::NewRun))
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.go(Route::NewRun, false, cx));
                    });
                fade_item(
                    "sidebar-nav-New",
                    &self.scroll,
                    fade_edges(theme),
                    item,
                    window,
                    cx,
                )
            })
            .child({
                let view = view.clone();
                let item = SidebarItem::new("sidebar-nav-Commander", "Commander")
                    .icon(Icon::Search)
                    .trailing(kbd("⌘K", &theme))
                    .trailing_on_hover()
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.open_commander(cx));
                    });
                fade_item(
                    "sidebar-nav-Commander",
                    &self.scroll,
                    fade_edges(theme),
                    item,
                    window,
                    cx,
                )
            });
        for (label, icon_name, target) in NAV.iter() {
            let view = view.clone();
            let target = target.clone();
            let selector = format!("sidebar-nav-{label}");
            let id = SharedString::from(selector.clone());
            let item = SidebarItem::new(id.clone(), *label)
                .icon(*icon_name)
                .active(page_selected(route, label))
                .on_click(move |_, cx| {
                    let _ = view.update(cx, |this, cx| this.go(target.clone(), false, cx));
                });
            column = column.child(
                div()
                    .w_full()
                    .debug_selector(move || selector.clone())
                    .child(fade_item(
                        id,
                        &self.scroll,
                        fade_edges(theme),
                        item,
                        window,
                        cx,
                    )),
            );
        }
        column.into_any_element()
    }

    /// `.dispatch-sidebar-runs`: 8px below the pages, the header and then the groups 8px
    /// apart, scrolling between the pages and the footer.
    fn runs_block(
        &self,
        theme: Theme,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = self.selected_id(&snapshot.route);
        let mut column = div()
            .mt(px(theme.space_2))
            .flex()
            .flex_col()
            .gap(px(theme.space_2))
            .child({
                let header = self.runs_header(theme, cx);
                fade_item(
                    "sidebar-runs-header",
                    &self.scroll,
                    fade_edges(theme),
                    header,
                    window,
                    cx,
                )
            });
        let notice = runs_notice(
            !snapshot.groups.is_empty(),
            snapshot.connecting,
            snapshot.show_error,
        );
        column = match notice {
            RunsNotice::None => column,
            // The core is starting: the runs, in bones, until it answers.
            RunsNotice::Loading => column.child(
                div()
                    .px(px(theme.space_2))
                    .child(list_skeleton(6, 32.0, window, cx)),
            ),
            RunsNotice::Empty => {
                column.child(list_empty("No runs to show.", MascotMood::Sleeping, cx))
            }
            RunsNotice::Unreachable => {
                column.child(list_empty(REFRESH_FAILED, MascotMood::Sad, cx))
            }
            RunsNotice::Stale => column.child(run_none(REFRESH_FAILED, &theme)),
        };
        for group in &snapshot.groups {
            column = column.child(self.group_block(group, selected.as_deref(), theme, window, cx));
        }
        column.into_any_element()
    }

    /// `.dispatch-runs-header`: "Runs" in 12px/500 muted with 8px before it, then the Display
    /// menu's ghost icon button and Add repository's `+` at the end. A repository added there
    /// opens New on it.
    fn runs_header(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view = cx.entity();
        let state = self.state.clone();
        div()
            .flex_none()
            .pl(px(theme.space_2))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(theme.font_size_sm))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.color_text_muted)
            .child("Runs")
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

    /// `.dispatch-run-section`: the group's toggle label (after a 20px bot for a conversation,
    /// before the "…" of a custom section) and its runs, 2px apart, while it is open.
    fn group_block(
        &self,
        group: &SidebarGroup,
        selected: Option<&str>,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let collapsed = self.is_collapsed(&group.id);
        let custom = group.kind == GroupKind::Section;
        let view = cx.entity();
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
                                if let Some(raw) = section_ref(&id) {
                                    let items = this.section_entries(raw.to_string(), name, cx);
                                    this.place_menu(x, y, items, window, cx);
                                }
                            });
                        }),
                    ),
            );
        }
        let mut section = div()
            .relative()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme.sidebar_row_gap))
            .child(fade_item(
                format!("sidebar-header-{}", group.id),
                &self.scroll,
                fade_edges(theme),
                heading,
                window,
                cx,
            ));
        if !collapsed {
            // Five rows, strictly: only the open run stays past the page, in the Display order.
            // A running row or one with an unseen update keeps its dot and bot state but does
            // not bypass the page.
            let page = page_sidebar_rows(&group.runs, self.revealed_limit(&group.id), |run| {
                selected == Some(run.id.as_str())
            });
            for run in page.rows.iter().copied() {
                let row = self.run_row(run, selected == Some(run.id.as_str()), theme, cx);
                section = section.child(fade_item(
                    format!("sidebar-run-{}", run.id),
                    &self.scroll,
                    fade_edges(theme),
                    row,
                    window,
                    cx,
                ));
            }
            if page.hidden > 0 {
                let more_id = format!("sidebar-show-more-{}", group.id);
                let reveal_id = group.id.clone();
                let next = page.next_limit;
                let reveal_view = view.clone();
                let more = SidebarItem::new(
                    SharedString::from(more_id.clone()),
                    format!(
                        "Show {} more · {} hidden",
                        page.hidden.min(SIDEBAR_PAGE_ROWS),
                        page.hidden
                    ),
                )
                .on_click(move |_, cx| {
                    let id = reveal_id.clone();
                    let _ = reveal_view.update(cx, |this, cx| this.reveal_more(id, next, cx));
                });
                let debug_id = more_id.clone();
                section = section.child(fade_item(
                    more_id,
                    &self.scroll,
                    fade_edges(theme),
                    div()
                        .w_full()
                        .debug_selector(move || debug_id.clone())
                        .child(more),
                    window,
                    cx,
                ));
            }
        }
        section.into_any_element()
    }

    /// A run row (`SidebarItem.dispatch-run-item`): the 40px bot in a 36px box, the agent in
    /// 14px/500 primary with the model in 11px muted 6px after it, over the prompt's first
    /// line in 12px muted, and the 6px status dot. A long hover opens its transcript.
    fn run_row(
        &self,
        run: &SidebarRun,
        active: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let view = cx.entity();
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
            .when(
                self.seen
                    .as_ref()
                    .is_some_and(|seen| has_unseen_update(run, seen)),
                |item| item.trailing(run_dot(&run.status, &theme)),
            )
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
                move |event, window, cx| {
                    let id = run_id.clone();
                    let (x, y) = (px_of(event.position.x), px_of(event.position.y));
                    let _ = view.update(cx, |this, cx| this.open_run_menu(&id, x, y, window, cx));
                }
            })
            .on_click(move |_, cx| {
                let id = run_id.clone();
                let _ = view.update(cx, |this, cx| this.go(Route::Run(id), true, cx));
            });
        let bounds = self.row_bounds.clone();
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
        self.scroll.set_offset(gpui::point(px(0.0), px(-y)));
        cx.notify();
    }

    fn overlay_top(&self, theme: Theme) -> f32 {
        let bounds = self.scroll.bounds();
        if px_of(bounds.size.height) > 1.0 {
            // The runs header sits under the pages, inside the same scroll.
            px_of(bounds.origin.y)
                + px_of(self.scroll.offset().y)
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
        let nav = self.nav_block(theme, &snapshot.route, window, cx);
        let runs = self.runs_block(theme, &snapshot, window, cx);
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
            // The pages and the runs scroll as one list that runs up under the titlebar and
            // down under Settings, which stays put; the rows fade out before either covers
            // them.
            .child(
                fade_scroll("sidebar-scroll", &self.scroll, theme.color_shell_glass)
                    // Over the glass and the window material: the rows fade themselves.
                    .bands(false)
                    .column(|column| {
                        column
                            .flex()
                            .flex_col()
                            .gap(px(theme.space_2))
                            .pt(px(theme.titlebar_height + theme.space_3))
                            .pb(px(footer_height(theme) + theme.space_2))
                    })
                    .child(nav)
                    .child(runs),
            )
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

fn page_selected(route: &Route, label: &str) -> bool {
    match label {
        "Roles" => matches!(route, Route::Roles | Route::RoleBoard(_)),
        "Intelligence" => matches!(route, Route::Intelligence),
        "Spend" => matches!(route, Route::Spend),
        "Stats" => matches!(route, Route::Stats),
        "Images" => matches!(route, Route::Images),
        _ => false,
    }
}

/// Settings at the foot, over the list: its 8px above, its row, and the sidebar's 8px under.
fn footer_height(theme: Theme) -> f32 {
    theme.space_2 * 2.0 + theme.sidebar_row_height
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
