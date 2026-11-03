use chrono::{TimeZone, Utc};
use dispatch_core::adapters::claude_limits::{observed, quota, remember, samples, set_state_dir};
#[test]
fn persists_and_expires_observed_limits() {
    let temp = tempfile::tempdir().unwrap();
    set_state_dir(temp.path());
    let at = Utc.timestamp_opt(1793570000, 0).unwrap();
    let limits = serde_json::json!({"unifiedWindows":{"five_hour":{"utilization":0.24,"resetsAt":1793580600},"seven_day":{"utilization":0.51,"resetsAt":1793772000},"unsupported":{"utilization":0.9,"resetsAt":1793772000}}});
    remember(samples(&limits, at));
    set_state_dir(temp.path());
    let seen = observed(at);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].window, "five_hour");
    assert_eq!(seen[1].window, "seven_day");
    assert_eq!(quota(&seen).unwrap().used_percent, 51.0);
    assert_eq!(observed(Utc.timestamp_opt(1793580660, 0).unwrap()).len(), 1);
    assert!(quota(&observed(Utc.timestamp_opt(1793772000, 0).unwrap())).is_err());
    set_state_dir("");
}
