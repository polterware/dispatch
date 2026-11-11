#[path = "runner_support.rs"]
mod support;

use chrono::Utc;
use dispatch_core::{
    adapters::{
        Capabilities,
        probe::{FlagCheck, ProbeReport},
        quota::SOURCE_EMPIRICAL,
    },
    runner::{
        AttemptError, CREDIT_COOLDOWN_FOR, Options, agent_log_path, empirical_quota, is_recursive,
        limit_notice, page, resets_in, store_cooldowns, summary,
    },
    types::{
        Attempt, Run, STATUS_COMPLETED, STATUS_FAILED, STATUS_ORPHANED, STATUS_RUNNING,
        STATUS_TIMED_OUT, Store,
    },
};
use std::collections::HashMap;
use std::time::Duration;
use support::*;
use tokio_util::sync::CancellationToken;

fn easy(prompt: &str) -> Options {
    Options {
        tier: "easy".into(),
        prompt: prompt.into(),
        ..Options::default()
    }
}

fn read_only(prompt: &str) -> Options {
    Options {
        read_only: true,
        ..easy(prompt)
    }
}

async fn run(t: &TestDeps, o: Options) -> Result<Run, dispatch_core::Error> {
    t.deps.run(&CancellationToken::new(), o).await
}

#[tokio::test]
async fn test_fallback() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r).fail("rate limited").arc(),
        Fake::new("b", &r).out("hello\n").arc(),
    ]);
    let rec = run(&t, easy("p")).await.unwrap();
    assert!(
        rec.status == STATUS_COMPLETED && rec.agent == "b" && rec.attempts.len() == 1,
        "{rec:?}"
    );
    assert_eq!(t.get(&rec.id).status, STATUS_COMPLETED);
}

#[tokio::test]
async fn test_all_fail() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r).fail("x").arc(),
        Fake::new("b", &r).fail("y").arc(),
    ]);
    let rec = run(&t, easy("p")).await.unwrap();
    assert!(
        rec.status == STATUS_FAILED && ran_list(&r).len() == 2,
        "{rec:?}"
    );
    assert_eq!(rec.error, "all 2 candidates failed; last: b/m2: y");
}

#[tokio::test]
async fn test_timeout_stops_fallback() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r)
            .error(AttemptError::DeadlineExceeded)
            .arc(),
        Fake::new("b", &r).out("x").arc(),
    ]);
    let o = Options {
        timeout: Duration::from_secs(1),
        ..easy("p")
    };
    let rec = run(&t, o).await.unwrap();
    assert!(
        rec.status == STATUS_TIMED_OUT && ran_list(&r).len() == 1,
        "{rec:?}"
    );
    assert_eq!(rec.error, "timeout after 1s");
}

#[tokio::test]
async fn test_recursion_refused() {
    let t = test_deps(vec![]);
    t.set_env("DISPATCH_DEPTH", "1");
    let err = run(&t, easy("")).await.unwrap_err();
    assert!(is_recursive(&err), "{err}");
}

#[tokio::test]
async fn test_no_delegate_refused() {
    let t = test_deps(vec![]);
    t.set_env("DISPATCH_NO_DELEGATE", "1");
    let err = run(&t, easy("")).await.unwrap_err();
    assert!(is_recursive(&err), "{err}");
}

#[tokio::test]
async fn test_child_environment_variables() {
    let r = ran();
    let f = Fake::new("a", &r).out("ok");
    let env = f.env.clone();
    let t = test_deps(vec![f.arc()]);
    t.set_env("DISPATCH_RUN_ID", "parent-run-456");
    let rec = run(&t, easy("p")).await.unwrap();
    let got: HashMap<String, String> = env
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| e.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
        .collect();
    for (k, want) in [
        ("DISPATCH_DEPTH", "1"),
        ("DISPATCH_RUN_ID", rec.id.as_str()),
        ("DISPATCH_PARENT_RUN_ID", "parent-run-456"),
        ("DISPATCH_NO_DELEGATE", "1"),
    ] {
        assert_eq!(got.get(k).map(String::as_str), Some(want), "env[{k}]");
    }
}

#[test]
fn test_page_and_summary() {
    let text = "a\nb\nc\nd\n";
    assert_eq!(page(text, 1, 2), ("b\nc\n".into(), 4, true));
    assert!(!page(text, 2, 10).2);
    assert_eq!(summary("l1\nl2\nl3\n", 2, 100), ("l1\nl2\n".into(), true));
    assert_eq!(summary("short", 10, 100), ("short".into(), false));
}

