use dispatch_core::adapters::{acp, binaries, cli, livelog::LiveLog, proc::run_command};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[test]
fn new_uuid() {
    let uuid = cli::new_uuid();
    assert_eq!(uuid.len(), 36);
    assert_eq!(&uuid[14..15], "4");
}

#[test]
fn custom_agents() {
    assert!(acp::valid_custom_agent("codex", &["x".into()]).is_err());
    assert!(acp::valid_custom_agent("Bad Name", &["x".into()]).is_err());
    assert!(acp::valid_custom_agent("mine", &[]).is_err());
    acp::set_custom_agents(HashMap::from([
        (
            "mine".into(),
            vec!["/opt/mine/bin/mine".into(), "serve".into(), "--acp".into()],
        ),
        ("codex".into(), vec!["x".into()]),
    ]));
    assert_eq!(binaries::bin("mine"), "/opt/mine/bin/mine");
    assert_eq!(binaries::bin("codex"), "codex");
    assert_eq!(&acp::custom_agents()["mine"][1..], ["serve", "--acp"]);
    assert_eq!(
        acp::Spec::for_agent("mine", false).command,
        ["serve", "--acp"]
    );
    assert!(binaries::detect().iter().any(|d| d.agent == "mine"));
    acp::set_custom_agents(HashMap::new());
}

#[tokio::test]
async fn fail_fast_stops_hanging_agent() {
    if let Err(err) = dispatch_core::process::Tree::capture(std::process::id() as i32).await {
        assert!(
            err.to_string().contains("Operation not permitted")
                || err.to_string().contains("Permission denied"),
            "{err}"
        );
        eprintln!("skipped: process table unreadable");
        return;
    }
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "echo 'ActionRequiredError: buy credits' >&2; sleep 30",
    ]);
    let start = std::time::Instant::now();
    let output = run_command(
        cmd,
        None,
        CancellationToken::new(),
        LiveLog::default(),
        None,
    )
    .await
    .unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(output.fatal.contains("ActionRequiredError"));
}

#[tokio::test]
async fn print_cli_streams() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("pi");
    let sent = dir.path().join("sent");
    fs::write(&bin, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$SENT\"\nprintf '%s\\n' '{\"type\":\"session\",\"id\":\"pi-1\"}' '{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"A heading.\"}],\"usage\":{\"input\":10,\"output\":2}}}'\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    binaries::set_binaries(HashMap::from([(
        "pi".into(),
        bin.to_string_lossy().into_owned(),
    )]));
    let req = dispatch_core::adapters::Request {
        prompt: "Read README.md".into(),
        effort: "low".into(),
        read_only: true,
        dir: dir.path().to_string_lossy().into_owned(),
        env: vec![format!("SENT={}", sent.display())],
        ..Default::default()
    };
    let result = cli::run("pi", &req, "openai/gpt-x", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.output, "A heading.");
    assert_eq!(result.session_id, "pi-1");
    assert_eq!(result.usage.unwrap().context_tokens, 12);
    let args = fs::read_to_string(sent).unwrap();
    for want in [
        "--mode\njson\n",
        "--session-id\n",
        "--model\nopenai/gpt-x\n",
        "--thinking\nlow\n",
        "--tools\nread,grep,find,ls\n",
        "--\nRead README.md\n",
    ] {
        assert!(args.contains(want), "{want}: {args}");
    }
    binaries::set_binaries(HashMap::new());
}

#[test]
fn effort_application() {
    let req = dispatch_core::adapters::Request {
        effort: "xhigh".into(),
        ..Default::default()
    };
    assert_eq!(
        dispatch_core::adapters::codex::turn_params(&req, "t-1")["effort"],
        "xhigh"
    );
    assert_eq!(
        dispatch_core::adapters::probe::capabilities("codex").effort_mode,
        "config"
    );
    assert_eq!(
        dispatch_core::adapters::probe::capabilities("antigravity").effort_mode,
        "model-suffix"
    );
    for name in ["cursor", "grok"] {
        assert!(
            dispatch_core::adapters::probe::capabilities(name)
                .effort_mode
                .is_empty()
        );
    }
}
