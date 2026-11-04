//! Runs, the sidebar's View all ([`Route::Runs`]): the current workspace's runs in the shared
//! `DataTable`, searched from the toolbar and filtered by Status, Agent, and Tier through the
//! shared `FilterPopover`, whose pills sit in the `ActiveFilterBar` under the toolbar.

use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::client::{CancelRunRequest, CoreError};
use crate::models::filter::Filters;
use crate::models::runs::{COLUMNS, RunRow, SORTABLE, filter_specs, visible_rows, workspace_rows};
use crate::models::sort::{SortDirection, next_sort};
use crate::state::{AppState, ConnectionState, Route};
use crate::theme::Theme;
use crate::ui::parts::runs::{
    self as parts, AVATAR, RunAction, agent_cell, changes_cell, run_cell, started_cell, status_cell,
};
use crate::ui::{
    Column, DataTable, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TextInput,
    ToastTone, page_empty, page_under, page_under_inset, toolbar,
};

/// The page's chrome state.
const PAGE: &str = "runs-page";
/// `formatAgo` labels move on the page's 15s clock.
const CLOCK: Duration = Duration::from_secs(15);

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
    /// The workspace's rows, built once per runs revision and workspace: a scroll or a hover
    /// redraws the page every frame, and rebuilding every run's row each time made it stutter.
    rows: Rc<Vec<RunRow>>,
    rows_key: Option<(u64, String)>,
    /// The rows the filters, the search, and the sort keep, for the inputs that kept them.
    visible: Rc<Vec<RunRow>>,
    visible_key: Option<VisibleKey>,
}

/// What the visible rows depend on: the rows, the filters, the search, and the sort.
type VisibleKey = (u64, String, Filters, String, Option<(usize, SortDirection)>);

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
            rows: Rc::default(),
            rows_key: None,
            visible: Rc::default(),
            visible_key: None,
        }
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    /// The workspace's rows and the ones on show, rebuilt only when what they come from moved,
    /// and the filter fields with them.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let (revision, workspace) = {
            let state = self.state.read(cx);
            let workspace = state
                .current_workspace()
                .map(|workspace| workspace.id.clone())
                .unwrap_or_default();
            (state.runs_revision(), workspace)
        };
        let rows_key = (revision, workspace.clone());
        if self.rows_key.as_ref() != Some(&rows_key) {
            let rows = if workspace.is_empty() {
                Vec::new()
            } else {
                workspace_rows(self.state.read(cx).runs(), &workspace)
            };
            self.sync_fields(&rows, cx);
            self.rows = Rc::new(rows);
            self.rows_key = Some(rows_key);
        }
        let filters = self.filters.read(cx).filters().clone();
        let visible_key = (revision, workspace, filters, self.query.clone(), self.sort);
        if self.visible_key.as_ref() != Some(&visible_key) {
            self.visible = Rc::new(visible_rows(
                &self.rows,
                &visible_key.2,
                &self.query,
                self.sort,
            ));
            self.visible_key = Some(visible_key);
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
        self.refresh_rows(cx);
        let visible = self.visible.clone();
        let connecting = matches!(
            self.state.read(cx).connection(),
            ConnectionState::Connecting
        );
        let bar = FilterPopover::bar(&self.filters, window, cx);
        // The toolbar's inset, so the table's edges line up with its buttons.
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, bar.is_some(), padding, window, cx);

        let content: AnyElement = if !visible.is_empty() {
            runs_table(visible, self, cx)
                .inset_top(top)
                .fades(theme.color_surface)
                .into_any_element()
        } else if connecting {
            div().into_any_element()
        } else {
            let (title, mood) = if self.rows.is_empty() {
                ("No runs yet", MascotMood::Sleeping)
            } else {
                ("No matches", MascotMood::Lost)
            };
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .pt(px(top))
                .child(page_empty(title, mood, cx))
                .into_any_element()
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
            // The table runs up under the toolbar, which fades it out on its trail.
            .child(page_under(
                PAGE,
                toolbar(div(), end, cx),
                bar,
                padding,
                content,
                window,
                cx,
            ))
    }
}

/// The runs `DataTable`: Run at its content's width (`44%` does not count when the browser
/// measures), Status, Agent, Changes, and Started on the right; Changes does not sort.
fn runs_table(rows: Rc<Vec<RunRow>>, view: &RunsView, cx: &mut Context<RunsView>) -> DataTable {
    let columns = COLUMNS
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let mut column = Column::new(*title);
            if index == 0 {
                column = column.fixed();
            }
            // Run's title and Agent's `agent / model` fade, so they narrow to fit the page.
            if index == 0 || index == 2 {
                column = column.shrinks();
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
