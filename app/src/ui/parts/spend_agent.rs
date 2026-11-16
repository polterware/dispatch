//! The Spend detail page's table: one row per run an agent spent (date, conversation,
//! repository, model, tier, tokens, cost, status), and under a grouping each section's
//! header and subtotal row.

use std::rc::Rc;

use gpui::{AnyElement, App, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};

use crate::models::runs::EM_DASH;
use crate::models::sort::SortDirection;
use crate::models::spend::format_tokens;
use crate::models::spend_agent::{
    COLUMNS, CostCell, EntryTotals, GroupBy, Layout, Line, SpendColumn, SpendEntry, cost_cell,
    tokens_title,
};
use crate::theme::{Theme, status_tone};
use crate::ui::data_table::{Column, DataTable, TableGroup, mono, sans, tabular, text_width};
use crate::ui::{chip, ellipsis, tooltip};

/// `.pw-status-chip`: 9px in from each side of a 1px frame.
const CHIP_CHROME: f32 = 20.0;
/// The gap between a cost and the note after it.
const NOTE_GAP: f32 = 6.0;
const NOTE_SIZE: f32 = 11.0;

/// What a row opens: the run behind it.
pub type OpenRun = Rc<dyn Fn(&str, &mut Window, &mut App)>;

fn status_label(entry: &SpendEntry) -> String {
    if entry.status.is_empty() {
        "unknown".to_string()
    } else {
        entry.status.clone()
    }
}

fn or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        EM_DASH.to_string()
    } else {
        value.to_string()
    }
}

/// A muted dash, where a figure is unknown.
fn dash(cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .text_color(theme.color_text_muted)
        .child(EM_DASH)
        .into_any_element()
}

/// A token count in tabular digits, its breakdown on hover; a dash when the run reported none.
fn tokens_cell(entry: &SpendEntry, index: usize, slot: usize, value: i64, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    if !entry.has_usage() {
        return dash(cx);
    }
    let cell = div()
        .id(("spend-tokens", index * 3 + slot))
        .whitespace_nowrap()
        .font(tabular(sans(theme, FontWeight::NORMAL)))
        .child(format_tokens(value));
    match tokens_title(entry) {
        Some(title) => cell.tooltip(tooltip(title)).into_any_element(),
        None => cell.into_any_element(),
    }
}

/// A cost and what qualifies it (its source, `partial`, `unpriced`), in a muted note.
fn cost_element(cell: &CostCell, id: (&'static str, usize), strong: bool, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let weight = if strong {
        FontWeight::MEDIUM
    } else {
        FontWeight::NORMAL
    };
    let known = cell.label != EM_DASH;
    div()
        .id(id)
        .flex()
        .items_baseline()
        .gap(px(NOTE_GAP))
        .whitespace_nowrap()
        .child(
            div()
                .font(tabular(sans(theme, weight)))
                .when(!known, |label| label.text_color(theme.color_text_muted))
                .child(cell.label.clone()),
        )
        .children(cell.note.clone().map(|note| {
            div()
                .text_size(px(NOTE_SIZE))
                .text_color(theme.color_text_muted)
                .child(note)
        }))
        .tooltip(tooltip(cell.title.clone()))
        .into_any_element()
}

fn entry_cells(entry: &SpendEntry, index: usize, cx: &App) -> Vec<AnyElement> {
    let theme = Theme::global(cx);
    let (input, cache, output) = entry.figures();
    COLUMNS
        .iter()
        .map(|column| match column {
            SpendColumn::Date => div()
                .whitespace_nowrap()
                .font(tabular(sans(theme, FontWeight::NORMAL)))
                .child(entry.date.clone())
                .into_any_element(),
            SpendColumn::Conversation => div()
                .min_w(px(0.0))
                .text_color(theme.color_text_primary)
                .child(ellipsis(entry.conversation.clone()))
                .into_any_element(),
            SpendColumn::Repository => div()
                .id(("spend-repository", index))
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .tooltip(tooltip(entry.repository_title.clone()))
                .child(SharedString::from(entry.repository.clone()))
                .into_any_element(),
            SpendColumn::Model => div()
                .min_w(px(0.0))
                .whitespace_nowrap()
                .font(mono(theme, FontWeight::NORMAL))
                .text_size(px(12.0))
                .child(ellipsis(or_dash(&entry.model)))
                .into_any_element(),
            SpendColumn::Tier => div()
                .whitespace_nowrap()
                .child(or_dash(&entry.tier))
                .into_any_element(),
            SpendColumn::Input => tokens_cell(entry, index, 0, input, cx),
            SpendColumn::Cache => tokens_cell(entry, index, 1, cache, cx),
            SpendColumn::Output => tokens_cell(entry, index, 2, output, cx),
            SpendColumn::Cost => cost_element(&cost_cell(entry), ("spend-cost", index), false, cx),
            SpendColumn::Status => {
                chip(status_label(entry), status_tone(&entry.status), cx).into_any_element()
            }
        })
        .collect()
}

