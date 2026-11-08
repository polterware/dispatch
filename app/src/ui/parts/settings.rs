//! Settings layout pieces. The shared radio group cannot host a detail, a trailing
//! status, or an in-place editor, so the classifier rows paint their own mark.

use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, IntoElement, Rgba, SharedString, Window, div, prelude::*, px,
};

use crate::theme::Theme;
use crate::ui::{Icon, TileTone, card, icon};

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

/// What an agent card's fold does when it is pressed.
pub type AgentFold = Rc<dyn Fn(&mut Window, &mut App)>;

/// An agent's card on Settings > Models: its bot and display name over its models, with its
/// switch at the heading's right and a view-only fold before it.
pub fn agent_section(
    agent: &str,
    name: impl Into<SharedString>,
    switch: AnyElement,
    collapsed: bool,
    on_fold: AgentFold,
    body: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let theme = *Theme::global(cx);
    let name: SharedString = name.into();
    let fold_id = SharedString::from(format!("settings-agent-fold-{agent}"));
    let fold = div()
        .id(fold_id)
        .cursor_pointer()
        .rounded(px(theme.radius_control))
        .p(px(4.0))
        .hover(|style| style.bg(theme.color_surface_hover))
        .on_click(move |_, window, cx| on_fold(window, cx))
        .child(icon(
            if collapsed {
                Icon::ChevronRight
            } else {
                Icon::ChevronDown
            },
            14.0,
            cx,
        ));
    let heading = div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(theme.space_3))
        .child(
            div()
                .flex_none()
                .child(crate::avatar::agent_avatar(agent, "", 20.0)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(14.0))
                .line_height(px(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.color_text_primary)
                .child(name),
        )
        .child(fold)
        .child(div().flex_none().child(switch));
    card(cx)
        .w_full()
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(theme.space_3))
        .child(heading)
        .when(!collapsed, |card| {
            card.child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(theme.space_2))
                    // A hairline sets the header apart from its model rows.
                    .child(div().w_full().h(px(1.0)).bg(theme.color_border_default))
                    .child(body),
            )
        })
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
