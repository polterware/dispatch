//! Runs, the sidebar's View all ([`Route::Runs`]): every workspace's runs as a grouped stack
//! of full-width rows (grouped by Status by default, with running work on top), searched from
//! the toolbar, filtered by Repository, Status, Agent, and Tier through the shared
//! `FilterPopover` (`ActiveFilterBar` under the toolbar), whose Display section groups,
//! sorts, and narrows what shows: one toolbar button for all of it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ListAlignment, ListOffset,
    ListState, Pixels, Render, SharedString, Window, div, prelude::*, px,
};

use crate::client::{CancelRunRequest, CoreError, each_at_once};
use crate::models::filter::Filters;
use crate::models::run_preview::{PreviewCache, fetch_preview};
use crate::models::runs::{
    COLLAPSED_PREFERENCE, DISPLAY_PREFERENCE, REVEALED_PREFERENCE, RunGroup, RunRow, all_rows,
    filter_specs, group_run_rows, is_run_active, page_run_rows, parse_runs_display,
    runs_default_display, visible_rows,
};
use crate::models::sidebar::{
    Direction, GroupBy, SIDEBAR_PAGE_ROWS, Show, SidebarDisplay, SortBy, conversation_ref,
};
use crate::state::{AppState, ConnectionState, POLL_INTERVAL, Route};
use crate::theme::Theme;
use crate::ui::parts::runs::{
    RunAction, RunCardState, card_row_count, group_header, run_stack_row,
};
use crate::ui::{
    FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, STACK_OVERDRAW, SidebarItem,
    TextInput, ToastTone, page_empty, page_under, page_under_inset, sync_virtual_list, toolbar,
    virtual_stack,
};
use polterware_ui::filter::FilterOption;
use polterware_ui::{DisplayPick, DisplayRow};

/// The page's chrome state.
const PAGE: &str = "runs-page";
/// `formatAgo` labels move on the page's 15s clock.
const CLOCK: Duration = Duration::from_secs(15);
/// The Display section's choices: (value, label), in menu order.
const GROUPINGS: [(&str, &str); 6] = [
    ("conversation", "Conversation"),
    ("repository", "Repository"),
    ("date", "Date"),
    ("status", "Status"),
    ("agent", "Agent"),
    ("none", "None"),
];
const ORDERINGS: [(&str, &str); 3] = [
    ("updated", "Last updated"),
    ("created", "Date created"),
    ("name", "Name"),
];
const SHOWS: [(&str, &str); 3] = [
    ("all", "All runs"),
    ("unmerged", "Hide merged"),
    ("active", "Running and needs merge"),
];
/// The gap inside a group, between its header and its cards and between the cards.
const ROW_GAP: f32 = 10.0;

pub struct RunsView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    display: SidebarDisplay,
    collapsed: Vec<String>,
    /// How many rows each group shows, by group id, once a Show more revealed past the first
    /// page; absent groups show [`SIDEBAR_PAGE_ROWS`].
    revealed: HashMap<String, usize>,
    query: String,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    cancelling: Option<String>,
    now_ms: i64,
    /// Every workspace's rows, built once per runs revision: a scroll or a hover redraws the
    /// page every frame, and rebuilding every run's row each time made it stutter.
    rows: Rc<Vec<RunRow>>,
    rows_key: Option<u64>,
    /// The rows the filters and the search keep, for the inputs that kept them.
    visible: Rc<Vec<RunRow>>,
    visible_key: Option<VisibleKey>,
    /// The grouped rows shown on the page, for the visible rows, the display, and the clock.
    groups: Rc<Vec<RunGroup>>,
    group_key: Option<(VisibleKey, SidebarDisplay, i64)>,
    /// One virtual item per header and card. Only a new structure replaces them.
    list: ListState,
    slots: Rc<Vec<StackSlot>>,
    slots_key: String,
    /// The collapsed ids the slots were built from.
    slot_collapsed: Vec<String>,
    /// The revealed limits the slots were built from.
    slot_revealed: HashMap<String, usize>,
    /// Tail previews cached per run id; only cards drawn by the virtual list fetch.
    previews: Rc<RefCell<PreviewCache>>,
    /// Run ids and active flags rendered on the latest stack pass.
    rendered: Rc<RefCell<Vec<(String, bool)>>>,
    /// Ensures at most one deferred preview fetch is queued per draw.
    fetch_queued: Rc<Cell<bool>>,
}

