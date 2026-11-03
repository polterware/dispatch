//! Dispatch stats (`stats-page.tsx`): the rankings, the runs chart, and the breakdown table,
//! in `PageContent` padded 1rem with 1rem between them. The rankings are a bare
//! `CollapsibleSection`; the chart and the table are `StatsCard`s, a `Card` padded 1rem around
//! a section with an icon. The toolbar holds the Group by and Since selects, 11rem wide, and
//! the runs in flight.
//!
//! It reads the app's current workspace (the one of the last run opened), as the Electron
//! page does, and has no workspace picker.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Context, Entity, IntoElement, Render, Window, canvas, div, prelude::*, px};

use crate::client::{CoreError, DispatchStatsRow, StatsRequest, StatsResponse};
use crate::models::sort::{SortDirection, next_sort, sort_rows};
use crate::models::stats::{
    BY_OPTIONS, SINCE_OPTIONS, STATS_COLUMNS, StatsColumn, by_label, chart_bars, empty_stats,
    is_time_series, section_titles, since_label, utc_now_ms,
};
use crate::state::{AppState, ConnectionState, Route, observe};
use crate::theme::{StatusTone, Theme, tw};
use crate::ui::parts::stats::{
    COMPACT_ROW_HEIGHT, ChartMark, TABLE_ROW_HEIGHT, ranking_grid, runs_columns, runs_groups,
    stats_table,
};
use crate::ui::{
    CollapsibleSection, Icon, MascotMood, Menu, MenuEntry, TileTone, ToastTone, card, chip,
    page_content, page_empty, select_trigger, table_height, toolbar,
};

/// Under this table width the breakdown drops to its compact columns.
const COMPACT_WIDTH: f32 = 560.0;
/// `w-44`: each select's width.
const SELECT_WIDTH: f32 = 44.0 * 13.0 / 4.0;

pub struct StatsView {
    state: Entity<AppState>,
    project_id: String,
    by: String,
    since: String,
    stats: StatsResponse,
    cache: HashMap<String, StatsResponse>,
    loaded_key: String,
    loaded_stamp: String,
    loading_key: Option<String>,
    generation: u64,
    stale: bool,
    rankings: Entity<CollapsibleSection>,
    /// The time chart and the groups' are two cards, as `Show` swaps one for the other.
    chart_time: Entity<CollapsibleSection>,
    chart_groups: Entity<CollapsibleSection>,
    table: Entity<CollapsibleSection>,
    /// The breakdown's width, as the last frame laid it out.
    table_width: Rc<Cell<f32>>,
    compact: bool,
    sort: Option<(StatsColumn, SortDirection)>,
    by_menu: Entity<Menu>,
    since_menu: Entity<Menu>,
}

