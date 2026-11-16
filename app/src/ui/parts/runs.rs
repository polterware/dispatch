//! Parts for the Runs page: collapsible group headers (`SidebarSectionLabel` + count) and a
//! card per run in two columns (the prompt and the run's facts beside a live preview of what
//! its agent printed).

use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, IntoElement, MouseButton, SharedString, Window, div, prelude::*,
    px,
};

use crate::avatar::{agent_avatar, agent_avatar_live};
use crate::models::avatar::avatar_status;
use crate::models::run_preview::RunPreview;
use crate::models::runs::{EM_DASH, RunGroup, RunRow, format_ago};
use crate::models::sidebar::group_title_tooltip;
use crate::theme::{Theme, status_tone, tw};
use crate::ui::{
    Button, ButtonKind, ButtonSize, CountBadge, CountTone, Icon, SidebarSectionLabel, TailLine,
    TailTone, chip, ellipsis, icon_colored, mono, primary_cell, sans, tabular, tail_preview,
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

/// The card's inner padding, in px.
const CARD_PAD_X: f32 = 18.0;
const CARD_PAD_Y: f32 = 16.0;
/// The gap between the card's two columns.
const CARD_GAP: f32 = 18.0;
/// The head of the left column: the bot's box, then the gap under it.
const HEAD_HEIGHT: f32 = 36.0;
const HEAD_GAP: f32 = 12.0;
/// One field's row and the gaps between fields, in the left column's grid.
const FIELD_HEIGHT: f32 = 26.0;
const FIELD_GAP_Y: f32 = 4.0;
const FIELD_GAP_X: f32 = 16.0;
/// The grid always holds at least this many rows, so cards with few fields stay in step.
const MIN_FIELD_ROWS: usize = 4;
/// Where a field's value starts: the icon, its label, and the gap.
const FIELD_LABEL_WIDTH: f32 = 84.0;

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

/// How many fields the card's grid holds for `row`: Agent, Repository, Changes, Started, and
/// Status always; Tier, Tokens, and Cost when known; Cancel while the run is active.
fn field_count(row: &RunRow) -> usize {
    5 + usize::from(!row.tier.trim().is_empty())
        + usize::from(row.tokens_label.is_some())
        + usize::from(row.cost_label.is_some())
        + usize::from(row.active)
}

/// How many rows the card's two-column grid takes, at least [`MIN_FIELD_ROWS`].
pub fn card_row_count(row: &RunRow) -> usize {
    field_count(row).div_ceil(2).max(MIN_FIELD_ROWS)
}

/// The height of the card's content, which is the left column's and so the preview's: the
/// head, then the rows of the two-column grid.
pub fn card_content_height(fields: usize) -> f32 {
    let rows = fields.div_ceil(2).max(MIN_FIELD_ROWS);
    HEAD_HEIGHT + HEAD_GAP + rows as f32 * FIELD_HEIGHT + (rows - 1) as f32 * FIELD_GAP_Y
}

/// What a card draws beside its run: whether Cancel is already on its way, and the tail of the
/// agent's output (`None` until the first read comes back).
pub struct RunCardState {
    pub cancelling: bool,
    pub preview: Option<Rc<RunPreview>>,
}

/// One run as a card under a group header, in two columns. The left one is the run's prompt
/// next to its live bot, then the facts in a grid of two columns with each label beside its
/// value (Agent, Repository, Tier, Changes, Tokens and Cost when known, Started, Status, and
/// Cancel while the run is active); the long ones end in an ellipsis. The right one, about half
/// the card and as tall as the left, is a live tail of what the agent printed
/// ([`tail_preview`]): the newest line at the bottom, fading into the card at both ends while
/// the run works and at its top once it ended, or a muted "Waiting for output…" while there is
/// none. Clicking the card opens the run.
pub fn run_stack_row(
    index: usize,
    row: &RunRow,
    state: RunCardState,
    now_ms: i64,
    on_open: RunAction,
    on_cancel: RunAction,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let content_height = card_content_height(field_count(row));
    let field = |icon: Icon, label: &'static str, value: AnyElement| {
        div()
            .h(px(FIELD_HEIGHT))
            .min_w(px(0.0))
            .flex()
            .items_center()
            .child(
                div()
                    .flex_none()
                    .w(px(FIELD_LABEL_WIDTH))
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(11.0))
                    .font(sans(theme, FontWeight::MEDIUM))
                    .text_color(theme.color_text_muted)
                    .child(icon_colored(icon, 12.0, theme.color_text_muted))
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .child(value),
            )
            .into_any_element()
    };
    // A value on one line that ends in an ellipsis at the width its field gives it.
    let text = |id: &'static str, text: &str, title: &str, font: gpui::Font, color: gpui::Rgba| {
        let mut cell = div()
            .id((id, index))
            .min_w(px(0.0))
            .flex_1()
            .font(font)
            .text_size(px(12.0))
            .text_color(color)
            .child(ellipsis(SharedString::from(text.to_string())));
        if !title.is_empty() {
            cell = cell.tooltip(tooltip(SharedString::from(title.to_string())));
        }
        cell.into_any_element()
    };
    let figure = |id: &'static str, value: &str, title: &str| {
        text(
            id,
            value,
            title,
            tabular(sans(theme, FontWeight::NORMAL)),
            theme.color_text_secondary,
        )
    };

    let mut fields = vec![
        field(
            Icon::Bot,
            "Agent",
            text(
                "run-agent",
                &row.agent_label,
                &row.agent_title,
                mono(theme, FontWeight::NORMAL),
                theme.color_text_secondary,
            ),
        ),
        field(
            Icon::Folder,
            "Repository",
            text(
                "run-source",
                &row.source,
                &row.source_title,
                sans(theme, FontWeight::NORMAL),
                theme.color_text_secondary,
            ),
        ),
    ];
    if !row.tier.trim().is_empty() {
        fields.push(field(
            Icon::Gauge,
            "Tier",
            figure("runs-tier", &row.tier, ""),
        ));
    }
    fields.push(field(Icon::FileDiff, "Changes", changes_cell(row, cx)));
    if let Some(tokens) = &row.tokens_label {
        fields.push(field(
            Icon::Cpu,
            "Tokens",
            figure("runs-tokens", tokens, &row.tokens_title),
        ));
    }
    if let Some(cost) = &row.cost_label {
        fields.push(field(
            Icon::CreditCard,
            "Cost",
            figure("runs-cost", cost, &row.cost_title),
        ));
    }
    fields.push(field(Icon::Clock, "Started", started_cell(row, now_ms, cx)));
    fields.push(field(Icon::Activity, "Status", status_cell(row, cx)));
    if row.active {
        let cancel_id = row.id.clone();
        fields.push(
            div()
                .h(px(FIELD_HEIGHT))
                .min_w(px(0.0))
                .flex()
                .items_center()
                .child(
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
                            .disabled(state.cancelling)
                            .on_click(move |window, cx| {
                                cx.stop_propagation();
                                on_cancel(&cancel_id, window, cx);
                            }),
                        ),
                )
                .into_any_element(),
        );
    }

    let head = div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .h(px(HEAD_HEIGHT))
        .child(
            div()
                .flex_none()
                .size(px(HEAD_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .child(agent_avatar_live(
                    &row.agent,
                    avatar_status(&row.status, &row.merged_at),
                    40.0,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .font(sans(theme, FontWeight::MEDIUM))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .text_color(theme.color_text_primary)
                .child(ellipsis(row.title.clone())),
        );
    let left = div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(content_height))
        .flex()
        .flex_col()
        .gap(px(HEAD_GAP))
        .child(head)
        .child(
            div()
                .grid()
                .grid_cols(2)
                .gap_x(px(FIELD_GAP_X))
                .gap_y(px(FIELD_GAP_Y))
                .children(fields),
        );

    let lines: Vec<TailLine> = state
        .preview
        .as_deref()
        .map(|preview| {
            preview
                .lines
                .iter()
                .map(|line| {
                    TailLine::new(SharedString::from(line.text.clone())).tone(if line.muted {
                        TailTone::Muted
                    } else {
                        TailTone::Normal
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let placeholder = if row.active || state.preview.is_none() {
        "Waiting for output…"
    } else {
        "No output"
    };
    let right = div().flex_1().min_w(px(0.0)).h(px(content_height)).child(
        tail_preview(lines, content_height, theme.color_surface_subtle)
            .live(row.active)
            .placeholder(placeholder),
    );

    let open_id = row.id.clone();
    let row_key = row.id.clone();
    let card = div()
        .id(SharedString::from(format!("runs-item-{}", row.id)))
        .relative()
        .w_full()
        .flex()
        .gap(px(CARD_GAP))
        .px(px(CARD_PAD_X))
        .py(px(CARD_PAD_Y))
        .rounded(px(theme.radius_card))
        .bg(theme.color_surface_subtle)
        .border_1()
        .border_color(theme.color_border_subtle)
        .cursor_pointer()
        .hover(|style| style.border_color(theme.color_border_strong))
        .on_click(move |_, window, cx| on_open(&open_id, window, cx))
        .child(left)
        .child(right);
    div()
        .w_full()
        .debug_selector(move || format!("runs-row-{row_key}"))
        .child(card)
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

#[cfg(test)]
mod tests {
    use super::{MIN_FIELD_ROWS, card_content_height, card_row_count};
    use crate::models::runs::RunRow;

    #[test]
    fn the_card_holds_at_least_four_rows_and_grows_by_whole_rows() {
        let four = card_content_height(MIN_FIELD_ROWS * 2);
        assert_eq!(card_content_height(5), four);
        assert_eq!(card_content_height(8), four);
        assert!(card_content_height(9) > four);
        assert_eq!(card_content_height(9), card_content_height(10));
    }

    #[test]
    fn row_count_tracks_optional_fields_and_active_cancel() {
        let mut row = RunRow::new(&crate::client::DispatchRun::default());
        assert_eq!(card_row_count(&row), MIN_FIELD_ROWS);
        row.tier = "normal".into();
        row.tokens_label = Some("1k tokens".into());
        row.cost_label = Some("$0.01".into());
        assert_eq!(card_row_count(&row), 4);
        row.active = true;
        assert_eq!(card_row_count(&row), 5);
    }
}
