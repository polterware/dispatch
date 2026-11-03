use dispatch_core::proto::{DispatchRun, DispatchUsage, WatchRunRequest};
use serde_json::json;

#[test]
fn proto_json_uses_camel_case_string_int64_and_omits_defaults() {
    let run = DispatchRun {
        id: "run".into(),
        queued_ms: Some(123),
        usage: Some(DispatchUsage {
            input_tokens: 9007199254740993,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_value(&run).unwrap(),
        json!({"id":"run","queuedMs":"123","usage":{"inputTokens":"9007199254740993"}})
    );
    let parsed: DispatchRun = serde_json::from_value(
        json!({"id":"run","queuedMs":"123","usage":{"inputTokens":"9007199254740993"}}),
    )
    .unwrap();
    assert_eq!(parsed, run);
    assert_eq!(
        serde_json::to_value(WatchRunRequest::default()).unwrap(),
        json!({})
    );
}
