//! Row, highlight, and search-field chrome for the Commander palette.

use gpui::{App, IntoElement, SharedString, div, prelude::*, px};

use crate::avatar::agent_avatar;
use crate::models::commander::{COMMAND_ROW_H, PaletteIcon, PaletteItem};
use crate::theme::Theme;
use crate::ui::{Icon, icon};

/// The sliding row fill. `press` (0–1) insets it a hair while the pointer is down.
pub fn highlight(y: f32, press: f32, theme: Theme) -> impl IntoElement {
    let inset = press.clamp(0.0, 1.0);
    div()
        .absolute()
        .left(px(theme.space_2 + inset))
        .right(px(theme.space_2 + inset))
        .top(px(y + inset))
        .h(px((COMMAND_ROW_H - inset * 2.0).max(0.0)))
        .rounded(px(theme.radius_item))
        .bg(theme.color_surface_hover)
}

pub fn group_heading(label: &str, theme: Theme) -> impl IntoElement {
    div()
        .h(px(crate::models::commander::GROUP_HEADER_H))
        .w_full()
        .px(px(theme.space_2))
        .flex()
        .items_center()
        .text_size(px(theme.font_size_xs))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.color_text_disabled)
        .child(label.to_uppercase())
}

pub fn command_mark(icon_kind: &PaletteIcon, cx: &App) -> gpui::AnyElement {
    let theme = Theme::global(cx);
    match icon_kind {
        PaletteIcon::Run { agent, status } => agent_avatar(agent, status, 20.0).into_any_element(),
        PaletteIcon::Reload => div()
            .size(px(16.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(14.0))
            .text_color(theme.color_text_secondary)
            .child("\u{21BB}")
            .into_any_element(),
        other => {
            let name = match other {
                PaletteIcon::Roles => Icon::Route,
                PaletteIcon::Intelligence => Icon::AiBrain01Icon,
                PaletteIcon::Spend => Icon::CreditCard,
                PaletteIcon::Stats => Icon::Activity,
                PaletteIcon::Settings => Icon::Settings,
                PaletteIcon::AgentSetup => Icon::Sliders,
                PaletteIcon::Reload | PaletteIcon::Run { .. } => Icon::Search,
            };
            icon(name, 16.0, cx)
        }
    }
}

/// One result row. The caller supplies the click and hover handlers.
pub fn command_label(item: &PaletteItem, active: bool, theme: Theme) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(0.0))
        .truncate()
        .text_size(px(theme.font_size_md))
        .text_color(if active {
            theme.color_text_primary
        } else {
            theme.color_text_secondary
        })
        .child(item.label.clone())
}

/// Borderless search line: the value, a caret, and the placeholder.
pub fn search_value(
    before: impl Into<SharedString>,
    after: impl Into<SharedString>,
    full: impl Into<SharedString>,
    selected: bool,
    placeholder: &str,
    theme: Theme,
) -> impl IntoElement {
    let before = before.into();
    let after = after.into();
    let full = full.into();
    let empty = full.is_empty();
    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(theme.field_height_md))
        .pr(px(theme.space_8))
        .flex()
        .items_center()
        .overflow_hidden()
        .text_size(px(18.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .line_height(px(24.0))
        .text_color(theme.color_text_primary)
        .child(if empty {
            div()
                .text_color(theme.color_text_disabled)
                .child(placeholder.to_string())
                .into_any_element()
        } else if selected {
            div()
                .px(px(theme.space_1))
                .rounded(px(theme.radius_sm))
                .bg(theme.color_surface_selected)
                .child(full)
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .min_w(px(0.0))
                .child(div().child(before))
                .child(div().w(px(1.5)).h(px(18.0)).bg(theme.color_text_primary))
                .child(div().child(after))
                .into_any_element()
        })
}

pub fn empty_results(theme: Theme) -> impl IntoElement {
    div()
        .w_full()
        .py(px(theme.space_4))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(theme.font_size_sm))
        .text_color(theme.color_text_muted)
        .child(crate::models::commander::EMPTY_LABEL)
}
