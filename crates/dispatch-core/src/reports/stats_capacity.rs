use super::{
    pace::linear,
    pace_history::{Snapshot, is_zero_utc, parse_time_or_zero, truncate_hour},
};
use crate::{
    Error,
    types::{QuotaSnapshot, Store},
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::time::Duration;

pub const CAPACITY_SCHEMA: &str = "dispatch.capacity.v1";
const FULL_PERCENT: f64 = 99.5;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CapacityReport {
    pub schema: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub since: String,
    pub note: String,
    pub keys: Vec<CapacityKey>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CapacityKey {
    pub key: String,
    pub windows: usize,
    pub hit_full: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumed_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leftover_pct: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub leftover: String,
    pub leftover_known: usize,
    pub leftover_unknown: usize,
    pub pace: String,
    #[serde(rename = "windowsDetail")]
    pub detail: Vec<WindowStat>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowStat {
    pub resets_at: String,
    pub open: bool,
    pub consumed_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leftover_pct: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub leftover: String,
    pub hit_full: bool,
    pub pace: String,
}

struct WinBucket {
    key: String,
    reset: DateTime<Utc>,
    window: Duration,
    snaps: Vec<Snapshot>,
}

pub fn capacity_from_store(
    store: Option<&dyn Store>,
    since: Option<DateTime<Utc>>,
    now: Option<DateTime<Utc>>,
) -> Result<CapacityReport, Error> {
    let Some(s) = store else {
        return Ok(capacity(&[], since, now));
    };
    let since_str = since
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default();
    let mut snaps = s.list_quota("", &since_str)?;
    if snaps.is_empty() {
        let mut keys: BTreeSet<String> = [
            "codex",
            "antigravity",
            "antigravity:claude",
            "antigravity:gemini-pro",
            "antigravity:gemini-flash",
            "cursor",
            "grok",
            "opencode",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        if let Ok(agents) = s.list_agents() {
            for a in agents {
                if !a.name.is_empty() {
                    keys.insert(a.name);
                }
            }
        }
        for k in keys {
            let ks = s.list_quota(&k, &since_str)?;
            snaps.extend(ks);
        }
    }
    Ok(capacity(&snaps, since, now))
}

pub fn capacity(
    snaps: &[QuotaSnapshot],
    since: Option<DateTime<Utc>>,
    now: Option<DateTime<Utc>>,
) -> CapacityReport {
    let now = now.unwrap_or_else(Utc::now);
    let base_note = "agents with no snapshots have unknown capacity; percentages are of each agent's window, not of tokens";
    let mut rep = CapacityReport {
        schema: CAPACITY_SCHEMA.into(),
        since: since
            .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
            .unwrap_or_default(),
        note: base_note.into(),
        keys: Vec::new(),
    };
    if snaps.is_empty() {
        rep.note = format!("unknown: no snapshots; {base_note}");
        return rep;
    }

    let mut groups: HashMap<String, WinBucket> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for s in snaps {
        let Some(used_percent) = s.used_percent else {
            continue;
        };
        if s.agent.is_empty() {
            continue;
        }
        let raw_reset = parse_time_or_zero(&s.resets_at);
        let reset = if is_zero_utc(raw_reset) {
            raw_reset
        } else {
            truncate_hour(raw_reset)
        };
        let reset_label = if is_zero_utc(reset) {
            "0001-01-01T00:00:00Z".to_owned()
        } else {
            reset.to_rfc3339_opts(SecondsFormat::Secs, true)
        };
        let id = format!("{}|{}", s.agent, reset_label);
        if !groups.contains_key(&id) {
            order.push(id.clone());
            groups.insert(
                id.clone(),
                WinBucket {
                    key: s.agent.clone(),
                    reset,
                    window: Duration::ZERO,
                    snaps: Vec::new(),
                },
            );
        }
        if let Some(b) = groups.get_mut(&id) {
            if s.window_minutes > 0 {
                b.window = Duration::from_secs((s.window_minutes as u64) * 60);
            }
            b.snaps.push(Snapshot {
                used_percent,
                resets_at: raw_reset,
                window_minutes: s.window_minutes,
                sampled_at: parse_time_or_zero(&s.sampled_at),
            });
        }
    }

    if order.is_empty() {
        rep.note = format!("unknown: no snapshots; {base_note}");
        return rep;
    }

    let mut by_key: BTreeMap<String, Vec<WindowStat>> = BTreeMap::new();
    let mut latest_at: HashMap<String, DateTime<Utc>> = HashMap::new();
    let mut latest_pace: HashMap<String, String> = HashMap::new();

    for id in order {
        let Some(b) = groups.get(&id) else {
            continue;
        };
        if !window_in_since(b, since) {
            continue;
        }
        let st = window_stat(b, now);
        let last = latest_sample(&b.snaps);
        let update_latest = match latest_at.get(&b.key) {
            None => true,
            Some(&prev) => is_zero_utc(prev) || last.sampled_at > prev,
        };
        if update_latest {
            latest_at.insert(b.key.clone(), last.sampled_at);
            latest_pace.insert(b.key.clone(), st.pace.clone());
        }
        by_key.entry(b.key.clone()).or_default().push(st);
    }

    for (k, mut ws) in by_key {
        ws.sort_by(|a, b| a.resets_at.cmp(&b.resets_at));
        let mut pace = latest_pace.remove(&k).unwrap_or_default();
        if pace.is_empty() {
            pace = "unknown".into();
        }
        let mut hit_full = 0usize;
        let mut leftover_known = 0usize;
        let mut leftover_unknown = 0usize;
        let mut consumed = Vec::with_capacity(ws.len());
        let mut leftover = Vec::new();
        for w in &ws {
            consumed.push(w.consumed_pct);
            if w.hit_full {
                hit_full += 1;
            }
            if let Some(lp) = w.leftover_pct {
                leftover.push(lp);
                leftover_known += 1;
            } else {
                leftover_unknown += 1;
            }
        }
        let consumed_pct = (!consumed.is_empty()).then(|| round1(mean_floats(&consumed)));
        let (leftover_pct, leftover_str) = if !leftover.is_empty() {
            (Some(round1(mean_floats(&leftover))), String::new())
        } else {
            (None, "unknown".into())
        };
        rep.keys.push(CapacityKey {
            key: k,
            windows: ws.len(),
            hit_full,
            consumed_pct,
            leftover_pct,
            leftover: leftover_str,
            leftover_known,
            leftover_unknown,
            pace,
            detail: ws,
        });
    }
    rep
}

fn window_in_since(b: &WinBucket, since: Option<DateTime<Utc>>) -> bool {
    let Some(since) = since else {
        return true;
    };
    if !is_zero_utc(b.reset) && b.reset >= since {
        return true;
    }
    b.snaps
        .iter()
        .any(|s| !is_zero_utc(s.sampled_at) && s.sampled_at >= since)
}

fn window_stat(b: &WinBucket, now: DateTime<Utc>) -> WindowStat {
    let last = latest_sample(&b.snaps);
    let peak = b
        .snaps
        .iter()
        .fold(b.snaps[0].used_percent, |acc, s| acc.max(s.used_percent));
    let reset = if is_zero_utc(last.resets_at) {
        b.reset
    } else {
        last.resets_at
    };
    let resets_at = if is_zero_utc(b.reset) {
        "0001-01-01T00:00:00Z".into()
    } else {
        b.reset.to_rfc3339_opts(SecondsFormat::Secs, true)
    };
    let (leftover_pct, leftover) = match leftover_at_reset(reset, b.window, &last, now) {
        Some(left) => (Some(round1(left)), String::new()),
        None => (None, "unknown".into()),
    };
    WindowStat {
        resets_at,
        open: !is_zero_utc(reset) && reset > now,
        consumed_pct: round1(last.used_percent),
        leftover_pct,
        leftover,
        hit_full: peak >= FULL_PERCENT,
        pace: pace_label(last.used_percent, b.window, reset, last.sampled_at),
    }
}

fn leftover_at_reset(
    reset: DateTime<Utc>,
    window: Duration,
    last: &Snapshot,
    now: DateTime<Utc>,
) -> Option<f64> {
    if (!is_zero_utc(reset) && reset > now) || is_zero_utc(last.sampled_at) {
        return None;
    }
    if !window.is_zero() && !is_zero_utc(reset) {
        let gap_ms = (reset - last.sampled_at).num_milliseconds();
        let quarter_ms = (window.as_millis() / 4) as i64;
        if gap_ms > quarter_ms {
            return None;
        }
    }
    Some((100.0 - last.used_percent).max(0.0))
}

fn latest_sample(snaps: &[Snapshot]) -> Snapshot {
    let mut best = snaps[0].clone();
    for s in &snaps[1..] {
        if s.sampled_at > best.sampled_at {
            best = s.clone();
        }
    }
    best
}

fn pace_label(used: f64, window: Duration, resets_at: DateTime<Utc>, at: DateTime<Utc>) -> String {
    if window.is_zero() || is_zero_utc(at) || is_zero_utc(resets_at) {
        return "unknown".into();
    }
    let p = linear(used, window, resets_at, at);
    if p.delta > 2.0 {
        "ahead".into()
    } else if p.delta < -2.0 {
        "behind".into()
    } else {
        "on pace".into()
    }
}

fn mean_floats(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / (xs.len() as f64)
}

fn round1(f: f64) -> f64 {
    (f * 10.0).round() / 10.0
}

impl CapacityReport {
    pub fn text(&self) -> String {
        let mut b = String::new();
        let _ = writeln!(b, "capacity  {}", self.note);
        if self.keys.is_empty() {
            b.push_str("no windows\n");
            return b;
        }
        for k in &self.keys {
            let left = k
                .leftover_pct
                .map_or_else(|| "unknown".into(), |v| format!("{v:.1}%"));
            let cons = k
                .consumed_pct
                .map_or_else(|| "unknown".into(), |v| format!("{v:.1}%"));
            let _ = writeln!(
                b,
                "{}  windows={}  hit100={}  consumed={}  leftover={}  unknownLeftover={}  pace={}",
                k.key, k.windows, k.hit_full, cons, left, k.leftover_unknown, k.pace
            );
            for w in &k.detail {
                let wl = w
                    .leftover_pct
                    .map_or_else(|| "unknown".into(), |v| format!("{v:.1}%"));
                let _ = writeln!(
                    b,
                    "  reset {}  consumed={:.1}%  leftover={}  hit100={}  pace={}  open={}",
                    w.resets_at, w.consumed_pct, wl, w.hit_full, w.pace, w.open
                );
            }
        }
        b
    }
}