#[tokio::test]
async fn test_resume_uses_same_agent_and_session() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r).out("one").arc(),
        Fake::new("b", &r).out("two").arc(),
    ]);
    let first = run(&t, read_only("p")).await.unwrap();
    assert_eq!(first.session_id, "sess-a");
    clear(&r);
    let second = run(
        &t,
        Options {
            prompt: "follow up".into(),
            read_only: true,
            resume: first.id.clone(),
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert!(
        second.status == STATUS_COMPLETED && second.resumed_of == first.id,
        "{second:?}"
    );
    assert_eq!(ran_list(&r), vec!["a/m1@sess-a"]);
}

#[tokio::test]
async fn test_write_run_commits_on_its_branch() {
    let (_dir, root) = repo(&[("README", "hi\n")]);
    let r = ran();
    let f = Fake::new("a", &r).out("ok\n").edit(|dir| {
        std::fs::write(std::path::Path::new(dir).join("extra.txt"), "x\n").unwrap();
    });
    let t = test_deps(vec![f.arc()]);
    let rec = run(
        &t,
        Options {
            dir: root,
            ..easy("p")
        },
    )
    .await
    .unwrap();
    assert!(
        rec.status == STATUS_COMPLETED
            && rec.exit_class == "ok"
            && rec
                .note
                .contains(&format!("changes committed on {}", rec.branch)),
        "{rec:?}"
    );
    assert_eq!(rec.files_changed, Some(1));
}

#[tokio::test]
async fn test_rate_limit_sets_cooldown_and_demotes() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r).fail("429 too many requests").arc(),
        Fake::new("b", &r).out("ok").arc(),
    ]);
    let _ = run(&t, read_only("p")).await;
    assert!(store_cooldowns(Some(t.store.as_ref()), Utc::now()).contains_key("a"));
    clear(&r);
    let _ = run(&t, read_only("p")).await;
    let got = ran_list(&r);
    assert!(!got.is_empty() && got[0] == "b/m2", "ran = {got:?}");
}

#[tokio::test]
async fn test_read_only_without_support_is_refused() {
    let r = ran();
    let caps = Capabilities {
        file_editing: true,
        session_resume: true,
        ..Capabilities::default()
    };
    let t = test_deps(vec![
        Fake::new("a", &r).out("x").caps(caps.clone()).arc(),
        Fake::new("b", &r).out("y").caps(caps).arc(),
    ]);
    let rec = run(&t, read_only("p")).await.unwrap();
    assert!(
        ran_list(&r).is_empty()
            && rec.status == STATUS_FAILED
            && rec.error.contains("does not support readOnly"),
        "{rec:?}"
    );
    assert!(!rec.error.contains("no installed agent"), "{}", rec.error);
}

#[tokio::test]
async fn test_file_editing_without_support_is_refused() {
    let r = ran();
    let caps = Capabilities {
        read_only: true,
        session_resume: true,
        ..Capabilities::default()
    };
    let t = test_deps(vec![Fake::new("a", &r).out("x").caps(caps).arc()]);
    let rec = run(&t, easy("p")).await.unwrap();
    assert!(
        ran_list(&r).is_empty() && rec.status == STATUS_FAILED && rec.error.contains("fileEditing"),
        "{rec:?}"
    );
}

