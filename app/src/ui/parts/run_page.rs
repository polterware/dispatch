//! Run-page visuals: the context meter and file counts.

use gpui::{AnyElement, App, FontWeight, Rgba, div, prelude::*, px};

use crate::models::run_page::{ContextMeter, FileCounts};
use crate::theme::{Theme, tw};
use crate::ui::{Icon, icon_colored, progress_bar_sm, sans, tabular};

/// `FileChangeLine`: the 14px diff icon, `N files`, `+ins` in success, and `−del` in danger,
/// 12px figures in the secondary color, `gap-1.5`. In the rail it keeps the value's weight.
pub fn file_change_line(counts: &FileCounts, theme: Theme) -> AnyElement {
    let mut row = div()
        .flex()
        .min_w(px(0.0))
        .items_center()
        .gap(px(tw(1.5)))
        .font(tabular(sans(&theme, FontWeight::MEDIUM)))
        .text_size(px(12.0))
        .text_color(theme.color_text_secondary)
        .child(icon_colored(
            Icon::FileDiff,
            14.0,
            theme.color_text_secondary,
        ));
    if let Some(files) = &counts.files_label {
        row = row.child(div().flex_none().child(files.clone()));
    }
    if let Some(insertions) = &counts.insertions_label {
        row = row.child(
            div()
                .flex_none()
                .text_color(theme.color_success)
                .child(insertions.clone()),
        );
    }
    if let Some(deletions) = &counts.deletions_label {
        row = row.child(
            div()
                .flex_none()
                .text_color(theme.color_danger)
                .child(deletions.clone()),
        );
    }
    row.into_any_element()
}

/// The Context value: its label in figures over a small `ProgressBar` as wide as the label
/// (`flex min-w-0 flex-col gap-1`). `fill` is the bar's spring position, 0 to 1.
pub fn context_meter(meter: &ContextMeter, fill: f32, color: Rgba, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(tw(1.0)))
        .child(
            div()
                .font(tabular(sans(theme, FontWeight::MEDIUM)))
                .child(meter.label.clone()),
        )
        .when(meter.percent.is_some(), |column| {
            column.child(progress_bar_sm(fill, color, cx))
        })
        .into_any_element()
}