/// One item in the runs stack. `after` is the gap under it; the list's own padding is separate.
enum StackSlot {
    Header {
        group: usize,
        collapsed: bool,
        after: f32,
        key: String,
    },
    Row {
        group: usize,
        row: usize,
        index: usize,
        after: f32,
        key: String,
    },
    /// The Show more row under a group's shown runs; it asks for `next` rows.
    More {
        group: usize,
        next: usize,
        after: f32,
        key: String,
    },
    /// The Show less row, back to the group's first page.
    Less {
        group: usize,
        after: f32,
        key: String,
    },
}

/// Copies of a slot the list closure can hold without borrowing the slot list.
enum StackItem {
    Header { group: usize, collapsed: bool },
    Row(usize, usize, usize),
    More(usize, usize),
    Less(usize),
}

impl StackSlot {
    fn key(&self) -> &str {
        match self {
            Self::Header { key, .. }
            | Self::Row { key, .. }
            | Self::More { key, .. }
            | Self::Less { key, .. } => key,
        }
    }

    fn after(&self) -> f32 {
        match self {
            Self::Header { after, .. }
            | Self::Row { after, .. }
            | Self::More { after, .. }
            | Self::Less { after, .. } => *after,
        }
    }
}

/// What the visible rows depend on: the rows, the filters, and the search.
type VisibleKey = (u64, Filters, String);

