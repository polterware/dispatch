//! Pure stats formatting. Period keys match the core: UTC days and ISO weeks.

use crate::client::{
    DispatchStatRank, DispatchStatsRankings, DispatchStatsRow, ProtoI64, StatsResponse,
};
use crate::models::sort::SortValue;

/// Group-by choices, in the original select order.
pub const BY_OPTIONS: &[(&str, &str)] = &[
    ("day", "Day"),
    ("week", "Week"),
    ("model", "Model"),
    ("agent", "Agent"),
    ("provider", "Provider"),
    ("tier", "Tier"),
    ("area", "Area"),
];

/// Windows for a day chart: each draws one bar per day.
const DAY_WINDOWS: &[(&str, &str)] = &[
    ("7d", "Last 7 days"),
    ("14d", "Last 14 days"),
    ("30d", "Last 30 days"),
];

/// Windows for a week chart: each draws one bar per ISO week.
const WEEK_WINDOWS: &[(&str, &str)] = &[
    ("4w", "Last 4 weeks"),
    ("12w", "Last 12 weeks"),
    ("26w", "Last 26 weeks"),
];

/// Windows for a grouping by model, agent, provider, tier, or area.
const GROUP_WINDOWS: &[(&str, &str)] = &[
    ("24h", "Last 24 hours"),
    ("7d", "Last 7 days"),
    ("30d", "Last 30 days"),
    ("all", "All time"),
];

/// The windows that fit a grouping: days over days, weeks over weeks, any span for a group.
/// The core reads every id (`Nh`, `Nd`, `Nw`, `all`).
pub fn since_options(by: &str) -> &'static [(&'static str, &'static str)] {
    match by {
        "day" => DAY_WINDOWS,
        "week" => WEEK_WINDOWS,
        _ => GROUP_WINDOWS,
    }
}

/// The window a grouping opens on.
pub fn default_since(by: &str) -> &'static str {
    match by {
        "week" => "12w",
        _ => "7d",
    }
}

/// `since` when it fits `by`, else the grouping's default.
pub fn fitting_since<'a>(by: &str, since: &'a str) -> &'a str {
    if since_options(by).iter().any(|(id, _)| *id == since) {
        since
    } else {
        default_since(by)
    }
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Column bars scale into 100px, and a zero period still draws a 4px mark.
const COLUMN_MIN_FRACTION: f32 = 0.04;
/// Grouped bars use a percent width, with a 2% minimum.
const GROUP_MIN_FRACTION: f32 = 0.02;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Clone, Debug, PartialEq)]
pub struct PeriodPoint {
    pub key: String,
    pub runs: i32,
}

/// One painted chart mark. `fraction` is the settled bar extent, 0 to 1.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartBar {
    pub key: String,
    pub label: String,
    pub runs: i32,
    pub fraction: f32,
    pub color_index: usize,
}

pub fn is_time_series(by: &str) -> bool {
    by == "day" || by == "week"
}

pub fn by_label(by: &str) -> String {
    option_label(BY_OPTIONS, by)
}

pub fn since_label(since: &str) -> String {
    [DAY_WINDOWS, WEEK_WINDOWS, GROUP_WINDOWS]
        .iter()
        .find_map(|options| options.iter().find(|(id, _)| *id == since))
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| since.to_string())
}

fn option_label(options: &[(&str, &str)], value: &str) -> String {
    options
        .iter()
        .find(|(id, _)| *id == value)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| value.to_string())
}

pub fn empty_rank() -> DispatchStatRank {
    DispatchStatRank {
        reason: "No runs in range".to_string(),
        ..DispatchStatRank::default()
    }
}

pub fn empty_stats(by: &str, since: &str) -> StatsResponse {
    StatsResponse {
        by: by.to_string(),
        since: since.to_string(),
        in_flight: 0,
        rows: Vec::new(),
        rankings: DispatchStatsRankings {
            most_used_model: empty_rank(),
            most_used_provider: empty_rank(),
            slowest_model: empty_rank(),
            most_failed_model: empty_rank(),
        },
    }
}