impl StatsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        observe(&state, cx);
        cx.observe(&state, |this, state, cx| {
            if !matches!(state.read(cx).route(), Route::Stats) {
                this.stale = true;
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
        let (chart_title, table_title) = section_titles("day");
        Self {
            state,
            project_id: String::new(),
            by: "day".to_string(),
            since: "7d".to_string(),
            stats: empty_stats("day", "7d"),
            cache: HashMap::new(),
            loaded_key: String::new(),
            loaded_stamp: String::new(),
            loading_key: None,
            generation: 0,
            stale: false,
            rankings: section("Rankings", None, cx),
            chart_time: section(
                &chart_title,
                Some((Icon::CalendarClock, TileTone::Observability)),
                cx,
            ),
            chart_groups: section(
                "Runs by model",
                Some((Icon::Gauge, TileTone::Execution)),
                cx,
            ),
            table: section(
                &table_title,
                Some((Icon::GridTableIcon, TileTone::Data)),
                cx,
            ),
            table_width: Rc::new(Cell::new(0.0)),
            compact: false,
            sort: None,
            by_menu: cx.new(Menu::new),
            since_menu: cx.new(Menu::new),
        }
    }

    fn resolved_project(&self, state: &AppState) -> String {
        state
            .current_workspace()
            .map(|workspace| workspace.id.clone())
            .unwrap_or_default()
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
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
        let key = format!("{}|{}|{}", project.trim(), self.by, self.since);
        if self.loaded_key == key && self.loaded_stamp == stamp && !self.stale {
            return;
        }
        if self.loading_key.as_deref() == Some(key.as_str()) && !self.stale {
            return;
        }
        self.stale = false;
        let Some(client) = client else {
            self.finish_empty(&key, stamp);
            return;
        };
        if project.trim().is_empty() {
            self.finish_empty(&key, stamp);
            return;
        }
        if let Some(cached) = self.cache.get(&key).cloned() {
            self.stats = cached;
        }
        self.loading_key = Some(key.clone());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let request = StatsRequest {
            project_id: project.trim().to_string(),
            by: self.by.clone(),
            since: self.since.clone(),
        };
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move { client.stats(&request) })
                .await;
            let _ = this.update(async_cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                view.loading_key = None;
                match result {
                    Ok(response) => {
                        view.cache.insert(key.clone(), response.clone());
                        view.stats = response;
                        view.loaded_key = key;
                        view.loaded_stamp = stamp;
                    }
                    Err(error) => {
                        view.loaded_key = key;
                        view.loaded_stamp = stamp;
                        let message = match error {
                            CoreError::Rpc(text) if !text.trim().is_empty() => text,
                            _ => "Could not load dispatch stats.".to_string(),
                        };
                        view.state.update(cx, |state, cx| {
                            state.push_toast(message, ToastTone::Danger);
                            cx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn finish_empty(&mut self, key: &str, stamp: String) {
        self.stats = empty_stats(&self.by, &self.since);
        self.loaded_key = key.to_string();
        self.loaded_stamp = stamp;
        self.loading_key = None;
    }

    fn sync_menus(&mut self, cx: &mut Context<Self>) {
        let view = cx.entity();
        let by = self.by.clone();
        let since = self.since.clone();
        let by_items = choice_entries(&BY_OPTIONS, &by, {
            let view = view.clone();
            move |id, cx| {
                view.update(cx, |this, cx| {
                    if this.by != id {
                        this.by = id;
                        cx.notify();
                    }
                });
            }
        });
        let since_items = choice_entries(&SINCE_OPTIONS, &since, {
            let view = view.clone();
            move |id, cx| {
                view.update(cx, |this, cx| {
                    if this.since != id {
                        this.since = id;
                        cx.notify();
                    }
                });
            }
        });
        self.by_menu.update(cx, |menu, _| menu.set_items(by_items));
        self.since_menu
            .update(cx, |menu, _| menu.set_items(since_items));
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
        match self.sort {
            Some((column, direction)) => {
                sort_rows(&self.stats.rows, direction, |row| column.sort_value(row))
            }
            None => self.stats.rows.clone(),
        }
    }

    /// The chart's marks: every period of a time grouping, its empty ones at zero, or each
    /// group in the next chart color.
    fn marks(&self) -> Vec<ChartMark> {
        chart_bars(&self.by, &self.since, &self.stats.rows, utc_now_ms())
            .into_iter()
            .map(|bar| ChartMark {
                label: bar.label,
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
        let (chart_title, table_title) = section_titles(&self.by);
        let rankings = self.stats.rankings.clone();
        self.rankings.update(cx, move |section, _| {
            section.set_body(Rc::new(move |window, app| {
                ranking_grid(&rankings, window, app)
            }));
        });

        let marks = Rc::new(self.marks());
        let chart = if is_time_series(&self.by) {
            &self.chart_time
        } else {
            &self.chart_groups
        };
        let time = is_time_series(&self.by);
        chart.update(cx, move |section, _| {
            section.set_title(chart_title);
            section.set_body(Rc::new(move |_, app| {
                if time {
                    runs_columns(&marks, app)
                } else {
                    runs_groups(&marks, app)
                }
            }));
        });

        let rows = Rc::new(self.sorted_rows());
        let by = by_label(&self.by);
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
            section.set_title(table_title);
            section.set_body(Rc::new(move |_, app| {
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
                    by.clone(),
                    rows.clone(),
                    time,
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
            }));
        });
    }

    fn toolbar_end(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let by_menu = self.by_menu.clone();
        let since_menu = self.since_menu.clone();
        let in_flight = self.stats.in_flight;
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(select(
                "stats-by",
                by_label(&self.by),
                "Group by",
                by_menu.clone(),
                since_menu.clone(),
                cx,
            ))
            .child(select(
                "stats-since",
                since_label(&self.since),
                "Since",
                since_menu,
                by_menu,
                cx,
            ))
            .when(in_flight > 0, |end| {
                end.child(chip(
                    format!("{in_flight} in flight"),
                    StatusTone::Syncing,
                    cx,
                ))
            })
    }
}

impl Render for StatsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);
        self.sync_menus(cx);
        self.sync_compact();
        self.sync_sections(cx);

        let theme = *Theme::global(cx);
        let padding = tw(4.0);
        let chart = if is_time_series(&self.by) {
            self.chart_time.clone()
        } else {
            self.chart_groups.clone()
        };
        let content = div()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .gap(px(padding))
            .child(self.rankings.clone())
            .child(card(cx).min_w(px(0.0)).p(px(padding)).child(chart))
            .child(
                card(cx)
                    .min_w(px(0.0))
                    .p(px(padding))
                    .child(self.table.clone()),
            );
        let bar = toolbar(div(), self.toolbar_end(cx), cx);
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

fn choice_entries(
    options: &[(&str, &str)],
    current: &str,
    on_pick: impl Fn(String, &mut App) + 'static,
) -> Vec<MenuEntry> {
    let on_pick = Rc::new(on_pick);
    options
        .iter()
        .map(|(id, label)| {
            let id = (*id).to_string();
            let picked = id == current;
            let on_pick = on_pick.clone();
            MenuEntry {
                label: (*label).to_string().into(),
                icon: picked.then_some(Icon::Check),
                danger: false,
                on_select: Rc::new(move |_window, cx| on_pick(id.clone(), cx)),
            }
        })
        .collect()
}

/// `Select` in the toolbar: a pill trigger over its menu, which closes the other one.
fn select(
    id: &'static str,
    label: String,
    title: &'static str,
    menu: Entity<Menu>,
    other: Entity<Menu>,
    cx: &App,
) -> impl IntoElement + use<> {
    let toggle = menu.clone();
    div()
        .relative()
        .w(px(SELECT_WIDTH))
        .child(select_trigger(
            id,
            label,
            Some(title),
            Rc::new(move |_, cx| {
                let open = toggle.read(cx).is_open();
                other.update(cx, |menu, cx| {
                    if menu.is_open() {
                        menu.set_open(false);
                        cx.notify();
                    }
                });
                toggle.update(cx, |menu, cx| {
                    menu.set_open(!open);
                    cx.notify();
                });
            }),
            cx,
        ))
        .child(menu)
}
