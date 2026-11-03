use dispatch_core::adapters::{Request, binaries, codex};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn codex_run_streams_and_resumes_failed_turn() {
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    let binary = dir.path().join("codex");
    let log = dir.path().join("log");
    let prefix = format!("SENT='{}'\nexport SENT\n", sent.display());
    fs::write(
        &binary,
        include_str!("fixtures/adapters/codex.sh").replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\n{prefix}"),
            1,
        ),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    binaries::set_binaries(HashMap::from([(
        "codex".into(),
        binary.to_string_lossy().into_owned(),
    )]));
    let req = Request {
        prompt: "do it".into(),
        effort: "high".into(),
        read_only: true,
        dir: dir.path().to_string_lossy().into_owned(),
        log_path: log.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let result = codex::run(&req, "gpt-x", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.output, "Done.");
    assert_eq!(result.session_id, "th-1");
    assert_eq!(result.exit_code, 0);
    let usage = result.usage.unwrap();
    assert_eq!(
        (
            usage.input_tokens,
            usage.cache_read_tokens,
            usage.output_tokens,
            usage.reasoning_tokens,
            usage.context_tokens,
            usage.context_window
        ),
        (600, 900, 60, 20, 510, 272000)
    );
    let output = fs::read_to_string(log).unwrap();
    for s in [
        "item/agentMessage/delta",
        "\"delta\":\"Hel\"",
        "commandExecution",
        "turn/completed",
        "\"input\":600",
        "\"context\":510",
    ] {
        assert!(output.contains(s), "{s}: {output}");
    }
    for s in [
        "mcpServer",
        "tokenUsage",
        "userMessage",
        "threadId",
        "turnId",
    ] {
        assert!(!output.contains(s), "{s}: {output}");
    }
    let requests = fs::read_to_string(&sent).unwrap();
    for s in [
        "thread/start",
        "read-only",
        "approvalPolicy",
        "gpt-x",
        "high",
        "do it",
        "th-1",
    ] {
        assert!(requests.contains(s), "{s}: {requests}");
    }
    let req = Request {
        prompt: "again".into(),
        resume_session: "th-1".into(),
        env: vec!["STATUS=failed".into()],
        ..Default::default()
    };
    let (partial, status) = codex::run_outcome(&req, "", CancellationToken::new()).await;
    assert!(status.unwrap_err().to_string().contains("failed"));
    assert_eq!(partial.session_id, "th-1");
    let requests = fs::read_to_string(sent).unwrap();
    assert!(requests.contains("thread/resume"));
    assert!(requests.contains("workspace-write"));
    binaries::set_binaries(HashMap::new());
}
