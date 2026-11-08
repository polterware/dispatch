use dispatch_core::{
    pass_model::Area,
    route_decision::{DecisionCandidate, POLICY_TIER_V1},
    router::Filters,
    store::{self, Store, Workspace},
    types::{self, Attempt, Run, Section, Store as _, Usage, WorktreeInfo},
};
use rusqlite::Connection;
use std::path::Path;

fn new_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::new(dir.path().join("dispatch.db").to_str().unwrap()).unwrap();
    (dir, s)
}

fn count(s: &Store, sql: &str) -> i64 {
    s.with_conn(|c| c.query_row(sql, [], |r| r.get(0)).unwrap())
}

#[test]
fn route_decision_round_trip() {
    let (_dir, s) = new_store();
    s.save_workspace(&Workspace {
        id: "proj".into(),
        name: "proj".into(),
        ..Default::default()
    })
    .unwrap();
    let now = "2026-11-02T12:00:00Z".to_string();
    s.save_run(&Run {
        id: "run-1".into(),
        project_id: "proj".into(),
        status: types::STATUS_PENDING.into(),
        tier: "normal".into(),
        created_at: now.clone(),
        ..Default::default()
    })
    .unwrap();
    let mut dec = types::RouteDecision {
        run_id: "run-1".into(),
        project_id: "proj".into(),
        policy_version: POLICY_TIER_V1.into(),
        route: "tier".into(),
        priority: "balanced".into(),
        classifier: "jev".into(),
        probs: vec![0.2, 0.5, 0.3],
        difficulty: Some(0.25),
        area: Area::new("web", "rust"),
        candidates: vec![DecisionCandidate {
            agent: "codex".into(),
            model: "m".into(),
            p_pass: Some(0.8),
            score: 12.0,
            ..Default::default()
        }],
        created_at: now.clone(),
        ..Default::default()
    };
    s.save_route_decision(&dec).unwrap();
    dec.chosen = 0;
    s.save_route_decision(&dec)
        .expect("a second save of the same run must update, not fail");
    let got = s.list_route_decisions("proj").unwrap();
    assert_eq!(got.len(), 1);
    let g = &got[0];
    assert_eq!(g.policy_version, POLICY_TIER_V1);
    assert_eq!(g.probs.len(), 3);
    assert_eq!(g.classifier, "jev");
    assert_eq!(g.difficulty, Some(0.25));
    assert_eq!(g.candidates.len(), 1);
    assert_eq!(g.candidates[0].p_pass, Some(0.8));
    assert_eq!(g.created_at, now);
    assert_eq!(g.area, Area::new("web", "rust"));
    assert!(s.get_route_decision("run-1").unwrap().is_some());
    assert!(s.get_route_decision("none").unwrap().is_none());
}

#[test]
fn workspace_at_picks_the_deepest_folder() {
    let (_dir, s) = new_store();
    for (id, path, project) in [("a", "/code", ""), ("b", "/code/app", ""), ("c", "", "p")] {
        s.save_workspace(&Workspace {
            id: id.into(),
            path: path.into(),
            polterware_project_id: project.into(),
            ..Default::default()
        })
        .unwrap();
    }
    assert_eq!(s.workspace_at("/code/app/src").unwrap().id, "b");
    assert_eq!(s.workspace_at("/code/app/../x").unwrap().id, "a");
    let err = s.workspace_at("/elsewhere").unwrap_err();
    assert!(store::is_no_workspace(&err), "outside: {err}");
}

#[test]
fn run_usage_round_trip_and_all_workspaces() {
    let (_dir, s) = new_store();
    let usage = Usage {
        input_tokens: 10,
        cache_read_tokens: 20,
        cache_write_tokens: 1,
        output_tokens: 5,
        reasoning_tokens: 2,
        cost_usd: Some(0.25),
        cost_source: "agent".into(),
        ..Default::default()
    };
    for r in [
        Run {
            id: "a".into(),
            project_id: "one".into(),
            status: types::STATUS_COMPLETED.into(),
            created_at: "2026-11-02T12:00:00Z".into(),
            usage: Some(usage),
            ..Default::default()
        },
        Run {
            id: "b".into(),
            project_id: "two".into(),
            status: types::STATUS_COMPLETED.into(),
            created_at: "2026-11-02T12:00:01Z".into(),
            ..Default::default()
        },
    ] {
        s.save_run(&r).unwrap();
    }
    let got = s.get_run("a").unwrap().usage.unwrap();
    assert_eq!(got.input_tokens, 10);
    assert_eq!(got.cache_write_tokens, 1);
    assert_eq!(got.cost_usd, Some(0.25));
    assert_eq!(got.cost_source, "agent");
    assert!(
        s.get_run("b").unwrap().usage.is_none(),
        "a run with no reported usage must read nil"
    );
    assert_eq!(s.list_runs("one", 0, 0, "").unwrap().len(), 1);
    assert_eq!(
        s.list_runs("", 0, 0, "").unwrap().len(),
        2,
        "an empty project must list every workspace's runs"
    );
}

