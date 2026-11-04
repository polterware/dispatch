//! Settings layout pieces. The shared radio group cannot host a detail, a trailing
//! status, or an in-place editor, so the classifier rows paint their own mark.

use gpui::{AnyElement, App, IntoElement, Rgba, SharedString, div, prelude::*, px};

use crate::theme::Theme;
use crate::ui::{Icon, TileTone};

/// A settings section as a card: its tile, title and description, an optional action, and
/// its rows (polterware-ui's [`SettingsSection`](crate::ui::patterns::SettingsSection)).
pub fn settings_section(
    icon: Icon,
    tone: TileTone,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
    body: impl IntoElement,
    _cx: &App,
) -> AnyElement {
    let mut section = crate::ui::patterns::settings_section(title)
        .description(description)
        .icon(icon, tone)
        .child(body);
    if let Some(action) = action {
        section = section.action(action);
    }
    section.into_any_element()
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

/// The lib's 14px radio control, set 2px down to sit on the label's first line.
pub fn radio_mark(on: bool, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .mt(px(2.0))
        .flex_none()
        .child(crate::ui::radio_mark(on, 14.0, theme))
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
