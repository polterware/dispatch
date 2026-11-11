use super::pace_history::{is_zero_utc, parse_time_or_zero};
use crate::{
    Error,
    types::{
        Area, Filters, Run, STATUS_COMPLETED, STATUS_FAILED, STATUS_ORPHANED, STATUS_PENDING,
        STATUS_RUNNING, STATUS_TIMED_OUT, Store, Usage,
    },
};
use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::LazyLock;

pub const MIN_SAMPLES: usize = 5;
pub const STATS_SCHEMA: &str = "dispatch.stats.v1";
pub const SCHEMA: &str = STATS_SCHEMA;
pub const RATE_LIMIT_PATTERN: &str =
    r"(?i)rate.?limit|quota|429|too many requests|usage limit|exhausted";

static RATE_LIMIT_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(RATE_LIMIT_PATTERN).ok());

pub fn is_rate_limit_message(text: &str) -> bool {
    RATE_LIMIT_RE.as_ref().is_some_and(|re| re.is_match(text))
}

pub fn observed_ms(run: &Run) -> Option<i64> {
    if let Some(ms) = run.run_ms
        && ms >= 0
    {
        return Some(ms);
    }
    let start_raw = if !run.started_at.trim().is_empty() {
        &run.started_at
    } else {
        &run.created_at
    };
    let start = parse_time_or_zero(start_raw);
    let finished = parse_time_or_zero(&run.finished_at);
    if is_zero_utc(start) || is_zero_utc(finished) || finished < start {
        return None;
    }
    Some((finished - start).num_milliseconds())
}

pub trait RunStatsExt {
    fn observed_ms(&self) -> Option<i64>;
}

