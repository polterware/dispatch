//! Intelligence's cells and the footer under its table. The table is the shared `DataTable`;
//! each cell matches `models-page.tsx`. The filters are the shared `FilterPopover`.

use gpui::{
    AnyElement, App, Font, FontWeight, InteractiveText, IntoElement, SharedString, StyledText, div,
    prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::models::intelligence::{
    Catalog, CatalogEntry, band_of, band_title, display_priority, percent_text, policy_bar,
    score_title, skill_digits, theta_source_label, theta_text, tier_chip_title, tier_label,
};
use crate::theme::{StatusTone, Theme, tw};
use crate::ui::chip::chip;
use crate::ui::data_table::{mono, primary_cell};
use crate::ui::ellipsis::fade;
use crate::ui::segment_display::segment_display;
use crate::ui::tooltip::tooltip;

/// The score cell's line box: the readout is `inline-flex` on the baseline of the cell's
/// 12.5px/18.75px line, so the box runs 5.25px past its foot and the cell centers that box.
const SCORE_LINE: f32 = 23.25;

/// The skill score as an 18px segment readout, padded to two places.
pub fn score_cell(row_id: &str, theta: f64, cx: &App) -> AnyElement {
    div()
        .id(SharedString::from(format!("intel-score-{row_id}")))
        .h(px(SCORE_LINE))
        .flex()
        .items_start()
        .tooltip(tooltip(score_title(theta)))
        .child(segment_display(skill_digits(theta), 18.0, cx))
        .into_any_element()
}

/// `DataTable.PrimaryCell`: the agent's bot, the model in mono, and its tier's chip.
pub fn model_cell(entry: &CatalogEntry, discover: bool, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let id = format!("{}:{}", entry.agent, entry.model);
    let tone = if entry.tier.is_empty() {
        StatusTone::Unknown
    } else {
        StatusTone::Muted
    };
    let mut tier = div()
        .id(SharedString::from(format!("intel-tier-{id}")))
        .child(chip(tier_label(&entry.tier, discover), tone, cx));
    if let Some(title) = tier_chip_title(&entry.tier, discover) {
        tier = tier.tooltip(tooltip(title));
    }
    primary_cell(
        agent_avatar(&entry.agent, "", 24.0),
        entry.model.clone(),
        mono(theme, FontWeight::MEDIUM),
        Some(tier.into_any_element()),
        cx,
    )
}

/// θ in mono, then where it comes from in 12px secondary text, on one baseline 6.5px apart.
pub fn theta_cell(entry: &CatalogEntry, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .w_full()
        .min_w(px(0.0))
        .flex()
        .items_baseline()
        .gap(px(6.5))
        .child(
            div()
                .flex_none()
                .font(mono(theme, FontWeight::NORMAL))
                .text_color(theme.color_text_primary)
                .child(SharedString::from(theta_text(entry.theta))),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(theme.color_text_secondary)
                .child(fade(theta_source_label(&entry.theta_source, entry.score))),
        )
        .into_any_element()
}

/// P(pass) and, in 11px, its lower bound, in mono: bright when the bound meets the bar.
pub fn band_cell(entry: &CatalogEntry, tier: &str, bar: f64, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let Some(pass) = band_of(entry, tier) else {
        return div()
            .text_color(theme.color_text_muted)
            .child("\u{2014}")
            .into_any_element();
    };
    let color = if pass.low >= bar {
        theme.color_text_primary
    } else {
        theme.color_text_muted
    };
    let font: Font = mono(theme, FontWeight::NORMAL);
    div()
        .id(SharedString::from(format!(
            "intel-band-{tier}-{}:{}",
            entry.agent, entry.model
        )))
        .flex()
        .items_baseline()
        .font(font)
        .text_color(color)
        .tooltip(tooltip(band_title(pass, bar)))
        .child(SharedString::from(format!(
            "{} ",
            percent_text(pass.p_pass)
        )))
        .child(
            div()
                .text_size(px(11.0))
                .line_height(px(16.5))
                .child(SharedString::from(format!(
                    "\u{2265}{}",
                    percent_text(pass.low)
                ))),
        )
        .into_any_element()
}

/// The note under the table, in 12px muted text on `leading-5` lines that wrap: how P(pass) reads, the
/// catalog's dates, and its sources, each naming its page as a tooltip.
pub fn catalog_footer(catalog: &Catalog, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let mut text = format!(
        "P(pass) = \u{03c3}(\u{03b8} \u{2212} d) at each band's difficulty; the lower bound in bright text meets the {} bar ({}).",
        display_priority(&catalog.priority),
        percent_text(policy_bar(catalog))
    );
    if !catalog.generated.is_empty() {
        text.push_str(&format!(" Catalog {}", catalog.generated));
    }
    if !catalog.prices_fetched.is_empty() {
        text.push_str(&format!(", prices {}", catalog.prices_fetched));
    }
    let mut sources: Vec<(std::ops::Range<usize>, SharedString)> = Vec::new();
    if !catalog.sources.is_empty() {
        text.push_str(" \u{00b7} ");
        for (index, source) in catalog.sources.iter().enumerate() {
            if index > 0 {
                text.push_str(", ");
            }
            let start = text.len();
            text.push_str(&source.name);
            if !source.url.is_empty() {
                sources.push((start..text.len(), source.url.clone().into()));
            }
        }
    }
    let line = InteractiveText::new("intel-footer", StyledText::new(text)).tooltip(
        move |index, window, cx| {
            sources
                .iter()
                .find(|(range, _)| range.contains(&index))
                .map(|(_, url)| tooltip(url.clone())(window, cx))
        },
    );
    div()
        .w_full()
        .flex_none()
        .text_size(px(12.0))
        .line_height(px(tw(5.0)))
        .text_color(theme.color_text_muted)
        .child(line)
        .into_any_element()
}
