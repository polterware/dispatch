use super::quota::{Quota, QuotaSample, SOURCE_CLAUDE_STREAM};
use crate::Error;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Observed {
    dir: PathBuf,
    samples: Vec<QuotaSample>,
    loaded: bool,
}
static OBSERVED: LazyLock<Mutex<Observed>> = LazyLock::new(|| Mutex::new(Observed::default()));

pub fn set_state_dir(dir: impl Into<PathBuf>) {
    if let Ok(mut state) = OBSERVED.lock() {
        *state = Observed {
            dir: dir.into(),
            ..Observed::default()
        };
    }
}
pub fn samples(limits: &Value, at: DateTime<Utc>) -> Vec<QuotaSample> {
    let mut out = Vec::new();
    for (name, w) in limits["unifiedWindows"].as_object().into_iter().flatten() {
        let secs = if name == "five_hour" {
            5 * 3600
        } else if name.starts_with("seven_day") {
            7 * 24 * 3600
        } else {
            continue;
        };
        let Some(used) = w["utilization"].as_f64() else {
            continue;
        };
        let Some(reset) = w["resetsAt"]
            .as_i64()
            .filter(|s| *s != 0)
            .and_then(|s| DateTime::from_timestamp(s, 0))
        else {
            continue;
        };
        out.push(QuotaSample {
            window: name.clone(),
            used: Some((used * 10000.0).round() / 100.0),
            reset_at: Some(reset),
            duration: Duration::from_secs(secs),
            source: SOURCE_CLAUDE_STREAM.into(),
            confidence: "official".into(),
            sampled_at: Some(at),
            ..QuotaSample::default()
        });
    }
    out.sort_by(|a, b| {
        a.used
            .partial_cmp(&b.used)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.window.cmp(&b.window))
    });
    out
}
pub fn remember(samples: Vec<QuotaSample>) {
    if samples.is_empty() {
        return;
    }
    if let Ok(mut state) = OBSERVED.lock() {
        state.samples = samples;
        state.loaded = true;
        if !state.dir.as_os_str().is_empty()
            && let Ok(bytes) = serde_json::to_vec(&state.samples)
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(state.dir.join("claude-rate-limits.json"))
            {
                let _ = file.write_all(&bytes);
            }
        }
    }
}
pub fn observed(now: DateTime<Utc>) -> Vec<QuotaSample> {
    let Ok(mut state) = OBSERVED.lock() else {
        return Vec::new();
    };
    if !state.loaded && !state.dir.as_os_str().is_empty() {
        if let Ok(bytes) = std::fs::read(state.dir.join("claude-rate-limits.json"))
            && let Ok(samples) = serde_json::from_slice(&bytes)
        {
            state.samples = samples;
        }
        state.loaded = true;
    }
    state
        .samples
        .iter()
        .filter(|s| s.used.is_some() && s.reset_at.is_some_and(|t| t > now))
        .cloned()
        .collect()
}
pub fn quota(samples: &[QuotaSample]) -> Result<Quota, Error> {
    let Some(top) = samples.iter().filter(|s| s.used.is_some()).max_by(|a, b| {
        a.used
            .partial_cmp(&b.used)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) else {
        return Err(Error::Message("quota unavailable".into()));
    };
    Ok(Quota {
        key: "claude".into(),
        used_percent: top.used.unwrap_or(0.0),
        window_minutes: (top.duration.as_secs() / 60) as i64,
        resets_at: top.reset_at,
        sampled_at: top.sampled_at,
        ..Quota::default()
    })
}