impl RunsView {
    /// The screenshot tour: scrolls the stack to item `index`.
    pub(crate) fn tour_scroll_to(&mut self, index: usize, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: px(0.0),
        });
        cx.notify();
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let display = match state
            .read(cx)
            .read_preference::<serde_json::Value>(DISPLAY_PREFERENCE)
        {
            Ok(Some(value)) => parse_runs_display(&value.to_string()),
            _ => runs_default_display(),
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
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search runs…").pill());
        cx.observe(&search, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.query {
                this.query = text;
                cx.notify();
            }
        })
        .detach();
        let filters = cx.new(|cx| FilterPopover::new("run-filters", "Run filters", true, cx));
        cx.subscribe(&filters, |_, _, _: &FiltersChanged, cx| cx.notify())
            .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        filters.update(cx, |popover, cx| {
            popover.set_display_rows(display_rows(&display), cx)
        });
        cx.subscribe(&filters, |this, _, pick: &DisplayPick, cx| {
            this.pick_display(pick, cx);
        })
        .detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                let alive = this
                    .update(cx, |view, cx| {
                        view.now_ms = unix_ms();
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let alive = this
                    .update(cx, |view, cx| {
                        if matches!(view.state.read(cx).route(), Route::Runs) {
                            view.fetch_rendered_previews(cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self {
            state,
            search,
            filters,
            display,
            collapsed,
            slot_revealed: revealed.clone(),
            revealed,
            query: String::new(),
            fields_key: String::new(),
            cancelling: None,
            now_ms: unix_ms(),
            rows: Rc::default(),
            rows_key: None,
            visible: Rc::default(),
            visible_key: None,
            groups: Rc::default(),
            group_key: None,
            list: ListState::new(0, ListAlignment::Top, px(STACK_OVERDRAW)),
            slots: Rc::default(),
            slots_key: String::new(),
            slot_collapsed: Vec::new(),
            previews: Rc::new(RefCell::new(PreviewCache::default())),
            rendered: Rc::new(RefCell::new(Vec::new())),
            fetch_queued: Rc::new(Cell::new(false)),
        }
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    /// Every workspace's rows and the ones on show, rebuilt only when what they come from
    /// moved, and the filter fields with them. A workspace's name or path only changes with a
    /// refresh, which moves the runs revision too.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let revision = self.state.read(cx).runs_revision();
        if self.rows_key != Some(revision) {
            let rows = {
                let state = self.state.read(cx);
                all_rows(state.workspaces(), state.runs())
            };
            if matches!(self.state.read(cx).connection(), ConnectionState::Online) {
                let ids: HashSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
                self.previews.borrow_mut().retain(&ids);
            }
            self.sync_fields(&rows, cx);
            self.rows = Rc::new(rows);
            self.rows_key = Some(revision);
        }
        let filters = self.filters.read(cx).filters();
        let visible_fresh = self
            .visible_key
            .as_ref()
            .is_some_and(|(rev, cached, query)| {
                *rev == revision && cached == filters && query == &self.query
            });
        if !visible_fresh {
            let filters = filters.clone();
            self.visible = Rc::new(visible_rows(&self.rows, &filters, &self.query, None));
            self.visible_key = Some((revision, filters, self.query.clone()));
        }
        let groups_stale = match &self.group_key {
            Some((key, display, now)) => {
                self.visible_key.as_ref() != Some(key)
                    || *display != self.display
                    || *now != self.now_ms
            }
            None => true,
        };
        if groups_stale {
            let mut groups = group_run_rows(&self.visible, &self.display, self.now_ms);
            let state = self.state.read(cx);
            for group in &mut groups {
                if let Some(name) =
                    conversation_ref(&group.id).and_then(|id| state.conversation_name(id))
                {
                    group.title = name.to_string();
                }
            }
            self.groups = Rc::new(groups);
            self.group_key = self
                .visible_key
                .clone()
                .map(|key| (key, self.display, self.now_ms));
        }
        let collapsed_changed = self.slot_collapsed != self.collapsed;
        let revealed_changed = self.slot_revealed != self.revealed;
        if groups_stale || collapsed_changed || revealed_changed {
            let gap = Theme::global(cx).space_6;
            let (slots, structure) =
                stack_slots(&self.groups, &self.collapsed, &self.revealed, gap);
            if structure != self.slots_key {
                let anchor = self.scroll_anchor();
                self.slots = slots;
                self.slots_key = structure;
                sync_virtual_list(&self.list, self.slots.len(), true);
                self.restore_scroll(anchor);
            }
            if collapsed_changed {
                self.slot_collapsed = self.collapsed.clone();
            }
            if revealed_changed {
                self.slot_revealed = self.revealed.clone();
            }
        }
    }

    /// The item under the scroll, so a collapse above it does not jump to another card.
    fn scroll_anchor(&self) -> Option<(String, Pixels)> {
        if self.slots.is_empty() {
            return None;
        }
        let top = self.list.logical_scroll_top();
        let index = top.item_ix.min(self.slots.len() - 1);
        Some((self.slots[index].key().to_string(), top.offset_in_item))
    }

    fn restore_scroll(&self, anchor: Option<(String, Pixels)>) {
        let Some((key, offset)) = anchor else {
            return;
        };
        let Some(item_ix) = self.slots.iter().position(|slot| slot.key() == key) else {
            return;
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: offset,
        });
    }

    /// The fields follow the runs' distinct values, rebuilt only when those change.
    fn sync_fields(&mut self, rows: &[RunRow], cx: &mut Context<Self>) {
        let [repository, status, agent, tier] = filter_specs(rows);
        let key = [&repository, &status, &agent, &tier]
            .iter()
            .map(|spec| {
                spec.options
                    .iter()
                    .map(|option| format!("{}\u{1d}{}", option.id, option.label))
                    .collect::<Vec<_>>()
                    .join("\u{1f}")
            })
            .collect::<Vec<_>>()
            .join("\u{1e}");
        if key == self.fields_key {
            return;
        }
        self.fields_key = key;
        self.filters.update(cx, |popover, _| {
            popover.set_fields(vec![
                FilterField::new(repository, Some(Icon::Folder)),
                FilterField::new(status, None),
                FilterField::new(agent, Some(Icon::Bot)),
                FilterField::new(tier, Some(Icon::Gauge)),
            ]);
        });
    }

    fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(pos) = self.collapsed.iter().position(|item| item == &id) {
            self.collapsed.remove(pos);
        } else {
            // Collapsing forgets the pages a Show more opened: opened again, the group
            // shows its first rows only.
            if self.revealed.remove(&id).is_some() {
                self.persist_revealed(cx);
            }
            self.collapsed.push(id);
        }
        self.persist_collapsed(cx);
        cx.notify();
    }

    /// A Show more adds a page of rows to its group.
    fn reveal_more(&mut self, id: String, limit: usize, cx: &mut Context<Self>) {
        self.revealed.insert(id, limit);
        self.persist_revealed(cx);
        cx.notify();
    }

    /// A Show less folds its group back to the first page.
    fn show_less(&mut self, id: String, cx: &mut Context<Self>) {
        if self.revealed.remove(&id).is_some() {
            self.persist_revealed(cx);
        }
        cx.notify();
    }

    fn persist_revealed(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(REVEALED_PREFERENCE, &self.revealed);
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

    /// Display picks change only the page's presentation, never its filter clauses.
    fn pick_display(&mut self, pick: &DisplayPick, cx: &mut Context<Self>) {
        match pick {
            DisplayPick::Reset => self.display = runs_default_display(),
            DisplayPick::Value { row, value } => match (row.as_str(), value.as_str()) {
                ("group", "conversation") => self.display.group_by = GroupBy::Conversation,
                ("group", "repository") => self.display.group_by = GroupBy::Repository,
                ("group", "date") => self.display.group_by = GroupBy::Date,
                ("group", "status") => self.display.group_by = GroupBy::Status,
                ("group", "agent") => self.display.group_by = GroupBy::Agent,
                ("group", "none") => self.display.group_by = GroupBy::None,
                ("sort", "updated") => self.display.sort_by = SortBy::Updated,
                ("sort", "created") => self.display.sort_by = SortBy::Created,
                ("sort", "name") => self.display.sort_by = SortBy::Name,
                ("sort", "desc") => self.display.direction = Direction::Desc,
                ("sort", "asc") => self.display.direction = Direction::Asc,
                ("show", "all") => self.display.show = Show::All,
                ("show", "unmerged") => self.display.show = Show::Unmerged,
                ("show", "active") => self.display.show = Show::Active,
                _ => return,
            },
        }
        self.persist_display(cx);
        self.filters.update(cx, |popover, cx| {
            popover.set_display_rows(display_rows(&self.display), cx)
        });
        cx.notify();
    }

    fn open_run(&mut self, id: String, cx: &mut Context<Self>) {
        let state = self.state.clone();
        cx.defer(move |cx| {
            state.update(cx, |state, cx| {
                state.navigate(Route::Run(id));
                state.request_refresh(cx);
                cx.notify();
            });
        });
    }

    fn cancel_run(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.cancelling = Some(id.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let request = CancelRunRequest { id: id.clone() };
            let result = cx
                .background_spawn(async move { client.cancel_run(&request).map(|_| ()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.cancelling.as_deref() == Some(id.as_str()) {
                    view.cancelling = None;
                }
                let state = view.state.clone();
                state.update(cx, |state, cx| {
                    if let Err(error) = result {
                        let message = match error {
                            CoreError::NotRunning => {
                                "Dispatch cannot reach its local core.".to_string()
                            }
                            CoreError::Rpc(message) => message,
                        };
                        state.push_toast(message, ToastTone::Danger);
                    }
                    state.request_refresh(cx);
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetches due tail previews for the cards currently rendered by the virtual list, off the
    /// main thread, updating the view only when a preview changed.
    fn fetch_rendered_previews(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        let now = Instant::now();
        let due: Vec<(String, bool)> = {
            let state = self.state.read(cx);
            let mut cache = self.previews.borrow_mut();
            let mut due = Vec::new();
            for (id, was_active) in self.rendered.borrow().iter() {
                let active = state
                    .run(id)
                    .map_or(*was_active, |run| is_run_active(&run.status));
                if cache.due(id, active, now, POLL_INTERVAL) {
                    cache.begin(id);
                    due.push((id.clone(), active));
                }
            }
            due
        };
        if due.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move {
                    each_at_once(&due, |(id, active)| {
                        (id.clone(), *active, fetch_preview(&client, id, *active))
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let now = Instant::now();
                let mut changed = false;
                {
                    let mut cache = view.previews.borrow_mut();
                    for (id, active, read) in results {
                        changed |= cache.finish(&id, read, active, now);
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for RunsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        self.refresh_rows(cx);
        let groups = self.groups.clone();
        let connecting = matches!(
            self.state.read(cx).connection(),
            ConnectionState::Connecting
        );
        let bar = FilterPopover::bar(&self.filters, window, cx);
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, bar.is_some(), padding, window, cx);
        let scrolls = !groups.is_empty();

        let content: AnyElement = if scrolls {
            self.runs_stack(top, theme.space_4, window, cx)
        } else if connecting {
            div().into_any_element()
        } else {
            let (title, mood) = if self.rows.is_empty() {
                ("No runs yet", MascotMood::Sleeping)
            } else {
                ("No matches", MascotMood::Lost)
            };
            page_empty(title, mood, cx).into_any_element()
        };

        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(self.search.clone()))
            .child(self.filters.clone());
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            // The groups run up under the toolbar, which fades them out on its trail.
            .child(page_under(
                PAGE,
                toolbar(div(), end, cx),
                bar,
                padding,
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .when(!scrolls, |slot| slot.pt(px(top)))
                    .child(content),
                window,
                cx,
            ))
    }
}

impl RunsView {
    /// The groups as a virtual stack: a header, then its cards, only the ones in view.
    fn runs_stack(
        &self,
        top: f32,
        bottom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let on_toggle: RunAction = {
            let entity = entity.clone();
            Rc::new(move |id: &str, _, cx| {
                let id = id.to_string();
                entity.update(cx, |this, cx| this.toggle_group(id, cx));
            })
        };
        let on_open: RunAction = {
            let entity = entity.clone();
            Rc::new(move |id: &str, _, cx| {
                let id = id.to_string();
                entity.update(cx, |this, cx| this.open_run(id, cx));
            })
        };
        let on_cancel: RunAction = {
            let entity = entity.clone();
            Rc::new(move |id: &str, _, cx| {
                let id = id.to_string();
                entity.update(cx, |this, cx| this.cancel_run(id, cx));
            })
        };
        let on_more: Rc<dyn Fn(&str, usize, &mut gpui::App)> = {
            let entity = entity.clone();
            Rc::new(move |id: &str, limit, cx| {
                let id = id.to_string();
                entity.update(cx, |this, cx| this.reveal_more(id, limit, cx));
            })
        };
        let on_less: RunAction = {
            let entity = entity.clone();
            Rc::new(move |id: &str, _, cx| {
                let id = id.to_string();
                entity.update(cx, |this, cx| this.show_less(id, cx));
            })
        };
        self.rendered.borrow_mut().clear();
        let slots = self.slots.clone();
        let groups = self.groups.clone();
        let now_ms = self.now_ms;
        let cancelling = self.cancelling.clone();
        let previews = self.previews.clone();
        let rendered = self.rendered.clone();
        let fetch_queued = self.fetch_queued.clone();
        let fetch_entity = entity.clone();
        virtual_stack(
            "runs-scroll",
            &self.list,
            top,
            bottom,
            move |index, _, cx| {
                let Some((after, item)) = slots.get(index).map(|slot| {
                    let after = slot.after();
                    let item = match slot {
                        StackSlot::Header {
                            group, collapsed, ..
                        } => StackItem::Header {
                            group: *group,
                            collapsed: *collapsed,
                        },
                        StackSlot::Row {
                            group, row, index, ..
                        } => StackItem::Row(*group, *row, *index),
                        StackSlot::More { group, next, .. } => StackItem::More(*group, *next),
                        StackSlot::Less { group, .. } => StackItem::Less(*group),
                    };
                    (after, item)
                }) else {
                    return div().into_any_element();
                };
                let body = match item {
                    StackItem::Header {
                        group: group_index,
                        collapsed,
                    } => {
                        let Some(group) = groups.get(group_index) else {
                            return div().into_any_element();
                        };
                        group_header(group, collapsed, on_toggle.clone(), cx)
                    }
                    StackItem::More(group, next) => {
                        let Some(group) = groups.get(group) else {
                            return div().into_any_element();
                        };
                        let id = group.id.clone();
                        let on_more = on_more.clone();
                        page_control("show-more", "Show more", &group.id, move |_, cx| {
                            on_more(&id, next, cx)
                        })
                    }
                    StackItem::Less(group) => {
                        let Some(group) = groups.get(group) else {
                            return div().into_any_element();
                        };
                        let id = group.id.clone();
                        let on_less = on_less.clone();
                        page_control("show-less", "Show less", &group.id, move |window, cx| {
                            on_less(&id, window, cx)
                        })
                    }
                    StackItem::Row(group, row, row_index) => {
                        let Some(row) = groups.get(group).and_then(|group| group.runs.get(row))
                        else {
                            return div().into_any_element();
                        };
                        record_rendered(&rendered, &row.id, row.active);
                        let preview = previews.borrow().get(&row.id);
                        let now = Instant::now();
                        if previews
                            .borrow()
                            .due(&row.id, row.active, now, POLL_INTERVAL)
                            && !fetch_queued.get()
                        {
                            fetch_queued.set(true);
                            let entity = fetch_entity.clone();
                            cx.defer(move |cx| {
                                entity.update(cx, |view, cx| {
                                    view.fetch_queued.set(false);
                                    view.fetch_rendered_previews(cx);
                                });
                            });
                        }
                        let busy = cancelling.as_deref() == Some(row.id.as_str());
                        run_stack_row(
                            row_index,
                            row,
                            RunCardState {
                                cancelling: busy,
                                preview,
                            },
                            now_ms,
                            on_open.clone(),
                            on_cancel.clone(),
                            cx,
                        )
                    }
                };
                div()
                    .w_full()
                    .when(after > 0.0, |item| item.mb(px(after)))
                    .child(body)
                    .into_any_element()
            },
            window,
            cx,
        )
    }
}

/// A group's Show more or Show less row, as the sidebar's: a `SidebarItem` with its label.
fn page_control(
    kind: &str,
    label: &'static str,
    group: &str,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let id = format!("runs-{kind}-{group}");
    let debug_id = id.clone();
    div()
        .w_full()
        .debug_selector(move || debug_id.clone())
        .child(SidebarItem::new(SharedString::from(id), label).on_click(on_click))
        .into_any_element()
}

/// Headers and cards, in order. The structure key ignores titles, so a clock tick that only
/// rewrites an "ago" label does not measure the stack again.
fn stack_slots(
    groups: &[RunGroup],
    collapsed: &[String],
    revealed: &HashMap<String, usize>,
    group_gap: f32,
) -> (Rc<Vec<StackSlot>>, String) {
    let mut slots = Vec::new();
    let mut structure = String::new();
    let mut row_index = 0usize;
    let last = groups.len().saturating_sub(1);
    for (group_index, group) in groups.iter().enumerate() {
        let folded = collapsed.iter().any(|id| id == &group.id);
        structure.push_str(&group.id);
        structure.push(if folded { 'c' } else { 'o' });
        let show_rows = !folded && !group.runs.is_empty();
        let header_after = if show_rows {
            ROW_GAP
        } else if group_index == last {
            0.0
        } else {
            group_gap
        };
        slots.push(StackSlot::Header {
            group: group_index,
            collapsed: folded,
            after: header_after,
            key: format!("h:{}", group.id),
        });
        if show_rows {
            let limit = revealed
                .get(&group.id)
                .copied()
                .unwrap_or(SIDEBAR_PAGE_ROWS);
            let page = page_run_rows(group.runs.len(), limit);
            // The rows past the page wait behind Show more; Show less folds them back.
            let controls = usize::from(page.hidden > 0) + usize::from(page.can_show_less);
            let tail_after = |last_item: bool| {
                if !last_item {
                    ROW_GAP
                } else if group_index == last {
                    0.0
                } else {
                    group_gap
                }
            };
            for (row, run) in group.runs.iter().take(page.shown).enumerate() {
                structure.push('\u{1f}');
                structure.push_str(&run.id);
                structure.push(':');
                structure.push_str(&card_row_count(run).to_string());
                let after = tail_after(controls == 0 && row + 1 == page.shown);
                slots.push(StackSlot::Row {
                    group: group_index,
                    row,
                    index: row_index,
                    after,
                    key: format!("r:{}", run.id),
                });
                row_index += 1;
            }
            if page.hidden > 0 {
                structure.push('m');
                slots.push(StackSlot::More {
                    group: group_index,
                    next: page.next_limit,
                    after: tail_after(!page.can_show_less),
                    key: format!("m:{}", group.id),
                });
            }
            if page.can_show_less {
                structure.push('l');
                slots.push(StackSlot::Less {
                    group: group_index,
                    after: tail_after(true),
                    key: format!("l:{}", group.id),
                });
            }
        }
        structure.push('\u{1e}');
    }
    (Rc::new(slots), structure)
}

/// The page's display as the Display section's values.
fn display_rows(display: &SidebarDisplay) -> Vec<DisplayRow> {
    let grouping = match display.group_by {
        GroupBy::Conversation => "conversation",
        GroupBy::Repository => "repository",
        GroupBy::Date => "date",
        GroupBy::Status => "status",
        GroupBy::Agent => "agent",
        GroupBy::None => "none",
    };
    let ordering = match display.sort_by {
        SortBy::Updated => "updated",
        SortBy::Created => "created",
        SortBy::Name => "name",
    };
    let show = match display.show {
        Show::All => "all",
        Show::Unmerged => "unmerged",
        Show::Active => "active",
    };
    let options = |choices: &[(&str, &str)]| {
        choices
            .iter()
            .map(|(id, label)| FilterOption::new(*id, *label))
            .collect()
    };
    vec![
        DisplayRow::new("group", "Group by", Icon::Layers).group(options(&GROUPINGS), grouping),
        DisplayRow::new("sort", "Sort by", Icon::ListOrdered)
            .group(options(&ORDERINGS), ordering)
            .group(
                options(&[("desc", "Descending"), ("asc", "Ascending")]),
                if display.direction == Direction::Desc {
                    "desc"
                } else {
                    "asc"
                },
            ),
        DisplayRow::new("show", "Show", Icon::ListFilter).group(options(&SHOWS), show),
    ]
}

fn record_rendered(rendered: &Rc<RefCell<Vec<(String, bool)>>>, id: &str, active: bool) {
    let mut rendered = rendered.borrow_mut();
    if let Some(slot) = rendered.iter_mut().find(|(known, _)| known == id) {
        slot.1 = active;
    } else {
        rendered.push((id.to_string(), active));
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
