//! `DispatchRunRow`'s cells: the run's bot and prompt (with Cancel while it runs), the
//! repository it came from, its `StatusChip`, `agent / model` in 12px mono, `FileChangeLine`, and when it started.

use std::rc::Rc;

use gpui::{AnyElement, App, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};

use crate::avatar::agent_avatar;
use crate::models::runs::{EM_DASH, RunRow, format_ago};
use crate::theme::{Theme, status_tone, tw};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Icon, chip, icon_colored, mono, primary_cell, sans, tabular,
    text_width, tooltip,
};

/// `DispatchAgentAvatar size={32}`; a row is the bot and 5px above and below it.
pub const AVATAR: f32 = 32.0;
/// `.pw-status-chip`: 9px in from each side of a 1px frame.
const CHIP_CHROME: f32 = 20.0;
/// `gap-1.5` between `FileChangeLine`'s icon and counts.
const CHANGE_GAP: f32 = 4.875;
const CHANGE_ICON: f32 = 14.0;
/// `<Stop size={12} />` in the Cancel button.
const CANCEL_ICON: f32 = 12.0;
/// `PrimaryCell`'s gap before its trailing slot.
const TRAILING_GAP: f32 = 10.0;

pub type RunAction = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// `DataTable.PrimaryCell`: the bot, the prompt, and Cancel while the run is active.
pub fn run_cell(row: &RunRow, cancelling: bool, on_cancel: RunAction, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let trailing = row.active.then(|| {
        let id = row.id.clone();
        Button::new(
            SharedString::from(format!("run-cancel-{}", row.id)),
            ButtonKind::Ghost,
        )
        .size(ButtonSize::Sm)
        .icon(Icon::Stop)
        .icon_size(CANCEL_ICON)
        .label("Cancel")
        .disabled(cancelling)
        .on_click(move |window, cx| {
            cx.stop_propagation();
            on_cancel(&id, window, cx);
        })
        .into_any_element()
    });
    primary_cell(
        agent_avatar(&row.agent, &row.status, AVATAR),
        row.title.clone(),
        sans(theme, FontWeight::MEDIUM),
        trailing,
        cx,
    )
}

/// `StatusChip`, labeled "unknown" when the run has no status.
pub fn status_cell(row: &RunRow, cx: &App) -> AnyElement {
    chip(status_label(row), status_tone(&row.status), cx).into_any_element()
}

fn status_label(row: &RunRow) -> String {
    if row.status.is_empty() {
        "unknown".to_string()
    } else {
        row.status.clone()
    }
}

/// The repository the run came from, in 12px secondary, with its full path as the tooltip.
pub fn source_cell(index: usize, row: &RunRow, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .id(("run-source", index))
        .min_w(px(0.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .font(sans(theme, FontWeight::NORMAL))
        .text_size(px(12.0))
        .text_color(theme.color_text_secondary)
        .tooltip(tooltip(row.source_title.clone()))
        .child(SharedString::from(row.source.clone()))
        .into_any_element()
}

/// `agent / model` in 12px mono secondary, `agent / model / provider` as its tooltip.
pub fn agent_cell(index: usize, row: &RunRow, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .id(("run-agent", index))
        .min_w(px(0.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .font(mono(theme, FontWeight::NORMAL))
        .text_size(px(12.0))
        .text_color(theme.color_text_secondary)
        .tooltip(tooltip(row.agent_title.clone()))
        .child(SharedString::from(row.agent_label.clone()))
        .into_any_element()
}

/// `FileChangeLine`: the diff icon, `N files`, `+ins` in success, `−del` in danger, or a
/// muted dash when the run reported none.
pub fn changes_cell(row: &RunRow, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    if !row.has_changes() {
        return div()
            .text_size(px(12.0))
            .text_color(theme.color_text_muted)
            .child(EM_DASH)
            .into_any_element();
    }
    let (files, insertions, deletions) = row.change_parts();
    div()
        .flex()
        .min_w(px(0.0))
        .items_center()
        .gap(px(CHANGE_GAP))
        .whitespace_nowrap()
        .font(tabular(sans(theme, FontWeight::NORMAL)))
        .text_size(px(12.0))
        .text_color(theme.color_text_secondary)
        .child(icon_colored(
            Icon::FileDiff,
            CHANGE_ICON,
            theme.color_text_secondary,
        ))
        .children(files.map(|files| div().child(files)))
        .children(insertions.map(|count| div().text_color(theme.color_success).child(count)))
        .children(deletions.map(|count| div().text_color(theme.color_danger).child(count)))
        .into_any_element()
}

/// When the run was created (or started), as `formatAgo` says it, in 12px muted figures.
pub fn started_cell(row: &RunRow, now_ms: i64, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .whitespace_nowrap()
        .font(tabular(sans(theme, FontWeight::NORMAL)))
        .text_size(px(12.0))
        .text_color(theme.color_text_muted)
        .child(format_ago(&row.started, now_ms))
        .into_any_element()
}

/// Each column's widest cell over `rows`, for the table's natural widths.
pub fn content_widths(rows: &[RunRow], now_ms: i64, window: &Window, cx: &App) -> Vec<f32> {
    let theme = Theme::global(cx);
    let title = sans(theme, FontWeight::MEDIUM);
    let figures = tabular(sans(theme, FontWeight::NORMAL));
    let code = mono(theme, FontWeight::NORMAL);
    // A small ghost button: `px-2.5` inside its 1px border, the 12px icon, `gap-1.5`, the label.
    let cancel = 2.0 * (tw(2.5) + 1.0)
        + CANCEL_ICON
        + tw(1.5)
        + text_width("Cancel", title.clone(), tw(3.0), window);
    let plain = sans(theme, FontWeight::NORMAL);
    let mut widths = vec![0.0_f32; 6];
    for row in rows {
        let changes = if row.has_changes() {
            let (files, insertions, deletions) = row.change_parts();
            CHANGE_ICON
                + [files, insertions, deletions]
                    .into_iter()
                    .flatten()
                    .map(|part| CHANGE_GAP + text_width(&part, figures.clone(), 12.0, window))
                    .sum::<f32>()
        } else {
            text_width(EM_DASH, figures.clone(), 12.0, window)
        };
        let cells = [
            AVATAR
                + 10.0
                + text_width(&row.title, title.clone(), 12.5, window)
                + if row.active {
                    TRAILING_GAP + cancel
                } else {
                    0.0
                },
            text_width(&row.source, plain.clone(), 12.0, window),
            text_width(&status_label(row), title.clone(), 11.0, window) + CHIP_CHROME,
            text_width(&row.agent_label, code.clone(), 12.0, window),
            changes,
            text_width(
                &format_ago(&row.started, now_ms),
                figures.clone(),
                12.0,
                window,
            ),
        ];
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = width.max(cell);
        }
    }
    widths
}
