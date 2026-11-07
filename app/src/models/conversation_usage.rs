//! The conversation Usage dialog's figures: one row per agent and model, with its runs,
//! its tokens split input, cache, and output, and its API-equivalent cost. The rows are the
//! same numbers the Spend page and `dispatch usage` print.

use crate::client::DispatchUsageSlice;
use crate::format::agent_display_name;
use crate::models::spend::{
    API_EQUIVALENT, TokenCounts, format_cost, format_tokens, token_breakdown,
};

/// The cost note under the grand total.
pub const USAGE_NOTE: &str = API_EQUIVALENT;

/// One row of the dialog: its agent and model, and each cell as it reads.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageRow {
    pub agent: String,
    pub model: String,
    pub runs: i32,
    pub input: String,
    pub cache: String,
    pub output: String,
    pub cost: String,
    pub cost_title: String,
    pub tokens_title: String,
    /// Every token in the row, cache included.
    pub tokens: i64,
}

impl UsageRow {
    /// The agent as people read it, with its bot.
    pub fn agent_label(&self) -> String {
        agent_display_name(&self.agent)
    }

    pub fn runs_label(&self) -> String {
        match self.runs {
            0 => "\u{2014}".to_string(),
            1 => "1 run".to_string(),
            n => format!("{n} runs"),
        }
    }
}

/// The rows of a section, dearest first as the core sends them.
pub fn rows(slices: &[DispatchUsageSlice]) -> Vec<UsageRow> {
    slices.iter().map(row).collect()
}

pub fn row(slice: &DispatchUsageSlice) -> UsageRow {
    let tokens = tokens_of(slice);
    let cache = tokens.cache_read + tokens.cache_write;
    let total = tokens.input + cache + tokens.output;
    UsageRow {
        agent: slice.agent.clone(),
        model: if slice.model.trim().is_empty() {
            "\u{2014}".to_string()
        } else {
            slice.model.clone()
        },
        runs: slice.runs,
        input: format_tokens(tokens.input),
        cache: format_tokens(cache),
        output: format_tokens(tokens.output),
        cost: match slice.cost_usd {
            Some(cost) => format_cost(cost),
            None => "Unknown".to_string(),
        },
        cost_title: cost_title(slice, total),
        tokens_title: token_breakdown(&tokens),
        tokens: total,
    }
}

/// The totals row, with no agent of its own.
pub fn total_row(slice: Option<&DispatchUsageSlice>) -> Option<UsageRow> {
    slice.map(row)
}

fn tokens_of(slice: &DispatchUsageSlice) -> TokenCounts {
    TokenCounts {
        input: slice.input_tokens.0,
        cache_read: slice.cache_read_tokens.0,
        cache_write: slice.cache_write_tokens.0,
        output: slice.output_tokens.0,
        reasoning: slice.reasoning_tokens.0,
    }
}

/// Whether any slice has tokens but no price, or a priced slice marked partial: the total's
/// cost is then a floor.
pub fn partial(slices: &[DispatchUsageSlice]) -> bool {
    slices.iter().any(|slice| slice.cost_partial)
}

fn cost_title(slice: &DispatchUsageSlice, total: i64) -> String {
    match slice.cost_usd {
        None => "No price is known for this model.".to_string(),
        Some(_) => {
            let mut title = USAGE_NOTE.to_string();
            if slice.cost_partial {
                title.push_str(" Some tokens had no price, so this cost is a floor.");
            }
            if total == 0 {
                title.push_str(" No usage was recorded.");
            }
            title
        }
    }
}

/// The caller's own session, or why it could not be measured.
pub fn caller_line(agent: &str, session: &str) -> String {
    let agent = if agent.trim().is_empty() {
        "Unknown agent".to_string()
    } else {
        agent_display_name(agent)
    };
    let session = session.trim();
    if session.is_empty() {
        agent
    } else {
        format!("{agent} \u{00b7} {session}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ProtoI64;

    fn slice(cost: Option<f64>, partial: bool) -> DispatchUsageSlice {
        DispatchUsageSlice {
            agent: "claude".into(),
            model: "opus".into(),
            runs: 2,
            input_tokens: ProtoI64(1200),
            cache_read_tokens: ProtoI64(300),
            cache_write_tokens: ProtoI64(0),
            output_tokens: ProtoI64(40),
            cost_usd: cost,
            cost_partial: partial,
            ..DispatchUsageSlice::default()
        }
    }

    #[test]
    fn a_row_splits_its_tokens_and_names_the_cost() {
        let row = row(&slice(Some(0.42), false));
        assert_eq!((row.input.as_str(), row.cache.as_str()), ("1.2k", "300"));
        assert_eq!(row.runs_label(), "2 runs");
        assert_eq!(row.cost, "$0.42");
        assert!(row.tokens_title.contains("Cache read 300"));
        assert!(row.cost_title.contains("API-equivalent"));
        assert!(!row.cost_title.contains("floor"));
    }

    #[test]
    fn an_unpriced_row_is_unknown_and_a_partial_one_says_so() {
        assert_eq!(row(&slice(None, false)).cost, "Unknown");
        assert!(row(&slice(Some(0.42), true)).cost_title.contains("floor"));
        assert!(!partial(&[slice(Some(0.42), false)]));
        assert!(partial(&[slice(Some(0.42), true)]));
    }
}
