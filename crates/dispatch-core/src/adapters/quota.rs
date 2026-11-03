use crate::Error;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

pub const SOURCE_CODEX_APP_SERVER: &str = "codex app-server";
pub const SOURCE_AGY_USAGE: &str = "agy /usage";
pub const SOURCE_CLAUDE_STREAM: &str = "claude stream-json";
pub const SOURCE_EMPIRICAL: &str = "empirical";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    pub used_percent: f64,
    pub window_minutes: i64,
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub window: Duration,
    #[serde(skip)]
    pub sampled_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSample {
    pub window: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "duration_zero",
        with = "duration_format"
    )]
    pub duration: Duration,
    pub source: String,
    pub confidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub risk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampled_at: Option<DateTime<Utc>>,
}
fn duration_zero(d: &Duration) -> bool {
    d.is_zero()
}
mod duration_format {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;
    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        let secs = d.as_secs();
        let text = if secs >= 3600 {
            format!("{}h{}m{}s", secs / 3600, secs % 3600 / 60, secs % 60)
        } else if secs >= 60 {
            format!("{}m{}s", secs / 60, secs % 60)
        } else {
            format!("{secs}s")
        };
        s.serialize_str(&text)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let text = String::deserialize(d)?;
        let mut total = 0u64;
        let mut digits = String::new();
        for c in text.chars() {
            if c.is_ascii_digit() {
                digits.push(c);
                continue;
            }
            let n: u64 = digits.parse().map_err(serde::de::Error::custom)?;
            digits.clear();
            total = total
                .checked_add(
                    n.checked_mul(match c {
                        'h' => 3600,
                        'm' => 60,
                        's' => 1,
                        _ => return Err(serde::de::Error::custom("invalid duration")),
                    })
                    .ok_or_else(|| serde::de::Error::custom("duration overflow"))?,
                )
                .ok_or_else(|| serde::de::Error::custom("duration overflow"))?;
        }
        if !digits.is_empty() || text.is_empty() {
            return Err(serde::de::Error::custom("invalid duration"));
        }
        Ok(Duration::from_secs(total))
    }
}
pub fn empirical_sample(
    now: DateTime<Utc>,
    cooled_until: Option<DateTime<Utc>>,
    recent_limits: usize,
) -> QuotaSample {
    let (risk, reset_at) = if cooled_until.is_some_and(|t| t > now) {
        ("cooldown", cooled_until)
    } else if recent_limits > 0 {
        ("recent_rate_limit", None)
    } else {
        ("unknown", None)
    };
    QuotaSample {
        window: "empirical".into(),
        source: SOURCE_EMPIRICAL.into(),
        confidence: "empirical".into(),
        risk: risk.into(),
        reset_at,
        ..QuotaSample::default()
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWindow {
    pub used_percent: f64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}
pub fn codex_quota(
    primary: Option<&CodexWindow>,
    secondary: Option<&CodexWindow>,
) -> Result<Quota, Error> {
    let w = secondary.or(primary).ok_or_else(no_quota)?;
    let mins = w.window_duration_mins.ok_or_else(no_quota)?;
    let reset = w
        .resets_at
        .and_then(|s| DateTime::from_timestamp(s, 0))
        .ok_or_else(no_quota)?;
    Ok(Quota {
        key: "codex".into(),
        used_percent: w.used_percent,
        window_minutes: mins,
        resets_at: Some(reset),
        ..Quota::default()
    })
}
pub fn codex_samples(
    primary: Option<&CodexWindow>,
    secondary: Option<&CodexWindow>,
) -> Result<Vec<QuotaSample>, Error> {
    let mut out = Vec::new();
    for (name, w) in [("primary", primary), ("secondary", secondary)] {
        if let Some((w, mins, reset)) = w.and_then(|w| {
            Some((
                w,
                w.window_duration_mins?,
                DateTime::from_timestamp(w.resets_at?, 0)?,
            ))
        }) {
            out.push(QuotaSample {
                window: name.into(),
                used: Some(w.used_percent),
                reset_at: Some(reset),
                duration: Duration::from_secs(mins.max(0) as u64 * 60),
                source: SOURCE_CODEX_APP_SERVER.into(),
                confidence: "official".into(),
                ..QuotaSample::default()
            });
        }
    }
    if out.is_empty() {
        Err(no_quota())
    } else {
        Ok(out)
    }
}
fn no_quota() -> Error {
    Error::Message("quota unavailable".into())
}
fn agy_samples(raw: &[u8]) -> Result<Vec<QuotaSample>, Error> {
    let env: Value = serde_json::from_slice(raw)?;
    let mut out = Vec::new();
    for group in env["command"]["data"]["groups"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = group["name"].as_str().unwrap_or("");
        for bucket in group["buckets"].as_array().into_iter().flatten() {
            let window = bucket["window"].as_str().unwrap_or("");
            let secs = match window {
                "weekly" => 7 * 24 * 3600,
                "5h" => 5 * 3600,
                _ => continue,
            };
            let Some(reset) = bucket["reset_time"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            else {
                continue;
            };
            out.push(QuotaSample {
                window: format!("{name}/{window}"),
                used: Some(100.0 * (1.0 - bucket["remaining_fraction"].as_f64().unwrap_or(0.0))),
                reset_at: Some(reset.with_timezone(&Utc)),
                duration: Duration::from_secs(secs),
                source: SOURCE_AGY_USAGE.into(),
                confidence: "official".into(),
                ..QuotaSample::default()
            });
        }
    }
    if out.is_empty() {
        Err(Error::Message("no usage groups in report".into()))
    } else {
        Ok(out)
    }
}
pub fn parse_agy_samples(raw: &[u8]) -> Result<Vec<QuotaSample>, Error> {
    agy_samples(raw)
}
pub fn parse_agy_usage(raw: &[u8]) -> Result<HashMap<String, Quota>, Error> {
    let samples = agy_samples(raw)?;
    let mut out = HashMap::new();
    for s in samples {
        let Some((group, window)) = s.window.rsplit_once('/') else {
            continue;
        };
        let used = s.used.unwrap_or(0.0);
        if window == "weekly" || !out.contains_key(group) || used >= 90.0 {
            if window == "weekly"
                && out
                    .get(group)
                    .is_some_and(|q: &Quota| q.window_minutes == 300 && q.used_percent >= 90.0)
            {
                continue;
            }
            out.insert(
                group.to_owned(),
                Quota {
                    used_percent: used,
                    window_minutes: (s.duration.as_secs() / 60) as i64,
                    resets_at: s.reset_at,
                    ..Quota::default()
                },
            );
        }
    }
    Ok(out)
}
