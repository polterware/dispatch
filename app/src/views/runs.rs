//! Runs, the sidebar's View all ([`Route::Runs`]): the current workspace's runs in the shared
//! `DataTable`, searched from the toolbar and filtered by Status, Agent, and Tier through the
//! shared `FilterPopover`, whose pills sit in the `ActiveFilterBar` under the toolbar.

use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::client::{CancelRunRequest, CoreError};
use crate::models::runs::{COLUMNS, RunRow, SORTABLE, filter_specs, visible_rows, workspace_rows};
use crate::models::sort::{SortDirection, next_sort};
use crate::state::{AppState, ConnectionState, Route};
use crate::theme::Theme;
use crate::ui::parts::runs::{
    self as parts, AVATAR, RunAction, agent_cell, changes_cell, run_cell, started_cell, status_cell,
};
use crate::ui::{
    Column, DataTable, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TextInput,
    ToastTone, page_empty, page_fill_bar, toolbar,
};

/// `formatAgo` labels move on the page's 15s clock.
const CLOCK: Duration = Duration::from_secs(15);
/// `div.overflow-auto.p-3` inside `PageContent p-0`.
const CONTENT_PAD: f32 = 9.75;

pub struct RunsView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    query: String,
    sort: Option<(usize, SortDirection)>,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    cancelling: Option<String>,
    now_ms: i64,
}

impl RunsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
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
        Self {
            state,
            search,
            filters,
            query: String::new(),
            sort: None,
            fields_key: String::new(),
            cancelling: None,
            now_ms: unix_ms(),
        }
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    fn rows(&self, cx: &Context<Self>) -> Vec<RunRow> {
        let state = self.state.read(cx);
        match state.current_workspace() {
            Some(workspace) => workspace_rows(state.runs(), &workspace.id),
            None => Vec::new(),
        }
    }

    /// The fields follow the runs' distinct values, rebuilt only when those change.
    fn sync_fields(&mut self, rows: &[RunRow], cx: &mut Context<Self>) {
        let [status, agent, tier] = filter_specs(rows);
        let key = [&status, &agent, &tier]
            .iter()
            .map(|spec| {
                spec.options
                    .iter()
                    .map(|option| option.id.as_str())
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
                FilterField::new(status, None),
                FilterField::new(agent, Some(Icon::Bot)),
                FilterField::new(tier, Some(Icon::Gauge)),
            ]);
        });
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

    fn toggle_sort(&mut self, column: usize, cx: &mut Context<Self>) {
        if SORTABLE.get(column).copied().unwrap_or(false) {
            self.sort = next_sort(self.sort, column);
            cx.notify();
        }
    }
}

impl Render for RunsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        let rows = self.rows(cx);
        self.sync_fields(&rows, cx);
        let visible = {
            let filters = self.filters.read(cx).filters();
            visible_rows(&rows, filters, &self.query, self.sort)
        };
        let connecting = matches!(
            self.state.read(cx).connection(),
            ConnectionState::Connecting
        );
        let bar = FilterPopover::bar(&self.filters, window, cx);

        let content: AnyElement = if !visible.is_empty() {
            runs_table(visible, self, cx).into_any_element()
        } else if connecting {
            div().into_any_element()
        } else {
            let (title, mood) = if rows.is_empty() {
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
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(page_fill_bar(
                toolbar(div(), end, cx),
                bar,
                CONTENT_PAD,
                content,
                cx,
            ))
    }
}

/// The runs `DataTable`: Run at its content's width (`44%` does not count when the browser
/// measures), Status, Agent, Changes, and Started on the right; Changes does not sort.
fn runs_table(rows: Vec<RunRow>, view: &RunsView, cx: &mut Context<RunsView>) -> DataTable {
    let rows = Rc::new(rows);
    let columns = COLUMNS
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let mut column = Column::new(*title);
            if index == 0 {
                column = column.fixed();
            }
            if index == 4 {
                column = column.end();
            }
            if SORTABLE[index] {
                column = column.sortable();
            }
            column
        })
        .collect();
    let entity = cx.entity();
    let on_cancel: RunAction = {
        let entity = entity.clone();
        Rc::new(move |id: &str, _, cx| {
            let id = id.to_string();
            entity.update(cx, |this, cx| this.cancel_run(id, cx));
        })
    };
    let cancelling = view.cancelling.clone();
    let now_ms = view.now_ms;
    let cells = rows.clone();
    let measured = rows.clone();
    let opened = rows.clone();
    let sorted = entity.clone();
    DataTable::new("runs", columns, rows.len(), move |row, _window, cx| {
        let entry = &cells[row.index];
        let busy = cancelling.as_deref() == Some(entry.id.as_str());
        vec![
            run_cell(entry, busy, on_cancel.clone(), cx),
            status_cell(entry, cx),
            agent_cell(row.index, entry, cx),
            changes_cell(entry, cx),
            started_cell(entry, now_ms, cx),
        ]
    })
    .row_height(AVATAR + 10.0)
    .measure(move |range, window, cx| {
        let end = range.end.min(measured.len());
        let start = range.start.min(end);
        parts::content_widths(&measured[start..end], now_ms, window, cx)
    })
    .sort(view.sort, move |index, _window, cx| {
        sorted.update(cx, |this, cx| this.toggle_sort(index, cx));
    })
    .on_activate(move |index, _window, cx| {
        let Some(id) = opened.get(index).map(|row| row.id.clone()) else {
            return;
        };
        entity.update(cx, |this, cx| this.open_run(id, cx));
    })
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
