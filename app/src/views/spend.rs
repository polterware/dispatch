//! Spend: one installed agent per row, its quota, and the window's token cost. The toolbar
//! holds the search and the shared `FilterPopover`, whose pills sit in the `ActiveFilterBar`;
//! the Spend window field picks the figures the page loads.

use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::client::{
    CapacityRequest, CoreClient, CoreError, DispatchAgent, DispatchCapacityKey,
    DispatchQuotaSnapshot, DispatchStatsRow, ListAgentsRequest, ListModelCatalogRequest,
    ListQuotaRequest, StatsRequest,
};
use crate::models::spend::{
    COLUMN_SORT, CatalogModel, SortDirection, SortKey, SpendRow, agent_rows, filter_specs,
    row_matches, since_value, sort_rows,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route};
use crate::theme::{Theme, tw};
use crate::ui::parts::spend as parts;
use crate::ui::{
    Column, DataTable, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TextInput,
    ToastTone, next_sort, page_empty, page_fill_bar, toolbar,
};

const RELOAD_GAP: Duration = Duration::from_secs(2);
const CLOCK: Duration = Duration::from_secs(15);

enum Slot<T> {
    Ready(T),
    Keep,
    Clear,
}

struct Bundle {
    agents: Slot<Vec<DispatchAgent>>,
    snapshots: Slot<Vec<DispatchQuotaSnapshot>>,
    pools: Slot<Vec<DispatchCapacityKey>>,
    stats: Slot<Vec<DispatchStatsRow>>,
    catalog: Slot<Vec<CatalogModel>>,
    catalog_error: Option<String>,
}

pub struct SpendView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    query: String,
    sort: Option<(SortKey, SortDirection)>,
    agents: Vec<DispatchAgent>,
    snapshots: Vec<DispatchQuotaSnapshot>,
    pools: Vec<DispatchCapacityKey>,
    stats: Vec<DispatchStatsRow>,
    catalog: Vec<CatalogModel>,
    loaded: bool,
    on_page: bool,
    in_flight: bool,
    again: bool,
    last_kick: Instant,
    fetched_since: Option<String>,
    ticket: u64,
    now_ms: i64,
    catalog_warned: bool,
    animator: Animator,
    enter: SpringValue,
}

