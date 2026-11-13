use super::{FiltersJson, parse_time};
use crate::{
    adapters::failfast::is_credit_error,
    types::{
        Run, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_FAILED, STATUS_ORPHANED, STATUS_RUNNING,
        STATUS_TIMED_OUT, Usage,
    },
};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

pub const SUMMARY_SCHEMA: &str = "dispatch.run-summary.v1";

/// Provider errors that mean "stop sending work here".
pub(crate) static RATE_LIMITED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)rate.?limit|quota|429|too many requests|usage limit|exhausted")
        .unwrap_or_else(|_| never())
});
static CONFLICT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)merge conflict|conflict marker|\bCONFLICT\b|<<<<<<<")
        .unwrap_or_else(|_| never())
});
static CRASH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bpanic:|goroutine \d+ \[|segmentation fault|core dumped|fatal signal|signal: (?:segmentation|abort|bus|killed)|exit status 13[479]",
    )
    .unwrap_or_else(|_| never())
});

/// A reply that is only the provider saying the account is out ("⚠ Individual quota reached.
/// … Resets in 4h54m49s."), which an agent such as Antigravity ends its turn with instead of
/// failing: short, and naming a limit that was reached.
static LIMIT_NOTICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:quota|usage limit|rate.?limit|credits?)\b[^.\n]{0,40}\b(?:reached|exceeded|exhausted|used up|ran out|run out)\b|\b(?:hit|reached|exceeded) (?:your |the )?(?:\w+ )?(?:quota|usage limit|rate.?limit)\b|\bout of (?:quota|credits)\b|\bresets? in \d+\s*[hms]",
    )
    .unwrap_or_else(|_| never())
});
static RESETS_IN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bresets? in\s+(?:(\d+)\s*h)?\s*(?:(\d+)\s*m)?\s*(?:(\d+)\s*s)?")
        .unwrap_or_else(|_| never())
});

/// The most a reply may hold to count as a limit notice rather than an answer that mentions
/// one.
const LIMIT_NOTICE_MAX: usize = 400;

/// The limit notice a reply is, when it is one: its text, which then fails the attempt.
pub fn limit_notice(reply: &str) -> Option<String> {
    let text = reply.trim();
    (!text.is_empty() && text.chars().count() <= LIMIT_NOTICE_MAX && LIMIT_NOTICE.is_match(text))
        .then(|| text.to_owned())
}

/// How long until the limit a notice names resets ("Resets in 4h54m49s").
pub fn resets_in(notice: &str) -> Option<Duration> {
    let caps = RESETS_IN.captures(notice)?;
    let part = |i: usize| {
        caps.get(i)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(0)
    };
    let secs = part(1) * 3600 + part(2) * 60 + part(3);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// A quota that ran out for one model ("⚠ Individual quota reached. … Resets in 140h9m53s.").
static MODEL_QUOTA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bindividual quota\b|\bquota\b[^.\n]{0,40}\b(?:reached|exceeded|exhausted|used up)\b",
    )
    .unwrap_or_else(|_| never())
});
/// Words that make a limit the whole account's rather than one model's.
static ACCOUNT_WIDE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:account|all models|every model|organi[sz]ation|workspace|project)\b")
        .unwrap_or_else(|_| never())
});

/// Whether a quota error names one model's quota, which Antigravity keeps apart for each
/// model, rather than a limit on the whole account (a 429, a rate limit, an account quota).
pub fn is_model_quota(err: &str) -> bool {
    MODEL_QUOTA.is_match(err) && !ACCOUNT_WIDE.is_match(err)
}

/// A regex that matches nothing, for a pattern that failed to compile.
fn never() -> Regex {
    Regex::new(r"[^\s\S]").unwrap_or_else(|_| never())
}

/// Labels an agent error. "failed" is the generic bucket.
pub fn class_of(msg: &str) -> String {
    if is_credit_error(msg) {
        "credits"
    } else if RATE_LIMITED.is_match(msg) {
        "rate_limit"
    } else if CONFLICT_RE.is_match(msg) {
        "merge_conflict"
    } else if CRASH_RE.is_match(msg) {
        "crash"
    } else {
        "failed"
    }
    .into()
}