#[tokio::test]
async fn test_resume_without_support_is_refused() {
    let r = ran();
    let f = Fake::new("a", &r).out("one");
    let caps = f.caps.clone();
    let t = test_deps(vec![f.arc()]);
    let first = run(&t, read_only("p")).await.unwrap();
    assert_eq!(first.status, STATUS_COMPLETED);
    caps.lock().unwrap().session_resume = false;
    clear(&r);
    let second = run(
        &t,
        Options {
            prompt: "follow up".into(),
            read_only: true,
            resume: first.id.clone(),
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert!(
        ran_list(&r).is_empty()
            && second.status == STATUS_FAILED
            && second.error.contains("session resume"),
        "{second:?}"
    );
}

#[tokio::test]
async fn test_unverified_probe_still_runs() {
    let r = ran();
    let report = ProbeReport {
        status: "UNVERIFIED".into(),
        notes: vec!["version unknown".into()],
        ..ProbeReport::default()
    };
    let t = test_deps(vec![Fake::new("a", &r).out("ok").probe(report).arc()]);
    let rec = run(&t, read_only("p")).await.unwrap();
    assert!(
        rec.status == STATUS_COMPLETED && ran_list(&r).len() == 1,
        "{rec:?}"
    );
    assert!(!rec.note.contains("DEGRADED"), "{}", rec.note);
}

#[tokio::test]
async fn test_degraded_probe_still_runs_unless_capability_missing() {
    let report = ProbeReport {
        status: "DEGRADED".into(),
        notes: vec!["missing flags: --mode".into()],
        flags: vec![FlagCheck {
            name: "--mode".into(),
            present: false,
            capability: "readOnly".into(),
        }],
        ..ProbeReport::default()
    };
    {
        let r = ran();
        let t = test_deps(vec![
            Fake::new("a", &r).out("ok").probe(report.clone()).arc(),
        ]);
        let rec = run(
            &t,
            Options {
                no_worktree: true,
                ..easy("p")
            },
        )
        .await
        .unwrap();
        assert!(
            rec.status == STATUS_COMPLETED && ran_list(&r).len() == 1,
            "{rec:?}"
        );
        assert!(rec.note.contains("DEGRADED"), "note {:?}", rec.note);
    }
    {
        let r = ran();
        let t = test_deps(vec![Fake::new("a", &r).out("ok").probe(report).arc()]);
        let rec = run(&t, read_only("p")).await.unwrap();
        assert!(
            ran_list(&r).is_empty()
                && rec.status == STATUS_FAILED
                && rec.error.contains("readOnly")
                && rec.error.contains("--mode"),
            "{rec:?}"
        );
    }
}

#[test]
fn test_empirical_quota() {
    let st = FakeStore::new();
    let now = Utc::now();
    st.save_run(&Run {
        id: "1".into(),
        status: STATUS_FAILED.into(),
        agent: "cursor".into(),
        error: "429 too many requests".into(),
        created_at: rfc(now),
        finished_at: rfc(now),
        attempts: vec![Attempt {
            agent: "cursor".into(),
            error: "429 too many requests".into(),
            exit_class: "rate_limit".into(),
            ..Attempt::default()
        }],
        ..Run::default()
    })
    .unwrap();
    let s = empirical_quota(Some(st.as_ref()), "cursor", Some(now));
    assert!(
        s.used.is_none()
            && s.confidence == "empirical"
            && s.risk == "recent_rate_limit"
            && s.source == SOURCE_EMPIRICAL,
        "{s:?}"
    );
    st.set_cooldown("grok", &rfc(now + chrono::TimeDelta::hours(1)))
        .unwrap();
    let g = empirical_quota(Some(st.as_ref()), "grok", Some(now));
    assert!(
        g.used.is_none() && g.risk == "cooldown" && g.reset_at.is_some(),
        "{g:?}"
    );
    let raw = serde_json::to_string(&s).unwrap();
    assert!(!raw.contains("\"used\""), "{raw}");
}

#[tokio::test]
async fn test_cancel_orphan_and_wait() {
    let t = test_deps(vec![]);
    let cancel = CancellationToken::new();
    for id in ["r-1", "r-2"] {
        t.store
            .save_run(&Run {
                id: id.into(),
                status: STATUS_RUNNING.into(),
                pid: 999_999,
                created_at: rfc(Utc::now()),
                ..Run::default()
            })
            .unwrap();
    }
    let out = t.deps.cancel(&cancel, "r-1").await.unwrap();
    assert_eq!(out.status, STATUS_ORPHANED, "{out:?}");
    let (w, err) = t.deps.wait(&cancel, "r-2", Duration::from_millis(10)).await;
    assert!(err.is_ok() && w.status == STATUS_ORPHANED, "{w:?} {err:?}");
}

/// RunMs describes the attempt that finished the run, not earlier failures,
/// otherwise speed evidence blames the model that rescued the run.
#[tokio::test]
async fn test_run_ms_excludes_failed_fallback_attempts() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r)
            .fail("boom")
            .delay(Duration::from_millis(400))
            .arc(),
        Fake::new("b", &r).out("ok\n").arc(),
    ]);
    let rec = run(&t, easy("p")).await.unwrap();
    assert!(
        rec.status == STATUS_COMPLETED && rec.agent == "b",
        "{rec:?}"
    );
    assert!(
        rec.run_ms.is_some_and(|ms| ms < 200),
        "RunMs {:?} includes the failed attempt",
        rec.run_ms
    );
}

