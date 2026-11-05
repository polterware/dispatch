//! Visual pieces for the roles board, the role summary, and the build peek.
//! Colors come from [`Theme`]. Motion springs live on the view that owns the pointers.

use gpui::{App, IntoElement, SharedString, div, prelude::*, px};

use crate::avatar::agent_avatar;
use crate::models::roles::{
    BuiltEntry, RoleProfile, RoleSource, TIER_COLUMNS, TierBuild, access_label, built_price,
    instructions_preview, role_tier, tier_column_label, when_display,
};
use crate::theme::{StatusTone, Theme};
use crate::ui::{chip, segment_display};

pub fn order_mark(row: usize, theme: &Theme, cx: &App) -> impl IntoElement {
    div()
        .w(px(72.0))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .child(segment_display(format!("#{}", row + 1), 22.0, cx))
        .child(
            div()
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(if row == 0 { "Primary" } else { "Fallback" }),
        )
}

pub fn role_summary(
    profile: &RoleProfile,
    source: RoleSource,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement {
    let when = when_display(&profile.when).to_string();
    let instructions = instructions_preview(&profile.instructions);
    div()
        .p(px(theme.space_4))
        .rounded(px(theme.radius_card))
        .bg(theme.color_surface_raised)
        .border_1()
        .border_color(theme.color_border_subtle)
        .flex()
        .flex_col()
        .gap(px(theme.space_2))
        .child(summary_line("When to use", when, theme))
        .child(summary_line("Instructions", instructions, theme))
        .child(summary_line(
            "Access",
            access_label(profile.readonly).to_string(),
            theme,
        ))
        .child(summary_line("Tier", role_tier(profile), theme))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme.space_6))
                .child(summary_label("Source", theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme.space_2))
                        .child(chip(
                            source.label(),
                            if source == RoleSource::Builtin {
                                StatusTone::Muted
                            } else {
                                StatusTone::Ok
                            },
                            cx,
                        ))
                        .when(profile.manual, |row| {
                            row.child(chip("Only with --role", StatusTone::Muted, cx))
                        }),
                ),
        )
}

fn summary_line(label: &'static str, value: String, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(theme.space_6))
        .child(summary_label(label, theme))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(theme.font_size_sm))
                .text_color(theme.color_text_primary)
                .child(value),
        )
}

fn summary_label(label: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .w(px(96.0))
        .flex_none()
        .text_size(px(theme.font_size_sm))
        .text_color(theme.color_text_muted)
        .child(label)
}

/// Under a repository's role: where it is edited.
pub fn role_board_note(theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(theme.font_size_sm))
        .text_color(theme.color_text_muted)
        .child("This role is in the repository's .dispatch/roles.json; edit it there.")
}

/// The proposed table: one section per tier, primary first.
pub fn build_sections(build: &TierBuild, theme: &Theme, cx: &App) -> impl IntoElement {
    let mut column = div().flex().flex_col().gap(px(theme.space_4));
    for (id, _label) in TIER_COLUMNS {
        let entries = build.tiers.get(*id).map(Vec::as_slice).unwrap_or(&[]);
        column = column.child(build_section(id, entries, theme, cx));
    }
    column
}

fn build_section(tier: &str, entries: &[BuiltEntry], theme: &Theme, cx: &App) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap(px(theme.space_1));
    for (index, entry) in entries.iter().enumerate() {
        list = list.child(build_row(index, entry, theme, cx));
    }
    if entries.is_empty() {
        list = list.child(
            div()
                .text_size(px(theme.font_size_sm))
                .text_color(theme.color_text_muted)
                .child("None"),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(px(theme.space_2))
        .child(
            div()
                .text_size(px(theme.font_size_sm))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.color_text_primary)
                .child(tier_column_label(tier)),
        )
        .child(list)
}

fn build_row(index: usize, entry: &BuiltEntry, theme: &Theme, cx: &App) -> impl IntoElement {
    let mut title = format!("{}/{}", entry.agent, entry.model);
    if !entry.effort.is_empty() {
        title.push_str(&format!(" @{}", entry.effort));
    }
    div()
        .flex()
        .items_center()
        .gap(px(theme.space_2))
        .py(px(theme.space_1))
        .child(
            div()
                .w(px(16.0))
                .flex_none()
                .text_size(px(theme.font_size_sm))
                .text_color(theme.color_text_muted)
                .child(format!("{}", index + 1)),
        )
        .child(agent_avatar(&entry.agent, "", 18.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(theme.font_mono)
                .text_size(px(theme.font_size_sm))
                .text_color(theme.color_text_primary)
                .child(title),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme.font_mono)
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(built_price(entry)),
        )
        .when(!entry.qualified, |row| {
            row.child(chip("Below bar", StatusTone::Muted, cx))
        })
}

pub fn muted_note(text: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
    div()
        .text_size(px(theme.font_size_sm))
        .text_color(theme.color_text_muted)
        .child(text.into())
}