pub(crate) fn conflict(s: &str) -> bool {
    CONFLICT_RE.is_match(s)
}

/// Refines the exit class with credits, rate_limit, crash and merge_conflict.
pub fn classify_run(rec: &Run) -> String {
    if rec.status == STATUS_FAILED {
        if let Some(last) = rec.attempts.last()
            && matches!(
                last.exit_class.as_str(),
                "credits" | "rate_limit" | "crash" | "merge_conflict"
            )
        {
            return last.exit_class.clone();
        }
        let cls = class_of(&rec.error);
        if cls != "failed" {
            return cls;
        }
    }
    if rec.status == STATUS_COMPLETED && (conflict(&rec.note) || conflict(&rec.error)) {
        return "merge_conflict".into();
    }
    super::exit_class(&rec.status).into()
}

/// How many agents were invoked. Failed attempts are recorded; the final
/// agent (success, timeout, cancel, orphan, or the one still running) is not
/// in that slice.
fn tries(rec: &Run) -> usize {
    let mut n = rec.attempts.len();
    if matches!(
        rec.status.as_str(),
        STATUS_COMPLETED | STATUS_TIMED_OUT | STATUS_CANCELLED | STATUS_ORPHANED | STATUS_RUNNING
    ) && !rec.agent.is_empty()
    {
        n += 1;
    }
    n
}

/// The run's measured duration: its run time, else start (or creation) to finish.
pub fn observed_ms(r: &Run) -> Option<i64> {
    if let Some(ms) = r.run_ms.filter(|ms| *ms >= 0) {
        return Some(ms);
    }
    let start = if r.started_at.is_empty() {
        &r.created_at
    } else {
        &r.started_at
    };
    let start = parse_time(start)?;
    let finished = parse_time(&r.finished_at)?;
    if finished < start {
        return None;
    }
    Some((finished - start).num_milliseconds())
}

/// The dispatch.run-summary.v1 document. It never carries the agent's free
/// text; that stays behind `dispatch result <id> --section agent`.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<FiltersJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<FiltersJson>,
    pub schema: String,
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tier: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insertions: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletions: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub attempts: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub exit_class: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resumed_of: String,
    pub details: BTreeMap<String, String>,
}

/// Builds a RunSummary. Absent numbers stay absent.
pub fn summarize(rec: &Run) -> RunSummary {
    RunSummary {
        schema: SUMMARY_SCHEMA.into(),
        requested: rec.requested.as_ref().map(Into::into),
        resolved: rec.resolved.as_ref().map(Into::into),
        id: rec.id.clone(),
        status: rec.status.clone(),
        agent: rec.agent.clone(),
        model: rec.model.clone(),
        tier: rec.tier.clone(),
        provider: rec.provider.clone(),
        effort: rec.effort.clone(),
        duration_ms: observed_ms(rec),
        queued_ms: rec.queued_ms,
        run_ms: rec.run_ms,
        files_changed: rec.files_changed,
        insertions: rec.insertions,
        deletions: rec.deletions,
        usage: rec.usage.clone(),
        attempts: tries(rec),
        exit_class: rec.exit_class.clone(),
        branch: rec.branch.clone(),
        worktree: rec.worktree.clone(),
        error: rec.error.clone(),
        note: rec.note.clone(),
        session_id: rec.session_id.clone(),
        resumed_of: rec.resumed_of.clone(),
        details: detail_cmds(&rec.id),
    }
}

fn detail_cmds(id: &str) -> BTreeMap<String, String> {
    ["summary", "files", "agent", "attempts", "meta"]
        .into_iter()
        .map(|s| (s.to_owned(), format!("dispatch result {id} --section {s}")))
        .collect()
}

