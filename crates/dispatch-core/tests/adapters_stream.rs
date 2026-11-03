use dispatch_core::adapters::{cli::*, model::allowed_model, stream::*, usage::*};

#[test]
fn parsers() {
    assert_eq!(
        parse_cursor_models(
            "Available models\n\nauto - Auto (default)\ngrok-4.7-low - Grok 4.7  Low\n"
        ),
        ["auto", "grok-4.7-low"]
    );
    assert_eq!(
        parse_agy_models(
            "Fetching available models...\ngemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n"
        ),
        ["gemini-3.8-flash-low"]
    );
    assert_eq!(
        parse_grok_models(
            "Default model: grok-4.7\n\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.6\n"
        ),
        ["grok-4.7", "grok-4.6"]
    );
}
#[test]
fn claude_filter() {
    for id in [
        "claude-opus-5-5-low",
        "claude-fable-5-thinking-high",
        "x-Sonnet-y",
    ] {
        assert!(!allowed_model("cursor", id));
    }
    assert!(allowed_model("cursor", "grok-4.7-low"));
}
#[test]
fn parse_cursor() {
    let stream = include_str!("fixtures/adapters/cursor/stream.ndjson");
    let expected = "The file `a.txt` contains only the word “hello”.";
    assert_eq!(
        parse_cursor_stream(stream),
        (
            expected.into(),
            "c943abc3-0729-419a-a828-b5b3aec75982".into()
        )
    );
    let cut = &stream[..stream.rfind("{\"type\":\"result\"").unwrap()];
    assert_eq!(parse_cursor_stream(cut).0, expected);
    assert_eq!(
        parse_cursor_stream("plain text reply\n"),
        ("plain text reply".into(), String::new())
    );
}
#[test]
fn parse_grok() {
    let stream = include_str!("fixtures/adapters/grok/stream.ndjson");
    assert_eq!(
        parse_grok_stream(stream),
        (
            "The file says hello.".into(),
            "3de270cb-e48b-467c-88f2-f78d2eca2b97".into()
        )
    );
    assert_eq!(parse_grok_stream("{\"type\":\"text\",\"data\":\"Let me check.\"}\n{\"type\":\"tool_call\"}\n{\"type\":\"text\",\"data\":\"Done.\"}\n").0, "Done.");
    assert_eq!(
        parse_grok_stream(
            "{\"type\":\"text\",\"data\":\"Edited it.\"}\n{\"type\":\"tool_call\"}\n"
        )
        .0,
        "Edited it."
    );
}
#[test]
fn usage_from_streams() {
    let cursor = usage_cursor(include_str!("fixtures/adapters/cursor/stream.ndjson")).unwrap();
    assert_eq!(
        cursor,
        Usage {
            input_tokens: 14276,
            output_tokens: 190,
            cache_read_tokens: 21917,
            ..Usage::default()
        }
    );
    let stream = include_str!("fixtures/adapters/grok/stream.ndjson");
    let grok = usage_grok(stream).unwrap();
    assert_eq!(
        (
            grok.input_tokens,
            grok.cache_read_tokens,
            grok.output_tokens,
            grok.reasoning_tokens,
            grok.cost_usd
        ),
        (15418, 29056, 134, 64, Some(0.092336))
    );
    let cut = usage_grok(&stream[..stream.rfind("{\"type\":\"end\"").unwrap()]).unwrap();
    assert_eq!(
        (cut.input_tokens, cut.output_tokens, cut.cost_usd),
        (15418, 134, None)
    );
    let agy = AcpUsage {
        input_tokens: 16060,
        output_tokens: 210,
        cached_read_tokens: 9000,
        total_tokens: 16270,
        ..AcpUsage::default()
    }
    .normalize();
    assert_eq!(
        agy,
        Usage {
            input_tokens: 7060,
            cache_read_tokens: 9000,
            output_tokens: 210,
            ..Usage::default()
        }
    );
    let open = AcpUsage {
        input_tokens: 18996,
        output_tokens: 62,
        cached_read_tokens: 36992,
        thought_tokens: 8,
        total_tokens: 56058,
        ..AcpUsage::default()
    }
    .normalize();
    assert_eq!(
        open,
        Usage {
            input_tokens: 18996,
            cache_read_tokens: 36992,
            output_tokens: 70,
            reasoning_tokens: 8,
            ..Usage::default()
        }
    );
    assert!(usage_claude("plain text\n").is_none());
    assert!(
        usage_cursor("plain text\n").is_none()
            && usage_grok("plain text\n").is_none()
            && usage_pi("plain text\n").is_none()
    );
}
#[test]
fn claude_usage_counts_every_model() {
    let result = r#"{"type":"result","subtype":"success","result":"ok","total_cost_usd":0.5,"usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":30,"iterations":[{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":30}]},"modelUsage":{"claude-opus-5-5":{"inputTokens":10,"outputTokens":30,"cacheReadInputTokens":100,"cacheCreationInputTokens":20,"thinkingTokens":5,"contextWindow":1000000},"claude-haiku-5-5":{"inputTokens":7,"outputTokens":3,"cacheReadInputTokens":50,"cacheCreationInputTokens":0,"thinkingTokens":0,"contextWindow":200000}}}"#;
    assert_eq!(
        usage_claude(result),
        Some(Usage {
            input_tokens: 17,
            cache_read_tokens: 150,
            cache_write_tokens: 20,
            output_tokens: 33,
            reasoning_tokens: 5,
            cost_usd: Some(0.5),
            context_tokens: 160,
            context_window: 1000000
        })
    );
}
#[test]
fn claude_usage_of_a_cut_off_run() {
    let stream = include_str!("fixtures/adapters/claude/stream.ndjson");
    let cut = stream
        .lines()
        .filter(|s| !s.contains("\"total_cost_usd\""))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        usage_claude(&cut),
        Some(Usage {
            input_tokens: 4,
            cache_read_tokens: 63139,
            cache_write_tokens: 35541,
            output_tokens: 190,
            reasoning_tokens: 23,
            context_tokens: 56842,
            ..Usage::default()
        })
    );
    assert!(allowed_model("claude", "claude-opus-5-5"));
    assert!(!allowed_model("codex", "claude-opus-5-5"));
}
