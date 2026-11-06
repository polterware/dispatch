//! Spend's cards: the window's totals as `StatTile`s and one `UsageCard` per agent Dispatch
//! ran, its API-equivalent cost over its tokens and runs, and the tokens' parts.

use gpui::{AnyElement, App, IntoElement, div, prelude::*, px};

use crate::avatar::agent_avatar;
use crate::models::spend::{API_EQUIVALENT, SpendRow, SpendTotals, format_cost, format_tokens};
use crate::theme::Theme;
use crate::ui::{Icon, IconTile, StatTile, TileTone, UsageCard};

const DASH: &str = "\u{2014}";
const AVATAR: f32 = 28.0;

/// A totals tile's mark: a 32px tile with a 16px icon in `tone`.
fn mark(icon: Icon, tone: TileTone) -> IconTile {
    IconTile::new(icon).size(32.0, 16.0).tone(tone)
}

/// Estimated cost, Tokens, Runs, and Per run, in four equal columns 8px apart.
pub fn totals_row(totals: &SpendTotals) -> AnyElement {
    let cost = totals.cost_usd.map(format_cost);
    let per_run = totals.per_run().map(format_cost);
    let priced = if totals.cost_runs < totals.runs {
        format!(
            " Sums the {} of {} runs that could be priced.",
            totals.cost_runs, totals.runs
        )
    } else {
        String::new()
    };
    div()
        .w_full()
        .grid()
        .grid_cols(4)
        .gap(px(8.0))
        .child(
            StatTile::new(
                "spend-total-cost",
                "Estimated cost",
                cost.unwrap_or_else(|| DASH.to_string()),
            )
            .leading(mark(Icon::CreditCard, TileTone::Infrastructure))
            .tooltip(format!("{API_EQUIVALENT}{priced}")),
        )
        .child(
            StatTile::new("spend-total-tokens", "Tokens", format_tokens(totals.tokens))
                .leading(mark(Icon::Layers, TileTone::Data)),
        )
        .child({
            let mut tile = StatTile::new("spend-total-runs", "Runs", totals.runs.to_string())
                .leading(mark(Icon::Play, TileTone::Execution));
            let mut notes = Vec::new();
            if totals.live_runs > 0 {
                notes.push(format!("{} working", totals.live_runs));
            }
            if totals.unreported_runs > 0 {
                notes.push(format!("{} without usage", totals.unreported_runs));
            }
            if !notes.is_empty() {
                tile = tile.caption(notes.join(" \u{00b7} ")).tooltip(
                    "A run without usage ended before its agent reported any, so its tokens and cost are unknown rather than zero. A working run counts what it has used so far.",
                );
            }
            tile
        })
        .child(
            StatTile::new(
                "spend-total-per-run",
                "Per run",
                per_run.unwrap_or_else(|| DASH.to_string()),
            )
            .leading(mark(Icon::Scale, TileTone::Observability))
            .tooltip("The estimated cost of a priced run, on average."),
        )
        .into_any_element()
}

/// The agent's card: its bot, name, and provider; the window's cost; its tokens and runs; the
/// tokens split into input, cache, and output.
pub fn agent_card(row: &SpendRow, cx: &App) -> AnyElement {
    let theme = *Theme::global(cx);
    let runs = if row.runs == 1 {
        "1 run".to_string()
    } else {
        format!("{} runs", row.runs)
    };
    let mut caption = match row.tokens.as_ref() {
        Some(tokens) => format!("{} tokens \u{00b7} {runs}", tokens.label),
        None => format!("No usage reported \u{00b7} {runs}"),
    };
    if row.live_runs > 0 {
        caption.push_str(&format!(" \u{00b7} {} working", row.live_runs));
    }
    let (figure, figure_tip) = match row.cost.as_ref() {
        Some(cost) => (cost.label.clone(), cost.title.clone()),
        None => (
            "Unpriced".to_string(),
            "No price is known for these runs' models.".to_string(),
        ),
    };
    let mut card = UsageCard::new(
        format!("spend-card-{}", row.agent),
        row.agent.clone(),
        figure,
    )
    .leading(agent_avatar(&row.agent, "", AVATAR))
    .aside(row.provider.clone())
    .figure_tooltip(figure_tip)
    .caption(caption);
    if let Some(tokens) = row.tokens.as_ref() {
        card = card.caption_tooltip(tokens.title.clone());
    }
    if let Some(counts) = row.counts.as_ref() {
        let cache = counts.cache_read.saturating_add(counts.cache_write);
        // Three hues far apart, since the bar's face softens each into the material.
        for (count, color, name) in [
            (counts.input, theme.chart_blue, "Input"),
            (cache, theme.chart_green, "Cache"),
            (counts.output, theme.chart_amber, "Output"),
        ] {
            card = card.part(
                count.max(0) as f32,
                color,
                format!("{name} {}", format_tokens(count.max(0))),
            );
        }
    }
    card.into_any_element()
}
