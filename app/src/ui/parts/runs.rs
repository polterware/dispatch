//! Parts for the Runs page: collapsible group headers (`SidebarSectionLabel` + count) and
//! full-width stacked run rows (`SidebarItem::run_row` with the live bot avatar, title, muted
//! metadata line, Cancel while active, and `StatusChip` at the right).

use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, IntoElement, MouseButton, SharedString, Window, div, prelude::*,
    px,
};

use crate::avatar::{agent_avatar, agent_avatar_live};
use crate::models::avatar::avatar_status;
use crate::models::runs::{EM_DASH, RunGroup, RunRow, format_ago};
use crate::models::sidebar::group_title_tooltip;
use crate::theme::{Theme, status_tone, tw};
use crate::ui::{
    Button, ButtonKind, ButtonSize, CountBadge, CountTone, Icon, SidebarItem, SidebarSectionLabel,
    chip, ellipsis, icon_colored, mono, primary_cell, sans, tabular, text_width, tooltip,
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
        agent_avatar(
            &row.agent,
            avatar_status(&row.status, &row.merged_at),
            AVATAR,
        ),
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

/// A collapsible group header on the Runs page: the group's title and chevron (like the
/// sidebar's group headers) and its run count badge at the right.
pub fn group_header(
    group: &RunGroup,
    collapsed: bool,
    on_toggle: RunAction,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let toggle_id = group.id.clone();
    let selector_id = group.id.clone();
    let label = SidebarSectionLabel::new(
        SharedString::from(format!("runs-header-{}", group.id)),
        group.title.clone(),
        collapsed,
    )
    .title_tooltip(group_title_tooltip(&group.title))
    .padding(theme.space_2, 44.0)
    .on_toggle(move |window, cx| {
        on_toggle(&toggle_id, window, cx);
    });
    div()
        .relative()
        .w_full()
        .debug_selector(move || format!("runs-group-{selector_id}"))
        .child(label)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(theme.space_2))
                .flex()
                .items_center()
                .child(
                    CountBadge::new(group.runs.len() as u64)
                        .tone(CountTone::Neutral)
                        .sidebar(),
                ),
        )
        .into_any_element()
}

/// One full-width stacked run row under a group header: the live 40px bot in a 36px box,
/// the run title over the muted metadata line (`agent / model`, repository, tier, `+/-`
/// changes, tokens/cost when known, and time ago), Cancel while running, and the status chip
/// at the right.
pub fn run_stack_row(
    index: usize,
    row: &RunRow,
    cancelling: bool,
    now_ms: i64,
    on_open: RunAction,
    on_cancel: RunAction,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let dot = || {
        div()
            .flex_none()
            .text_color(theme.color_text_muted)
            .child("\u{00b7}")
    };
    let mut meta = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(12.0))
        .text_color(theme.color_text_muted)
        .child(div().flex_none().child(agent_cell(index, row, cx)))
        .child(dot())
        .child(div().flex_none().child(source_cell(index, row, cx)));
    if !row.tier.trim().is_empty() {
        meta = meta.child(dot()).child(
            div()
                .flex_none()
                .font(sans(theme, FontWeight::NORMAL))
                .child(SharedString::from(row.tier.clone())),
        );
    }
    if row.has_changes() {
        meta = meta
            .child(dot())
            .child(div().flex_none().child(changes_cell(row, cx)));
    }
    if let Some(tokens) = &row.tokens_label {
        let mut cell = div()
            .id(("runs-tokens", index))
            .flex_none()
            .font(tabular(sans(theme, FontWeight::NORMAL)))
            .child(SharedString::from(tokens.clone()));
        if !row.tokens_title.is_empty() {
            cell = cell.tooltip(tooltip(row.tokens_title.clone()));
        }
        meta = meta.child(dot()).child(cell);
    }
    if let Some(cost) = &row.cost_label {
        let mut cell = div()
            .id(("runs-cost", index))
            .flex_none()
            .font(tabular(sans(theme, FontWeight::NORMAL)))
            .child(SharedString::from(cost.clone()));
        if !row.cost_title.is_empty() {
            cell = cell.tooltip(tooltip(row.cost_title.clone()));
        }
        meta = meta.child(dot()).child(cell);
    }
    meta = meta
        .child(dot())
        .child(div().flex_none().child(started_cell(row, now_ms, cx)));

    let content = div()
        .flex()
        .flex_col()
        .gap(px(3.0))
        .min_w(px(0.0))
        .child(
            div()
                .min_w(px(0.0))
                .font(sans(theme, FontWeight::MEDIUM))
                .text_size(px(13.0))
                .text_color(theme.color_text_primary)
                .child(ellipsis(row.title.clone())),
        )
        .child(meta);

    let mut trailing = div().flex_none().flex().items_center().gap(px(8.0));
    if row.active {
        let cancel_id = row.id.clone();
        trailing = trailing.child(
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
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
                        on_cancel(&cancel_id, window, cx);
                    }),
                ),
        );
    }
    trailing = trailing.child(status_cell(row, cx));

    let open_id = row.id.clone();
    let row_key = row.id.clone();
    let item = SidebarItem::new(SharedString::from(format!("runs-item-{}", row.id)), "")
        .run_row()
        .leading(
            agent_avatar_live(&row.agent, avatar_status(&row.status, &row.merged_at), 40.0),
            36.0,
        )
        .content(content)
        .trailing(trailing)
        .on_click(move |window, cx| {
            on_open(&open_id, window, cx);
        });
    div()
        .w_full()
        .debug_selector(move || format!("runs-row-{row_key}"))
        .child(item)
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