#[tokio::test]
async fn test_credit_error_cools_down_model_scope_and_marks_log() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r)
            .fail("a: stopped early: ActionRequiredError: m1 is not included in your weekly usage")
            .arc(),
        Fake::new("b", &r).out("ok").arc(),
    ]);
    let rec = run(&t, read_only("p")).await.unwrap();
    assert!(
        rec.status == STATUS_COMPLETED
            && rec.attempts.len() == 1
            && rec.attempts[0].exit_class == "credits",
        "{rec:?}"
    );
    let cool = store_cooldowns(Some(t.store.as_ref()), Utc::now());
    assert!(
        !cool.contains_key("a"),
        "credit error must not cool the whole agent"
    );
    let until = cool.get("a:m1").copied();
    assert!(
        until.is_some_and(|u| u > Utc::now() + CREDIT_COOLDOWN_FOR - chrono::TimeDelta::minutes(1)),
        "cooldowns = {cool:?}"
    );
    let log = std::fs::read_to_string(agent_log_path(&t.state_dir, &rec.id)).unwrap();
    assert!(
        log.contains("── dispatch: a/m1 failed (credits); trying b/m2 ──"),
        "log = {log:?}"
    );
    clear(&r);
    let _ = run(&t, read_only("p")).await;
    assert_eq!(ran_list(&r), vec!["b/m2"], "a/m1 should be skipped");
}

const AGY_QUOTA: &str = "⚠ Individual quota reached. Please upgrade your subscription to increase your limits. Resets in 4h54m49s.";

#[test]
fn test_limit_notice_is_a_short_reply_naming_a_reached_limit() {
    assert_eq!(
        limit_notice(&format!("\n{AGY_QUOTA}\n")).as_deref(),
        Some(AGY_QUOTA)
    );
    assert!(limit_notice("You've hit your usage limit. Try again later.").is_some());
    assert!(limit_notice("Usage limit reached for this week.").is_some());
    // An answer about quotas is not a notice.
    assert!(limit_notice("Done: the quota check now reads the remaining fraction.").is_none());
    assert!(
        limit_notice(&format!(
            "{} {AGY_QUOTA}",
            "Here is the change. ".repeat(30)
        ))
        .is_none()
    );
    assert!(limit_notice("").is_none());
    assert_eq!(
        resets_in(AGY_QUOTA),
        Some(Duration::from_secs(4 * 3600 + 54 * 60 + 49))
    );
    assert_eq!(resets_in("resets in 12m"), Some(Duration::from_secs(720)));
    assert_eq!(resets_in("no reset named"), None);
}

#[tokio::test]
async fn test_a_quota_reply_fails_the_attempt_and_falls_back() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r).out(AGY_QUOTA).arc(),
        Fake::new("b", &r).out("ok").arc(),
    ]);
    let rec = run(&t, read_only("p")).await.unwrap();
    assert!(
        rec.status == STATUS_COMPLETED
            && rec.agent == "b"
            && rec.attempts.len() == 1
            && rec.attempts[0].exit_class == "rate_limit",
        "{rec:?}"
    );
    // The agent waits until the reset the notice named, not the default hour.
    let until = store_cooldowns(Some(t.store.as_ref()), Utc::now())
        .get("a")
        .copied();
    assert!(
        until.is_some_and(|u| u > Utc::now() + chrono::TimeDelta::hours(4)),
        "cooldown until {until:?}"
    );
}

#[tokio::test]
async fn test_antigravity_quota_cools_only_that_provider() {
    let r = ran();
    let mut t = test_deps(vec![
        Fake::new("antigravity", &r).out(AGY_QUOTA).arc(),
        Fake::new("b", &r).out("ok").arc(),
    ]);
    t.deps.table = table(&[(
        "easy",
        vec![entry("antigravity", "claude-opus-5-5"), entry("b", "m2")],
    )]);
    let rec = run(&t, read_only("p")).await.unwrap();
    assert!(
        rec.agent == "b"
            && rec
                .attempts
                .first()
                .is_some_and(|a| a.exit_class == "rate_limit"),
        "{rec:?}"
    );
    let cool = store_cooldowns(Some(t.store.as_ref()), Utc::now());
    // Its Gemini models keep their own quota: only Anthropic's inside Antigravity waits.
    assert!(!cool.contains_key("antigravity"), "{cool:?}");
    assert!(cool.contains_key("antigravity:anthropic"), "{cool:?}");
}

