//! Spend's cells. The table is the shared `DataTable`; each cell matches the Spend table in
//! `index.tsx`. The filters are the shared `FilterPopover`.

use gpui::{AnyElement, App, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};

use crate::avatar::agent_avatar;
use crate::models::spend::{Cell, SpendRow};
use crate::theme::{StatusTone, Theme};
use crate::ui::data_table::{primary_cell, sans, tabular, text_width};
use crate::ui::ellipsis::fade;
use crate::ui::{chip, segment_display, tooltip};

const DASH: &str = "\u{2014}";
/// The avatar's size: the rows grow to it in the cell's 5px padding, to 42px.
pub const AVATAR: f32 = 32.0;
/// The Usage readout is `inline-flex` on the baseline of the cell's 12.5px/18.75px line, so
/// its line box runs 5.25px past the readout's foot and the cell centers that box.
const USAGE_LINE: f32 = 23.25;
/// A `StatusChip`: its label in 11px/500, 9px in from each side of a 1px frame.
const CHIP_CHROME: f32 = 20.0;
const CHIP_GAP: f32 = 6.0;

/// `DataTable.PrimaryCell`: the agent's bot, its name, and its provider in 12px muted text.
pub fn agent_cell(row: &SpendRow, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let provider = (!row.display_provider.is_empty()).then(|| {
        div()
            .text_size(px(12.0))
            .text_color(theme.color_text_muted)
            .child(SharedString::from(row.display_provider.clone()))
            .into_any_element()
    });
    primary_cell(
        agent_avatar(&row.agent, "", AVATAR),
        row.agent.clone(),
        sans(theme, FontWeight::MEDIUM),
        provider,
        cx,
    )
}

/// The used share as an 18px `SegmentDisplay`, where the number comes from as its tooltip.
pub fn usage_cell(index: usize, row: &SpendRow, cx: &App) -> AnyElement {
    let mut cell = div()
        .id(("spend-usage", index))
        .h(px(USAGE_LINE))
        .flex()
        .items_start();
    if let Some(source) = row.source_tooltip.as_deref() {
        cell = cell.tooltip(tooltip(one_line(source)));
    }
    cell.child(segment_display(&row.usage_label, 18.0, cx))
        .into_any_element()
}

/// The pace's `StatusChip`, then a Cooldown chip while one runs, 6px apart.
pub fn pace_cell(index: usize, row: &SpendRow, cx: &App) -> AnyElement {
    let mut chips = div().flex().items_center().gap(px(CHIP_GAP)).child(chip(
        row.pace_label.clone(),
        pace_tone(&row.pace_label),
        cx,
    ));
    if let Some(cooldown) = row.cooldown_tooltip.as_deref() {
        chips = chips.child(
            div()
                .id(("spend-cooldown", index))
                .flex_none()
                .tooltip(tooltip(one_line(cooldown)))
                .child(chip("Cooldown", StatusTone::Warn, cx)),
        );
    }
    chips.into_any_element()
}

/// The quota window and when it resets, in 12px secondary tabular figures, cut with `…`,
/// how the windows went as its tooltip.
pub fn window_cell(index: usize, row: &SpendRow, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let mut cell = div()
        .id(("spend-window", index))
        .min_w(px(0.0))
        .font(tabular(sans(theme, FontWeight::NORMAL)))
        .text_size(px(12.0))
        .text_color(theme.color_text_secondary);
    if let Some(tip) = row.window_tooltip.as_deref().filter(|tip| !tip.is_empty()) {
        cell = cell.tooltip(tooltip(one_line(tip)));
    }
    cell.child(fade(row.window_label.clone()))
        .into_any_element()
}

/// `SpendCell`: a price, tokens, or a cost in 12px secondary tabular figures with what it
/// sums as its tooltip, or a muted dash. A figure wider than its column fades out, as the
/// cell's `.pw-inline-fade` masks it.
pub fn spend_cell(id: (&'static str, usize), cell: Option<&Cell>, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let text = div()
        .id(id)
        .min_w(px(0.0))
        .font(tabular(sans(theme, FontWeight::NORMAL)))
        .text_size(px(12.0));
    match cell {
        Some(cell) => {
            let text = text
                .text_color(theme.color_text_secondary)
                .child(fade(cell.label.clone()));
            if cell.title.is_empty() {
                text.into_any_element()
            } else {
                text.tooltip(tooltip(one_line(&cell.title)))
                    .into_any_element()
            }
        }
        None => text
            .text_color(theme.color_text_muted)
            .child(DASH)
            .into_any_element(),
    }
}

/// Each column's widest content over `rows`, as the browser measures it at `max-content`.
pub fn content_widths(rows: &[SpendRow], window: &Window, cx: &App) -> Vec<f32> {
    let theme = Theme::global(cx);
    let figures = tabular(sans(theme, FontWeight::NORMAL));
    let label = sans(theme, FontWeight::MEDIUM);
    let chip_width = |text: &str| text_width(text, label.clone(), 11.0, window) + CHIP_CHROME;
    let spend = |cell: Option<&Cell>| match cell {
        Some(cell) => text_width(&cell.label, figures.clone(), 12.0, window),
        None => text_width(DASH, figures.clone(), 12.0, window),
    };
    let mut widths = vec![0.0_f32; 7];
    for row in rows {
        let provider = if row.display_provider.is_empty() {
            0.0
        } else {
            10.0 + text_width(&row.display_provider, figures.clone(), 12.0, window)
        };
        let pace = chip_width(&row.pace_label)
            + if row.cooldown_tooltip.is_some() {
                CHIP_GAP + chip_width("Cooldown")
            } else {
                0.0
            };
        let cells = [
            AVATAR + 10.0 + text_width(&row.agent, label.clone(), 12.5, window) + provider,
            readout_width(&row.usage_label),
            pace,
            text_width(&row.window_label, figures.clone(), 12.0, window),
            spend(row.task.as_ref()),
            spend(row.tokens.as_ref()),
            spend(row.cost.as_ref()),
        ];
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = width.max(cell);
        }
    }
    widths
}

/// An 18px `SegmentDisplay`'s width: 10px glyphs, 4px for `:`, `.`, and space, 2.52px apart.
fn readout_width(value: &str) -> f32 {
    let glyphs: Vec<f32> = value
        .chars()
        .map(|ch| {
            if matches!(ch, ':' | '.' | ' ') {
                4.0
            } else {
                10.0
            }
        })
        .collect();
    glyphs.iter().sum::<f32>() + glyphs.len().saturating_sub(1) as f32 * 18.0 * 0.14
}

fn pace_tone(label: &str) -> StatusTone {
    if label.eq_ignore_ascii_case("ahead") {
        StatusTone::Warn
    } else if label.is_empty() || label.eq_ignore_ascii_case("unknown") {
        StatusTone::Unknown
    } else {
        StatusTone::Ok
    }
}

/// Shared tooltips draw one line. Task prices keep each model, joined with a dot.
fn one_line(text: &str) -> String {
    text.replace('\n', " · ")
}
