//! Dispatch stats (`stats-page.tsx`): the rankings, the runs chart, and the breakdown table,
//! in `PageContent` padded 1rem with 1rem between them. The rankings are their cards alone;
//! the chart (Runs) and the table (Breakdown) are `StatsCard`s, a `Card` padded 1rem around a
//! section with an icon, each with its Group by and Since as select-like pills in its heading
//! (`FilterPopover::pills`, on a control with no button): Group by and a Since that fits the grouping (days for a day chart, weeks for a week
//! chart, any span for a group), so each card reads its own figures. Both fields always hold
//! a value: a removed pill comes back at its default. The rankings follow the chart's Since.
//! Runs still in flight are left out: the core's report counts only runs that ended.
//!
//! It reads the app's current workspace (the one of the last run opened), as the Electron
//! page does, and has no workspace picker.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, Render, Window, canvas, div, prelude::*, px,
};

use crate::client::{CoreError, DispatchStatsRow, StatsRequest, StatsResponse};
use crate::models::filter::{FilterOption, FilterSpec};
use crate::models::sort::{SortDirection, next_sort, sort_rows};
use crate::models::stats::{
    BY_OPTIONS, STATS_COLUMNS, StatsColumn, by_label, chart_bars, column_ticks, column_title,
    default_since, empty_stats, fitting_since, is_time_series, since_options, utc_now_ms,
};
use crate::state::{AppState, ConnectionState, Route, observe};
use crate::theme::{Theme, tw};
use crate::ui::parts::stats::{
    COMPACT_ROW_HEIGHT, ChartMark, TABLE_ROW_HEIGHT, ranking_grid, runs_columns, runs_groups,
    stats_table,
};
use crate::ui::{
    CollapsibleSection, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TileTone,
    ToastTone, card, page_content, page_empty, table_height, toolbar,
};
use crate::ui::{SkeletonLayout, page_skeleton};

/// Under this table width the breakdown drops to its compact columns.
const COMPACT_WIDTH: f32 = 560.0;
const DEFAULT_BY: &str = "day";

/// The card a query feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Chart,
    Table,
}

/// One card's own figures: its grouping and window, as its filters hold them, and the
/// answer for them.
struct Query {
    by: String,
    since: String,
    filters: Entity<FilterPopover>,
    stats: StatsResponse,
    loaded_key: String,
    loaded_stamp: String,
    loading_key: Option<String>,
    generation: u64,
    stale: bool,
}

impl Query {
    fn new(pane: Pane, cx: &mut Context<StatsView>) -> Self {
        let (id, label) = match pane {
            Pane::Chart => ("stats-chart-filters", "Chart filters"),
            Pane::Table => ("stats-table-filters", "Breakdown filters"),
        };
        let filters = cx.new(|cx| {
            let mut popover = FilterPopover::new(id, label, false, cx).without_trigger();
            popover.set_fields(fields(DEFAULT_BY));
            popover.add_filter("by", &[DEFAULT_BY.to_string()], cx);
            popover.add_filter("since", &[default_since(DEFAULT_BY).to_string()], cx);
            popover
        });
        cx.subscribe(&filters, move |this, _, _: &FiltersChanged, cx| {
            this.filters_changed(pane, cx);
        })
        .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        let since = default_since(DEFAULT_BY);
        Self {
            by: DEFAULT_BY.to_string(),
            since: since.to_string(),
            filters,
            stats: empty_stats(DEFAULT_BY, since),
            loaded_key: String::new(),
            loaded_stamp: String::new(),
            loading_key: None,
            generation: 0,
            stale: false,
        }
    }
}

/// A card's fields: Group by, then the windows that fit `by`.
fn fields(by: &str) -> Vec<FilterField> {
    let options = |values: &[(&str, &str)]| {
        values
            .iter()
            .map(|(id, label)| FilterOption::new(*id, *label))
            .collect()
    };
    vec![
        FilterField::new(
            FilterSpec::multi("by", "Group by", options(BY_OPTIONS))
                .single()
                .plural("groupings"),
            Some(Icon::Layers),
        ),
        FilterField::new(
            FilterSpec::multi("since", "Since", options(since_options(by)))
                .single()
                .plural("windows"),
            Some(Icon::CalendarClock),
        ),
    ]
}

pub struct StatsView {
    state: Entity<AppState>,
    project_id: String,
    chart: Query,
    breakdown: Query,
    /// Answers by project, grouping, and window, shared by both cards.
    cache: HashMap<String, StatsResponse>,
    /// The time chart and the groups' are two cards, as `Show` swaps one for the other.
    chart_time: Entity<CollapsibleSection>,
    chart_groups: Entity<CollapsibleSection>,
    table: Entity<CollapsibleSection>,
    /// The breakdown's width, as the last frame laid it out.
    table_width: Rc<Cell<f32>>,
    compact: bool,
    sort: Option<(StatsColumn, SortDirection)>,
}