impl SpendView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            TextInput::new(cx)
                .placeholder("Search agents and models\u{2026}")
                .pill()
        });
        cx.observe(&search, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.query {
                this.query = text;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&state, |this, _, cx| this.kick(cx)).detach();
        let filters = cx.new(|cx| FilterPopover::new("spend-filters", "Agent filters", true, cx));
        cx.subscribe(&filters, |this, _, _: &FiltersChanged, cx| {
            this.kick(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        let view = Self {
            state,
            search,
            filters,
            fields_key: String::new(),
            query: String::new(),
            sort: None,
            agents: Vec::new(),
            snapshots: Vec::new(),
            pools: Vec::new(),
            stats: Vec::new(),
            catalog: Vec::new(),
            loaded: false,
            on_page: false,
            in_flight: false,
            again: false,
            last_kick: Instant::now(),
            fetched_since: None,
            ticket: 0,
            now_ms: unix_ms(),
            catalog_warned: false,
            animator: Animator::new(),
            enter: SpringValue::new(0.0, SpringTier::Moderate),
        };
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |view, cx| view.kick(cx));
        })
        .detach();
        view.start_clock(cx);
        view
    }

    fn start_clock(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                let alive = this
                    .update(cx, |view, cx| {
                        if view.on_page {
                            view.now_ms = unix_ms();
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    fn kick(&mut self, cx: &mut Context<Self>) {
        let on_spend = matches!(self.state.read(cx).route(), Route::Spend);
        if !on_spend {
            self.on_page = false;
            return;
        }
        let entered = !self.on_page;
        self.on_page = true;
        let since = since_value(self.filters.read(cx).filters()).to_string();
        let since_changed = self.fetched_since.as_deref() != Some(since.as_str());
        if self.in_flight {
            if entered || since_changed {
                self.again = true;
            }
            return;
        }
        if !entered && !since_changed && self.last_kick.elapsed() < RELOAD_GAP {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            let offline = matches!(
                self.state.read(cx).connection(),
                ConnectionState::Offline { .. }
            );
            if offline && self.clear_for_offline() {
                cx.notify();
            }
            return;
        };
        self.in_flight = true;
        self.again = false;
        self.last_kick = Instant::now();
        self.fetched_since = Some(since.clone());
        self.ticket = self.ticket.wrapping_add(1);
        let ticket = self.ticket;
        cx.spawn(async move |this, cx| {
            let bundle = cx
                .background_spawn(async move { load_bundle(client, since) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.ticket != ticket {
                    return;
                }
                view.in_flight = false;
                view.apply(bundle, cx);
                if view.again {
                    view.again = false;
                    view.kick(cx);
                }
            });
        })
        .detach();
    }

    fn clear_for_offline(&mut self) -> bool {
        let dirty = !self.loaded
            || !self.agents.is_empty()
            || !self.snapshots.is_empty()
            || !self.pools.is_empty()
            || !self.stats.is_empty()
            || !self.catalog.is_empty();
        if !dirty {
            return false;
        }
        self.agents.clear();
        self.snapshots.clear();
        self.pools.clear();
        self.stats.clear();
        self.catalog.clear();
        self.loaded = true;
        self.enter.set_target(1.0);
        true
    }

    fn apply(&mut self, bundle: Bundle, cx: &mut Context<Self>) {
        match bundle.agents {
            Slot::Ready(agents) => self.agents = agents,
            Slot::Keep => {}
            Slot::Clear => self.agents.clear(),
        }
        match bundle.snapshots {
            Slot::Ready(snapshots) => self.snapshots = snapshots,
            Slot::Keep => {}
            Slot::Clear => self.snapshots.clear(),
        }
        match bundle.pools {
            Slot::Ready(pools) => self.pools = pools,
            Slot::Keep => {}
            Slot::Clear => self.pools.clear(),
        }
        match bundle.stats {
            Slot::Ready(stats) => self.stats = stats,
            Slot::Keep => {}
            Slot::Clear => self.stats.clear(),
        }
        match bundle.catalog {
            Slot::Ready(catalog) => {
                self.catalog = catalog;
                self.catalog_warned = false;
            }
            Slot::Keep => {
                if let Some(message) = bundle.catalog_error {
                    if !self.catalog_warned {
                        self.catalog_warned = true;
                        self.defer_toast(message, cx);
                    }
                }
            }
            Slot::Clear => self.catalog.clear(),
        }
        self.loaded = true;
        self.enter.set_target(1.0);
        cx.notify();
    }

    fn defer_toast(&self, message: String, cx: &mut Context<Self>) {
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |state, cx| {
                state.push_toast(message, ToastTone::Danger);
                cx.notify();
            });
        })
        .detach();
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    fn toggle_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.sort = next_sort(self.sort, key);
        cx.notify();
    }

    fn tick_motion(&mut self, window: &mut Window) {
        self.enter.set_target(if self.loaded { 1.0 } else { 0.0 });
        let dt = self.animator.dt(window, !self.enter.settled());
        if dt > 0.0 {
            self.enter.step(dt);
        }
    }

    /// The fields follow the rows' providers, rebuilt only when those change.
    fn sync_fields(&mut self, rows: &[SpendRow], cx: &mut Context<Self>) {
        let specs = filter_specs(rows);
        let key = specs[0]
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>()
            .join("\u{1f}");
        if key == self.fields_key && !self.filters.read(cx).fields().is_empty() {
            return;
        }
        self.fields_key = key;
        let icons = [
            Icon::Cpu,
            Icon::Gauge,
            Icon::Clock,
            Icon::CalendarClock,
            Icon::Route,
        ];
        self.filters.update(cx, |popover, _| {
            popover.set_fields(
                specs
                    .into_iter()
                    .zip(icons)
                    .map(|(spec, icon)| FilterField::new(spec, Some(icon)))
                    .collect(),
            );
        });
    }

    fn rows(&self, cx: &App) -> (Vec<SpendRow>, Vec<SpendRow>) {
        let all = agent_rows(
            &self.agents,
            &self.snapshots,
            &self.pools,
            &self.stats,
            &self.catalog,
            self.now_ms,
        );
        let mut visible = all.clone();
        let filters = self.filters.read(cx).filters();
        visible.retain(|row| row_matches(row, &self.query, filters));
        if let Some((key, direction)) = self.sort {
            visible = sort_rows(&visible, key, direction);
        }
        (all, visible)
    }
}