#[test]
fn run_reads_stored_worktree_status() {
    let (_dir, s) = new_store();
    s.save_run(&Run {
        id: "run".into(),
        project_id: "workspace".into(),
        ..Default::default()
    })
    .unwrap();
    for status in ["", types::WORKTREE_MERGED, types::WORKTREE_REMOVED] {
        if !status.is_empty() {
            s.save_worktree(&WorktreeInfo {
                id: "tree".into(),
                run_id: "run".into(),
                project_id: "workspace".into(),
                status: status.into(),
                ..Default::default()
            })
            .unwrap();
        }
        assert_eq!(s.get_run("run").unwrap().worktree_status, status);
        let runs = s.list_runs("workspace", 0, 0, "").unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].worktree_status, status);
    }
}

#[test]
fn conversation_migration_from_0005() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.db");
    let migrations = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/migrations");
    {
        let db = Connection::open(&path).unwrap();
        for name in [
            "0001_baseline.sql",
            "0002_drop_verify.sql",
            "0003_run_usage.sql",
            "0004_run_subagent.sql",
            "0005_run_role.sql",
        ] {
            let body = std::fs::read_to_string(migrations.join(name)).unwrap();
            db.execute_batch(&body).unwrap();
        }
        db.execute_batch(
            "PRAGMA user_version=5; INSERT INTO dispatch_runs(id, project_id, session_id, created_at) VALUES ('old','workspace','worker-session','2026-11-02T12:00:00Z')",
        )
        .unwrap();
    }
    let s = Store::new(path.to_str().unwrap()).unwrap();
    let old = s.get_run("old").unwrap();
    assert_eq!(old.session_id, "worker-session");
    assert_eq!(old.conversation_id, "");
    assert_eq!(old.caller_agent, "");
    assert_eq!(count(&s, "PRAGMA user_version"), 13);
    assert_eq!(
        count(
            &s,
            "SELECT COUNT(*) FROM pragma_index_info('dispatch_runs_conversation_created') WHERE name IN ('conversation_id','created_at')"
        ),
        2
    );
    drop(s);
    // Opening the database again must not rerun ALTER TABLE.
    Store::new(path.to_str().unwrap()).unwrap();
}

#[test]
fn tier_speed_strength_migration_keeps_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.db");
    let migrations = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/migrations");
    {
        let db = Connection::open(&path).unwrap();
        for name in [
            "0001_baseline.sql",
            "0002_drop_verify.sql",
            "0003_run_usage.sql",
            "0004_run_subagent.sql",
            "0005_run_role.sql",
            "0006_conversations_sections.sql",
        ] {
            let body = std::fs::read_to_string(migrations.join(name)).unwrap();
            db.execute_batch(&body).unwrap();
        }
        db.execute_batch(
            "PRAGMA user_version=6;
             INSERT INTO dispatch_tiers(name, provider, agent, model, speed, strength, position)
             VALUES ('easy','openai','codex','gpt-5.4-mini',5,1,0),
                    ('hard','anthropic','claude','opus',2,5,1)",
        )
        .unwrap();
    }
    let s = Store::new(path.to_str().unwrap()).unwrap();
    assert_eq!(count(&s, "PRAGMA user_version"), 13);
    assert_eq!(
        count(
            &s,
            "SELECT COUNT(*) FROM pragma_table_info('dispatch_tiers') WHERE name IN ('speed','strength')"
        ),
        0
    );
    let tiers = s.list_tiers().unwrap();
    assert_eq!(
        tiers,
        vec![
            types::Tier {
                name: "easy".into(),
                provider: "openai".into(),
                agent: "codex".into(),
                model: "gpt-5.4-mini".into(),
                position: 0,
            },
            types::Tier {
                name: "hard".into(),
                provider: "anthropic".into(),
                agent: "claude".into(),
                model: "opus".into(),
                position: 1,
            },
        ]
    );
}

