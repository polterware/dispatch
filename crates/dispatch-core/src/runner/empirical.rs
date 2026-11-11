use super::{parse_time, store_cooldowns, summary::RATE_LIMITED};
use crate::{
    adapters::quota::{QuotaSample, empirical_sample},
    types::Store,
};
use chrono::{DateTime, TimeDelta, Utc};

const EMPIRICAL_WINDOW: TimeDelta = TimeDelta::hours(24);

/// The risk sample for an agent with no official quota. Used is always None.
pub fn empirical_quota(
    st: Option<&dyn Store>,
    agent: &str,
    now: Option<DateTime<Utc>>,
) -> QuotaSample {
    let now = now.unwrap_or_else(Utc::now);
    let until = store_cooldowns(st, now).get(agent).copied();
    let mut n = 0;
    if let Some(recs) = st.and_then(|st| st.list_runs("", 0, 0, "").ok()) {
        let cutoff = now - EMPIRICAL_WINDOW;
        let after = |t: &str| parse_time(t).is_some_and(|t| t > cutoff);
        for r in recs {
            if !after(&r.finished_at) && !after(&r.created_at) {
                continue;
            }
            if r.agent == agent && RATE_LIMITED.is_match(&r.error) {
                n += 1;
            }
            n += r
                .attempts
                .iter()
                .filter(|at| {
                    at.agent == agent
                        && at.exit_class != "near_limit"
                        && (at.exit_class == "rate_limit" || RATE_LIMITED.is_match(&at.error))
                })
                .count();
        }
    }
    empirical_sample(now, until, n)
}
