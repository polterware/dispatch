use crate::types::QuotaSnapshot;
use chrono::{DateTime, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

const WEEK_MINUTES: i64 = 7 * 24 * 60;
const GRID_POINTS: usize = 169;
const MIN_SAMPLES: usize = 6;
const MIN_WEEKS: usize = 5;
const AGE_HALF_LIFE: f64 = 3.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
    pub window_minutes: i64,
    pub sampled_at: DateTime<Utc>,
}

pub(crate) fn zero_utc() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
        .single()
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

pub(crate) fn is_zero_utc(t: DateTime<Utc>) -> bool {
    t == zero_utc()
}

pub(crate) fn parse_time_or_zero(s: &str) -> DateTime<Utc> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed == "0001-01-01T00:00:00Z" {
        return zero_utc();
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| zero_utc())
}

pub(crate) fn truncate_hour(t: DateTime<Utc>) -> DateTime<Utc> {
    t.with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t)
}

pub(crate) fn round_hour(t: DateTime<Utc>) -> DateTime<Utc> {
    let truncated = truncate_hour(t);
    let nanos_in_hour = u64::from(t.minute()) * 60 * 1_000_000_000
        + u64::from(t.second()) * 1_000_000_000
        + u64::from(t.timestamp_subsec_nanos());
    if nanos_in_hour >= 30 * 60 * 1_000_000_000 {
        truncated + chrono::Duration::hours(1)
    } else {
        truncated
    }
}

pub fn snapshots_from_quota(snaps: &[QuotaSnapshot]) -> Vec<Snapshot> {
    let mut out = Vec::new();
    for s in snaps {
        let Some(used_percent) = s.used_percent else {
            continue;
        };
        out.push(Snapshot {
            used_percent,
            resets_at: parse_time_or_zero(&s.resets_at),
            window_minutes: s.window_minutes,
            sampled_at: parse_time_or_zero(&s.sampled_at),
        });
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub valid: bool,
    pub weeks: usize,
    pub risk: f64,
    pub runs_out_in: Duration,
    pub expected: f64,
}

impl From<&History> for crate::router::History {
    fn from(h: &History) -> Self {
        Self {
            valid: h.valid,
            risk: h.risk,
        }
    }
}

impl From<History> for crate::router::History {
    fn from(h: History) -> Self {
        Self::from(&h)
    }
}

struct WeekCurve {
    curve: [f64; GRID_POINTS],
    w: f64,
}

pub fn historical(
    snaps: &[Snapshot],
    used_percent: f64,
    resets_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> History {
    let curves = build_curves(snaps, resets_at, now);
    let mut h = History {
        weeks: curves.len(),
        ..Default::default()
    };
    if curves.len() < MIN_WEEKS {
        return h;
    }
    let start = resets_at - chrono::Duration::weeks(1);
    let mut elapsed_secs = (now - start).num_milliseconds() as f64 / 1000.0;
    if elapsed_secs < 0.0 {
        elapsed_secs = 0.0;
    }
    let mut idx = (elapsed_secs / 3600.0).round() as usize;
    if idx >= GRID_POINTS {
        idx = GRID_POINTS - 1;
    }

    let mut sw = 0.0;
    let mut sw2 = 0.0;
    for c in &curves {
        sw += c.w;
        sw2 += c.w * c.w;
    }
    let n_eff = sw * sw / sw2;
    let lambda = clamp_nan((n_eff - 2.0) / 6.0, 0.0, 1.0);
    let linear = (idx as f64) / ((GRID_POINTS - 1) as f64) * 100.0;
    let vals: Vec<f64> = curves.iter().map(|c| c.curve[idx]).collect();
    let weights: Vec<f64> = curves.iter().map(|c| c.w).collect();
    let med = weighted_median(&vals, &weights);
    h.expected = (lambda * med + (1.0 - lambda) * linear).min(linear);

    let mut cross_mass = 0.0;
    let mut cross_hours = Vec::new();
    let mut cross_w = Vec::new();
    for c in &curves {
        let shift = used_percent - c.curve[idx];
        for j in idx..GRID_POINTS {
            if c.curve[j] + shift >= 100.0 {
                cross_mass += c.w;
                cross_hours.push((j - idx) as f64);
                cross_w.push(c.w);
                break;
            }
        }
    }
    h.valid = true;
    h.risk = (cross_mass + 0.5) / (sw + 1.0);
    if !cross_hours.is_empty() {
        let hours = weighted_median(&cross_hours, &cross_w);
        let nanos = (hours * 3_600_000_000_000.0).max(0.0) as u64;
        h.runs_out_in = Duration::from_nanos(nanos);
    }
    h
}

fn build_curves(
    snaps: &[Snapshot],
    current_reset: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Vec<WeekCurve> {
    let mut by_week: BTreeMap<i64, Vec<Snapshot>> = BTreeMap::new();
    for s in snaps {
        if s.window_minutes != WEEK_MINUTES || is_zero_utc(s.resets_at) || is_zero_utc(s.sampled_at)
        {
            continue;
        }
        let key = round_hour(s.resets_at).timestamp();
        by_week.entry(key).or_default().push(s.clone());
    }
    let cur_key = round_hour(current_reset).timestamp();
    let mut out = Vec::new();
    for (key, mut ss) in by_week {
        if key == cur_key {
            continue;
        }
        let Some(end) = DateTime::from_timestamp(key, 0) else {
            continue;
        };
        if end > now || ss.len() < MIN_SAMPLES {
            continue;
        }
        let start = end - chrono::Duration::weeks(1);
        ss.sort_by_key(|s| s.sampled_at);
        let first = ss[0].sampled_at;
        let last = ss[ss.len() - 1].sampled_at;
        if first > start + chrono::Duration::hours(24) || last < end - chrono::Duration::hours(24) {
            continue;
        }
        let mut curve = [0.0; GRID_POINTS];
        let mut si = 0usize;
        let mut cur = 0.0f64;
        let mut peak = 0.0f64;
        for (g, slot) in curve.iter_mut().enumerate() {
            let t = start + chrono::Duration::hours(g as i64);
            while si < ss.len() && ss[si].sampled_at <= t {
                cur = ss[si].used_percent;
                si += 1;
            }
            if g == 0 && si == 0 {
                cur = ss[0].used_percent;
            }
            peak = peak.max(cur);
            *slot = peak;
        }
        let age_hours = (now - end).num_milliseconds() as f64 / 3_600_000.0;
        let age = age_hours / (7.0 * 24.0);
        let w = (-age / AGE_HALF_LIFE).exp();
        out.push(WeekCurve { curve, w });
    }
    out
}

fn weighted_median(vals: &[f64], weights: &[f64]) -> f64 {
    let mut xs: Vec<(f64, f64)> = vals.iter().copied().zip(weights.iter().copied()).collect();
    let total: f64 = weights.iter().sum();
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut acc = 0.0;
    for (v, w) in xs {
        acc += w;
        if acc >= total / 2.0 {
            return v;
        }
    }
    0.0
}

fn clamp_nan(x: f64, lo: f64, hi: f64) -> f64 {
    if x.is_nan() {
        if lo <= 0.0 && 0.0 <= hi {
            return 0.0;
        }
        return lo;
    }
    x.clamp(lo, hi)
}