#[test]
fn runs_conversation_round_trip_and_filter() {
    let (_dir, s) = new_store();
    for (i, (id, project, conv, caller, session)) in [
        ("first", "one", "codex:chat", "codex", "worker-first"),
        ("second", "two", "codex:chat", "codex", "worker-second"),
        ("other", "one", "claude:other", "claude", ""),
        ("third", "one", "codex:chat", "codex", ""),
    ]
    .into_iter()
    .enumerate()
    {
        s.save_run(&Run {
            id: id.into(),
            project_id: project.into(),
            conversation_id: conv.into(),
            caller_agent: caller.into(),
            session_id: session.into(),
            created_at: format!("2026-11-02T12:00:0{i}Z"),
            ..Default::default()
        })
        .unwrap();
    }
    let mut got = s.get_run("first").unwrap();
    assert_eq!(got.conversation_id, "codex:chat");
    assert_eq!(got.caller_agent, "codex");
    assert_eq!(got.session_id, "worker-first");
    got.note = "updated".into();
    s.save_run(&got).unwrap();
    let got = s.get_run("first").unwrap();
    assert_eq!(got.conversation_id, "codex:chat");
    assert_eq!(got.note, "updated");
    assert_eq!(s.list_runs("", 0, 0, "codex:chat").unwrap().len(), 3);
    let page = s.list_runs("one", 1, 1, "codex:chat").unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, "first");
}

