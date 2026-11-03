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
        page, store_cooldowns, summary,
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
