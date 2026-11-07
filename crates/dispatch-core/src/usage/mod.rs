//! What a conversation used: the Dispatch runs it started, by agent and model, and the
//! calling agent's own session, read from that agent's local transcript.
//!
//! The runs' figures are the stored `usage_json` the run page's Run group and `dispatch
//! usage` read. The caller's figures are measured from one session only, never from its
//! prompts or message text: [`caller`] keeps numbers and model names.

pub mod caller;

use crate::{
    catalog::Catalog,
    types::{Run, Usage},
};

/// One agent and model's share of a conversation: its runs (none for the caller's own
/// session), its tokens, and its API-equivalent cost when a price covers it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slice {
    pub agent: String,
    pub model: String,
    pub runs: i64,
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_partial: bool,
}

impl Slice {
    pub fn tokens(&self) -> i64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens + self.output_tokens
    }

    /// Adds `other`'s figures to this slice. Tokens that no price covered mark a priced
    /// slice partial, as the Spend page marks a run's cost that leaves an attempt out.
    pub fn add(&mut self, other: &Slice) {
        let unpriced_tokens = self.cost_usd.is_none() && self.tokens() > 0;
        self.runs += other.runs;
        self.input_tokens += other.input_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        match (self.cost_usd.as_mut(), other.cost_usd) {
            (Some(total), Some(cost)) => {
                *total += cost;
            }
            (None, Some(cost)) => {
                self.cost_usd = Some(cost);
                // Tokens already counted that no price covered.
                self.cost_partial |= unpriced_tokens;
            }
            (Some(_), None) => self.cost_partial |= other.tokens() > 0,
            (None, None) => {}
        }
        self.cost_partial |= other.cost_partial && self.cost_usd.is_some();
    }
}

/// The conversation's runs, grouped by agent and model, the use of each first.
pub fn from_runs(runs: &[Run]) -> Vec<Slice> {
    let slices = runs
        .iter()
        .map(|run| {
            let usage = run.usage.clone().unwrap_or_default();
            slice_of(&run.agent, &run.model, 1, &usage)
        })
        .collect();
    sort_and_merge(slices)
}

/// Groups `slices` by agent and model, summing each group, then sorts them.
pub(crate) fn sort_and_merge(slices: Vec<Slice>) -> Vec<Slice> {
    let mut out: Vec<Slice> = Vec::new();
    for slice in slices {
        match out
            .iter_mut()
            .find(|s| s.agent == slice.agent && s.model == slice.model)
        {
            Some(existing) => existing.add(&slice),
            None => out.push(slice),
        }
    }
    sort(&mut out);
    out
}

/// One slice from a run's or a session's usage.
pub fn slice_of(agent: &str, model: &str, runs: i64, usage: &Usage) -> Slice {
    Slice {
        agent: agent.to_string(),
        model: model.to_string(),
        runs,
        input_tokens: usage.input_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cost_usd: usage.cost_usd,
        cost_partial: usage.cost_partial,
    }
}

/// The totals over `slices`, with no agent or model of their own.
pub fn total(slices: &[Slice]) -> Slice {
    let mut out = Slice::default();
    for slice in slices {
        out.add(slice);
    }
    out
}

/// The sum of two totals.
pub fn plus(a: &Slice, b: &Slice) -> Slice {
    let mut out = a.clone();
    out.add(b);
    out
}

/// Dearest first, then by name.
pub fn sort(slices: &mut [Slice]) {
    slices.sort_by(|a, b| {
        b.tokens()
            .cmp(&a.tokens())
            .then_with(|| a.agent.cmp(&b.agent))
            .then_with(|| a.model.cmp(&b.model))
    });
}

/// The caller's own session and whether it could be measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallerUsage {
    pub agent: String,
    pub session: String,
    pub measured: bool,
    pub note: String,
    pub slices: Vec<Slice>,
}

/// Measures the calling agent's own session inside `conversation_id`, for the agents whose
/// local transcript this build reads. An agent it cannot read says so rather than guessing.
pub fn caller_usage(conversation_id: &str, catalog: &Catalog) -> CallerUsage {
    caller::measure(conversation_id, catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: i64, output: i64, cost: Option<f64>, partial: bool) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
            cost_partial: partial,
            ..Usage::default()
        }
    }

    #[test]
    fn runs_group_by_agent_and_model() {
        let run = |agent: &str, model: &str, u: Option<Usage>| Run {
            agent: agent.into(),
            model: model.into(),
            usage: u,
            ..Run::default()
        };
        let runs = vec![
            run("claude", "opus", Some(usage(10, 1, Some(0.5), false))),
            run("claude", "opus", Some(usage(20, 2, Some(0.25), false))),
            run("codex", "gpt", Some(usage(5, 1, None, false))),
            run("codex", "gpt", None),
        ];
        let slices = from_runs(&runs);
        assert_eq!(slices.len(), 2);
        let claude = slices.iter().find(|s| s.agent == "claude").unwrap();
        assert_eq!((claude.runs, claude.input_tokens), (2, 30));
        assert_eq!(claude.cost_usd, Some(0.75));
        assert!(!claude.cost_partial);
        let codex = slices.iter().find(|s| s.agent == "codex").unwrap();
        assert_eq!((codex.runs, codex.input_tokens), (2, 5));
        assert_eq!(codex.cost_usd, None);
        // Codex has more runs but fewer tokens, so Claude leads.
        assert_eq!(slices[0].agent, "claude");
    }

    #[test]
    fn a_priced_slice_with_unpriced_tokens_is_partial() {
        let mut slice = slice_of("a", "m", 1, &usage(10, 1, None, false));
        slice.add(&slice_of("a", "m", 1, &usage(5, 1, Some(0.2), true)));
        assert_eq!(slice.cost_usd, Some(0.2));
        assert!(slice.cost_partial);
        assert_eq!(total(&[slice.clone()]).tokens(), slice.tokens());
    }
}