/// A window's length in `unit`-sized steps: `Nd` and `Nw` ids, rounded up to whole steps.
fn window_steps(since: &str, unit_days: i64) -> Option<i64> {
    let (count, scale) = if let Some(days) = since.strip_suffix('d') {
        (days, 1)
    } else if let Some(weeks) = since.strip_suffix('w') {
        (weeks, 7)
    } else {
        return None;
    };
    let days = count.parse::<i64>().ok()?.checked_mul(scale)?;
    (days > 0).then(|| (days + unit_days - 1) / unit_days)
}

/// Every period of a window, oldest first, at zero: a day chart draws one per day of its
/// window (7 when it has no length), a week chart one per week touched (8 for all time).
/// Any other grouping is empty.
pub fn empty_periods(by: &str, since: &str, now_ms: i64) -> Vec<PeriodPoint> {
    if by == "day" {
        let days = window_steps(since, 1).unwrap_or(7);
        return (0..days)
            .map(|index| PeriodPoint {
                key: utc_date_key(now_ms - (days - 1 - index) * DAY_MS),
                runs: 0,
            })
            .collect();
    }
    if by == "week" {
        let weeks = window_steps(since, 7).unwrap_or(8);
        return (0..weeks)
            .map(|index| PeriodPoint {
                key: iso_week_key(now_ms - (weeks - 1 - index) * 7 * DAY_MS),
                runs: 0,
            })
            .collect();
    }
    Vec::new()
}

/// The chart's points. A time chart spans its whole window, the periods with no runs at
/// zero, so a longer window shows as more bars; a grouping lists its rows.
pub fn chart_columns(
    by: &str,
    since: &str,
    rows: &[DispatchStatsRow],
    now_ms: i64,
) -> Vec<PeriodPoint> {
    if !is_time_series(by) {
        return rows
            .iter()
            .map(|row| PeriodPoint {
                key: row.key.clone(),
                runs: row.runs,
            })
            .collect();
    }
    let mut points = empty_periods(by, since, now_ms);
    for row in rows {
        match points.iter_mut().find(|point| point.key == row.key) {
            Some(point) => point.runs = row.runs,
            None => points.push(PeriodPoint {
                key: row.key.clone(),
                runs: row.runs,
            }),
        }
    }
    // Day and week keys sort as text.
    points.sort_by(|left, right| left.key.cmp(&right.key));
    points
}

pub fn chart_bars(by: &str, since: &str, rows: &[DispatchStatsRow], now_ms: i64) -> Vec<ChartBar> {
    let time = is_time_series(by);
    if !time && rows.is_empty() {
        return Vec::new();
    }
    let points = chart_columns(by, since, rows, now_ms);
    let max = max_runs(points.iter().map(|point| point.runs));
    let minimum = if time {
        COLUMN_MIN_FRACTION
    } else {
        GROUP_MIN_FRACTION
    };
    points
        .into_iter()
        .enumerate()
        .map(|(index, point)| ChartBar {
            label: if time {
                period_label(&point.key)
            } else {
                display_key(&point.key)
            },
            fraction: bar_fraction(point.runs, max, minimum),
            color_index: if time { 0 } else { index },
            key: point.key,
            runs: point.runs,
        })
        .collect()
}

/// Above this many columns, a column's tick is cut short.
const DENSE_COLUMNS: usize = 10;

/// The ticks under a time chart's columns. Past ten columns a day keeps only its day of the
/// month, with the month on the first column and where a month starts, and a week drops its
/// year; their full names are in [`column_title`].
pub fn column_ticks(bars: &[ChartBar]) -> Vec<String> {
    let dense = bars.len() > DENSE_COLUMNS;
    let mut month = None;
    bars.iter()
        .map(|bar| {
            if let Some((_, m, d)) = date_parts(&bar.key) {
                let starts = month != Some(m);
                month = Some(m);
                if dense && !starts {
                    return d.to_string();
                }
                return bar.label.clone();
            }
            if let Some((_, week)) = week_parts(&bar.key) {
                return format!("W{week}");
            }
            bar.label.clone()
        })
        .collect()
}

