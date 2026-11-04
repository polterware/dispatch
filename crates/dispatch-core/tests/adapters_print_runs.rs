use dispatch_core::adapters::{Request, binaries, claude_limits, cli, usage::Usage};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tokio_util::sync::CancellationToken;
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn setup(name: &str, dir: &std::path::Path) -> (Request, PathBuf) {
    let bin = dir.join(name);
    let sent = dir.join("sent");
    let stream = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/adapters/streams")
        .join(format!("{name}.ndjson"));
    fs::write(&bin, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SENT\"\n/bin/cat > \"$SENT.stdin\"\n/bin/cat \"$STREAM\"\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    binaries::set_binaries(HashMap::from([(
        name.into(),
        bin.to_string_lossy().into_owned(),
    )]));
    (
        Request {
            prompt: "Read README.md".into(),
            read_only: true,
            dir: dir.to_string_lossy().into_owned(),
            log_path: dir.join("run.agent.log").to_string_lossy().into_owned(),
            env: vec![
                format!("SENT={}", sent.display()),
                format!("STREAM={}", stream.display()),
            ],
            ..Default::default()
        },
        sent,
    )
}
#[tokio::test(flavor = "current_thread")]
async fn claude_run_reads_its_stream() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    claude_limits::set_state_dir(dir.path());
    let (mut req, sent) = setup("claude", dir.path());
    req.effort = "high".into();
    let res = cli::run("claude", &req, "claude-opus-5-5", CancellationToken::new())
        .await
        .unwrap();
    assert!(res.output.starts_with("The README has a \"Probe\" heading"));
    assert_eq!(res.session_id, "928305c0-0399-49d0-aa60-e86bf07821e2");
    let mut usage = res.usage.unwrap();
    assert!((0.3007..0.3008).contains(&usage.cost_usd.unwrap()));
    usage.cost_usd = None;
    assert_eq!(
        usage,
        Usage {
            input_tokens: 4,
            cache_read_tokens: 63139,
            cache_write_tokens: 35541,
            output_tokens: 190,
            reasoning_tokens: 23,
            context_tokens: 56842,
            context_window: 1000000,
            ..Default::default()
        }
    );
    let args = fs::read_to_string(&sent).unwrap();
    for want in [
        "-p\n",
        "--output-format\nstream-json\n",
        "--include-partial-messages\n",
        "--permission-mode\nplan\n",
        "--model\nclaude-opus-5-5\n",
        "--effort\nhigh\n",
    ] {
        assert!(args.contains(want), "{want}: {args}");
    }
    assert_eq!(
        fs::read_to_string(sent.with_extension("stdin")).unwrap(),
        "Read README.md"
    );
    let log = fs::read_to_string(&req.log_path).unwrap();
    for want in [
        "\"type\":\"text_delta\"",
        "\"type\":\"tool_use\"",
        "\"type\":\"tool_result\"",
        "\"type\":\"result\"",
        "\"dispatch\":\"usage\"",
        "\"context\":56842",
    ] {
        assert!(log.contains(want), "missing {want}");
    }
    for quiet in [
        "rate_limit_event",
        "\"signature\"",
        "\"subtype\":\"init\"",
        "message_start",
        "tool_use_result",
    ] {
        assert!(!log.contains(quiet), "unexpected {quiet}");
    }
    claude_limits::set_state_dir(dir.path());
    let at = chrono::DateTime::from_timestamp(1793580600 - 3600, 0).unwrap();
    let seen = claude_limits::observed(at);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].window, "five_hour");
    assert_eq!(seen[0].used, Some(24.0));
    assert_eq!(seen[1].window, "seven_day");
    assert_eq!(seen[1].used, Some(51.0));
    assert_eq!(claude_limits::quota(&seen).unwrap().used_percent, 51.0);
    assert_eq!(
        claude_limits::observed(chrono::DateTime::from_timestamp(1793580660, 0).unwrap()).len(),
        1
    );
    assert!(
        claude_limits::quota(&claude_limits::observed(
            chrono::DateTime::from_timestamp(1793772000, 0).unwrap()
        ))
        .is_err()
    );
    claude_limits::set_state_dir("");
    binaries::set_binaries(HashMap::new());
}
#[tokio::test(flavor = "current_thread")]
async fn pi_run_reads_its_stream() {
    let _guard = LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (mut req, sent) = setup("pi", dir.path());
    req.effort = "low".into();
    let res = cli::run("pi", &req, "openai/gpt-x", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(res.output, "A heading.");
    assert_eq!(res.session_id, "pi-1");
    let mut usage = res.usage.unwrap();
    assert!((0.00499..0.00501).contains(&usage.cost_usd.unwrap()));
    usage.cost_usd = None;
    assert_eq!(
        usage,
        Usage {
            input_tokens: 1240,
            cache_read_tokens: 2000,
            cache_write_tokens: 800,
            output_tokens: 42,
            context_tokens: 2052,
            ..Default::default()
        }
    );
    let args = fs::read_to_string(sent).unwrap();
    for want in [
        "--mode\njson\n",
        "--session-id\n",
        "--model\nopenai/gpt-x\n",
        "--thinking\nlow\n",
        "--tools\nread,grep,find,ls\n",
        "--\nRead README.md\n",
    ] {
        assert!(args.contains(want));
    }
    let log = fs::read_to_string(&req.log_path).unwrap();
    for want in [
        "\"delta\":\"A heading.\"",
        "\"type\":\"tool_execution_start\"",
        "\"type\":\"tool_execution_end\"",
        "\"dispatch\":\"usage\"",
    ] {
        assert!(log.contains(want), "missing {want}");
    }
    for quiet in [
        "tool_execution_update",
        "agent_start",
        "\"content\":[{\"type\":\"thinking\"",
    ] {
        assert!(!log.contains(quiet));
    }
    assert_eq!(
        dispatch_core::adapters::cli::parse_pi_models(
            "provider  model  context\nopenai gpt-x 400K\n"
        ),
        ["openai/gpt-x"]
    );
    // With no provider logged in, pi says so and lists no model: "No" and "models" are words.
    assert!(
        dispatch_core::adapters::cli::parse_pi_models(
            "No models available. Use /login to log into a provider via OAuth or API key. See:\n  /Users/someone/.pi/docs/providers.md\n"
        )
        .is_empty()
    );
    binaries::set_binaries(HashMap::new());
}
