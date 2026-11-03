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
use crate::avatar::agent_avatar;
use crate::client::{CoreError, DeleteSectionRequest, MoveToSectionRequest, SaveSectionRequest};
use crate::format::agent_display_name;
use crate::models::sidebar::{
    COLLAPSED_PREFERENCE, DISPLAY_PREFERENCE, Direction, GroupBy, GroupKind, HOVER_CLOSE_MS,
    HOVER_OPEN_MS, Show, SidebarDisplay, SidebarGroup, SidebarRun, SortBy, conversation_ref,
    group_sidebar_runs, now_unix_ms, parse_sidebar_display, section_ref, sidebar_run_title,
    sidebar_runs,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route, SettingsPage, observe};
use crate::theme::Theme;
use crate::ui::parts::sidebar::{
    action_row, menu_heading, menu_separator, preview_surface, run_dot, run_none,
};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Icon, Menu, MenuEntry, RadioOption, SidebarItem,
    SidebarSectionLabel, TextInput, ToastTone, button, ellipsis, icon_button, kbd, radio_group,
};

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
    groups: Vec<SidebarGroup>,
    route: Route,
    show_error: bool,
}

const NAV: &[(&str, Icon, Route)] = &[
    ("Roles", Icon::Route, Route::Roles),
    ("Intelligence", Icon::AiBrain01Icon, Route::Intelligence),
    ("Spend", Icon::CreditCard, Route::Spend),
    ("Stats", Icon::Activity, Route::Stats),
];