/// A column's full name and runs, for its tooltip: "Oct 21, 2026 · 3 runs".
pub fn column_title(bar: &ChartBar) -> String {
    let runs = if bar.runs == 1 {
        "1 run".to_string()
    } else {
        format!("{} runs", bar.runs)
    };
    let name = if let Some((y, m, d)) = date_parts(&bar.key) {
        format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
    } else if let Some((y, week)) = week_parts(&bar.key) {
        format!("Week {week} of {y}")
    } else {
        bar.label.clone()
    };
    format!("{name} · {runs}")
}

/// A `YYYY-MM-DD` key's year, month, and day.
fn date_parts(key: &str) -> Option<(i32, u32, u32)> {
    let bytes = key.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (y, m, d) = (
        key[0..4].parse::<i32>().ok()?,
        key[5..7].parse::<u32>().ok()?,
        key[8..10].parse::<u32>().ok()?,
    );
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// A `YYYY-Www` key's year and week.
fn week_parts(key: &str) -> Option<(i32, u32)> {
    let (year, week) = key.split_once("-W")?;
    Some((year.parse().ok()?, week.parse().ok()?))
}

pub fn max_runs(values: impl IntoIterator<Item = i32>) -> i32 {
    values.into_iter().max().unwrap_or(0).max(1)
}

pub fn bar_fraction(runs: i32, max_runs: i32, minimum: f32) -> f32 {
    let max_runs = max_runs.max(1) as f32;
    (runs as f32 / max_runs).clamp(minimum, 1.0)
}

pub fn display_key(key: &str) -> String {
    if key.is_empty() || key == "unknown" {
        "Unknown".to_string()
    } else {
        key.to_string()
    }
}

/// A day key (`2026-10-31`) reads `Oct 31`. A week key stays as the core sent it.
pub fn period_label(key: &str) -> String {
    let bytes = key.as_bytes();
    if bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' {
        if let (Ok(year), Ok(month), Ok(day)) = (
            key[0..4].parse::<i32>(),
            key[5..7].parse::<u32>(),
            key[8..10].parse::<u32>(),
        ) {
            if (1..=12).contains(&month) && (1..=31).contains(&day) && year >= 0 {
                return format!("{} {day}", MONTHS[(month - 1) as usize]);
            }
        }
    }
    display_key(key)
}

pub fn plural(count: i32, word: &str) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        format!("{count} {word}s")
    }
}

pub fn format_duration_ms(ms: Option<f64>) -> String {
    let Some(ms) = ms else {
        return "unknown".to_string();
    };
    if !ms.is_finite() {
        return "unknown".to_string();
    }
    if ms < 1000.0 {
        return format!("{}ms", ms.round() as i64);
    }
    let seconds = ms / 1000.0;
    if seconds < 60.0 {
        return format!("{seconds:.1}s");
    }
    let minutes = (seconds / 60.0).floor() as i64;
    let rem = (seconds % 60.0).round() as i64;
    if rem > 0 {
        format!("{minutes}m {rem}s")
    } else {
        format!("{minutes}m")
    }
}

pub fn format_pct(pct: f64, digits: usize) -> String {
    if !pct.is_finite() {
        return "0%".to_string();
    }
    if digits == 0 {
        format!("{}%", pct.round() as i64)
    } else {
        format!("{pct:.1}%")
    }
}

pub fn millis(value: Option<ProtoI64>) -> Option<f64> {
    value.map(|ms| ms.0 as f64)
}

/// The core's reason a ranking has no answer, in plain words.
pub fn rank_reason(reason: &str) -> String {
    if let Some(needed) = minimum_samples(reason) {
        if reason.trim_start().starts_with("needs two models") {
            return format!("Needs two models with {needed} runs");
        }
        return format!("Needs {needed} runs to rank");
    }
    let text = reason.trim();
    if text.is_empty() {
        return "No runs in range".to_string();
    }
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return "No runs in range".to_string();
    };
    let mut out = first.to_uppercase().collect::<String>();
    out.push_str(chars.as_str());
    out
}