#[tokio::test]
async fn test_a_run_that_changed_nothing_leaves_no_worktree() {
    let (_dir, root) = repo(&[("README", "hi\n")]);
    let r = ran();
    let t = test_deps(vec![Fake::new("a", &r).out("ok\n").arc()]);
    let rec = run(
        &t,
        Options {
            dir: root.clone(),
            ..easy("p")
        },
    )
    .await
    .unwrap();
    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    assert!(!rec.worktree.is_empty(), "the run had no worktree: {rec:?}");
    assert!(
        !std::path::Path::new(&rec.worktree).exists(),
        "the empty worktree stayed"
    );
    assert_eq!(git(&root, &["branch", "--list", &rec.branch]), "");
    assert!(rec.note.contains("no changes to merge"), "{}", rec.note);
    assert_eq!(rec.files_changed, Some(0));
    let tree = t.store.list_worktrees("").unwrap();
    assert!(
        tree.iter()
            .any(|w| w.run_id == rec.id && w.status == "removed"),
        "{tree:?}"
    );
}

#[tokio::test]
async fn test_a_follow_up_cuts_an_emptied_worktree_again_where_it_was() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (_dir, root) = repo(&[("README", "hi\n")]);
    let r = ran();
    let write = std::sync::Arc::new(AtomicBool::new(false));
    let edits = write.clone();
    let f = Fake::new("a", &r).out("ok\n").edit(move |dir| {
        if edits.load(Ordering::SeqCst) {
            std::fs::write(std::path::Path::new(dir).join("later.txt"), "x\n").unwrap();
        }
    });
    let t = test_deps(vec![f.arc()]);
    let first = run(
        &t,
        Options {
            dir: root.clone(),
            ..easy("p")
        },
    )
    .await
    .unwrap();
    assert!(!std::path::Path::new(&first.worktree).exists());
    write.store(true, Ordering::SeqCst);
    let second = run(
        &t,
        Options {
            prompt: "now write".into(),
            resume: first.id.clone(),
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(second.status, STATUS_COMPLETED, "{second:?}");
    // The agent's session was there: the follow-up works at the same path.
    assert_eq!(second.worktree, first.worktree);
    assert_eq!(second.branch, format!("dispatch/{}", second.id));
    assert!(
        std::path::Path::new(&second.worktree)
            .join("later.txt")
            .exists()
    );
    assert_eq!(second.files_changed, Some(1));
    assert!(!std::path::Path::new(&root).join("later.txt").exists());
}

#[tokio::test]
async fn test_rate_limit_builds_handoff_and_continues_from_it() {
    use dispatch_core::runner::{HANDOFF_FILE, HANDOFF_INSTRUCTION};
    use serde_json::json;

    let (_dir, root) = repo(&[("README", "hi\n")]);
    let stream_log = [
        json!({"type": "item.completed", "item": {"id": "c1", "type": "command_execution", "command": "cat README", "aggregated_output": "RAW_FIRST_TRANSCRIPT_TOOL_OUTPUT"}}).to_string(),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "TodoWrite", "input": {"todos": [
            {"content": "Write extra.txt", "status": "completed"},
            {"content": "Verify tests", "status": "in_progress"}
        ]}}]}}).to_string(),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Wrote extra.txt; hitting limit before tests."}]}}).to_string(),
        String::new(),
    ]
    .join("\n");

    let r = ran();
    let first = Fake::new("a", &r)
        .edit(|dir| {
            std::fs::write(std::path::Path::new(dir).join("extra.txt"), "hello\n").unwrap();
        })
        .log(&stream_log)
        .fail("429 rate limit exceeded");
    let second = Fake::new("b", &r).out("finished\n");
    let second_reqs = second.requests.clone();

    let mut t = test_deps(vec![first.arc(), second.arc()]);
    // Use the default repo-local worktree path so `.dispatch/` is excluded in the repo.
    t.deps.worktree_dir.clear();

    let rec = run(
        &t,
        Options {
            dir: root.clone(),
            ..easy("implement extra.txt and verify")
        },
    )
    .await
    .unwrap();

    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    assert_eq!(rec.agent, "b");
    assert_eq!(rec.attempts.len(), 1);

    let handoff = &rec.attempts[0].handoff;
    assert!(!handoff.is_empty(), "attempt handoff should be recorded");
    assert!(
        handoff.contains("implement extra.txt and verify"),
        "{handoff}"
    );
    assert!(handoff.contains("a/m1 (rate_limit)"), "{handoff}");
    assert!(handoff.contains("429 rate limit exceeded"), "{handoff}");
    assert!(handoff.contains("extra.txt (+1 -0)"), "{handoff}");
    assert!(handoff.contains("- [x] Write extra.txt"), "{handoff}");
    assert!(handoff.contains("- [~] Verify tests"), "{handoff}");
    assert!(
        handoff.contains("Wrote extra.txt; hitting limit before tests."),
        "{handoff}"
    );
    assert!(
        !handoff.contains("RAW_FIRST_TRANSCRIPT_TOOL_OUTPUT"),
        "hand-off must not include raw tool transcript: {handoff}"
    );

    let stored = t.get(&rec.id);
    assert_eq!(stored.attempts[0].handoff, *handoff);

    let reqs = second_reqs.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let second_prompt = &reqs[0].prompt;
    assert!(
        second_prompt.starts_with(HANDOFF_INSTRUCTION),
        "second prompt: {second_prompt}"
    );
    assert!(
        second_prompt.contains(handoff),
        "second prompt: {second_prompt}"
    );
    assert!(!second_prompt.contains("RAW_FIRST_TRANSCRIPT_TOOL_OUTPUT"));

    let handoff_file = std::path::Path::new(&rec.worktree).join(HANDOFF_FILE);
    assert!(
        handoff_file.exists(),
        ".dispatch/handoff.md should exist when .dispatch/ is excluded"
    );
    assert_eq!(std::fs::read_to_string(handoff_file).unwrap(), *handoff);

    // When `.dispatch/` is not excluded in the worktree, `.dispatch/handoff.md` is not written,
    // and non-limit failures (crash) keep the original bare prompt without a hand-off.
    let (_dir2, root2) = repo(&[("README", "hi\n")]);
    let r2 = ran();
    let crash_first = Fake::new("a", &r2).fail("process crashed");
    let crash_second = Fake::new("b", &r2).out("recovered\n");
    let crash_second_reqs = crash_second.requests.clone();
    let t2 = test_deps(vec![crash_first.arc(), crash_second.arc()]);
    let rec2 = run(
        &t2,
        Options {
            dir: root2,
            ..easy("bare task prompt")
        },
    )
    .await
    .unwrap();
    assert_eq!(rec2.status, STATUS_COMPLETED);
    assert!(rec2.attempts[0].handoff.is_empty());
    assert_eq!(
        crash_second_reqs.lock().unwrap()[0].prompt,
        "bare task prompt"
    );
}