impl Render for SpendView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tick_motion(window);
        let theme = *Theme::global(cx);
        let (all, visible) = self.rows(cx);
        self.sync_fields(&all, cx);
        let view = cx.entity();
        let enter = self.enter.value().clamp(0.0, 1.0);

        let mut content = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(tw(6.0)))
            .opacity(enter);
        content = if !self.loaded {
            content
        } else if visible.is_empty() {
            let (title, mood) = if all.is_empty() {
                ("No agents found", MascotMood::Sad)
            } else {
                ("No matches", MascotMood::Lost)
            };
            content.child(page_empty(title, mood, cx))
        } else {
            content.child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(agents_table(visible, self.sort, view.clone())),
            )
        };

        // The `SearchBar` pill, then `FilterPopover`'s icon button, 8px apart.
        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(self.search.clone()))
            .child(self.filters.clone());
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(page_fill_bar(
                toolbar(div(), end, cx),
                filter_bar,
                tw(4.0),
                content,
                cx,
            ))
    }
}

/// The agents `DataTable`: Agent and Usage at their content's width (`28%` and `24%` do not
/// count when the browser measures), the rest resizable, API cost taking what is left.
fn agents_table(
    rows: Vec<SpendRow>,
    sort: Option<(SortKey, SortDirection)>,
    view: Entity<SpendView>,
) -> DataTable {
    let rows = Rc::new(rows);
    let titles = [
        "Agent",
        "Usage",
        "Pace",
        "Window",
        "Cost / task",
        "Tokens",
        "API cost",
    ];
    let columns = titles
        .iter()
        .zip(COLUMN_SORT)
        .enumerate()
        .map(|(index, (title, key))| {
            let mut column = Column::new(*title);
            if index < 2 {
                column = column.fixed();
            }
            if index >= 4 {
                column = column.end();
            }
            if key.is_some() {
                column = column.sortable();
            }
            column
        })
        .collect();
    let sorted = sort.and_then(|(key, direction)| {
        COLUMN_SORT
            .iter()
            .position(|item| *item == Some(key))
            .map(|index| (index, direction))
    });
    let cells = rows.clone();
    let measured = rows.clone();
    DataTable::new(
        "spend-agents",
        columns,
        rows.len(),
        move |row, _window, cx| {
            let entry = &cells[row.index];
            vec![
                parts::agent_cell(entry, cx),
                parts::usage_cell(row.index, entry, cx),
                parts::pace_cell(row.index, entry, cx),
                parts::window_cell(row.index, entry, cx),
                parts::spend_cell(("spend-task", row.index), entry.task.as_ref(), cx),
                parts::spend_cell(("spend-tokens", row.index), entry.tokens.as_ref(), cx),
                parts::spend_cell(("spend-cost", row.index), entry.cost.as_ref(), cx),
            ]
        },
    )
    .row_height(parts::AVATAR + 10.0)
    .measure(move |range, window, cx| {
        let end = range.end.min(measured.len());
        let start = range.start.min(end);
        parts::content_widths(&measured[start..end], window, cx)
    })
    .sort(sorted, move |index, _window, cx| {
        let Some(key) = COLUMN_SORT.get(index).copied().flatten() else {
            return;
        };
        view.update(cx, |this, cx| this.toggle_sort(key, cx));
    })
}

fn load_bundle(client: CoreClient, since: String) -> Bundle {
    let agents = match client.list_agents(&ListAgentsRequest::default()) {
        Ok(response) => Slot::Ready(response.agents),
        Err(_) => Slot::Keep,
    };
    let snapshots = match client.list_quota(&ListQuotaRequest {
        agent: String::new(),
        since: String::new(),
    }) {
        Ok(response) => Slot::Ready(response.snapshots),
        Err(_) => Slot::Keep,
    };
    let pools = match client.capacity(&CapacityRequest {
        since: String::new(),
    }) {
        Ok(response) => Slot::Ready(response.keys),
        Err(_) => Slot::Keep,
    };
    let stats = match client.stats(&StatsRequest {
        project_id: String::new(),
        by: "agent".into(),
        since,
    }) {
        Ok(response) => Slot::Ready(response.rows),
        Err(_) => Slot::Clear,
    };
    let (catalog, catalog_error) =
        match client.list_model_catalog(&ListModelCatalogRequest::default()) {
            Ok(response) => (
                Slot::Ready(crate::models::spend::parse_catalog_entries(&response.json)),
                None,
            ),
            Err(error) => (Slot::Keep, Some(catalog_message(&error))),
        };
    Bundle {
        agents,
        snapshots,
        pools,
        stats,
        catalog,
        catalog_error,
    }
}

fn catalog_message(error: &CoreError) -> String {
    match error {
        CoreError::Rpc(message) if !message.trim().is_empty() => message.clone(),
        _ => "Could not load the model catalog.".to_string(),
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
