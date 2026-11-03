//! Pieces of the sidebar that the shared controls do not cover: the run dot, the empty line,
//! display-menu headings, menu actions, and the transcript card frame.

use std::rc::Rc;

use gpui::{AnyElement, App, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};

use crate::theme::Theme;
use crate::ui::{Icon, icon};

/// `.dispatch-run-dot`: 6px, info while running or pending, danger once it failed, timed out,
/// or was orphaned, muted when cancelled, success otherwise (`runTone`).
pub fn run_dot(status: &str, theme: &Theme) -> gpui::Div {
    let color = match status {
        "running" | "pending" => theme.color_info,
        "failed" | "orphaned" | "timed_out" => theme.color_danger,
        "cancelled" => theme.color_text_muted,
        _ => theme.color_success,
    };
    div().size(px(6.0)).flex_none().rounded_full().bg(color)
}

/// `.dispatch-run-none`: a line in 12px muted with 4px 8px padding.
pub fn run_none(text: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .px(px(theme.space_2))
        .py(px(theme.space_1))
        .text_size(px(theme.font_size_sm))
        .text_color(theme.color_text_muted)
        .child(text)
}

pub fn menu_heading(label: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .px(px(theme.space_2))
        .pt(px(theme.space_3))
        .pb(px(theme.space_1))
        .text_size(px(theme.font_size_xs))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.color_text_muted)
        .child(label.into())
}

pub fn menu_separator(theme: &Theme) -> impl IntoElement {
    div()
        .mt(px(theme.space_2))
        .mx(px(theme.space_2))
        .h(px(1.0))
        .bg(theme.color_border_subtle)
}

/// Left-aligned display-menu action. Shared `Menu` rows cannot sit beside radio groups.
pub fn action_row(
    id: impl Into<ElementId>,
    name: Icon,
    label: impl Into<SharedString>,
    danger: bool,
    on_click: Rc<dyn Fn(&mut Window, &mut App)>,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let fg = if danger {
        theme.color_danger
    } else {
        theme.color_text_primary
    };
    let hover = theme.color_surface_hover;
    div()
        .id(id)
        .h(px(theme.control_height_md))
        .px(px(theme.space_2))
        .flex()
        .items_center()
        .gap(px(theme.space_2))
        .rounded(px(theme.radius_sm))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(icon(name, 14.0, cx))
        .child(
            div()
                .text_size(px(theme.font_size_sm))
                .text_color(fg)
                .child(label.into()),
        )
}

/// Card frame for the run transcript. `slide` drops the face a few pixels as it enters.
pub fn preview_surface(
    width: f32,
    height: f32,
    opacity: f32,
    slide: f32,
    theme: &Theme,
    body: impl IntoElement,
) -> AnyElement {
    div()
        .relative()
        .w(px(width))
        .h(px(height + slide))
        .child(
            div()
                .absolute()
                .top(px(slide))
                .left(px(0.0))
                .w(px(width))
                .h(px(height))
                .opacity(opacity)
                .rounded(px(theme.radius_lg))
                .bg(theme.color_surface_raised)
                .border_1()
                .border_color(theme.color_border)
                .overflow_hidden()
                .occlude()
                .child(body),
        )
        .into_any_element()
}