#[tokio::test]
async fn test_near_limit_resumes_session_for_model_handoff_and_switches() {
    use dispatch_core::runner::{HANDOFF_FILE, HANDOFF_INSTRUCTION, HANDOFF_MODEL_PROMPT};

    let (_dir, root) = repo(&[("README", "hi\n")]);
    let r = ran();
    let model_handoff_reply =
        "Done: wrote extra.txt. Left: run tests. Decisions: kept layout flat. Next: cargo test.";
    let first = Fake::new("a", &r)
        .edit(|dir| {
            std::fs::write(std::path::Path::new(dir).join("extra.txt"), "hello\n").unwrap();
        })
        .delay(Duration::from_secs(5))
        .run_quota(Quota {
            used_percent: 95.0,
            ..Quota::default()
        })
        .resume_out(model_handoff_reply);
    let first_reqs = first.requests.clone();
    let second = Fake::new("b", &r).out("completed from model handoff\n");
    let second_reqs = second.requests.clone();

    let mut t = test_deps(vec![first.arc(), second.arc()]);
    t.deps.worktree_dir.clear();

    let rec = run(
        &t,
        Options {
            dir: root,
            ..easy("implement extra.txt and verify")
        },
    )
    .await
    .unwrap();

    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    assert_eq!(rec.agent, "b");
    assert_eq!(ran_list(&r), vec!["a/m1", "a/m1@sess-a", "b/m2"]);
    assert_eq!(rec.attempts.len(), 1);
    assert_eq!(rec.attempts[0].exit_class, "near_limit");
    assert_eq!(rec.attempts[0].error, "95% of usage limit");
    assert_eq!(rec.attempts[0].handoff, model_handoff_reply);

    let first_requests = first_reqs.lock().unwrap();
    assert_eq!(first_requests.len(), 2);
    assert_eq!(first_requests[1].resume_session, "sess-a");
    assert_eq!(first_requests[1].prompt, HANDOFF_MODEL_PROMPT);

    let second_requests = second_reqs.lock().unwrap();
    assert_eq!(second_requests.len(), 1);
    assert!(second_requests[0].prompt.starts_with(HANDOFF_INSTRUCTION));
    assert!(second_requests[0].prompt.contains(model_handoff_reply));

    let handoff_file = std::path::Path::new(&rec.worktree).join(HANDOFF_FILE);
    assert_eq!(
        std::fs::read_to_string(handoff_file).unwrap(),
        model_handoff_reply
    );

    // Near-limit hand-off must not put the account in cooldown.
    let cool = store_cooldowns(Some(t.store.as_ref()), Utc::now());
    assert!(
        !cool.contains_key("a") && !cool.contains_key("a:m1"),
        "near_limit must not cool down the account: {cool:?}"
    );
}