impl RunSummary {
    /// The three-line human rendering. Unknown fields say "unknown".
    pub fn text(&self) -> String {
        let line1 = format!(
            "{} {}/{} tier={} {} {} attempts={}",
            self.status,
            unk(&self.agent),
            unk(&self.model),
            unk(&self.tier),
            fmt_ms(self.duration_ms),
            fmt_files(self),
            self.attempts
        );
        let mut line2 = format!(
            "branch={} worktree={}",
            unk(&self.branch),
            unk(&self.worktree)
        );
        let line3 = format!(
            "details: dispatch result {} --section summary|files|agent|attempts|meta",
            self.id
        );
        if !self.error.is_empty() {
            line2.push_str(" error=");
            line2.push_str(&self.error);
        }
        if !self.note.is_empty() {
            line2.push_str(" note=");
            line2.push_str(&self.note);
        }
        if let Some(r) = &self.requested {
            let resolved = self
                .resolved
                .as_ref()
                .map_or_else(|| "<nil>".to_owned(), |f| format!("&{}", go_filters(f)));
            line2.push_str(&format!(" requested={} resolved={resolved}", go_filters(r)));
        }
        format!("{line1}\n{line2}\n{line3}\n")
    }
}

/// Filters as Go's `%+v` prints them.
fn go_filters(f: &FiltersJson) -> String {
    format!(
        "{{Provider:{} Model:{} Effort:{} Agent:{} Priority:{} Relax:{}}}",
        f.provider, f.model, f.effort, f.agent, f.priority, f.relax
    )
}

fn unk(s: &str) -> &str {
    if s.is_empty() { "unknown" } else { s }
}

fn fmt_ms(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "unknown".into();
    };
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let rounded = (ms + 50) / 100 * 100;
    go_duration(Duration::from_millis(u64::try_from(rounded).unwrap_or(0)))
}

fn fmt_files(s: &RunSummary) -> String {
    let Some(files) = s.files_changed else {
        return "files=unknown".into();
    };
    match (s.insertions, s.deletions) {
        (Some(i), Some(d)) => format!("files={files} +{i} -{d}"),
        _ => format!("files={files} +/- unknown"),
    }
}

/// A duration as Go's `time.Duration.String` prints it.
pub fn go_duration(d: Duration) -> String {
    let n = d.as_nanos();
    if n == 0 {
        return "0s".into();
    }
    if n < 1_000_000_000 {
        let (unit, digits) = if n < 1_000 {
            ("ns", 0)
        } else if n < 1_000_000 {
            ("µs", 3)
        } else {
            ("ms", 6)
        };
        return format!("{}{unit}", decimal(n, digits));
    }
    let secs = n / 1_000_000_000;
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if h > 0 || m > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&decimal(s * 1_000_000_000 + n % 1_000_000_000, 9));
    out.push('s');
    out
}

/// `n / 10^digits` with its fraction, trailing zeros dropped.
fn decimal(n: u128, digits: u32) -> String {
    let div = 10u128.pow(digits);
    let (whole, frac) = (n / div, n % div);
    if frac == 0 {
        return whole.to_string();
    }
    let f = format!("{frac:0width$}", width = digits as usize);
    format!("{whole}.{}", f.trim_end_matches('0'))
}

/// The first `max_lines` lines of `output`, at most `max_bytes` bytes, cut at
/// a character boundary, and whether it was truncated.
pub fn summary(output: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    let b = output.as_bytes();
    let (mut end, mut lines) = (0, 0);
    while end < b.len() && lines < max_lines && end < max_bytes {
        if b[end] == b'\n' {
            lines += 1;
        }
        end += 1;
    }
    let truncated = end < b.len();
    while end > 0 && end < b.len() && !output.is_char_boundary(end) {
        end -= 1;
    }
    (output[..end].to_owned(), truncated)
}

/// Lines `[offset, offset+limit)` of `text`, the total line count, and
/// whether more lines follow.
pub fn page(text: &str, offset: i64, limit: i64) -> (String, usize, bool) {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.is_empty() || text.ends_with('\n') {
        lines.pop();
    }
    let total = lines.len();
    let offset = usize::try_from(offset.max(0)).unwrap_or(0).min(total);
    let mut end = total;
    if limit > 0 {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        if offset.saturating_add(limit) < total {
            end = offset + limit;
        }
    }
    let out: String = lines[offset..end]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    (out, total, end < total)
}
