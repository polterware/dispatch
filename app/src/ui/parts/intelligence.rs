//! Intelligence's cells and the model dialog's body. The table is the shared `DataTable`;
//! the filters are the shared `FilterPopover`. The Score and the model explain themselves in
//! a rich tooltip (how the score was read; which agents on this machine offer the model), a
//! band in a plain one, and a click on a row opens the model's dialog (`composer_card` on a
//! `modal_layer`), which says all of it at once.

use gpui::{
    AnyElement, App, Div, Font, FontWeight, IntoElement, SharedString, div, prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::models::intelligence::{
    BANDS, BandPass, ModelRow, ScoreDetail, band_label, band_of, band_title, effort_label,
    is_guess, offer_name, offered_line, percent_text, routing_note, row_id, skill_digits,
    theta_source_label, theta_text,
};
use crate::models::settings::agent_label;
use crate::theme::Theme;
use crate::ui::data_table::{mono, primary_cell};
use crate::ui::ellipsis::ellipsis;
use crate::ui::segment_display::segment_display;
use crate::ui::tooltip::tooltip;
use crate::ui::{
    rail_group, rail_properties, rail_property, rich_tooltip, tooltip_heading, tooltip_note,
    tooltip_row, wrap_text,
};

/// The score cell's line box: the readout is `inline-flex` on the baseline of the cell's
/// 12.5px/18.75px line, so the box runs 5.25px past its foot and the cell centers that box.
const SCORE_LINE: f32 = 23.25;

/// The tooltips' width, so their sentences wrap inside it.
const TIP_WIDTH: f32 = 300.0;

/// How strong a Score no benchmark backs reads: dimmed, a guess and not a measure.
const GUESS_OPACITY: f32 = 0.4;

/// The skill score as an 18px segment readout, padded to two places, dimmed when it is a
/// guess; its tooltip shows how it was read.
pub fn score_cell(row: &ModelRow, detail: ScoreDetail, cx: &App) -> AnyElement {
    div()
        .id(SharedString::from(format!("intel-score-{}", row_id(row))))
        .h(px(SCORE_LINE))
        .flex()
        .items_start()
        .when(is_guess(row), |cell| cell.opacity(GUESS_OPACITY))
        .tooltip(rich_tooltip(TIP_WIDTH, move |_, cx| score_tip(&detail, cx)))
        .child(segment_display(skill_digits(row.entry.theta), 18.0, cx))
        .into_any_element()
}

fn score_tip(detail: &ScoreDetail, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let code = |text: &str| {
        div()
            .font(mono(theme, FontWeight::NORMAL))
            .child(SharedString::from(text.to_string()))
    };
    let mut tip = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(tooltip_heading(
            detail.title.clone(),
            Some(
                "100\u{00b7}\u{03c3}(\u{03b8}): the chance to pass a task of difficulty 0.".into(),
            ),
            cx,
        ))
        .child(tooltip_row("Effort", detail.effort.clone(), cx))
        .child(tooltip_row("Skill \u{03b8}", code(&detail.theta), cx))
        .child(tooltip_row("From", detail.source.clone(), cx));
    if let Some(bound) = &detail.bound {
        tip = tip.child(tooltip_row("Lower bound", code(bound), cx));
    }
    tip = tip.child(
        div()
            .text_color(theme.color_text_secondary)
            .child(detail.how.clone()),
    );
    if !detail.note.is_empty() {
        tip = tip.child(tooltip_note(detail.note.clone(), cx));
    }
    tip.into_any_element()
}

/// `DataTable.PrimaryCell`: the bot of the agent that leads its offers and the model in
/// mono; its tooltip lists every agent here that offers it, at each effort.
pub fn model_cell(row: &ModelRow, discover: bool, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let lead = row
        .offers
        .first()
        .map(|offer| offer.agent.clone())
        .unwrap_or_default();
    let tip = row.clone();
    div()
        .id(SharedString::from(format!("intel-model-{}", row_id(row))))
        .w_full()
        .min_w(px(0.0))
        .tooltip(rich_tooltip(TIP_WIDTH, move |_, cx| {
            model_tip(&tip, discover, cx)
        }))
        .child(primary_cell(
            agent_avatar(&lead, "", 24.0),
            row.name.clone(),
            mono(theme, FontWeight::MEDIUM),
            None,
            cx,
        ))
        .into_any_element()
}

fn model_tip(row: &ModelRow, discover: bool, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let mut tip = div().flex().flex_col().gap(px(8.0)).child(tooltip_heading(
        row.name.clone(),
        Some(offered_line(row).into()),
        cx,
    ));
    for offer in &row.offers {
        let mut name = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(agent_avatar(&offer.agent, "", 16.0))
            .child(SharedString::from(agent_label(&offer.agent)));
        if !offer.tier.is_empty() {
            name = name.child(
                div()
                    .text_color(theme.color_text_muted)
                    .child(SharedString::from(format!("{} tier", offer.tier))),
            );
        }
        tip = tip.child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(div().flex_none().child(name))
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_right()
                        .font(mono(theme, FontWeight::NORMAL))
                        .text_color(theme.color_text_secondary)
                        .child(SharedString::from(offer_name(offer))),
                ),
        );
    }
    tip.child(tooltip_note(routing_note(row, discover), cx))
        .into_any_element()
}