#[tokio::test]
async fn test_near_limit_resume_failure_falls_back_to_layer_one_handoff() {
    use dispatch_core::runner::HANDOFF_INSTRUCTION;
    use serde_json::json;

    let (_dir, root) = repo(&[("README", "hi\n")]);
    let stream_log = [
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Edited extra.txt before reaching 96%."}]}}).to_string(),
        String::new(),
    ]
    .join("\n");

    let r = ran();
    let first = Fake::new("a", &r)
        .edit(|dir| {
            std::fs::write(std::path::Path::new(dir).join("extra.txt"), "hello\n").unwrap();
        })
        .log(&stream_log)
        .delay(Duration::from_secs(5))
        .run_quota(Quota {
            used_percent: 96.0,
            ..Quota::default()
        })
        .resume_fail("resume failed");
    let second = Fake::new("b", &r).out("completed from layer-1 fallback\n");
    let second_reqs = second.requests.clone();

    let t = test_deps(vec![first.arc(), second.arc()]);
    let rec = run(
        &t,
        Options {
            dir: root,
            ..easy("implement extra.txt and verify")
        },
    )
    .await
    .unwrap();

    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    assert_eq!(rec.agent, "b");
    assert_eq!(ran_list(&r), vec!["a/m1", "a/m1@sess-a", "b/m2"]);
    assert_eq!(rec.attempts.len(), 1);
    assert_eq!(rec.attempts[0].exit_class, "near_limit");

    let handoff = &rec.attempts[0].handoff;
    assert!(handoff.contains("## Original task"), "{handoff}");
    assert!(handoff.contains("a/m1 (near_limit)"), "{handoff}");
    assert!(handoff.contains("96% of usage limit"), "{handoff}");
    assert!(handoff.contains("extra.txt (+1 -0)"), "{handoff}");
    assert!(
        handoff.contains("Edited extra.txt before reaching 96%."),
        "{handoff}"
    );

    let second_requests = second_reqs.lock().unwrap();
    assert_eq!(second_requests.len(), 1);
    assert!(second_requests[0].prompt.starts_with(HANDOFF_INSTRUCTION));
    assert!(second_requests[0].prompt.contains(handoff));
}

#[tokio::test]
async fn test_near_limit_with_no_other_candidate_runs_to_completion() {
    let r = ran();
    let only = Fake::new("a", &r)
        .out("finished untouched\n")
        .delay(Duration::from_millis(60))
        .run_quota(Quota {
            used_percent: 98.0,
            ..Quota::default()
        });
    let mut t = test_deps(vec![only.arc()]);
    t.deps.table = table(&[("easy", vec![entry("a", "m1")])]);

    let rec = run(&t, read_only("solo task")).await.unwrap();
    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    assert_eq!(rec.agent, "a");
    assert_eq!(rec.output, "finished untouched");
    assert!(rec.attempts.is_empty(), "{:?}", rec.attempts);
    assert_eq!(ran_list(&r), vec!["a/m1"]);
}