impl RunStatsExt for Run {
    fn observed_ms(&self) -> Option<i64> {
        observed_ms(self)
    }
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

#[derive(Clone, Debug, Default)]
pub struct StatsOptions {
    pub project_id: String,
    /// A repository's folder: only runs in it or below it.
    pub dir: String,
    pub by: String,
    pub since: Option<DateTime<Utc>>,
    pub now: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Report {
    pub schema: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub since: String,
    pub by: String,
    pub min_samples: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub in_flight: usize,
    pub rows: Vec<Row>,
    pub rankings: Rankings,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Row {
    pub key: String,
    pub runs: usize,
    pub success_pct: f64,
    pub first_try_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p90_ms: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub duration: String,
    #[serde(rename = "durationSamples")]
    pub duration_n: usize,
    pub fallbacks: usize,
    pub rate_limits: usize,
    pub orphans: usize,
    pub spend: Spend,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Spend {
    pub usage_runs: usize,
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    #[serde(rename = "costUsd")]
    pub cost_usd: f64,
    pub cost_runs: usize,
    /// Runs still working, whose usage so far is counted.
    pub live_runs: usize,
    /// Priced runs with an attempt no price covered.
    pub partial_cost_runs: usize,
}

impl Spend {
    fn add(&mut self, u: Option<&Usage>) {
        let Some(u) = u else {
            return;
        };
        self.usage_runs += 1;
        self.input_tokens += u.input_tokens;
        self.cache_read_tokens += u.cache_read_tokens;
        self.cache_write_tokens += u.cache_write_tokens;
        self.output_tokens += u.output_tokens;
        self.reasoning_tokens += u.reasoning_tokens;
        if let Some(cost) = u.cost_usd {
            self.cost_usd += cost;
            self.cost_runs += 1;
            if u.cost_partial {
                self.partial_cost_runs += 1;
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rankings {
    pub most_used_model: StatRank,
    pub most_used_provider: StatRank,
    pub slowest_model: StatRank,
    pub most_failed_model: StatRank,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatRank {
    pub key: String,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub runs: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub samples: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_pct: Option<f64>,
    /// Most failed: the runs that failed or timed out, of `runs`.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub failures: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

pub fn valid_by(name: &str) -> bool {
    matches!(
        name,
        "model"
            | "agent"
            | "provider"
            | "tier"
            | "effort"
            | "area"
            | "day"
            | "week"
            | "requested"
            | "resolved"
    )
}

pub fn parse_since(s: &str, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, Error> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s == "all" || s == "0" {
        return Ok(None);
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits == s.len() {
        return Err(Error::Message(format!(
            "bad --since {s:?} (want 7d, 24h, 30m, 2w, all)"
        )));
    }
    let n: i64 = s[..digits]
        .parse()
        .map_err(|_| Error::Message(format!("bad --since {s:?} (want 7d, 24h, 30m, 2w, all)")))?;
    let span = match &s[digits..] {
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        "w" => chrono::Duration::weeks(n),
        _ => {
            return Err(Error::Message(format!(
                "bad --since {s:?} (want 7d, 24h, 30m, 2w, all)"
            )));
        }
    };
    Ok(Some(now - span))
}

pub fn from_store(store: Option<&dyn Store>, opt: StatsOptions) -> Result<Report, Error> {
    from_store_live(store, opt, &|_| None)
}

/// [`from_store`], with `live` giving a run still working its usage so far, which the spend
/// counts as live.
pub fn from_store_live(
    store: Option<&dyn Store>,
    opt: StatsOptions,
    live: &dyn Fn(&Run) -> Option<Usage>,
) -> Result<Report, Error> {
    let Some(s) = store else {
        return compute(&[], opt);
    };
    let mut recs = s.list_run_history(&opt.project_id, &opt.dir)?;
    if opt.by == "area" {
        let decs = s.list_route_decisions(&opt.project_id)?;
        let mut areas: HashMap<String, Area> = HashMap::with_capacity(decs.len());
        for d in decs {
            areas.insert(d.run_id, d.area);
        }
        for r in &mut recs {
            r.area = areas.get(&r.id).cloned().unwrap_or_default();
        }
    }
    for r in &mut recs {
        if (r.status == STATUS_PENDING || r.status == STATUS_RUNNING)
            && let Some(usage) = live(r)
        {
            r.usage = Some(usage);
        }
    }
    compute(&recs, opt)
}

pub fn compute(recs: &[Run], mut opt: StatsOptions) -> Result<Report, Error> {
    if opt.by.is_empty() {
        opt.by = "model".into();
    }
    if !valid_by(&opt.by) {
        return Err(Error::Message(format!("unknown --by {:?}", opt.by)));
    }
    let _now = opt.now.unwrap_or_else(Utc::now);
    let mut rep = Report {
        schema: STATS_SCHEMA.into(),
        since: opt
            .since
            .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
            .unwrap_or_default(),
        by: opt.by.clone(),
        min_samples: MIN_SAMPLES,
        in_flight: 0,
        rows: Vec::new(),
        rankings: Rankings::default(),
        note:
            "a missing field is unknown; duration uses runMs, else finishedAt-startedAt/createdAt"
                .into(),
    };
    let mut kept: Vec<&Run> = Vec::new();
    let mut working: Vec<&Run> = Vec::new();
    for r in recs {
        if let Some(since) = opt.since {
            let created = parse_time_or_zero(&r.created_at);
            if is_zero_utc(created) || created < since {
                continue;
            }
        }
        if r.status == STATUS_PENDING || r.status == STATUS_RUNNING {
            rep.in_flight += 1;
            working.push(r);
            continue;
        }
        kept.push(r);
    }
    let mut buckets = stats_buckets(&kept, &opt.by);
    // A run still working counts toward spend only, as live: it has no outcome yet.
    for r in working {
        if r.usage.is_none() {
            continue;
        }
        let a = buckets.entry(stats_group_key(r, &opt.by)).or_default();
        let (usage_runs, cost_runs, partial) = (
            a.spend.usage_runs,
            a.spend.cost_runs,
            a.spend.partial_cost_runs,
        );
        a.spend.add(r.usage.as_ref());
        // Its tokens and cost count; the run counts once it ends.
        (
            a.spend.usage_runs,
            a.spend.cost_runs,
            a.spend.partial_cost_runs,
        ) = (usage_runs, cost_runs, partial);
        a.spend.live_runs += 1;
    }
    rep.rows = rows_of(&buckets, &opt.by);
    rep.rankings = rank_all_stats(&kept);
    Ok(rep)
}

#[derive(Default)]
struct StatsAcc {
    runs: usize,
    ok: usize,
    first: usize,
    fallbacks: usize,
    rates: usize,
    orphans: usize,
    durs: Vec<i64>,
    /// The completed runs' durations: how fast the model finishes what it finishes.
    done_durs: Vec<i64>,
    /// The runs the model itself ended (completed, failed, or timed out), leaving out a
    /// cancel (the user's call), an orphan (the core went away), and a rate-limited end (the
    /// account had no quota left, the model never tried), and of those, the failures.
    judged: usize,
    failed: usize,
    spend: Spend,
}

fn stats_buckets(recs: &[&Run], by: &str) -> HashMap<String, StatsAcc> {
    let mut out: HashMap<String, StatsAcc> = HashMap::new();
    for r in recs {
        let k = stats_group_key(r, by);
        let a = out.entry(k).or_default();
        a.runs += 1;
        if r.status == STATUS_COMPLETED {
            a.ok += 1;
            if r.attempts.is_empty() {
                a.first += 1;
            }
        }
        if !r.attempts.is_empty() {
            a.fallbacks += 1;
        }
        if hit_rate_limit(r) {
            a.rates += 1;
        }
        if r.status == STATUS_ORPHANED {
            a.orphans += 1;
        }
        let ended = r.status == STATUS_COMPLETED
            || r.status == STATUS_FAILED
            || r.status == STATUS_TIMED_OUT;
        if ended && !(r.status != STATUS_COMPLETED && hit_rate_limit(r)) {
            a.judged += 1;
            if r.status != STATUS_COMPLETED {
                a.failed += 1;
            }
        }
        if let Some(ms) = observed_ms(r) {
            a.durs.push(ms);
            if r.status == STATUS_COMPLETED {
                a.done_durs.push(ms);
            }
        }
        a.spend.add(r.usage.as_ref());
    }
    out
}

#[derive(Serialize)]
struct FiltersGoJson<'a> {
    #[serde(rename = "Provider")]
    provider: &'a str,
    #[serde(rename = "Model")]
    model: &'a str,
    #[serde(rename = "Effort")]
    effort: &'a str,
    #[serde(rename = "Agent")]
    agent: &'a str,
    #[serde(rename = "Priority")]
    priority: &'a str,
    #[serde(rename = "Relax")]
    relax: bool,
}

fn format_filters_key(f: Option<&Filters>) -> String {
    let Some(f) = f else {
        return "unknown".into();
    };
    serde_json::to_string(&FiltersGoJson {
        provider: &f.provider,
        model: &f.model,
        effort: &f.effort,
        agent: &f.agent,
        priority: &f.priority,
        relax: f.relax,
    })
    .unwrap_or_else(|_| "unknown".into())
}

fn stats_known(s: &str) -> String {
    if s.is_empty() {
        "unknown".into()
    } else {
        s.into()
    }
}

fn stats_group_key(r: &Run, by: &str) -> String {
    match by {
        "requested" => format_filters_key(r.requested.as_ref()),
        "resolved" => format_filters_key(r.resolved.as_ref()),
        "model" => stats_known(&r.model),
        "agent" => stats_known(&r.agent),
        "provider" => stats_known(&r.provider),
        "tier" => stats_known(&r.tier),
        "effort" => stats_known(&r.effort),
        "area" => {
            let mut parts: Vec<&str> = r
                .area
                .kinds
                .iter()
                .chain(r.area.tech.iter())
                .map(String::as_str)
                .collect();
            parts.retain(|s| !s.is_empty());
            stats_known(&parts.join(", "))
        }
        "day" => {
            let dt = parse_time_or_zero(&r.created_at);
            if is_zero_utc(dt) {
                "unknown".into()
            } else {
                dt.format("%Y-%m-%d").to_string()
            }
        }
        "week" => {
            let dt = parse_time_or_zero(&r.created_at);
            if is_zero_utc(dt) {
                "unknown".into()
            } else {
                let iso = dt.iso_week();
                format!("{:04}-W{:02}", iso.year(), iso.week())
            }
        }
        _ => "unknown".into(),
    }
}

fn rows_of(groups: &HashMap<String, StatsAcc>, by: &str) -> Vec<Row> {
    let mut rows = Vec::with_capacity(groups.len());
    for (k, a) in groups {
        let p50_ms = pctile(&a.durs, 0.50);
        let p90_ms = pctile(&a.durs, 0.90);
        let duration_n = a.durs.len();
        let duration = if duration_n == 0 {
            "unknown".into()
        } else {
            String::new()
        };
        rows.push(Row {
            key: k.clone(),
            runs: a.runs,
            success_pct: stats_ratio(a.ok, a.runs),
            first_try_pct: stats_ratio(a.first, a.runs),
            p50_ms,
            p90_ms,
            duration,
            duration_n,
            fallbacks: a.fallbacks,
            rate_limits: a.rates,
            orphans: a.orphans,
            spend: a.spend.clone(),
        });
    }
    rows.sort_by(|a, b| {
        if by == "day" || by == "week" {
            return a.key.cmp(&b.key);
        }
        b.runs.cmp(&a.runs).then_with(|| a.key.cmp(&b.key))
    });
    rows
}

fn stats_ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        ((n as f64 / d as f64) * 1000.0).round() / 10.0
    }
}

pub(crate) fn pctile(xs: &[i64], p: f64) -> Option<i64> {
    if xs.is_empty() {
        return None;
    }
    let mut s = xs.to_vec();
    s.sort_unstable();
    let raw = (p * s.len() as f64).ceil() as isize - 1;
    let idx = raw.clamp(0, s.len() as isize - 1) as usize;
    Some(s[idx])
}

fn hit_rate_limit(r: &Run) -> bool {
    if r.exit_class == "rate_limit" || is_rate_limit_message(&r.error) {
        return true;
    }
    r.attempts.iter().any(|a| {
        a.exit_class != "near_limit"
            && (a.exit_class == "rate_limit" || is_rate_limit_message(&a.error))
    })
}

fn rank_all_stats(recs: &[&Run]) -> Rankings {
    let models = stats_buckets(recs, "model");
    let providers = stats_buckets(recs, "provider");
    Rankings {
        most_used_model: most_used_stat(&models),
        most_used_provider: most_used_stat(&providers),
        slowest_model: slowest_stat(&models),
        most_failed_model: most_failed_stat(&models),
    }
}

fn withheld_rank(reason: impl Into<String>) -> StatRank {
    StatRank {
        key: "unknown".into(),
        reason: reason.into(),
        ..Default::default()
    }
}

fn most_used_stat(groups: &HashMap<String, StatsAcc>) -> StatRank {
    if groups.is_empty() {
        return withheld_rank("no runs in range");
    }
    let mut best: Option<(&str, usize)> = None;
    for (k, a) in groups {
        let take = match best {
            None => true,
            Some((bk, bn)) => a.runs > bn || (a.runs == bn && k.as_str() < bk),
        };
        if take {
            best = Some((k.as_str(), a.runs));
        }
    }
    match best {
        Some((k, runs)) => StatRank {
            key: k.into(),
            runs,
            ..Default::default()
        },
        None => withheld_rank("no runs in range"),
    }
}

/// Why a comparison ranking has no answer: no model has enough runs, or only one does, and
/// one model is not slower or more failed than anything.
fn too_few(qualified: usize) -> StatRank {
    if qualified == 0 {
        withheld_rank(format!("insufficient samples (minimum {MIN_SAMPLES})"))
    } else {
        withheld_rank(format!(
            "needs two models with enough runs (minimum {MIN_SAMPLES})"
        ))
    }
}

/// The model whose completed runs take longest at the median, among the models with at
/// least [`MIN_SAMPLES`] completed runs, once two have them. A failed or cancelled run's time
/// says nothing of the model's speed, so it is left out.
fn slowest_stat(groups: &HashMap<String, StatsAcc>) -> StatRank {
    let qualified = groups
        .values()
        .filter(|a| a.done_durs.len() >= MIN_SAMPLES)
        .count();
    if qualified < 2 {
        return too_few(qualified);
    }
    let mut best: Option<(StatRank, i64, i64)> = None;
    for (k, a) in groups {
        if a.done_durs.len() < MIN_SAMPLES {
            continue;
        }
        let Some(p50) = pctile(&a.done_durs, 0.50) else {
            continue;
        };
        let p90 = pctile(&a.done_durs, 0.90).unwrap_or(p50);
        let cand = StatRank {
            key: k.clone(),
            runs: a.runs,
            samples: a.done_durs.len(),
            p50_ms: Some(p50),
            ..Default::default()
        };
        let take = match &best {
            None => true,
            Some((b_rank, b_p50, b_p90)) => {
                p50 > *b_p50
                    || (p50 == *b_p50 && (p90 > *b_p90 || (p90 == *b_p90 && k < &b_rank.key)))
            }
        };
        if take {
            best = Some((cand, p50, p90));
        }
    }
    match best {
        Some((rank, _, _)) => rank,
        None => too_few(0),
    }
}

/// The model that fails most, among the models with at least [`MIN_SAMPLES`] runs it ended
/// itself, once two have them: ranked by the 95% Wilson lower bound of its failure rate, so
/// 1 failure in 5 runs does not outrank 3 in 18 on a rate five runs cannot pin down. A model
/// that never failed is never named; with no failures there is no answer.
fn most_failed_stat(groups: &HashMap<String, StatsAcc>) -> StatRank {
    let qualified = groups.values().filter(|a| a.judged >= MIN_SAMPLES).count();
    if qualified < 2 {
        return too_few(qualified);
    }
    let mut best: Option<(&str, &StatsAcc, f64)> = None;
    for (k, a) in groups {
        if a.judged < MIN_SAMPLES || a.failed == 0 {
            continue;
        }
        let low = wilson_low(a.failed, a.judged);
        let take = match best {
            None => true,
            Some((bk, ba, bl)) => {
                low > bl
                    || (low == bl
                        && (a.failed > ba.failed || (a.failed == ba.failed && k.as_str() < bk)))
            }
        };
        if take {
            best = Some((k.as_str(), a, low));
        }
    }
    match best {
        Some((k, a, _)) => StatRank {
            key: k.into(),
            runs: a.judged,
            success_pct: Some(stats_ratio(a.judged - a.failed, a.judged)),
            failures: a.failed,
            ..Default::default()
        },
        None => withheld_rank("no failures in range"),
    }
}

/// The 95% Wilson score interval's lower bound for `hits` in `n`.
fn wilson_low(hits: usize, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let z = 1.96_f64;
    let n = n as f64;
    let p = hits as f64 / n;
    let z2 = z * z;
    let center = p + z2 / (2.0 * n);
    let margin = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    ((center - margin) / (1.0 + z2 / n)).max(0.0)
}

impl Report {
    pub fn text(&self) -> String {
        let mut b = String::new();
        let since = if self.since.is_empty() {
            "all"
        } else {
            &self.since
        };
        let _ = writeln!(
            b,
            "since {}  by {}  minSamples {}",
            since, self.by, self.min_samples
        );
        if self.in_flight > 0 {
            let _ = writeln!(b, "inFlight {} (fora das taxas)", self.in_flight);
        }
        if self.rows.is_empty() {
            b.push_str("no runs in range\n");
        } else {
            let _ = writeln!(
                b,
                "{:<24} {:>5} {:>6} {:>6} {:>12} {:>12} {:>9} {:>10} {:>7}",
                self.by,
                "runs",
                "ok%",
                "first%",
                "p50",
                "p90",
                "fallbacks",
                "rateLimits",
                "orphans"
            );
            for row in &self.rows {
                let _ = writeln!(
                    b,
                    "{:<24} {:>5} {:>6.1} {:>6.1} {:>12} {:>12} {:>9} {:>10} {:>7}",
                    stats_trunc(&row.key, 24),
                    row.runs,
                    row.success_pct,
                    row.first_try_pct,
                    stats_fmt_dur(row.p50_ms),
                    stats_fmt_dur(row.p90_ms),
                    row.fallbacks,
                    row.rate_limits,
                    row.orphans
                );
            }
        }
        b.push_str(&self.rankings.most_used_model.line("most used model"));
        b.push_str(&self.rankings.most_used_provider.line("most used provider"));
        b.push_str(&self.rankings.slowest_model.line("slowest model"));
        b.push_str(&self.rankings.most_failed_model.line("most failed model"));
        b
    }
}

impl StatRank {
    fn line(&self, label: &str) -> String {
        if !self.reason.is_empty() || self.key.is_empty() {
            let reason = if self.reason.is_empty() {
                "unknown"
            } else {
                &self.reason
            };
            return format!("{label}: unknown ({reason})\n");
        }
        if self.p50_ms.is_some() {
            format!(
                "{label}: {} (p50 {}, {} samples)\n",
                self.key,
                stats_fmt_dur(self.p50_ms),
                self.samples
            )
        } else if self.failures > 0 {
            format!(
                "{label}: {} ({} of {} runs failed)\n",
                self.key, self.failures, self.runs
            )
        } else if let Some(pct) = self.success_pct {
            format!("{label}: {} ({pct:.1}% ok, {} runs)\n", self.key, self.runs)
        } else {
            format!("{label}: {} ({} runs)\n", self.key, self.runs)
        }
    }
}

fn stats_fmt_dur(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "unknown".into();
    };
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let tenths = (ms + 50) / 100;
    let hours = tenths / 36_000;
    let rem = tenths % 36_000;
    let mins = rem / 600;
    let rem_tenths = rem % 600;
    let secs = rem_tenths / 10;
    let frac = rem_tenths % 10;
    let sec_part = if frac == 0 {
        format!("{secs}s")
    } else {
        format!("{secs}.{frac}s")
    };
    if hours > 0 {
        format!("{hours}h{mins}m{sec_part}")
    } else if mins > 0 {
        format!("{mins}m{sec_part}")
    } else {
        sec_part
    }
}

fn stats_trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.into();
    }
    if n <= 1 {
        let mut end = n.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        return s[..end].into();
    }
    let mut end = (n - 1).min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