/// The totals line closing a section: the runs it counts, then its tokens and cost.
fn subtotal_cells(totals: &EntryTotals, section: usize, cx: &App) -> Vec<AnyElement> {
    let theme = Theme::global(cx);
    let strong = |text: String| {
        div()
            .whitespace_nowrap()
            .font(tabular(sans(theme, FontWeight::MEDIUM)))
            .text_color(theme.color_text_primary)
            .child(text)
            .into_any_element()
    };
    let (input, cache, output) = totals.figures();
    let tokens = |value: i64| {
        if totals.usage_runs == 0 {
            dash(cx)
        } else {
            strong(format_tokens(value))
        }
    };
    COLUMNS
        .iter()
        .map(|column| match column {
            SpendColumn::Date => div()
                .whitespace_nowrap()
                .font(sans(theme, FontWeight::MEDIUM))
                .text_color(theme.color_text_primary)
                .child("Subtotal")
                .into_any_element(),
            SpendColumn::Conversation => div()
                .whitespace_nowrap()
                .text_color(theme.color_text_muted)
                .child(runs_text(totals.runs))
                .into_any_element(),
            SpendColumn::Input => tokens(input),
            SpendColumn::Cache => tokens(cache),
            SpendColumn::Output => tokens(output),
            SpendColumn::Cost => cost_element(
                &subtotal_cost(totals),
                ("spend-subtotal", section),
                true,
                cx,
            ),
            _ => div().into_any_element(),
        })
        .collect()
}

fn runs_text(runs: i64) -> String {
    if runs == 1 {
        "1 run".to_string()
    } else {
        format!("{runs} runs")
    }
}

/// A section's cost: the sum of its priced runs, qualified when some were not.
fn subtotal_cost(totals: &EntryTotals) -> CostCell {
    use crate::models::spend::{API_EQUIVALENT, format_cost};
    match totals.cost_usd {
        Some(usd) => CostCell {
            label: format_cost(usd),
            note: totals.cost_note(),
            title: format!(
                "{API_EQUIVALENT} Sums the {} of {} runs that could be priced.",
                totals.cost_runs, totals.runs
            ),
        },
        None => CostCell {
            label: EM_DASH.to_string(),
            note: totals.cost_note(),
            title: "No run in this group could be priced.".to_string(),
        },
    }
}

/// How wide a cell's content is, for the columns' natural widths.
fn text_widths(
    texts: &[(SpendColumn, String)],
    cost: Option<&CostCell>,
    window: &Window,
    cx: &App,
) -> Vec<f32> {
    let theme = Theme::global(cx);
    let plain = sans(theme, FontWeight::NORMAL);
    let figures = tabular(plain.clone());
    let code = mono(theme, FontWeight::NORMAL);
    texts
        .iter()
        .map(|(column, text)| match column {
            SpendColumn::Date | SpendColumn::Input | SpendColumn::Cache | SpendColumn::Output => {
                text_width(text, figures.clone(), 12.5, window)
            }
            SpendColumn::Model => text_width(text, code.clone(), 12.0, window),
            SpendColumn::Cost => cost.map_or(0.0, |cost| {
                text_width(&cost.label, figures.clone(), 12.5, window)
                    + cost.note.as_ref().map_or(0.0, |note| {
                        NOTE_GAP
                            + text_width(note, sans(theme, FontWeight::NORMAL), NOTE_SIZE, window)
                    })
            }),
            SpendColumn::Status => {
                text_width(text, sans(theme, FontWeight::MEDIUM), 11.0, window) + CHIP_CHROME
            }
            _ => text_width(text, plain.clone(), 12.5, window).ceil(),
        })
        .collect()
}