fn minimum_samples(reason: &str) -> Option<String> {
    let rest = reason.split("minimum ").nth(1)?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

pub fn rank_is_known(rank: &DispatchStatRank) -> bool {
    rank.reason.trim().is_empty() && !rank.key.is_empty() && rank.key != "unknown"
}

pub fn most_used_detail(rank: &DispatchStatRank) -> String {
    plural(rank.runs, "run")
}

/// The core times only completed runs (the heading's tooltip says so); a card has room for
/// little more than the figure.
pub fn slowest_detail(rank: &DispatchStatRank) -> String {
    format!(
        "p50 {} over {}",
        format_duration_ms(millis(rank.p50_ms)),
        plural(rank.samples, "run")
    )
}

/// How many of the runs the model ended itself failed, read back from the success rate the
/// core rounds to a tenth of a percent.
pub fn most_failed_detail(rank: &DispatchStatRank) -> String {
    match rank.success_pct {
        Some(pct) if rank.runs > 0 => {
            let passed = (rank.runs as f64 * pct / 100.0).round() as i64;
            let failed = (rank.runs as i64 - passed).max(0);
            format!("{failed} of {} failed", plural(rank.runs, "run"))
        }
        _ => plural(rank.runs, "run"),
    }
}

/// What each comparison ranking reads, for its card's tooltip.
pub const SLOWEST_ABOUT: &str = "The median duration of completed runs for each model, among models with at least 5 completed runs. Failed, cancelled, timed-out, and still-running runs are excluded. Larger tasks take longer, so this describes your runs, not the model's speed.";

/// The breakdown table's columns, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatsColumn {
    Key,
    Runs,
    Success,
    FirstTry,
    P50,
    P90,
    Fallbacks,
    RateLimits,
    Orphans,
}

pub const STATS_COLUMNS: [StatsColumn; 9] = [
    StatsColumn::Key,
    StatsColumn::Runs,
    StatsColumn::Success,
    StatsColumn::FirstTry,
    StatsColumn::P50,
    StatsColumn::P90,
    StatsColumn::Fallbacks,
    StatsColumn::RateLimits,
    StatsColumn::Orphans,
];

impl StatsColumn {
    /// The header; the key's is the grouping's label.
    pub fn title(self) -> &'static str {
        match self {
            StatsColumn::Key => "",
            StatsColumn::Runs => "Runs",
            StatsColumn::Success => "Success",
            StatsColumn::FirstTry => "First try",
            StatsColumn::P50 => "p50",
            StatsColumn::P90 => "p90",
            StatsColumn::Fallbacks => "Fallbacks",
            StatsColumn::RateLimits => "Rate limits",
            StatsColumn::Orphans => "Orphans",
        }
    }

    /// The figure columns' `width` (in rem, at 13px) and `minWidth`; the key has neither.
    pub fn width(self) -> Option<(f32, f32)> {
        let rem = |value: f32| value * 13.0;
        match self {
            StatsColumn::Key => None,
            StatsColumn::Runs => Some((rem(4.25), 56.0)),
            StatsColumn::Success | StatsColumn::FirstTry => Some((rem(5.25), 84.0)),
            StatsColumn::P50 | StatsColumn::P90 | StatsColumn::RateLimits => Some((rem(6.0), 96.0)),
            StatsColumn::Fallbacks => Some((rem(5.5), 88.0)),
            StatsColumn::Orphans => Some((rem(5.0), 80.0)),
        }
    }

    /// Whether the compact table, under 560px, keeps the column.
    pub fn in_compact(self) -> bool {
        matches!(
            self,
            StatsColumn::Key | StatsColumn::Runs | StatsColumn::Success
        )
    }

    /// A row's cell, as the table shows it.
    pub fn cell(self, row: &DispatchStatsRow, time: bool) -> String {
        match self {
            StatsColumn::Key if time => period_label(&row.key),
            StatsColumn::Key => display_key(&row.key),
            StatsColumn::Runs => row.runs.to_string(),
            StatsColumn::Success => format_pct(row.success_pct, 0),
            StatsColumn::FirstTry => format_pct(row.first_try_pct, 0),
            StatsColumn::P50 => format_duration_ms(millis(row.p50_ms)),
            StatsColumn::P90 => format_duration_ms(millis(row.p90_ms)),
            StatsColumn::Fallbacks => row.fallbacks.to_string(),
            StatsColumn::RateLimits => row.rate_limits.to_string(),
            StatsColumn::Orphans => row.orphans.to_string(),
        }
    }

    /// The column's `sortBy`: the raw key, the figures, and a missing percentile as -1.
    pub fn sort_value(self, row: &DispatchStatsRow) -> SortValue {
        let percentile = |ms: Option<ProtoI64>| millis(ms).unwrap_or(-1.0);
        SortValue::Number(match self {
            StatsColumn::Key => return SortValue::Text(row.key.clone()),
            StatsColumn::Runs => row.runs as f64,
            StatsColumn::Success => row.success_pct,
            StatsColumn::FirstTry => row.first_try_pct,
            StatsColumn::P50 => percentile(row.p50_ms),
            StatsColumn::P90 => percentile(row.p90_ms),
            StatsColumn::Fallbacks => row.fallbacks as f64,
            StatsColumn::RateLimits => row.rate_limits as f64,
            StatsColumn::Orphans => row.orphans as f64,
        })
    }
}

