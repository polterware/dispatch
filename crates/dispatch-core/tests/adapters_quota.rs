use chrono::{TimeZone, Utc};
use dispatch_core::adapters::quota::*;
#[test]
fn empirical_sample_omits_percent() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    let cool = empirical_sample(now, Some(now + chrono::Duration::hours(1)), 3);
    assert_eq!(
        (
            cool.used,
            cool.risk.as_str(),
            cool.confidence.as_str(),
            cool.source.as_str()
        ),
        (None, "cooldown", "empirical", "empirical")
    );
    assert!(cool.reset_at.is_some());
    let recent = empirical_sample(now, None, 2);
    assert_eq!(recent.risk, "recent_rate_limit");
    assert!(recent.reset_at.is_none());
    let none = empirical_sample(now, None, 0);
    assert_eq!(none.risk, "unknown");
    for sample in [cool, recent, none] {
        assert!(!serde_json::to_string(&sample).unwrap().contains("\"used\""));
    }
}
#[test]
fn codex_quota_samples() {
    let primary = CodexWindow {
        used_percent: 10.0,
        window_duration_mins: Some(300),
        resets_at: Some(1_780_000_000),
    };
    let secondary = CodexWindow {
        used_percent: 40.0,
        window_duration_mins: Some(10080),
        resets_at: Some(1_780_000_000),
    };
    let q = codex_quota(Some(&primary), Some(&secondary)).unwrap();
    assert_eq!(
        (q.used_percent, q.window_minutes, q.key.as_str()),
        (40.0, 10080, "codex")
    );
    let samples = codex_samples(Some(&primary), Some(&secondary)).unwrap();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0].window, "primary");
    assert_eq!(samples[1].window, "secondary");
    assert_eq!(
        (
            samples[1].source.as_str(),
            samples[1].confidence.as_str(),
            samples[1].used
        ),
        (SOURCE_CODEX_APP_SERVER, "official", Some(40.0))
    );
    assert_eq!(samples[1].duration.as_secs(), 10080 * 60);
    let json = serde_json::to_string(&samples[1]).unwrap();
    for key in [
        "window",
        "used",
        "resetAt",
        "duration",
        "source",
        "confidence",
    ] {
        assert!(json.contains(&format!("\"{key}\"")));
    }
    assert!(codex_quota(Some(&primary), Some(&CodexWindow::default())).is_err());
    let q = codex_quota(Some(&primary), None).unwrap();
    assert_eq!((q.used_percent, q.window_minutes), (10.0, 300));
}
const AGY: &str = r#"{"command":{"data":{"groups":[{"name":"Gemini Models","buckets":[{"window":"weekly","remaining_fraction":0.97,"reset_time":"2026-10-20T18:45:17Z"},{"window":"5h","remaining_fraction":1,"reset_time":"2026-10-15T08:05:33Z"}]},{"name":"Claude and GPT models","buckets":[{"window":"weekly","remaining_fraction":0.5,"reset_time":"2026-10-22T03:17:31Z"},{"window":"5h","remaining_fraction":0.05,"reset_time":"2026-10-15T08:17:31Z"}]}]}}}"#;
#[test]
fn parse_agy_usage_test() {
    let groups = parse_agy_usage(AGY.as_bytes()).unwrap();
    let q = &groups["Gemini Models"];
    assert_eq!(q.window_minutes, 10080);
    assert!(q.used_percent > 2.9 && q.used_percent < 3.1);
    assert_eq!(groups["Claude and GPT models"].window_minutes, 300);
}
#[test]
fn parse_agy_samples_test() {
    let samples = parse_agy_samples(AGY.as_bytes()).unwrap();
    assert_eq!(samples.len(), 4);
    for sample in &samples {
        assert!(sample.used.is_some());
        assert_eq!(sample.source, SOURCE_AGY_USAGE);
        assert_eq!(sample.confidence, "official");
    }
    let zero = samples
        .iter()
        .find(|s| s.window == "Gemini Models/5h")
        .unwrap();
    assert_eq!(zero.duration.as_secs(), 5 * 3600);
    assert_eq!(zero.used, Some(0.0));
    let week = samples
        .iter()
        .find(|s| s.window == "Claude and GPT models/weekly")
        .unwrap();
    assert_eq!(week.duration.as_secs(), 7 * 24 * 3600);
    assert_eq!(week.used, Some(50.0));
    assert!(serde_json::to_string(zero).unwrap().contains("\"used\":0"));
}