fn entry_texts(entry: &SpendEntry) -> Vec<(SpendColumn, String)> {
    let (input, cache, output) = entry.figures();
    let tokens = |value: i64| {
        if entry.has_usage() {
            format_tokens(value)
        } else {
            EM_DASH.to_string()
        }
    };
    COLUMNS
        .iter()
        .map(|column| {
            let text = match column {
                SpendColumn::Date => entry.date.clone(),
                SpendColumn::Conversation => entry.conversation.clone(),
                SpendColumn::Repository => entry.repository.clone(),
                SpendColumn::Model => or_dash(&entry.model),
                SpendColumn::Tier => or_dash(&entry.tier),
                SpendColumn::Input => tokens(input),
                SpendColumn::Cache => tokens(cache),
                SpendColumn::Output => tokens(output),
                SpendColumn::Cost => String::new(),
                SpendColumn::Status => status_label(entry),
            };
            (*column, text)
        })
        .collect()
}

fn subtotal_texts(totals: &EntryTotals) -> Vec<(SpendColumn, String)> {
    let (input, cache, output) = totals.figures();
    COLUMNS
        .iter()
        .map(|column| {
            let text = match column {
                SpendColumn::Date => "Subtotal".to_string(),
                SpendColumn::Conversation => runs_text(totals.runs),
                SpendColumn::Input => format_tokens(input),
                SpendColumn::Cache => format_tokens(cache),
                SpendColumn::Output => format_tokens(output),
                _ => String::new(),
            };
            (*column, text)
        })
        .collect()
}

/// The table: the lines of `layout` over `entries`, sortable by every column, grouped when the
/// layout has sections. A click on a run's row opens it; a subtotal row opens nothing.
pub fn spend_agent_table(
    id: SharedString,
    by: GroupBy,
    entries: Rc<Vec<SpendEntry>>,
    layout: Rc<Layout>,
    sort: Option<(usize, SortDirection)>,
    on_sort: impl Fn(usize, &mut Window, &mut App) + 'static,
    on_open: OpenRun,
) -> DataTable {
    let columns = COLUMNS
        .iter()
        .map(|column| {
            let header = Column::new(column.title()).sortable();
            match column {
                // These narrow first, so Tokens, Cost and Status fit without a sideways scroll.
                SpendColumn::Conversation => header.min_width(88.0).shrinks(),
                SpendColumn::Repository => header.min_width(64.0).shrinks(),
                SpendColumn::Model => header.min_width(72.0).shrinks(),
                column if column.is_figure() => header.end(),
                _ => header,
            }
        })
        .collect();
    let groups: Vec<TableGroup> = layout
        .sections
        .iter()
        .map(|section| {
            TableGroup::new(
                format!("{}:{}", by.id(), section.key),
                section.label.clone(),
                section.lines.clone(),
            )
            .count(section.totals.runs as usize)
        })
        .collect();
    let cells = (entries.clone(), layout.clone());
    let measured = (entries.clone(), layout.clone());
    let opened = (entries, layout.clone());
    DataTable::new(
        gpui::ElementId::Name(id),
        columns,
        layout.lines.len(),
        move |row, _window, cx| match cells.1.lines[row.index] {
            Line::Entry(entry) => entry_cells(&cells.0[entry], row.index, cx),
            Line::Subtotal(section) => {
                subtotal_cells(&cells.1.sections[section].totals, section, cx)
            }
        },
    )
    .groups(groups)
    .measure(move |range, window, cx| {
        let end = range.end.min(measured.1.lines.len());
        let mut widths = vec![0.0_f32; COLUMNS.len()];
        for line in &measured.1.lines[range.start.min(end)..end] {
            let (texts, cost) = match *line {
                Line::Entry(entry) => {
                    let entry = &measured.0[entry];
                    (entry_texts(entry), Some(cost_cell(entry)))
                }
                Line::Subtotal(section) => {
                    let totals = &measured.1.sections[section].totals;
                    (subtotal_texts(totals), Some(subtotal_cost(totals)))
                }
            };
            for (width, line) in
                widths
                    .iter_mut()
                    .zip(text_widths(&texts, cost.as_ref(), window, cx))
            {
                *width = width.max(line);
            }
        }
        widths
    })
    .sort(sort, on_sort)
    .on_activate(move |index, window, cx| {
        if let Some(Line::Entry(entry)) = opened.1.lines.get(index).copied()
            && let Some(entry) = opened.0.get(entry)
        {
            on_open(&entry.run_id, window, cx);
        }
    })
}