/// The compact key cell's two lines of figures.
pub fn compact_lines(row: &DispatchStatsRow) -> [String; 2] {
    [
        format!(
            "First try {} / p50 {} / p90 {}",
            format_pct(row.first_try_pct, 1),
            format_duration_ms(millis(row.p50_ms)),
            format_duration_ms(millis(row.p90_ms)),
        ),
        format!(
            "{} fallbacks / {} rate limits / {} orphans",
            row.fallbacks, row.rate_limits, row.orphans
        ),
    ]
}

pub fn utc_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn utc_date_key(ms: i64) -> String {
    let (year, month, day) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{year:04}-{month:02}-{day:02}")
}

/// ISO week of the UTC calendar date, `YYYY-Www`, matching the core and the original page.
fn iso_week_key(utc_ms: i64) -> String {
    let day_index = utc_ms.div_euclid(DAY_MS);
    // Unix day 0 is Thursday. JavaScript's getUTCDay uses 0 for Sunday.
    let js_weekday = (day_index + 4).rem_euclid(7);
    let iso_weekday = if js_weekday == 0 { 7 } else { js_weekday };
    let thursday = day_index + 4 - iso_weekday;
    let (year, _, _) = civil_from_days(thursday);
    let year_start = days_from_civil(year, 1, 1);
    let week = (thursday - year_start + 1 + 6) / 7;
    format!("{year}-W{week:02}")
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    if month <= 2 {
        year -= 1;
    }
    let year = i64::from(year);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + u64::from(doy);
    era * 146097 + doe as i64 - 719468
}

