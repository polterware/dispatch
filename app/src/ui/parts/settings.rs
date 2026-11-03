//! Settings layout pieces. The shared radio group cannot host a detail, a trailing
//! status, or an in-place editor, so the classifier rows paint their own mark.

use gpui::{AnyElement, App, IntoElement, Rgba, SharedString, div, prelude::*, px};

use crate::theme::Theme;

pub fn settings_section(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
    body: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let mut header = div()
        .w_full()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(theme.space_3))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(theme.space_1))
                .child(
                    div()
                        .text_size(px(theme.font_size_md))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.color_text_primary)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(theme.font_size_sm))
                        .text_color(theme.color_text_secondary)
                        .child(description.into()),
                ),
        );
    if let Some(action) = action {
        header = header.child(div().flex_none().child(action));
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme.space_3))
        .child(header)
        .child(body)
        .into_any_element()
}

pub fn settings_row(
    label: impl IntoElement,
    description: Option<SharedString>,
    trailing: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let mut text = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(theme.font_size_sm))
                .text_color(theme.color_text_primary)
                .child(label),
        );
    if let Some(description) = description.filter(|text| !text.is_empty()) {
        text = text.child(
            div()
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(description),
        );
    }
    div()
        .min_h(px(theme.row_height))
        .w_full()
        .py(px(theme.space_2))
        .flex()
        .items_center()
        .gap(px(theme.space_3))
        .child(text)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_wrap()
                .justify_end()
                .items_center()
                .gap(px(theme.space_2))
                .child(trailing),
        )
        .into_any_element()
}

pub fn settings_field(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(theme.space_1))
        .child(
            div()
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(label.into()),
        )
        .child(control)
        .into_any_element()
}

/// 14px ring and 8px mark, the same metrics as `radio_group`.
pub fn radio_mark(on: bool, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let mark = if on {
        theme.color_primary
    } else {
        theme.color_surface
    };
    let border = if on {
        theme.color_primary
    } else {
        theme.color_border_strong
    };
    div()
        .mt(px(2.0))
        .size(px(14.0))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(px(8.0)).rounded_full().bg(mark))
        .into_any_element()
}

/// Hover or press wash. Alpha follows the spring, so a resting row stays clear
/// and a sliding indicator behind it remains visible.
pub fn wash(color: Rgba, amount: f32) -> Rgba {
    let amount = amount.clamp(0.0, 1.0);
    Rgba {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a * amount,
    }
}
