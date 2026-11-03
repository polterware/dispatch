use dispatch_core::adapters::{
    Request,
    acp::{self, Spec},
    binaries, cli, model,
};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn acp_run_streams_resumes_models_and_rejects_invalid_setup() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    let binary = dir.path().join("opencode");
    let script = format!("SENT='{}'\nexport SENT\n", sent.display());
    let fixture = include_str!("fixtures/adapters/acp.sh").replacen(
        "#!/bin/sh\n",
        &format!("#!/bin/sh\n{script}"),
        1,
    );
    fs::write(&binary, fixture).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    binaries::set_binaries(HashMap::from([
        ("opencode".into(), binary.to_string_lossy().into_owned()),
        ("kimi".into(), binary.to_string_lossy().into_owned()),
    ]));
    let log = dir.path().join("log");
    let request = Request {
        dir: dir.path().to_string_lossy().into_owned(),
        prompt: "look".into(),
        effort: "high".into(),
        read_only: true,
        log_path: log.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let result = acp::run("opencode", &request, "m-a", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.output, "Done.");
    assert_eq!(result.session_id, "s-1");
    assert_eq!(result.exit_code, 0);
    let usage = result.usage.unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.cache_read_tokens,
            usage.output_tokens,
            usage.reasoning_tokens,
            usage.context_tokens,
            usage.context_window,
            usage.cost_usd
        ),
        (1000, 4000, 60, 10, 5000, 200000, Some(0.25))
    );
    let log_text = fs::read_to_string(&log).unwrap();
    for value in [
        "agent_message_chunk",
        "Looking",
        "tool_call",
        "[note] read-only run: refused edit Edit README.md",
        "\"dispatch\":\"usage\"",
        "\"context\":5000",
    ] {
        assert!(log_text.contains(value), "{value}: {log_text}");
    }
    for value in ["available_commands_update", "usage_update", "sessionId"] {
        assert!(!log_text.contains(value), "{value}: {log_text}");
    }
    let sent_text = fs::read_to_string(&sent).unwrap();
    for value in [
        "session/new",
        "session/set_model",
        "\"modelId\":\"m-a\"",
        "\"configId\":\"effort\"",
        "session/set_mode",
        "\"modeId\":\"plan\"",
        "\"optionId\":\"no\"",
    ] {
        assert!(sent_text.contains(value), "{value}: {sent_text}");
    }
    let mut resumed = request.clone();
    resumed.resume_session = "s-0".into();
    resumed.env = vec![
        "STOP=refusal".into(),
        format!("PATH={}", std::env::var("PATH").unwrap()),
    ];
    resumed.log_path = dir
        .path()
        .join("resumed-log")
        .to_string_lossy()
        .into_owned();
    let (partial, status) = acp::run_outcome(
        &Spec::for_agent("opencode", true),
        &resumed,
        "",
        CancellationToken::new(),
    )
    .await;
    let err = status.unwrap_err();
    assert!(err.to_string().contains("refused"), "{err}");
    assert_eq!(partial.session_id, "s-0");
    assert!(partial.usage.unwrap().cost_usd.is_none());
    assert!(
        !fs::read_to_string(&resumed.log_path)
            .unwrap()
            .contains("OLD TURN")
    );
    assert!(fs::read_to_string(&sent).unwrap().contains("session/load"));
    let mut basic = request.clone();
    basic.read_only = false;
    assert!(
        acp::run("opencode", &basic, "m-z", CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("\"m-z\" is not offered")
    );
    let mut strict = Spec::for_agent("opencode", true);
    strict.read_mode = "ask".into();
    assert!(
        acp::run_spec(&strict, &request, "", CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("no ask mode")
    );
    assert!(
        acp::run(
            "opencode",
            &basic,
            "claude-opus-5-5",
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let ids = acp::session_models(&Spec::for_agent("kimi", true), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(ids, ["m-a", "m-b"]);
    let opencode = Spec::for_agent("opencode", true);
    assert_eq!(opencode.command, ["acp"]);
    assert_eq!(opencode.read_mode, "plan");
    assert_eq!(
        model::provider_of("opencode", "openrouter/some-model"),
        "openrouter"
    );
    assert_eq!(
        cli::parse_opencode_models("opencode/a\nnoise line\nx/y\n").len(),
        2
    );
    binaries::set_binaries(HashMap::new());
}
