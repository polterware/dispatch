//! Pieces of the sidebar that the shared controls do not cover: the run dot, the empty line,
//! and the transcript card frame.

use gpui::{AnyElement, IntoElement, div, prelude::*, px};

use crate::theme::Theme;

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
                // The transcript keeps clear of the card's rounded edges.
                .child(
                    div()
                        .size_full()
                        .px(px(theme.space_4))
                        .py(px(theme.space_3))
                        .child(body),
                ),
        )
        .into_any_element()
}
