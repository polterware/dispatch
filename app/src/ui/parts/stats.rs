//! The Stats page's parts (`stats-page.tsx`): the ranking cards, the runs columns and groups,
//! and the breakdown table.

use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, IntoElement, Rgba, SharedString, Window, div, prelude::*, px,
};

use crate::client::{DispatchStatRank, DispatchStatsRankings, DispatchStatsRow};
use crate::models::sort::SortDirection;
use crate::models::stats::{
    STATS_COLUMNS, StatsColumn, compact_lines, most_failed_detail, most_used_detail, plural,
    rank_is_known, rank_reason, slowest_detail,
};
use crate::theme::{Theme, mix_oklab, tw};
use crate::ui::data_table::{Column, DataTable, sans, tabular, text_width};
use crate::ui::ellipsis::ellipsis;
use crate::ui::icon_tile::{IconTile, TileTone};
use crate::ui::icons::Icon;
use crate::ui::material::{self, Edge, face};
use crate::ui::{card, progress_bar, tooltip};

/// A runs column's bar: its count over the busiest period's, into 100px, at least 4px.
const COLUMN_BAR_PX: f32 = 100.0;
/// The table's row in the default density: one line of 12.5px text in the cell's 5px padding.
pub const TABLE_ROW_HEIGHT: f32 = 12.5 * 1.5 + 10.0;
/// A compact row adds two 16px lines of figures 0.25rem under the key.
pub const COMPACT_ROW_HEIGHT: f32 = TABLE_ROW_HEIGHT + 3.25 + 32.0;

/// One mark of the chart: a column's period and count, or a group's key and share.
#[derive(Clone, Debug)]
pub struct ChartMark {
    pub label: String,
    pub runs: i32,
    /// The bar's extent, 0 to 1, its minimum included.
    pub fraction: f32,
    pub color_index: usize,
}

/// `CHART_TOKENS`, which the groups cycle through.
pub fn chart_color(theme: &Theme, index: usize) -> Rgba {
    match index % 6 {
        0 => theme.chart_cyan,
        1 => theme.chart_blue,
        2 => theme.chart_green,
        3 => theme.chart_amber,
        4 => theme.chart_red,
        _ => theme.chart_violet,
    }
}

/// The rankings' grid: four columns from a 1024px viewport, two from 640px, else one.
pub fn ranking_grid(rankings: &DispatchStatsRankings, window: &Window, cx: &App) -> AnyElement {
    let viewport = f32::from(window.viewport_size().width);
    let columns = if viewport >= 1024.0 {
        4
    } else if viewport >= 640.0 {
        2
    } else {
        1
    };
    div()
        .grid()
        .grid_cols(columns)
        .gap(px(tw(3.0)))
        .child(ranking_card(
            "Most used model",
            Icon::AiBrain01Icon,
            TileTone::Execution,
            &rankings.most_used_model,
            most_used_detail,
            cx,
        ))
        .child(ranking_card(
            "Most used provider",
            Icon::Cloud,
            TileTone::Integration,
            &rankings.most_used_provider,
            most_used_detail,
            cx,
        ))
        .child(ranking_card(
            "Slowest model",
            Icon::Clock,
            TileTone::Observability,
            &rankings.slowest_model,
            slowest_detail,
            cx,
        ))
        .child(ranking_card(
            "Most failed model",
            Icon::ThumbsDownIcon,
            TileTone::Security,
            &rankings.most_failed_model,
            most_failed_detail,
            cx,
        ))
        .into_any_element()
}

/// `RankingCard`: what it ranks, the winner (or a muted dash), and why.
fn ranking_card(
    title: &'static str,
    icon: Icon,
    tone: TileTone,
    rank: &DispatchStatRank,
    detail: fn(&DispatchStatRank) -> String,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let known = rank_is_known(rank);
    let value = div()
        .id(SharedString::from(format!("stats-rank-{title}")))
        .min_w(px(0.0))
        .text_size(px(17.0))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(px(tw(6.0)))
        .text_color(if known {
            theme.color_text_primary
        } else {
            theme.color_text_muted
        })
        .when(known, |value| value.tooltip(tooltip(rank.key.clone())))
        .child(ellipsis(if known {
            rank.key.clone()
        } else {
            "—".to_string()
        }));
    let reason = if known {
        detail(rank)
    } else {
        rank_reason(&rank.reason)
    };
    card(cx)
        .flex()
        .flex_col()
        .h_full()
        .min_w(px(0.0))
        .gap(px(tw(2.5)))
        .p(px(tw(3.5)))
        .child(
            div()
                .flex()
                .min_w(px(0.0))
                .items_center()
                .gap(px(tw(2.0)))
                .child(
                    div()
                        .flex_none()
                        .child(IconTile::new(icon).size(24.0, 14.0).tone(tone)),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .line_height(px(18.0))
                        .text_color(theme.color_text_muted)
                        .child(ellipsis(title)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .gap(px(tw(0.5)))
                .child(value)
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(theme.color_text_secondary)
                        .child(ellipsis(reason)),
                ),
        )
        .into_any_element()
}

/// `RunsColumns`: a 148px strip of columns at most 72px wide, 0.5rem apart, each its count,
/// its raised bar, and its period.
pub fn runs_columns(marks: &[ChartMark], cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let figures = tabular(sans(theme, FontWeight::NORMAL));
    let top = mix_oklab(theme.chart_cyan, theme.material_face_top, 0.65);
    let bottom = mix_oklab(theme.chart_cyan, theme.material_face_bottom, 0.65);
    let radius = theme.radius_control;
    let mut strip = div().h(px(148.0)).flex().items_end().gap(px(tw(2.0)));
    for mark in marks {
        strip = strip.child(
            div()
                .h_full()
                .min_w(px(0.0))
                .max_w(px(72.0))
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_end()
                .gap(px(tw(1.5)))
                .text_size(px(11.0))
                .line_height(px(16.5))
                .child(
                    div()
                        .font(figures.clone())
                        .text_color(theme.color_text_secondary)
                        .child(mark.runs.to_string()),
                )
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(mark.fraction * COLUMN_BAR_PX))
                        .rounded(px(radius))
                        .bg(face(top, bottom))
                        .shadow(material::control_drop(theme))
                        .child(material::edge(Edge::Strong, radius, theme)),
                )
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .flex()
                        .justify_center()
                        .text_color(theme.color_text_muted)
                        .child(ellipsis(mark.label.clone())),
                ),
        );
    }
    strip.into_any_element()
}