fn civil_from_days(mut z: i64) -> (i32, u32, u32) {
    z += 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    #[test]
    fn many_columns_cut_their_ticks_short_and_keep_full_titles() {
        let bar = |key: &str, runs| super::ChartBar {
            key: key.into(),
            label: super::period_label(key),
            runs,
            fraction: 0.0,
            color_index: 0,
        };
        let few = vec![bar("2026-10-30", 0), bar("2026-10-31", 1)];
        assert_eq!(super::column_ticks(&few), ["Oct 30", "Oct 31"]);
        let many: Vec<_> = (20..=31)
            .map(|d| bar(&format!("2026-10-{d}"), 0))
            .chain([bar("2026-11-01", 2)])
            .collect();
        let ticks = super::column_ticks(&many);
        assert_eq!(ticks[0], "Oct 20");
        assert_eq!(ticks[1], "21");
        assert_eq!(ticks.last().unwrap(), "Nov 1");
        assert_eq!(super::column_title(&many[12]), "Nov 1, 2026 · 2 runs");
        let week = bar("2026-W45", 1);
        assert_eq!(super::column_ticks(std::slice::from_ref(&week)), ["W45"]);
        assert_eq!(super::column_title(&week), "Week 45 of 2026 · 1 run");
    }

    use super::{
        STATS_COLUMNS, StatsColumn, bar_fraction, chart_bars, compact_lines, days_from_civil,
        display_key, empty_periods, format_duration_ms, format_pct, most_failed_detail,
        period_label, plural, rank_is_known, rank_reason, slowest_detail, utc_date_key,
    };
    use crate::client::{DispatchStatRank, DispatchStatsRow, ProtoI64};
    use crate::models::sort::SortValue;

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
        days_from_civil(year, month, day) * super::DAY_MS
            + i64::from(((hour * 60 + minute) * 60 + second) * 1000)
    }

    #[test]
    fn civil_days_match_unix_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
        assert_eq!(utc_date_key(at(2026, 11, 1, 12, 0, 0)), "2026-11-01");
    }

    #[test]
    fn empty_window_lists_zero_periods_keyed_like_the_core() {
        let now = at(2026, 11, 1, 12, 0, 0);
        let days = empty_periods("day", "7d", now);
        assert_eq!(days.len(), 7);
        assert_eq!(days[0].key, "2026-10-26");
        assert_eq!(days[0].runs, 0);
        assert_eq!(days[6].key, "2026-11-01");
        assert_eq!(days[6].runs, 0);
        assert_eq!(empty_periods("day", "30d", now).len(), 30);
        assert_eq!(empty_periods("day", "all", now).len(), 7);

        assert_eq!(
            empty_periods("week", "7d", now),
            vec![super::PeriodPoint {
                key: "2026-W44".to_string(),
                runs: 0,
            }]
        );
        assert_eq!(
            empty_periods("week", "30d", now)
                .into_iter()
                .map(|row| row.key)
                .collect::<Vec<_>>(),
            vec!["2026-W40", "2026-W41", "2026-W42", "2026-W43", "2026-W44"]
        );
        assert_eq!(empty_periods("week", "all", now).len(), 8);
        assert!(empty_periods("model", "7d", now).is_empty());
        assert_eq!(empty_periods("day", "14d", now).len(), 14);
        assert_eq!(empty_periods("week", "12w", now).len(), 12);
        assert_eq!(empty_periods("week", "4w", now).len(), 4);
    }

    #[test]
    fn a_time_chart_spans_its_window_around_the_days_that_ran() {
        let now = at(2026, 11, 1, 12, 0, 0);
        let rows = vec![crate::client::DispatchStatsRow {
            key: "2026-10-31".into(),
            runs: 5,
            ..crate::client::DispatchStatsRow::default()
        }];
        let week = super::chart_columns("day", "7d", &rows, now);
        assert_eq!(week.len(), 7);
        assert_eq!(week[5].key, "2026-10-31");
        assert_eq!(week[5].runs, 5);
        assert_eq!(super::chart_columns("day", "30d", &rows, now).len(), 30);
        assert_eq!(super::chart_columns("model", "7d", &rows, now).len(), 1);
    }

    #[test]
    fn each_grouping_offers_the_windows_that_fit_it() {
        use super::{default_since, fitting_since, since_label, since_options};
        assert_eq!(since_options("day")[1].0, "14d");
        assert_eq!(since_options("week")[0].0, "4w");
        assert_eq!(since_options("model").last().unwrap().0, "all");
        assert_eq!(default_since("week"), "12w");
        assert_eq!(fitting_since("week", "7d"), "12w");
        assert_eq!(fitting_since("model", "30d"), "30d");
        assert_eq!(fitting_since("day", "all"), "7d");
        assert_eq!(since_label("12w"), "Last 12 weeks");
    }

    #[test]
    fn empty_time_chart_keeps_zero_bars_and_groups_stay_empty() {
        let now = at(2026, 11, 1, 12, 0, 0);
        let days = chart_bars("day", "7d", &[], now);
        assert_eq!(days.len(), 7);
        assert_eq!(days[0].key, "2026-10-26");
        assert_eq!(days[0].label, "Oct 26");
        assert_eq!(days[0].runs, 0);
        assert_eq!(days[0].fraction, 0.04);
        assert!(chart_bars("model", "7d", &[], now).is_empty());
    }

    #[test]
    fn duration_percent_and_rank_copy_match_the_page() {
        assert_eq!(format_duration_ms(None), "unknown");
        assert_eq!(format_duration_ms(Some(f64::NAN)), "unknown");
        assert_eq!(format_duration_ms(Some(500.4)), "500ms");
        assert_eq!(format_duration_ms(Some(999.5)), "1000ms");
        assert_eq!(format_duration_ms(Some(1500.0)), "1.5s");
        assert_eq!(format_duration_ms(Some(60_000.0)), "1m");
        assert_eq!(format_duration_ms(Some(90_000.0)), "1m 30s");
        assert_eq!(format_pct(66.6, 0), "67%");
        assert_eq!(format_pct(12.34, 1), "12.3%");
        assert_eq!(plural(1, "run"), "1 run");
        assert_eq!(plural(0, "run"), "0 runs");
        assert_eq!(period_label("2026-10-31"), "Oct 31");
        assert_eq!(period_label("2026-W44"), "2026-W44");
        assert_eq!(display_key("unknown"), "Unknown");
        assert_eq!(display_key(""), "Unknown");
        assert_eq!(
            rank_reason("insufficient samples (minimum 5)"),
            "Needs 5 runs to rank"
        );
        assert_eq!(rank_reason("no runs in range"), "No runs in range");
        assert_eq!(rank_reason("  "), "No runs in range");

        let known = DispatchStatRank {
            key: "gpt".to_string(),
            runs: 4,
            samples: 6,
            p50_ms: Some(ProtoI64(1500)),
            success_pct: Some(66.4),
            reason: String::new(),
        };
        assert!(rank_is_known(&known));
        assert!(!rank_is_known(&DispatchStatRank {
            key: "unknown".to_string(),
            ..DispatchStatRank::default()
        }));
        assert_eq!(slowest_detail(&known), "p50 1.5s over 6 runs");
        assert_eq!(
            most_failed_detail(&DispatchStatRank {
                runs: 16,
                success_pct: Some(93.8),
                ..known.clone()
            }),
            "1 of 16 runs failed"
        );
        assert_eq!(
            rank_reason("needs two models with enough runs (minimum 5)"),
            "Needs two models with 5 runs"
        );
        assert_eq!(rank_reason("no failures in range"), "No failures in range");
        assert_eq!(
            most_failed_detail(&DispatchStatRank {
                runs: 2,
                success_pct: None,
                ..known.clone()
            }),
            "2 runs"
        );
        assert_eq!(bar_fraction(0, 1, 0.04), 0.04);
        assert_eq!(bar_fraction(3, 3, 0.02), 1.0);
    }

    #[test]
    fn table_cells_follow_the_original_columns() {
        let row = DispatchStatsRow {
            key: "2026-10-31".to_string(),
            runs: 2,
            success_pct: 50.4,
            first_try_pct: 12.34,
            p50_ms: Some(ProtoI64(1500)),
            p90_ms: None,
            fallbacks: 1,
            rate_limits: 0,
            orphans: 3,
            ..DispatchStatsRow::default()
        };
        let cells: Vec<String> = STATS_COLUMNS
            .iter()
            .map(|column| column.cell(&row, true))
            .collect();
        assert_eq!(
            cells,
            [
                "Oct 31", "2", "50%", "12%", "1.5s", "unknown", "1", "0", "3"
            ]
        );
        assert_eq!(StatsColumn::Key.cell(&row, false), "2026-10-31");
        let compact: Vec<StatsColumn> = STATS_COLUMNS
            .iter()
            .copied()
            .filter(|column| column.in_compact())
            .collect();
        assert_eq!(compact.len(), 3);
        assert_eq!(
            compact_lines(&row)[0],
            "First try 12.3% / p50 1.5s / p90 unknown"
        );
        assert_eq!(StatsColumn::P90.sort_value(&row), SortValue::Number(-1.0));
        assert_eq!(StatsColumn::Runs.width(), Some((55.25, 56.0)));
    }
}
