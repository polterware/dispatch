//! Settings layout pieces. The shared radio group cannot host a detail, a trailing
//! status, or an in-place editor, so the classifier rows paint their own mark.

use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, IntoElement, Rgba, SharedString, Window, div, prelude::*, px,
};

use crate::theme::Theme;
use crate::ui::{Icon, TileTone, icon};

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

/// An agent card's heading on Settings > Models: its bot and display name, the fold before
/// its switch at the right.
fn agent_heading(
    agent: &str,
    name: impl Into<SharedString>,
    switch: AnyElement,
    collapsed: bool,
    on_fold: AgentFold,
    cx: &App,
) -> AnyElement {
    let theme = *Theme::global(cx);
    let name: SharedString = name.into();
    let fold_id = SharedString::from(format!("settings-agent-fold-{agent}"));
    let fold_selector = fold_id.clone();
    let fold = div()
        .id(fold_id)
        .debug_selector(move || fold_selector.to_string())
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
    div()
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
        .child(div().flex_none().child(switch))
        .into_any_element()
}

/// The face and edge color every segment of an agent card shares; each segment draws the
/// edges its part of the card keeps.
fn card_segment(cx: &App) -> gpui::Div {
    let theme = Theme::global(cx);
    div()
        .w_full()
        .bg(theme.color_surface_subtle)
        .border_color(theme.color_border_default)
}

/// A folded agent's card: its heading alone, with all four corners and edges.
pub fn agent_card_folded(
    agent: &str,
    name: impl Into<SharedString>,
    switch: AnyElement,
    on_fold: AgentFold,
    cx: &App,
) -> AnyElement {
    let theme = *Theme::global(cx);
    card_segment(cx)
        .rounded(px(theme.radius_card))
        .border(px(0.5))
        .p(px(16.0))
        .child(agent_heading(agent, name, switch, true, on_fold, cx))
        .into_any_element()
}

/// An open agent's card, first segment: its heading, the hairline that sets the rows apart,
/// its top and side edges, and the card's top corners.
pub fn agent_card_top(
    agent: &str,
    name: impl Into<SharedString>,
    switch: AnyElement,
    on_fold: AgentFold,
    cx: &App,
) -> AnyElement {
    let theme = *Theme::global(cx);
    card_segment(cx)
        .rounded_t(px(theme.radius_card))
        .border_t(px(0.5))
        .border_l(px(0.5))
        .border_r(px(0.5))
        .px(px(16.0))
        .pt(px(16.0))
        // The 8px the first row's segment keeps above its row.
        .pb(px(theme.space_2))
        .child(
            div()
                .flex()
                .flex_col()
                .child(agent_heading(agent, name, switch, false, on_fold, cx))
                .child(
                    div()
                        .w_full()
                        .mt(px(theme.space_3))
                        .h(px(1.0))
                        .bg(theme.color_border_default),
                ),
        )
        .into_any_element()
}

/// One model row of an open agent's card: its row and the 8px between it and the next, with
/// the card's sides alone; [`agent_card_bottom`] closes the card under it.
pub fn agent_card_row(row: impl IntoElement, cx: &App) -> AnyElement {
    let theme = *Theme::global(cx);
    card_segment(cx)
        .border_l(px(0.5))
        .border_r(px(0.5))
        .px(px(16.0))
        .pb(px(theme.space_2))
        .child(row)
        .into_any_element()
}

/// An agent that lists no models: its note under the card's hairline, drawn as a row would
/// be between [`agent_card_top`] and [`agent_card_bottom`].
pub fn agent_card_note(note: impl IntoElement, cx: &App) -> AnyElement {
    let theme = *Theme::global(cx);
    card_segment(cx)
        .border_l(px(0.5))
        .border_r(px(0.5))
        .px(px(16.0))
        .pb(px(theme.space_2))
        .child(note)
        .into_any_element()
}

/// An open agent's card, last segment: the card's bottom padding with its bottom corners and
/// its side and bottom edges.
pub fn agent_card_bottom(cx: &App) -> AnyElement {
    let theme = *Theme::global(cx);
    card_segment(cx)
        .h(px(theme.space_2))
        .rounded_b(px(theme.radius_card))
        .border_l(px(0.5))
        .border_r(px(0.5))
        .border_b(px(0.5))
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
