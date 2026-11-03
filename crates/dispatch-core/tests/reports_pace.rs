use chrono::{DateTime, Duration as ChronoDuration, TimeZone, Utc};
use dispatch_core::{
    reports::{FAR, ON_TRACK, SLIGHTLY, Snapshot, historical, linear, snapshots_from_quota},
    types::QuotaSnapshot,
};
use std::time::Duration;

const WEEK_SECS: u64 = 7 * 24 * 3600;

fn week_snaps(end: DateTime<Utc>, final_pct: f64) -> Vec<Snapshot> {
    let mut out = Vec::new();
    let start = end - ChronoDuration::weeks(1);
    for h in (0..=168).step_by(12) {
        let t = start + ChronoDuration::hours(h);
        out.push(Snapshot {
            used_percent: final_pct * (h as f64) / 168.0,
            resets_at: end,
            window_minutes: 7 * 24 * 60,
            sampled_at: t,
        });
    }
    out
}

#[test]
fn test_linear() {
    let week = Duration::from_secs(WEEK_SECS);
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    let reset = now + ChronoDuration::seconds((WEEK_SECS / 2) as i64);

    let cases = [
        ("on track", 50.0, false, true, ON_TRACK),
        ("reserve", 20.0, false, true, FAR),
        ("deficit runs out", 80.0, true, false, FAR),
        ("slightly ahead", 54.0, true, false, SLIGHTLY),
    ];
    for (name, used, deficit, will_last, stage) in cases {
        let p = linear(used, week, reset, now);
        assert_eq!(p.deficit, deficit, "{name}: {p:?}");
        assert_eq!(p.will_last, will_last, "{name}: {p:?}");
        assert_eq!(p.stage, stage, "{name}: {p:?}");
    }
}

#[test]
fn test_linear_runs_out_in() {
    let week = Duration::from_secs(WEEK_SECS);
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
    let reset = now + ChronoDuration::seconds((WEEK_SECS / 2) as i64);
    let p = linear(80.0, week, reset, now);
    // 80% in 3.5d -> 20% left lasts 0.875d
    let want = Duration::from_secs_f64(0.875 * 24.0 * 3600.0);
    let diff = p.runs_out_in.abs_diff(want);
    assert!(
        diff <= Duration::from_secs(1),
        "runs_out_in = {:?}, want {:?}",
        p.runs_out_in,
        want
    );
}

#[test]
fn test_historical_needs_five_weeks() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
    let reset = now + ChronoDuration::days(3);
    let mut snaps = Vec::new();
    for i in 1..=4 {
        snaps.extend(week_snaps(reset - ChronoDuration::weeks(i), 100.0));
    }
    let h = historical(&snaps, 50.0, reset, now);
    assert!(!h.valid, "{h:?}");
    assert_eq!(h.weeks, 4);
}

#[test]
fn test_historical_risk() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
    let reset = now + ChronoDuration::days(3); // 4 of 7 days elapsed
    let build = |final_pct: f64| {
        let mut s = Vec::new();
        for i in 1..=6 {
            s.extend(week_snaps(reset - ChronoDuration::weeks(i), final_pct));
        }
        s
    };

    // Heavy history, already ahead of the typical curve: crossing is likely.
    let heavy = historical(&build(100.0), 80.0, reset, now);
    assert!(heavy.valid, "heavy: {heavy:?}");
    assert_eq!(heavy.weeks, 6);
    assert!(heavy.risk >= 0.8, "heavy: {heavy:?}");
    assert!(
        !heavy.runs_out_in.is_zero() && heavy.runs_out_in <= Duration::from_secs(3 * 24 * 3600),
        "runs_out_in {:?}",
        heavy.runs_out_in
    );

    // Light history: shifting to current usage still never crosses.
    let light = historical(&build(30.0), 20.0, reset, now);
    assert!(light.risk <= 0.2, "light: {light:?}");
    assert_eq!(light.runs_out_in, Duration::ZERO);
}

#[test]
fn test_snapshots_from_quota_skips_unknown_percent() {
    let snaps = snapshots_from_quota(&[
        QuotaSnapshot {
            agent: "codex".into(),
            used_percent: None,
            window_minutes: 10080,
            resets_at: "2026-10-18T00:00:00Z".into(),
            sampled_at: "2026-10-15T00:00:00Z".into(),
            ..Default::default()
        },
        QuotaSnapshot {
            agent: "codex".into(),
            used_percent: Some(42.5),
            window_minutes: 10080,
            resets_at: "2026-10-18T00:00:00Z".into(),
            sampled_at: "2026-10-15T00:00:00Z".into(),
            ..Default::default()
        },
    ]);
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].used_percent, 42.5);
    assert_eq!(snaps[0].window_minutes, 10080);
}