/// P(pass) and, in 11px, its lower bound, in mono: bright when the bound meets the bar.
pub fn band_cell(row: &ModelRow, tier: &str, bar: f64, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    match band_of(&row.entry, tier) {
        Some(pass) => div()
            .id(SharedString::from(format!(
                "intel-band-{tier}-{}",
                row_id(row)
            )))
            .tooltip(tooltip(band_title(pass, bar)))
            .child(band_value(pass, bar, cx))
            .into_any_element(),
        None => div()
            .text_color(theme.color_text_muted)
            .child("\u{2014}")
            .into_any_element(),
    }
}

fn band_value(pass: &BandPass, bar: f64, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let color = if pass.low >= bar {
        theme.color_text_primary
    } else {
        theme.color_text_muted
    };
    let font: Font = mono(theme, FontWeight::NORMAL);
    div()
        .flex()
        .items_baseline()
        .font(font)
        .text_color(color)
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

/// Secondary text that wraps at its column's width.
fn prose(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = Theme::global(cx);
    div()
        .w_full()
        .px(px(8.0))
        .text_size(px(12.5))
        .line_height(px(19.0))
        .text_color(theme.color_text_secondary)
        .child(wrap_text(text))
}

/// The model dialog's body: the Score with what it means and the effort it is read at, each
/// effort's Score, then Skill (θ, its source, the lower bound's margin, and how the source
/// became θ) beside Pass chance (each band's P(pass) and bound at its `d`), both at the best
/// effort, then the agents here that offer the model, and the catalog's date and sources.
pub fn model_details(
    row: &ModelRow,
    detail: &ScoreDetail,
    discover: bool,
    bar: f64,
    bar_note: &str,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let code = |text: &str| {
        div()
            .font(mono(theme, FontWeight::NORMAL))
            .child(SharedString::from(text.to_string()))
            .into_any_element()
    };
    let plain = |text: &str| wrap_text(text.to_string()).into_any_element();

    let mut meaning = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(13.0))
                .line_height(px(19.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(detail.title.clone())),
        )
        .child(
            div()
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(theme.color_text_secondary)
                .child(wrap_text(
                    "100\u{00b7}\u{03c3}(\u{03b8}): the chance to pass a task of difficulty 0.",
                )),
        );
    meaning = meaning.child(
        div()
            .text_size(px(12.0))
            .line_height(px(18.0))
            .text_color(theme.color_text_muted)
            .child(SharedString::from(detail.effort.clone())),
    );
    let summary = div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(16.0))
        .px(px(8.0))
        .child(
            div()
                .flex_none()
                .when(is_guess(row), |score| score.opacity(GUESS_OPACITY))
                .child(segment_display(skill_digits(row.entry.theta), 32.0, cx)),
        )
        .child(meaning);

    // Every effort the agents here run it at, its Score and where θ comes from.
    let mut effort_rows = rail_properties();
    for entry in &row.efforts {
        let value = div()
            .flex()
            .items_baseline()
            .gap(px(10.0))
            .child(
                div()
                    .flex_none()
                    .font(mono(theme, FontWeight::MEDIUM))
                    .child(SharedString::from(
                        skill_digits(entry.theta).trim().to_string(),
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .font(mono(theme, FontWeight::NORMAL))
                    .text_size(px(12.0))
                    .text_color(theme.color_text_secondary)
                    .child(SharedString::from(format!(
                        "\u{03b8} {}",
                        theta_text(entry.theta)
                    ))),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(theme.color_text_muted)
                    .child(ellipsis(theta_source_label(
                        &entry.theta_source,
                        entry.score,
                    ))),
            )
            .into_any_element();
        effort_rows = effort_rows.child(rail_property(
            &effort_label(&entry.effort),
            value,
            false,
            cx,
        ));
    }
    let efforts = rail_group("Efforts", cx).child(effort_rows);

    let mut skill_rows = rail_properties()
        .child(rail_property(
            "Skill \u{03b8}",
            code(&detail.theta),
            false,
            cx,
        ))
        .child(rail_property("From", plain(&detail.source), false, cx));
    if let Some(bound) = &detail.bound {
        skill_rows = skill_rows.child(rail_property("Lower bound", code(bound), false, cx));
    }
    let skill = rail_group("Skill", cx)
        .min_w(px(0.0))
        .child(skill_rows)
        .child(prose(detail.how.clone(), cx));

    let mut band_rows = rail_properties();
    for tier in BANDS {
        let value = match band_of(&row.entry, tier) {
            Some(pass) => div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .child(band_value(pass, bar, cx))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.color_text_muted)
                        .child(SharedString::from(format!("at d {:.1}", pass.d))),
                )
                .into_any_element(),
            None => div()
                .text_color(theme.color_text_muted)
                .child("\u{2014}")
                .into_any_element(),
        };
        band_rows = band_rows.child(rail_property(&band_label(tier), value, false, cx));
    }
    let bands = rail_group("Pass chance", cx)
        .min_w(px(0.0))
        .child(band_rows)
        .child(prose(bar_note.to_string(), cx));

    let mut offers = rail_properties();
    for offer in &row.offers {
        offers = offers.child(offer_row(&offer.agent, &offer_name(offer), &offer.tier, cx));
    }
    let machine = rail_group("On this machine", cx)
        .child(prose(offered_line(row), cx))
        .child(offers)
        .child(prose(routing_note(row, discover), cx));

    let mut body = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(20.0))
        .pt(px(4.0))
        .child(summary)
        .child(efforts)
        .child(
            div()
                .w_full()
                .grid()
                .grid_cols(2)
                .gap(px(24.0))
                .child(skill)
                .child(bands),
        )
        .child(machine);
    if !detail.note.is_empty() {
        body = body.child(
            div()
                .w_full()
                .pt(px(12.0))
                .border_t_1()
                .border_color(theme.color_border_subtle)
                .child(
                    div()
                        .px(px(8.0))
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(theme.color_text_muted)
                        .child(wrap_text(detail.note.clone())),
                ),
        );
    }
    body.into_any_element()
}

/// One agent that offers the model, in `rail_property`'s metrics: its bot and name, the
/// model as that agent names it in mono, and its tier in muted text when it holds one.
fn offer_row(agent: &str, model: &str, tier: &str, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    div()
        .w_full()
        .min_w(px(0.0))
        .min_h(px(30.0))
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .child(div().flex_none().child(agent_avatar(agent, "", 20.0)))
        .child(
            div()
                .w(px(112.0))
                .flex_none()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .child(ellipsis(agent_label(agent))),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .font(mono(theme, FontWeight::NORMAL))
                .text_size(px(12.5))
                .text_color(theme.color_text_secondary)
                .child(ellipsis(model.to_string())),
        )
        .when(!tier.is_empty(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(theme.color_text_muted)
                    .child(SharedString::from(format!("{} tier", band_label(tier)))),
            )
        })
        .into_any_element()
}