#[test]
fn sections_store() {
    let (_dir, s) = new_store();
    let save = |id: &str, name: &str, pos: i64| {
        s.save_section(&Section {
            id: id.into(),
            name: name.into(),
            position: pos,
            ..Default::default()
        })
        .unwrap()
    };
    let a = save("", "First", 0);
    let b = save("", "Second", 1);
    assert!(!a.id.is_empty() && !a.created_at.is_empty() && s.list_sections().unwrap().len() == 2);
    let renamed = save(&a.id, "Renamed", 2);
    assert_eq!(renamed.name, "Renamed");
    assert_eq!(renamed.created_at, a.created_at);
    assert_eq!(s.list_sections().unwrap()[0].id, b.id);

    let mv = |kind: &str, r: &str, target: &str| s.move_to_section(kind, r, target).unwrap();
    mv("conversation", "codex:chat", &a.id);
    mv("run", "run-1", &a.id);
    mv("run", "run-1", &a.id);
    let sections = s.list_sections().unwrap();
    assert_eq!(sections[1].items.len(), 2);
    assert_eq!(sections[1].items[0].r#ref, "codex:chat");
    assert_eq!(sections[1].items[1].position, 1);

    mv("conversation", "codex:chat", &b.id);
    let sections = s.list_sections().unwrap();
    assert_eq!(sections[0].items.len(), 1);
    assert_eq!(sections[1].items.len(), 1);

    mv("run", "run-1", "");
    assert!(s.list_sections().unwrap()[1].items.is_empty());

    s.delete_section(&b.id).unwrap();
    assert_eq!(count(&s, "SELECT COUNT(*) FROM dispatch_section_items"), 0);
    let sections = s.list_sections().unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].id, a.id);

    let blank = s
        .save_section(&Section {
            name: " ".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert!(store::is_invalid_section(&blank), "blank name: {blank}");
    let negative = s
        .save_section(&Section {
            name: "x".into(),
            position: -1,
            ..Default::default()
        })
        .unwrap_err();
    assert!(
        store::is_invalid_section(&negative),
        "negative position: {negative}"
    );
    let unknown = s
        .save_section(&Section {
            id: "missing".into(),
            name: "x".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert!(store::is_unknown_section(&unknown), "unknown: {unknown}");
    let kind = s.move_to_section("invalid", "ref", &a.id).unwrap_err();
    assert!(store::is_invalid_section(&kind), "kind: {kind}");
    let blank_ref = s.move_to_section("run", "", &a.id).unwrap_err();
    assert!(store::is_invalid_section(&blank_ref), "ref: {blank_ref}");
    let target = s.move_to_section("run", "ref", "missing").unwrap_err();
    assert!(store::is_unknown_section(&target), "target: {target}");
    let delete = s.delete_section("missing").unwrap_err();
    assert!(
        store::is_unknown_section(&delete),
        "delete unknown: {delete}"
    );
    // Check the SQL constraint too, independently of the validation.
    let bad = s.with_conn(|c| {
        c.execute(
            "INSERT INTO dispatch_section_items VALUES (?, 'invalid', 'ref', 0)",
            [&a.id],
        )
    });
    assert!(bad.is_err(), "invalid kind accepted by SQLite");
}

#[test]
fn go_encodings_round_trip() {
    let (_dir, s) = new_store();
    let run = Run {
        id: "r".into(),
        project_id: "p".into(),
        created_at: "2026-11-02T12:00:00Z".into(),
        requested: Some(Filters {
            provider: "openai".into(),
            relax: true,
            ..Default::default()
        }),
        attempts: vec![Attempt {
            agent: "codex".into(),
            duration_ms: Some(7),
            ..Default::default()
        }],
        prefer: vec!["codex".into()],
        ..Default::default()
    };
    s.save_run(&run).unwrap();
    let (requested, attempts, avoid, usage, role) = s.with_conn(|c| {
        c.query_row(
            "SELECT requested_json, attempts_json, avoid_json, usage_json, role_json FROM dispatch_runs WHERE id = 'r'",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .unwrap()
    });
    assert_eq!(
        requested,
        r#"{"Provider":"openai","Model":"","Effort":"","Agent":"","Priority":"","Relax":true}"#
    );
    assert_eq!(
        attempts,
        r#"[{"Agent":"codex","Model":"","Error":"","ExitClass":"","DurationMs":7}]"#
    );
    assert_eq!(avoid, "[]");
    assert_eq!((usage.as_str(), role.as_str()), ("", ""));
    let got = s.get_run("r").unwrap();
    assert_eq!(got.requested.unwrap().provider, "openai");
    assert_eq!(got.attempts[0].duration_ms, Some(7));
    assert!(got.resolved.is_none());
}

#[test]
fn events_agents_quota_and_tiers() {
    let (_dir, s) = new_store();
    s.save_run(&Run {
        id: "r".into(),
        project_id: "p".into(),
        agent: "codex".into(),
        tier: "easy".into(),
        ..Default::default()
    })
    .unwrap();
    let ev = |kind: &str| types::RunEvent {
        run_id: "r".into(),
        kind: kind.into(),
        ..Default::default()
    };
    s.append_event(&ev(types::EVENT_STARTED)).unwrap();
    s.append_event(&ev(types::EVENT_OUTPUT)).unwrap();
    let events = s.list_events("r", 0, 0).unwrap();
    assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(
        (events[0].project_id.as_str(), events[0].agent.as_str()),
        ("p", "codex")
    );
    assert_eq!(s.list_events("r", 1, 0).unwrap().len(), 1);
    assert_eq!(s.list_history("p", "", "", 0, 0).unwrap().len(), 1);
    let missing = s
        .append_event(&types::RunEvent {
            run_id: "none".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert!(types::is_unknown_run(&missing));
    assert!(types::is_unknown_run(&s.get_run("none").unwrap_err()));

    s.set_cooldown("codex", "2026-11-02T12:00:00Z").unwrap();
    s.upsert_agent(&types::Agent {
        name: "codex".into(),
        bin: "codex".into(),
        ..Default::default()
    })
    .unwrap();
    let agents = s.list_agents().unwrap();
    assert_eq!(
        agents[0].cooldown_until.as_deref(),
        Some("2026-11-02T12:00:00Z")
    );
    s.clear_cooldown("codex").unwrap();
    assert!(s.list_agents().unwrap()[0].cooldown_until.is_none());

    s.save_quota(&types::QuotaSnapshot {
        agent: "codex".into(),
        sampled_at: "2026-11-02T12:00:00Z".into(),
        used_percent: Some(12.5),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        s.list_quota("codex", "2026-11-02T11:00:00Z").unwrap().len(),
        1
    );
    assert!(
        s.list_quota("codex", "2026-11-02T13:00:00Z")
            .unwrap()
            .is_empty()
    );

    let tiers = vec![types::Tier {
        name: "easy".into(),
        agent: "codex".into(),
        model: "m".into(),
        position: 1,
        ..Default::default()
    }];
    s.replace_tiers(&tiers).unwrap();
    assert_eq!(s.list_tiers().unwrap(), tiers);
    let other = Store::new(":memory:").unwrap();
    assert_eq!(other.list_tiers().unwrap().len(), 0);
}

#[test]
fn event_pages_read_sizes_and_slices_on_whole_characters() {
    let (_dir, s) = new_store();
    s.save_run(&Run {
        id: "r".into(),
        project_id: "p".into(),
        ..Default::default()
    })
    .unwrap();
    // "é" and "ö" are two bytes each: "h é llo w ö rld" is 13 bytes.
    s.append_event(&types::RunEvent {
        run_id: "r".into(),
        kind: types::EVENT_OUTPUT.into(),
        text: "h\u{e9}llo w\u{f6}rld".into(),
        ..Default::default()
    })
    .unwrap();
    s.append_event(&types::RunEvent {
        run_id: "r".into(),
        kind: types::EVENT_MERGED.into(),
        text: "into main".into(),
        ..Default::default()
    })
    .unwrap();
    let sizes = s.event_sizes("r").unwrap();
    assert_eq!(
        sizes,
        vec![
            types::EventSize {
                seq: 1,
                kind: types::EVENT_OUTPUT.into(),
                bytes: 13,
            },
            types::EventSize {
                seq: 2,
                kind: types::EVENT_MERGED.into(),
                bytes: 9,
            },
        ]
    );
    // Byte 2 is inside "é": the start moves on to the "l" after it.
    let (event, start) = s.event_slice("r", 1, 2, 13).unwrap().unwrap();
    assert_eq!((event.text.as_str(), start), ("llo w\u{f6}rld", 3));
    // Byte 8 is inside "ö": the end moves back before it.
    let (event, start) = s.event_slice("r", 1, 0, 8).unwrap().unwrap();
    assert_eq!((event.text.as_str(), start), ("h\u{e9}llo w", 0));
    assert_eq!(event.kind, types::EVENT_OUTPUT);
    assert_eq!(
        s.event_slice("r", 2, 0, 9).unwrap().unwrap().0.text,
        "into main"
    );
    assert!(s.event_slice("r", 9, 0, 1).unwrap().is_none());
}

#[test]
fn disabled_models_persist_and_provider_switch_keeps_model_choices() {
    use dispatch_core::config::is_model_disabled;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dispatch.db");
    let path = db.to_str().unwrap();
    let store = Store::new(path).unwrap();
    store
        .set_model_enabled("anthropic", "claude-opus-5-5-high", false)
        .unwrap();
    store.set_model_enabled("anthropic", "", false).unwrap();
    store.set_model_enabled("anthropic", "", true).unwrap();
    let disabled = store.list_disabled_models().unwrap();
    assert!(is_model_disabled(
        &disabled,
        "anthropic",
        "antigravity",
        "claude-opus-5-5-low"
    ));
    assert!(!is_model_disabled(
        &disabled,
        "anthropic",
        "claude",
        "claude-sonnet-5-5"
    ));
    drop(store);
    let store = Store::new(path).unwrap();
    assert_eq!(store.list_disabled_models().unwrap(), disabled);
    store
        .set_model_enabled("anthropic", "claude-opus-5-5", true)
        .unwrap();
    assert!(store.list_disabled_models().unwrap().is_empty());
}

#[test]
fn an_agents_disabled_model_stays_off_for_that_agent_alone() {
    use dispatch_core::config::is_model_disabled;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dispatch.db");
    let path = db.to_str().unwrap();
    let store = Store::new(path).unwrap();
    store
        .set_agent_model_enabled("fake", "fake-fast-high", false)
        .unwrap();
    let disabled = store.list_disabled_models().unwrap();
    assert_eq!(disabled.len(), 1);
    assert_eq!(disabled[0].agent, "fake");
    assert_eq!(disabled[0].model, "fake-fast");
    assert!(is_model_disabled(
        &disabled,
        "fake",
        "fake",
        "fake-fast-low"
    ));
    assert!(!is_model_disabled(
        &disabled,
        "fake",
        "other",
        "fake-fast-low"
    ));
    drop(store);
    let store = Store::new(path).unwrap();
    assert_eq!(store.list_disabled_models().unwrap(), disabled);
    store
        .set_agent_model_enabled("fake", "fake-fast", true)
        .unwrap();
    assert!(store.list_disabled_models().unwrap().is_empty());
}
