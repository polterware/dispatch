//! Ported from runner_summary_test.go.

#[path = "runner_support.rs"]
mod support;

use dispatch_core::{
    adapters::model::{effort_of, provider_of},
    runner::{Options, SUMMARY_SCHEMA, classify_run, summarize},
    types::{Attempt, Run, STATUS_COMPLETED, STATUS_FAILED, STATUS_RUNNING},
};
use support::*;
use tokio_util::sync::CancellationToken;

#[test]
fn test_effort_and_provider() {
    assert_eq!(effort_of("gemini-3.8-flash-high"), "high");
    assert_eq!(effort_of("gpt-5.6-sol-xhigh"), "xhigh");
    assert_eq!(effort_of("grok-4.7-medium"), "medium");
    for m in ["grok-4.7", "grok-4.7-build-fast", "composer-2.5"] {
        assert_eq!(
            effort_of(m),
            "",
            "suffix that is not an effort must stay unknown: {m}"
        );
    }
    for (agent, model, want) in [
        ("antigravity", "gemini-3.8-flash-low", "google"),
        ("cursor", "grok-4.7-medium", "xai"),
        ("codex", "gpt-5.6-luna", "openai"),
        ("cursor", "composer-2.5", "cursor"),
        ("codex", "", "openai"),
        ("", "", ""),
    ] {
        assert_eq!(
            provider_of(agent, model),
            want,
            "provider_of({agent},{model})"
        );
    }
}

#[test]
fn test_classify_exit() {
    let failed = |f: fn(&mut Run)| {
        let mut r = Run {
            status: STATUS_FAILED.into(),
            ..Run::default()
        };
        f(&mut r);
        classify_run(&r)
    };
    assert_eq!(
        failed(|r| r.attempts = vec![Attempt {
            exit_class: "rate_limit".into(),
            ..Attempt::default()
        }]),
        "rate_limit"
    );
    assert_eq!(failed(|r| r.error = "panic: boom".into()), "crash");
    assert_eq!(
        failed(|r| r.error = "merge conflict in f.go".into()),
        "merge_conflict"
    );
    let done = |note: &str| {
        classify_run(&Run {
            status: STATUS_COMPLETED.into(),
            note: note.into(),
            ..Run::default()
        })
    };
    assert_eq!(done(""), "ok");
    assert_eq!(done("commit failed: CONFLICT (content)"), "merge_conflict");
}

#[test]
fn test_run_summary_omits_agent_text() {
    let rec = Run {
        id: "id1".into(),
        status: STATUS_COMPLETED.into(),
        agent: "codex".into(),
        model: "gpt-5.6-sol-xhigh".into(),
        tier: "hard".into(),
        provider: "openai".into(),
        effort: "xhigh".into(),
        output: "AGENT_FREE_TEXT_SHOULD_NOT_LEAK".into(),
        branch: "dispatch/id1".into(),
        worktree: "/tmp/wt".into(),
        run_ms: Some(1500),
        files_changed: Some(2),
        insertions: Some(10),
        deletions: Some(3),
        attempts: vec![Attempt {
            agent: "grok".into(),
            model: "grok-4.7".into(),
            error: "rate limit".into(),
            ..Attempt::default()
        }],
        ..Run::default()
    };
    let s = summarize(&rec);
    let b = serde_json::to_string(&s).unwrap();
    assert!(!b.contains("AGENT_FREE_TEXT"), "{b}");
    assert!(
        s.schema == SUMMARY_SCHEMA
            && s.status == STATUS_COMPLETED
            && s.agent == "codex"
            && s.model == "gpt-5.6-sol-xhigh"
            && s.tier == "hard",
        "{s:?}"
    );
    assert!(s.duration_ms == Some(1500) && s.attempts == 2, "{s:?}");
    assert!(
        s.files_changed == Some(2) && s.insertions == Some(10) && s.deletions == Some(3),
        "files"
    );
    assert_eq!(s.details["agent"], "dispatch result id1 --section agent");
    let text = s.text();
    assert!(
        text.matches('\n').count() == 3 && !text.contains("AGENT_FREE_TEXT"),
        "{text:?}"
    );
    assert!(text.contains("files=2 +10 -3"), "{text}");

    let unknown = summarize(&Run {
        id: "x".into(),
        status: STATUS_RUNNING.into(),
        ..Run::default()
    });
    let raw = serde_json::to_string(&unknown).unwrap();
    for field in [
        "durationMs",
        "filesChanged",
        "insertions",
        "deletions",
        "queuedMs",
        "runMs",
    ] {
        assert!(
            !raw.contains(&format!("\"{field}\"")),
            "unknown {field} leaked: {raw}"
        );
    }
    assert!(unknown.text().contains("unknown"), "{}", unknown.text());
    // RunMs 0 is a known duration even without timestamps.
    let zero = summarize(&Run {
        id: "z".into(),
        status: STATUS_COMPLETED.into(),
        files_changed: Some(0),
        insertions: Some(0),
        deletions: Some(0),
        run_ms: Some(0),
        finished_at: rec.finished_at.clone(),
        ..Run::default()
    });
    let raw = serde_json::to_string(&zero).unwrap();
    assert!(
        raw.contains("\"filesChanged\":0") && raw.contains("\"durationMs\":0"),
        "{raw}"
    );
}