impl StatsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        observe(&state, cx);
        cx.observe(&state, |this, state, cx| {
            if !matches!(state.read(cx).route(), Route::Stats) {
                this.chart.stale = true;
                this.breakdown.stale = true;
            }
        })
        .detach();
        let section = |title: &str, icon: Option<(Icon, TileTone)>, cx: &mut Context<Self>| {
            let title = title.to_string();
            cx.new(move |cx| {
                let mut section = CollapsibleSection::new(title, cx);
                if let Some((icon, tone)) = icon {
                    section.set_icon(icon, tone);
                }
                section
            })
        };
        let chart = Query::new(Pane::Chart, cx);
        let breakdown = Query::new(Pane::Table, cx);
        Self {
            state,
            project_id: String::new(),
            chart,
            breakdown,
            cache: HashMap::new(),
            chart_time: section(
                "Runs",
                Some((Icon::CalendarClock, TileTone::Observability)),
                cx,
            ),
            chart_groups: section("Runs", Some((Icon::Gauge, TileTone::Execution)), cx),
            table: section("Breakdown", Some((Icon::GridTableIcon, TileTone::Data)), cx),
            table_width: Rc::new(Cell::new(0.0)),
            compact: false,
            sort: None,
        }
    }

    fn query(&self, pane: Pane) -> &Query {
        match pane {
            Pane::Chart => &self.chart,
            Pane::Table => &self.breakdown,
        }
    }

    fn query_mut(&mut self, pane: Pane) -> &mut Query {
        match pane {
            Pane::Chart => &mut self.chart,
            Pane::Table => &mut self.breakdown,
        }
    }

    fn resolved_project(&self, state: &AppState) -> String {
        state
            .current_workspace()
            .map(|workspace| workspace.id.clone())
            .unwrap_or_default()
    }

    fn ensure_loaded(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let (resolved, client, stamp) = {
            let state = self.state.read(cx);
            (
                self.resolved_project(state),
                state.client(),
                snapshot_stamp(state),
            )
        };
        self.project_id = resolved;
        let project = self.project_id.clone();
        let cached = {
            let query = self.query(pane);
            let key = format!("{}|{}|{}", project.trim(), query.by, query.since);
            if query.loaded_key == key && query.loaded_stamp == stamp && !query.stale {
                return;
            }
            if query.loading_key.as_deref() == Some(key.as_str()) && !query.stale {
                return;
            }
            (key.clone(), self.cache.get(&key).cloned())
        };
        let (key, cached) = cached;
        let query = self.query_mut(pane);
        query.stale = false;
        let Some(client) = client else {
            finish_empty(query, &key, stamp);
            return;
        };
        if project.trim().is_empty() {
            finish_empty(query, &key, stamp);
            return;
        }
        if let Some(cached) = cached {
            query.stats = cached;
        }
        query.loading_key = Some(key.clone());
        query.generation = query.generation.wrapping_add(1);
        let generation = query.generation;
        let request = StatsRequest {
            project_id: project.trim().to_string(),
            by: query.by.clone(),
            since: query.since.clone(),
        };
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move { client.stats(&request) })
                .await;
            let _ = this.update(async_cx, |view, cx| {
                let toast = {
                    let query = view.query_mut(pane);
                    if query.generation != generation {
                        return;
                    }
                    query.loading_key = None;
                    query.loaded_key = key.clone();
                    query.loaded_stamp = stamp;
                    match result {
                        Ok(response) => {
                            query.stats = response.clone();
                            view.cache.insert(key, response);
                            None
                        }
                        Err(error) => Some(match error {
                            CoreError::Rpc(text) if !text.trim().is_empty() => text,
                            _ => "Could not load dispatch stats.".to_string(),
                        }),
                    }
                };
                if let Some(message) = toast {
                    view.state.update(cx, |state, cx| {
                        state.push_toast(message, ToastTone::Danger);
                        cx.notify();
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A card's filter control, for the screenshot tour.
    pub fn filters(&self, pane: Pane) -> Entity<FilterPopover> {
        self.query(pane).filters.clone()
    }

    /// A card's filters moved: read its grouping and window back, keep both fields filled,
    /// and move the window to the grouping's default when it does not fit the new grouping.
    fn filters_changed(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let filters = self.query(pane).filters.clone();
        let (by, since) = {
            let picked = filters.read(cx).filters();
            (
                picked.values_of("by").first().cloned(),
                picked.values_of("since").first().cloned(),
            )
        };
        let Some(by) = by else {
            filters.update(cx, |popover, cx| {
                popover.set_filter("by", &[DEFAULT_BY.to_string()], cx);
            });
            return;
        };
        if by != self.query(pane).by {
            filters.update(cx, |popover, _| popover.set_fields(fields(&by)));
        }
        let fit = fitting_since(&by, since.as_deref().unwrap_or("")).to_string();
        if since.as_deref() != Some(fit.as_str()) {
            filters.update(cx, |popover, cx| {
                popover.set_filter("since", std::slice::from_ref(&fit), cx);
            });
        }
        let query = self.query_mut(pane);
        let regrouped = query.by != by;
        query.by = by;
        query.since = fit;
        if pane == Pane::Table && regrouped {
            self.sort = None;
        }
        cx.notify();
    }

    fn toggle_sort(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(column) = self.shown_columns().get(index).copied() else {
            return;
        };
        self.sort = next_sort(self.sort, column);
        cx.notify();
    }

    fn shown_columns(&self) -> Vec<StatsColumn> {
        STATS_COLUMNS
            .iter()
            .copied()
            .filter(|column| !self.compact || column.in_compact())
            .collect()
    }

    fn sorted_rows(&self) -> Vec<DispatchStatsRow> {
        let rows = &self.breakdown.stats.rows;
        match self.sort {
            Some((column, direction)) => sort_rows(rows, direction, |row| column.sort_value(row)),
            None => rows.clone(),
        }
    }

    /// The chart's marks: every period of a time grouping, its empty ones at zero, or each
    /// group in the next chart color.
    fn marks(&self) -> Vec<ChartMark> {
        let chart = &self.chart;
        let bars = chart_bars(&chart.by, &chart.since, &chart.stats.rows, utc_now_ms());
        let time = is_time_series(&chart.by);
        let ticks = if time {
            column_ticks(&bars)
        } else {
            bars.iter().map(|bar| bar.label.clone()).collect()
        };
        bars.into_iter()
            .zip(ticks)
            .map(|(bar, tick)| ChartMark {
                title: if time {
                    column_title(&bar)
                } else {
                    String::new()
                },
                label: tick,
                runs: bar.runs,
                fraction: bar.fraction,
                color_index: bar.color_index,
            })
            .collect()
    }

    /// Crossing 560px remounts the table in the other layout, which starts unsorted.
    fn sync_compact(&mut self) {
        let width = self.table_width.get();
        let compact = width > 0.0 && width < COMPACT_WIDTH;
        if compact != self.compact {
            self.compact = compact;
            self.sort = None;
        }
    }

    fn sync_sections(&mut self, cx: &mut Context<Self>) {
        let marks = Rc::new(self.marks());
        let chart = if is_time_series(&self.chart.by) {
            &self.chart_time
        } else {
            &self.chart_groups
        };
        let time = is_time_series(&self.chart.by);
        let chart_filters = self.chart.filters.clone();
        chart.update(cx, move |section, _| {
            let holder = chart_filters.clone();
            section.set_actions(Some(Rc::new(move |window, app| {
                heading_pills(&holder, window, app)
            })));
            section.set_body(Rc::new(move |_, app| {
                if time {
                    runs_columns(&marks, app)
                } else {
                    runs_groups(&marks, app)
                }
            }));
        });

        let rows = Rc::new(self.sorted_rows());
        let by = by_label(&self.breakdown.by);
        let table_time = is_time_series(&self.breakdown.by);
        let table_filters = self.breakdown.filters.clone();
        let compact = self.compact;
        let sort = self.sort.and_then(|(column, direction)| {
            self.shown_columns()
                .iter()
                .position(|shown| *shown == column)
                .map(|index| (index, direction))
        });
        let view = cx.entity();
        let width = self.table_width.clone();
        self.table.update(cx, move |section, _| {
            let holder = table_filters.clone();
            section.set_actions(Some(Rc::new(move |window, app| {
                heading_pills(&holder, window, app)
            })));
            section.set_body(Rc::new(move |_, app| {
                table_body(&rows, &by, table_time, compact, sort, &view, &width, app)
            }));
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn table_body(
    rows: &Rc<Vec<DispatchStatsRow>>,
    by: &str,
    table_time: bool,
    compact: bool,
    sort: Option<(usize, SortDirection)>,
    view: &Entity<StatsView>,
    width: &Rc<Cell<f32>>,
    app: &mut gpui::App,
) -> gpui::AnyElement {
    if rows.is_empty() {
        // `DataTable.Empty` hides the header; its cell pads `PageEmpty` 36px 16px.
        return div()
            .w_full()
            .flex()
            .flex_col()
            .py(px(36.0))
            .px(px(16.0))
            .child(page_empty("No stats yet", MascotMood::Sleeping, app))
            .into_any_element();
    }
    let row_height = if compact {
        COMPACT_ROW_HEIGHT
    } else {
        TABLE_ROW_HEIGHT
    };
    let view = view.clone();
    let table = stats_table(
        by.to_string(),
        rows.clone(),
        table_time,
        compact,
        sort,
        move |index, _, cx| {
            view.update(cx, |this, cx| this.toggle_sort(index, cx));
        },
    );
    let width = width.clone();
    let probe = canvas(
        move |bounds, window, _| {
            let now = f32::from(bounds.size.width);
            let last = width.get();
            width.set(now);
            if (now < COMPACT_WIDTH) != (last < COMPACT_WIDTH) || last == 0.0 {
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top(px(0.0))
    .left(px(0.0))
    .size_full();
    div()
        .relative()
        .w_full()
        .min_w(px(0.0))
        .h(px(table_height(rows.len(), row_height)))
        .child(table)
        .child(probe)
        .into_any_element()
}

/// A card's Group by and Since, as select-like pills in its heading; the control beside them
/// draws no button and holds their menus.
fn heading_pills(
    filters: &Entity<FilterPopover>,
    window: &mut Window,
    app: &mut gpui::App,
) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .children(FilterPopover::pills(filters, window, app))
        .child(filters.clone())
        .into_any_element()
}

impl StatsView {
    /// The loaded page: the rankings, the chart, and the table, `padding` apart.
    fn content(
        &self,
        padding: f32,
        chart: Entity<CollapsibleSection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .gap(px(padding))
            .child(ranking_grid(&self.chart.stats.rankings, window, cx))
            .child(card(cx).min_w(px(0.0)).p(px(padding)).child(chart))
            .child(
                card(cx)
                    .min_w(px(0.0))
                    .p(px(padding))
                    .child(self.table.clone()),
            )
    }
}

impl Render for StatsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(Pane::Chart, cx);
        self.ensure_loaded(Pane::Table, cx);
        self.sync_compact();
        self.sync_sections(cx);

        let theme = *Theme::global(cx);
        let padding = tw(4.0);
        let chart = if is_time_series(&self.chart.by) {
            self.chart_time.clone()
        } else {
            self.chart_groups.clone()
        };
        // Nothing to show yet (the core still starting, or the first answers not back): the
        // rankings, chart, and table in bones. A card that changes its range keeps its last
        // figures until the new ones come.
        let first = |query: &Query| {
            query.loaded_key.is_empty()
                && query
                    .loading_key
                    .as_ref()
                    .is_some_and(|key| !self.cache.contains_key(key))
        };
        let waiting = matches!(
            self.state.read(cx).connection(),
            ConnectionState::Connecting
        ) || first(&self.chart)
            || first(&self.breakdown);
        let content = if waiting {
            div()
                .w_full()
                .h(px(560.0))
                .flex()
                .flex_col()
                .child(page_skeleton(SkeletonLayout::Page).without_toolbar())
        } else {
            self.content(padding, chart, window, cx)
        };
        let bar = toolbar(div(), div(), cx);
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .flex()
            .flex_col()
            .font_family(theme.font_sans)
            .text_color(theme.color_text_primary)
            .child(page_content(
                "stats-page",
                bar,
                padding,
                content,
                window,
                cx,
            ))
    }
}

fn snapshot_stamp(state: &AppState) -> String {
    let mut stamp = match state.connection() {
        ConnectionState::Connecting => "connecting".to_string(),
        ConnectionState::Offline { .. } => "offline".to_string(),
        ConnectionState::Online => "online".to_string(),
    };
    for workspace in state.workspaces() {
        stamp.push('|');
        stamp.push_str(&workspace.id);
    }
    stamp.push('#');
    stamp.push_str(&state.runs().len().to_string());
    for run in state.runs() {
        stamp.push('|');
        stamp.push_str(&run.id);
        stamp.push(':');
        stamp.push_str(&run.status);
    }
    stamp
}

/// No core or no workspace: the card's figures are the empty ones for its range.
fn finish_empty(query: &mut Query, key: &str, stamp: String) {
    query.stats = empty_stats(&query.by, &query.since);
    query.loaded_key = key.to_string();
    query.loaded_stamp = stamp;
    query.loading_key = None;
}
