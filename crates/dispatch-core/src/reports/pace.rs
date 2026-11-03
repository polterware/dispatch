use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub type Stage = String;

pub const ON_TRACK: &str = "on_track";
pub const SLIGHTLY: &str = "slightly";
pub const AHEAD: &str = "ahead";
pub const FAR: &str = "far";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pace {
    pub expected: f64,
    pub actual: f64,
    pub delta: f64,
    pub stage: Stage,
    pub deficit: bool,
    pub will_last: bool,
    pub runs_out_in: Duration,
}

impl From<&Pace> for crate::router::Pace {
    fn from(p: &Pace) -> Self {
        Self {
            actual: p.actual,
            delta: p.delta,
            deficit: p.deficit,
            will_last: p.will_last,
        }
    }
}

impl From<Pace> for crate::router::Pace {
    fn from(p: Pace) -> Self {
        Self::from(&p)
    }
}

pub fn linear(
    used_percent: f64,
    window: Duration,
    resets_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Pace {
    let window_ns = window.as_nanos() as i128;
    let diff = resets_at - now;
    let mut remaining_ns = diff
        .num_nanoseconds()
        .map(i128::from)
        .unwrap_or_else(|| i128::from(diff.num_milliseconds()) * 1_000_000);
    if remaining_ns < 0 {
        remaining_ns = 0;
    }
    let mut elapsed_ns = window_ns - remaining_ns;
    let mut p = Pace {
        actual: used_percent,
        ..Default::default()
    };
    if window_ns <= 0 {
        p.will_last = true;
        return p;
    }
    if elapsed_ns < 0 {
        elapsed_ns = 0;
    }
    p.expected = (elapsed_ns as f64) / (window_ns as f64) * 100.0;
    p.delta = used_percent - p.expected;
    p.deficit = p.delta > 0.0;
    p.stage = pace_stage(p.delta).into();

    if used_percent <= 0.0 || elapsed_ns == 0 {
        p.will_last = true;
        return p;
    }
    let rate = used_percent / (elapsed_ns as f64);
    let mut eta_ns = ((100.0 - used_percent) / rate) as i128;
    if used_percent >= 100.0 || eta_ns < 0 {
        eta_ns = 0;
    }
    if eta_ns >= remaining_ns {
        p.will_last = true;
    } else {
        p.runs_out_in = Duration::from_nanos(u64::try_from(eta_ns).unwrap_or(u64::MAX));
    }
    p
}

fn pace_stage(delta: f64) -> &'static str {
    let d = delta.abs();
    if d <= 2.0 {
        ON_TRACK
    } else if d <= 6.0 {
        SLIGHTLY
    } else if d <= 12.0 {
        AHEAD
    } else {
        FAR
    }
}