/// The groups' bars: each key and its runs over a `ProgressBar` in the next chart color.
pub fn runs_groups(marks: &[ChartMark], cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    if marks.is_empty() {
        return div()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(theme.color_text_muted)
            .child("No runs in range")
            .into_any_element();
    }
    let figures = tabular(sans(theme, FontWeight::NORMAL));
    let mut list = div().flex().flex_col().gap(px(tw(2.0)));
    for mark in marks {
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(tw(1.0)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(tw(2.0)))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .text_color(theme.color_text_primary)
                                .child(ellipsis(mark.label.clone())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font(figures.clone())
                                .text_color(theme.color_text_secondary)
                                .child(plural(mark.runs, "run")),
                        ),
                )
                .child(progress_bar(
                    mark.fraction,
                    chart_color(theme, mark.color_index),
                    cx,
                )),
        );
    }
    list.into_any_element()
}

/// The breakdown `DataTable` in the default density: the key, then the figures right-aligned
/// in tabular digits, each sortable. Compact keeps the key, Runs, and Success, with the other
/// figures under the key.
pub fn stats_table(
    by_label: String,
    rows: Rc<Vec<DispatchStatsRow>>,
    time: bool,
    compact: bool,
    sort: Option<(usize, SortDirection)>,
    on_sort: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> DataTable {
    let shown: Vec<StatsColumn> = STATS_COLUMNS
        .iter()
        .copied()
        .filter(|column| !compact || column.in_compact())
        .collect();
    let columns = shown
        .iter()
        .map(|column| {
            let mut out = Column::new(if *column == StatsColumn::Key {
                by_label.clone()
            } else {
                column.title().to_string()
            })
            .sortable();
            if let Some((width, min)) = column.width() {
                out = out.width(width).min_width(min).end();
            }
            out
        })
        .collect();
    let cells = rows.clone();
    let measured = rows.clone();
    let cell_columns = shown.clone();
    DataTable::new(
        ("stats-table", u64::from(compact)),
        columns,
        rows.len(),
        move |row, _window, cx| {
            let theme = Theme::global(cx);
            let figures = tabular(sans(theme, FontWeight::NORMAL));
            let entry = &cells[row.index];
            cell_columns
                .iter()
                .map(|column| {
                    let text = column.cell(entry, time);
                    if *column != StatsColumn::Key {
                        return div()
                            .font(figures.clone())
                            .whitespace_nowrap()
                            .child(text)
                            .into_any_element();
                    }
                    let key = div().min_w(px(0.0)).child(ellipsis(text));
                    if !compact {
                        return key.into_any_element();
                    }
                    let [first, second] = compact_lines(entry);
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .child(key)
                        .child(
                            div()
                                .mt(px(tw(1.0)))
                                .text_size(px(11.0))
                                .line_height(px(16.0))
                                .text_color(theme.color_text_muted)
                                .child(ellipsis(first))
                                .child(ellipsis(second)),
                        )
                        .into_any_element()
                })
                .collect()
        },
    )
    .default_density()
    .row_height(if compact {
        COMPACT_ROW_HEIGHT
    } else {
        TABLE_ROW_HEIGHT
    })
    .measure(move |range, window, cx| {
        let theme = Theme::global(cx);
        let text = sans(theme, FontWeight::NORMAL);
        let figures = tabular(text.clone());
        let end = range.end.min(measured.len());
        let mut widths = vec![0.0_f32; shown.len()];
        for row in &measured[range.start.min(end)..end] {
            for (width, column) in widths.iter_mut().zip(&shown) {
                let font = if *column == StatsColumn::Key {
                    text.clone()
                } else {
                    figures.clone()
                };
                let cell = text_width(&column.cell(row, time), font, 12.5, window);
                *width = width.max(cell);
            }
        }
        widths
    })
    .sort(sort, on_sort)
}