#[tokio::test]
async fn test_telemetry_on_fallback() {
    let r = ran();
    let mut t = test_deps(vec![
        Fake::new("a", &r).fail("429 too many requests").arc(),
        Fake::new("b", &r).out("UNIQUE_AGENT_TEXT").arc(),
    ]);
    t.deps.table = table(&[(
        "easy",
        vec![
            entry("a", "gemini-3.8-flash-high"),
            entry("b", "gpt-5.6-terra"),
        ],
    )]);
    let rec = t
        .deps
        .run(
            &CancellationToken::new(),
            Options {
                tier: "easy".into(),
                prompt: "p".into(),
                read_only: true,
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert!(
        rec.status == STATUS_COMPLETED && rec.exit_class == "ok",
        "{rec:?}"
    );
    assert!(
        rec.provider == "openai" && rec.effort.is_empty() && rec.model == "gpt-5.6-terra",
        "provider/effort {rec:?}"
    );
    assert!(
        rec.queued_ms.is_some() && rec.run_ms.is_some(),
        "missing timings {:?} {:?}",
        rec.queued_ms,
        rec.run_ms
    );
    assert!(
        rec.files_changed.is_none(),
        "readonly run must leave the diff unknown"
    );
    assert!(
        rec.attempts.len() == 1
            && rec.attempts[0].duration_ms.is_some()
            && rec.attempts[0].exit_class == "rate_limit",
        "attempt {:?}",
        rec.attempts
    );
    let raw = serde_json::to_string(&summarize(&rec)).unwrap();
    assert!(!raw.contains("UNIQUE_AGENT_TEXT"), "{raw}");
    assert_eq!(summarize(&rec).attempts, 2);
}

#[tokio::test]
async fn test_write_run_records_diff_stat() {
    let (_dir, root) = repo(&[("README", "hi\n")]);
    let r = ran();
    let f = Fake::new("a", &r).out("ok\n").edit(|dir| {
        let _ = std::fs::write(format!("{dir}/extra.txt"), "x\n");
    });
    let t = test_deps(vec![f.arc()]);
    let rec = t
        .deps
        .run(
            &CancellationToken::new(),
            Options {
                tier: "easy".into(),
                prompt: "p".into(),
                dir: root,
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert!(
        rec.files_changed == Some(1) && rec.insertions == Some(1) && rec.deletions == Some(0),
        "diff {} files={:?} +{:?} -{:?}",
        rec.status,
        rec.files_changed,
        rec.insertions,
        rec.deletions
    );
}