pub struct SidebarView {
    state: Entity<AppState>,
    display: SidebarDisplay,
    collapsed: Vec<String>,
    /// The run rows' bounds as last laid out, which place the hover preview beside its row.
    row_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    hover_run: Option<String>,
    preview_run: Option<String>,
    preview_view: Option<Entity<TranscriptView>>,
    preview_open: bool,
    preview_card_hover: bool,
    preview_progress: SpringValue,
    hover_gen: u64,
    display_open: bool,
    display_progress: SpringValue,
    context_menu: Entity<Menu>,
    context_x: f32,
    context_y: f32,
    editor: Option<SectionDraft>,
    editor_focus: bool,
    section_name: Entity<TextInput>,
    editor_progress: SpringValue,
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
        Self {
            state,
            display,
            collapsed,
            row_bounds: Rc::new(RefCell::new(HashMap::new())),
            hover_run: None,
            preview_run: None,
            preview_view: None,
            preview_open: false,
            preview_card_hover: false,
            preview_progress: SpringValue::new(0.0, SpringTier::Fast),
            hover_gen: 0,
            display_open: false,
            display_progress: SpringValue::new(0.0, SpringTier::Fast),
            context_menu: cx.new(Menu::new),
            context_x: 0.0,
            context_y: 0.0,
            editor: None,
            editor_focus: false,
            section_name,
            editor_progress: SpringValue::new(0.0, SpringTier::Fast),
            scroll: ScrollHandle::new(),
            animator: Animator::new(),
        }
    }

    fn read_snapshot(&self, cx: &gpui::App) -> Snapshot {
        let app = self.state.read(cx);
        let runs = sidebar_runs(app.workspaces(), app.runs());
        let groups = group_sidebar_runs(&runs, app.sections(), &self.display, now_unix_ms());
        let show_error = matches!(app.connection(), ConnectionState::Offline { .. })
            || app.last_error().is_some();
        Snapshot {
            groups,
            route: app.route().clone(),
            show_error,
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
        self.display_progress
            .set_target(if self.display_open { 1.0 } else { 0.0 });
        self.editor_progress
            .set_target(if self.editor.is_some() { 1.0 } else { 0.0 });
        self.preview_progress
            .set_target(if self.preview_open { 1.0 } else { 0.0 });
    }

    fn advance(&mut self, window: &mut Window) {
        let dt = self.animator.dt(window, self.moving());
        if dt > 0.0 {
            self.preview_progress.step(dt);
            self.display_progress.step(dt);
            self.editor_progress.step(dt);
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
            || !self.display_progress.settled()
            || !self.editor_progress.settled()
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

    fn persist_collapsed(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(COLLAPSED_PREFERENCE, &self.collapsed);
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

    fn toggle_display(&mut self, cx: &mut Context<Self>) {
        self.display_open = !self.display_open;
        if self.display_open {
            self.editor = None;
            self.editor_focus = false;
            self.close_menu(cx);
        }
        cx.notify();
    }

    fn reset_display(&mut self, cx: &mut Context<Self>) {
        self.display = SidebarDisplay::default();
        self.persist_display(cx);
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
            1 => GroupBy::Workspace,
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
        let theme = *Theme::global(cx);
        let viewport = window.viewport_size();
        let width = theme.sidebar_width * 0.8;
        let max_x = (px_of(viewport.width) - width).max(0.0);
        let max_y = (px_of(viewport.height) - theme.control_height_md).max(0.0);
        self.context_x = x.clamp(0.0, max_x);
        self.context_y = (y - theme.control_height_md).clamp(0.0, max_y);
        self.display_open = false;
        self.context_menu.update(cx, |menu, cx| {
            menu.set_items(items);
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

    fn move_entries(&self, kind: &str, item_ref: &str, cx: &mut Context<Self>) -> Vec<MenuEntry> {
        let view = cx.entity();
        let mut items = Vec::new();
        for (id, name) in self.section_names(cx) {
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

    fn open_run_menu(
        &mut self,
        run_id: &str,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.move_entries("run", run_id, cx);
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
            self.section_entries(id, group.title.clone(), cx)
        } else if let Some(conversation) = conversation_ref(&group.id) {
            self.move_entries("conversation", conversation, cx)
        } else {
            return;
        };
        self.place_menu(x, y, items, window, cx);
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

    /// The pages: `<nav class="pw-sidebar-group">`, Commander and then each page, 2px apart.
    fn nav_block(&self, theme: Theme, route: &Route, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view = cx.entity();
        let mut column = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme.sidebar_row_gap))
            .child({
                let view = view.clone();
                SidebarItem::new("sidebar-nav-Commander", "Commander")
                    .icon(Icon::Search)
                    .trailing(kbd("⌘K", &theme))
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.open_commander(cx));
                    })
            });
        for (label, icon_name, target) in NAV.iter() {
            let view = view.clone();
            let target = target.clone();
            column = column.child(
                SidebarItem::new(SharedString::from(format!("sidebar-nav-{label}")), *label)
                    .icon(*icon_name)
                    .active(page_selected(route, label))
                    .on_click(move |_, cx| {
                        let _ = view.update(cx, |this, cx| this.go(target.clone(), false, cx));
                    }),
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
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = self.selected_id(&snapshot.route);
        let mut column = div()
            .id("sidebar-scroll")
            .mt(px(theme.space_2))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .gap(px(theme.space_2))
            .child(self.runs_header(theme, cx));
        if snapshot.show_error {
            column = column.child(run_none("Could not refresh runs and sections.", &theme));
        }
        if snapshot.groups.is_empty() {
            column = column.child(run_none("No runs to show.", &theme));
        }
        for group in &snapshot.groups {
            column = column.child(self.group_block(group, selected.as_deref(), theme, cx));
        }
        column.into_any_element()
    }

    /// `.dispatch-runs-header`: "Runs" in 12px/500 muted with 8px before it, and the Display
    /// menu's ghost icon button at the end.
    fn runs_header(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let view = cx.entity();
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
                    .child(icon_button(
                        "sidebar-display",
                        Icon::ListFilter,
                        "Display",
                        Rc::new(move |_, cx| {
                            let _ = view.update(cx, |this, cx| this.toggle_display(cx));
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
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let collapsed = self.is_collapsed(&group.id);
        let conversation =
            group.kind == GroupKind::Conversation && conversation_ref(&group.id).is_some();
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
            .padding(
                if conversation { 36.0 } else { theme.space_2 },
                if custom { 32.0 } else { theme.space_2 },
            )
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

        let mut section = div()
            .relative()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(theme.sidebar_row_gap))
            .child(label);
        if conversation {
            section = section.child(
                div()
                    .absolute()
                    .top(px(theme.space_1))
                    .left(px(theme.space_2))
                    .child(agent_avatar(group.agent.as_deref().unwrap_or(""), "", 20.0)),
            );
        }
        if custom {
            let section_id = group.id.clone();
            let section_name = group.title.clone();
            let view = view.clone();
            section = section.child(
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
        if !collapsed {
            for run in &group.runs {
                section =
                    section.child(self.run_row(run, selected == Some(run.id.as_str()), theme, cx));
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
            .leading(agent_avatar(&run.agent, &run.status, 40.0), 36.0)
            .content(content)
            .trailing(run_dot(&run.status, &theme))
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
        div()
            .relative()
            .w_full()
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

    fn overlay_top(&self, theme: Theme) -> f32 {
        let bounds = self.scroll.bounds();
        if px_of(bounds.size.height) > 1.0 {
            px_of(bounds.origin.y)
                + px_of(self.scroll.offset().y)
                + theme.space_2
                + theme.control_height_md
        } else {
            theme.titlebar_height
                + theme.space_3
                + nav_height(theme)
                + theme.space_2 * 3.0
                + theme.control_height_md
        }
    }

    fn display_panel(
        &self,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let t = self.display_progress.value().clamp(0.0, 1.0);
        if !self.display_open && t < 0.01 {
            return None;
        }
        let view = cx.entity();
        let top = self.overlay_top(theme) + (1.0 - t) * theme.space_2;
        let viewport_h = px_of(window.viewport_size().height);
        let max_h = (viewport_h - top - theme.space_3)
            .max(theme.row_height * 3.0)
            .min(theme.row_height * 8.0);
        let width = theme.sidebar_width - theme.space_4;
        let group_labels = [
            "Conversation",
            "Workspace",
            "Date",
            "Status",
            "Agent",
            "None",
        ];
        let group_index = match self.display.group_by {
            GroupBy::Conversation => 0,
            GroupBy::Workspace => 1,
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
        Some(
            deferred(
                div()
                    .absolute()
                    .left(px(theme.space_2))
                    .top(px(top))
                    .w(px(width))
                    .occlude()
                    .opacity(t)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .rounded(px(theme.radius_lg))
                            .bg(theme.color_surface_raised)
                            .border_1()
                            .border_color(theme.color_border)
                            .child(
                                div()
                                    .id("sidebar-display-scroll")
                                    .max_h(px(max_h))
                                    .overflow_y_scroll()
                                    .p(px(theme.space_2))
                                    .flex()
                                    .flex_col()
                                    .child(menu_heading("Group by", &theme))
                                    .child(radio_group(
                                        "sidebar-group-by",
                                        &labels(&group_labels),
                                        group_index,
                                        Rc::new({
                                            let view = view.clone();
                                            move |index, _, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.pick_group(index, cx)
                                                });
                                            }
                                        }),
                                        cx,
                                    ))
                                    .child(menu_heading("Sort by", &theme))
                                    .child(radio_group(
                                        "sidebar-sort-by",
                                        &labels(&["Last updated", "Date created", "Name"]),
                                        sort_index,
                                        Rc::new({
                                            let view = view.clone();
                                            move |index, _, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.pick_sort(index, cx)
                                                });
                                            }
                                        }),
                                        cx,
                                    ))
                                    .child(menu_separator(&theme))
                                    .child(radio_group(
                                        "sidebar-direction",
                                        &labels(&["Descending", "Ascending"]),
                                        direction_index,
                                        Rc::new({
                                            let view = view.clone();
                                            move |index, _, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.pick_direction(index, cx)
                                                });
                                            }
                                        }),
                                        cx,
                                    ))
                                    .child(menu_heading("Show", &theme))
                                    .child(radio_group(
                                        "sidebar-show",
                                        &labels(&[
                                            "All runs",
                                            "Hide merged",
                                            "Running and needs merge",
                                        ]),
                                        show_index,
                                        Rc::new({
                                            let view = view.clone();
                                            move |index, _, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.pick_show(index, cx)
                                                });
                                            }
                                        }),
                                        cx,
                                    ))
                                    .child(menu_separator(&theme))
                                    .child(action_row(
                                        "sidebar-new-section",
                                        Icon::Plus,
                                        "New section",
                                        false,
                                        Rc::new({
                                            let view = view.clone();
                                            move |window, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.open_editor(
                                                        None,
                                                        String::new(),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }),
                                        &theme,
                                        cx,
                                    ))
                                    .child(action_row(
                                        "sidebar-reset-display",
                                        Icon::Close,
                                        "Reset to defaults",
                                        false,
                                        Rc::new({
                                            let view = view.clone();
                                            move |_, cx| {
                                                let _ = view
                                                    .update(cx, |this, cx| this.reset_display(cx));
                                            }
                                        }),
                                        &theme,
                                        cx,
                                    ))
                                    .child(action_row(
                                        "sidebar-view-all",
                                        Icon::More,
                                        "View all",
                                        false,
                                        Rc::new(move |_, cx| {
                                            let _ = view.update(cx, |this, cx| this.view_all(cx));
                                        }),
                                        &theme,
                                        cx,
                                    )),
                            ),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
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
        self.advance(window);
        if self.editor_focus {
            self.section_name.read(cx).focus_handle(cx).focus(window);
            self.editor_focus = false;
        }
        let nav = self.nav_block(theme, &snapshot.route, cx);
        let runs = self.runs_block(theme, &snapshot, cx);
        let footer = self.footer_block(theme, &snapshot.route, cx);
        let display = self.display_panel(theme, window, cx);
        let editor = self.editor_panel(theme, window, cx);
        let preview = self.preview_card(theme, &snapshot, window, cx);
        let menu = self.menu_anchor(theme);

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
            .pt(px(theme.titlebar_height + theme.space_3))
            .px(px(theme.space_2))
            .pb(px(theme.space_2))
            .flex()
            .flex_col()
            .gap(px(theme.space_2))
            .font_family(theme.font_sans)
            .text_color(theme.color_text_primary)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss_panels(cx)),
            )
            .child(nav)
            .child(runs)
            .child(footer)
            .child(menu);
        if let Some(display) = display {
            root = root.child(display);
        }
        if let Some(editor) = editor {
            root = root.child(editor);
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
        _ => false,
    }
}

fn nav_height(theme: Theme) -> f32 {
    5.0 * theme.sidebar_row_height + 4.0 * theme.sidebar_row_gap
}

fn labels(items: &[&str]) -> Vec<RadioOption> {
    items
        .iter()
        .map(|label| RadioOption {
            label: SharedString::from(label.to_string()),
        })
        .collect()
}

fn core_message(error: CoreError) -> String {
    match error {
        CoreError::NotRunning => "Dispatch core is not running".to_string(),
        CoreError::Rpc(message) => message,
    }
}

fn px_of(value: impl Into<f32>) -> f32 {
    value.into()
}
