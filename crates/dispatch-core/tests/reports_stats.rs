use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, TimeZone, Utc};
use dispatch_core::{
    reports::{
        MIN_SAMPLES, Row, STATS_SCHEMA, StatsOptions, capacity, capacity_from_store, compute,
        evidence_by_tier_model, evidence_key, from_store, parse_since,
    },
    store::Store as SqliteStore,
    types::{
        Area, Attempt, Filters, QuotaSnapshot, RouteDecision, Run, STATUS_CANCELLED,
        STATUS_COMPLETED, STATUS_FAILED, STATUS_ORPHANED, STATUS_RUNNING, STATUS_TIMED_OUT, Store,
    },
};

fn rfc3339(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn find_row<'a>(rows: &'a [Row], key: &str) -> &'a Row {
    rows.iter()
        .find(|r| r.key == key)
        .unwrap_or_else(|| panic!("missing {key} in {rows:?}"))
}

#[test]
fn test_compute_rates_percentiles_and_ranks() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    let created = rfc3339(now - ChronoDuration::hours(1));
    let mk = |id: &str,
              model: &str,
              provider: &str,
              status: &str,
              run_ms: Option<i64>,
              attempts: Vec<Attempt>,
              exit_class: &str|
     -> Run {
        Run {
            id: id.into(),
            model: model.into(),
            provider: provider.into(),
            agent: "codex".into(),
            tier: "normal".into(),
            status: status.into(),
            created_at: created.clone(),
            run_ms,
            attempts,
            exit_class: exit_class.into(),
            ..Default::default()
        }
    };

    let mut recs = Vec::new();
    for (i, d) in [10, 20, 30, 40, 50, 60].into_iter().enumerate() {
        recs.push(mk(
            &format!("a{i}"),
            "gpt-5.6-terra",
            "openai",
            STATUS_COMPLETED,
            Some(d),
            vec![],
            "ok",
        ));
    }
    let rl = vec![Attempt {
        agent: "grok".into(),
        model: "grok-4.7".into(),
        error: "429 too many requests".into(),
        exit_class: "rate_limit".into(),
        duration_ms: None,
        handoff: String::new(),
    }];
    recs.push(mk(
        "a6",
        "gpt-5.6-terra",
        "openai",
        STATUS_COMPLETED,
        Some(70),
        rl.clone(),
        "ok",
    ));
    recs.push(mk(
        "a7",
        "gpt-5.6-terra",
        "openai",
        STATUS_COMPLETED,
        Some(80),
        rl.clone(),
        "ok",
    ));
    recs.push(mk(
        "a8",
        "gpt-5.6-terra",
        "openai",
        STATUS_FAILED,
        Some(90),
        rl,
        "rate_limit",
    ));
    recs.push(mk(
        "a9",
        "gpt-5.6-terra",
        "openai",
        STATUS_ORPHANED,
        None,
        vec![],
        "",
    ));
    for i in 0..5 {
        recs.push(mk(
            &format!("b{i}"),
            "gemini-3.8-flash-low",
            "google",
            STATUS_FAILED,
            Some(200),
            vec![],
            "failed",
        ));
    }
    // A second model with enough completed runs, so the slowest has something to beat.
    for i in 0..5 {
        recs.push(mk(
            &format!("c{i}"),
            "claude-opus-5-5",
            "anthropic",
            STATUS_COMPLETED,
            Some(300),
            vec![],
            "ok",
        ));
    }
    recs.push(Run {
        id: "fly".into(),
        status: STATUS_RUNNING.into(),
        model: "gpt-5.6-terra".into(),
        created_at: rfc3339(now),
        ..Default::default()
    });

    let rep = compute(
        &recs,
        StatsOptions {
            by: "model".into(),
            since: Some(now - ChronoDuration::days(7)),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("compute");
    assert_eq!(rep.schema, STATS_SCHEMA);
    assert_eq!(rep.in_flight, 1);
    assert_eq!(rep.min_samples, MIN_SAMPLES);

    let gpt = find_row(&rep.rows, "gpt-5.6-terra");
    assert_eq!(gpt.runs, 10);
    assert_eq!(gpt.success_pct, 80.0);
    assert_eq!(gpt.first_try_pct, 60.0);
    assert_eq!(gpt.fallbacks, 3);
    assert_eq!(gpt.rate_limits, 3);
    assert_eq!(gpt.orphans, 1);
    assert_eq!(gpt.p50_ms, Some(50));
    assert_eq!(gpt.p90_ms, Some(90));
    assert_eq!(gpt.duration_n, 9);

    let gem = find_row(&rep.rows, "gemini-3.8-flash-low");
    assert_eq!(gem.runs, 5);
    assert_eq!(gem.success_pct, 0.0);
    assert_eq!(gem.p50_ms, Some(200));

    assert_eq!(rep.rankings.most_used_model.key, "gpt-5.6-terra");
    assert_eq!(rep.rankings.most_used_model.runs, 10);
    assert_eq!(rep.rankings.most_used_provider.key, "openai");
    assert_eq!(rep.rankings.most_used_provider.runs, 10);
    // Gemini never completed, so its 200ms says nothing of its speed.
    assert_eq!(rep.rankings.slowest_model.key, "claude-opus-5-5");
    assert_eq!(rep.rankings.slowest_model.p50_ms, Some(300));
    assert_eq!(rep.rankings.slowest_model.samples, 5);
    assert_eq!(rep.rankings.most_failed_model.key, "gemini-3.8-flash-low");
    assert_eq!(rep.rankings.most_failed_model.success_pct, Some(0.0));
    assert_eq!(rep.rankings.most_failed_model.failures, 5);

    let text = rep.text();
    assert!(text.contains("gpt-5.6-terra"), "{text}");
    assert!(text.contains("slowest model: claude-opus-5-5"), "{text}");
    assert!(
        text.contains("most failed model: gemini-3.8-flash-low (5 of 5 runs failed)"),
        "{text}"
    );

    // Too few samples to call a model slow or failure-prone.
    let small = compute(
        &[
            mk(
                "s1",
                "fast",
                "openai",
                STATUS_COMPLETED,
                Some(1),
                vec![],
                "ok",
            ),
            mk(
                "s2",
                "slow",
                "openai",
                STATUS_FAILED,
                Some(999),
                vec![],
                "failed",
            ),
        ],
        StatsOptions {
            by: "model".into(),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("small compute");
    assert_eq!(small.rankings.slowest_model.key, "unknown");
    assert!(small.rankings.slowest_model.reason.contains("minimum"));
    assert!(!small.rankings.most_failed_model.reason.is_empty());
    assert!(!small.rankings.most_used_model.key.is_empty());
    assert!(small.text().contains("unknown"));
}

/// Runs of `model`, one per status in `statuses`, each 1s long.
fn runs_of(model: &str, statuses: &[(&str, usize)]) -> Vec<Run> {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    let mut out = Vec::new();
    for (status, count) in statuses {
        for i in 0..*count {
            out.push(Run {
                id: format!("{model}-{status}-{i}"),
                model: model.into(),
                provider: "p".into(),
                agent: "codex".into(),
                status: (*status).into(),
                created_at: rfc3339(now - ChronoDuration::hours(1)),
                run_ms: Some(1000),
                ..Default::default()
            });
        }
    }
    out
}

fn rankings_of(runs: Vec<Vec<Run>>) -> dispatch_core::reports::Rankings {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    let recs: Vec<Run> = runs.into_iter().flatten().collect();
    compute(
        &recs,
        StatsOptions {
            by: "model".into(),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("compute")
    .rankings
}

#[test]
fn test_most_failed_counts_only_the_runs_a_model_ended_itself() {
    // Orphans (the core went away), cancels (the user's call), and a rate-limited end are not
    // the model failing: "flaky" failed once in the 16 runs it ended.
    let mut limited = runs_of("flaky", &[(STATUS_FAILED, 1)]);
    limited[0].exit_class = "rate_limit".into();
    limited[0].id = "flaky-limited".into();
    let ranks = rankings_of(vec![
        runs_of(
            "flaky",
            &[
                (STATUS_COMPLETED, 15),
                (STATUS_ORPHANED, 2),
                (STATUS_CANCELLED, 1),
                (STATUS_FAILED, 1),
            ],
        ),
        limited,
        runs_of("steady", &[(STATUS_COMPLETED, 5), (STATUS_ORPHANED, 3)]),
    ]);
    let failed = &ranks.most_failed_model;
    assert_eq!(failed.key, "flaky");
    assert_eq!(failed.runs, 16);
    assert_eq!(failed.failures, 1);
    assert_eq!(failed.success_pct, Some(93.8));
}

#[test]
fn test_most_failed_weighs_how_many_runs_back_the_rate() {
    // 1 in 5 is a higher rate than 3 in 18, but five runs cannot tell 20% from 2%.
    let ranks = rankings_of(vec![
        runs_of("few", &[(STATUS_COMPLETED, 4), (STATUS_FAILED, 1)]),
        runs_of("many", &[(STATUS_COMPLETED, 15), (STATUS_TIMED_OUT, 3)]),
    ]);
    assert_eq!(ranks.most_failed_model.key, "many");
    assert_eq!(ranks.most_failed_model.failures, 3);
}

#[test]
fn test_comparison_rankings_need_two_models_and_a_failure() {
    // No failures: no model is named most failed.
    let clean = rankings_of(vec![
        runs_of("a", &[(STATUS_COMPLETED, 5)]),
        runs_of("b", &[(STATUS_COMPLETED, 6)]),
    ]);
    assert_eq!(clean.most_failed_model.key, "unknown");
    assert_eq!(clean.most_failed_model.reason, "no failures in range");
    assert_eq!(clean.slowest_model.key, "a");

    // One model with enough runs is slower and more failed than nothing.
    let alone = rankings_of(vec![
        runs_of("a", &[(STATUS_COMPLETED, 5), (STATUS_FAILED, 2)]),
        runs_of("b", &[(STATUS_COMPLETED, 2)]),
    ]);
    for rank in [&alone.slowest_model, &alone.most_failed_model] {
        assert_eq!(rank.key, "unknown");
        assert_eq!(
            rank.reason,
            format!("needs two models with enough runs (minimum {MIN_SAMPLES})")
        );
    }
    assert_eq!(alone.most_used_model.key, "a");
}

#[test]
fn test_unknown_group_since_and_zero_duration() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    let old = now - ChronoDuration::days(30);
    let recs = vec![
        Run {
            id: "e1".into(),
            status: STATUS_COMPLETED.into(),
            effort: "high".into(),
            model: "gemini-3.8-flash-high".into(),
            created_at: rfc3339(now),
            run_ms: Some(0),
            ..Default::default()
        },
        Run {
            id: "e2".into(),
            status: STATUS_FAILED.into(),
            model: "grok-4.7".into(),
            created_at: rfc3339(now),
            ..Default::default()
        },
        Run {
            id: "old".into(),
            status: STATUS_COMPLETED.into(),
            model: "gpt-5.6-terra".into(),
            created_at: rfc3339(old),
            run_ms: Some(5),
            ..Default::default()
        },
        Run {
            id: "d1".into(),
            status: STATUS_COMPLETED.into(),
            model: "m".into(),
            created_at: rfc3339(Utc.with_ymd_and_hms(2026, 10, 14, 8, 0, 0).unwrap()),
            ..Default::default()
        },
        Run {
            id: "d2".into(),
            status: STATUS_COMPLETED.into(),
            model: "m".into(),
            created_at: rfc3339(Utc.with_ymd_and_hms(2026, 10, 15, 8, 0, 0).unwrap()),
            ..Default::default()
        },
    ];

    let rep = compute(
        &recs,
        StatsOptions {
            by: "effort".into(),
            since: Some(now - ChronoDuration::days(7)),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("effort compute");
    let high = find_row(&rep.rows, "high");
    assert_eq!(high.runs, 1);
    assert_eq!(high.duration_n, 1);
    assert_eq!(high.p50_ms, Some(0));
    assert!(high.duration.is_empty());

    let unk = find_row(&rep.rows, "unknown");
    assert_eq!(unk.runs, 3);
    assert_eq!(unk.duration, "unknown");
    assert_eq!(unk.p50_ms, None);
    assert_eq!(rep.rows.len(), 2);

    let day = compute(
        &recs[3..],
        StatsOptions {
            by: "day".into(),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("day compute");
    assert_eq!(day.rows.len(), 2);
    assert_eq!(day.rows[0].key, "2026-10-14");
    assert_eq!(day.rows[1].key, "2026-10-15");

    let week = compute(
        &recs[3..],
        StatsOptions {
            by: "week".into(),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("week compute");
    assert_eq!(week.rows.len(), 1);
    assert!(week.rows[0].key.starts_with("2026-W"));

    assert!(
        compute(
            &recs,
            StatsOptions {
                by: "nope".into(),
                ..Default::default()
            }
        )
        .is_err()
    );

    let start = parse_since("7d", now).expect("7d");
    assert_eq!(start, Some(now - ChronoDuration::days(7)));
    assert_eq!(parse_since("all", now).expect("all"), None);
    assert!(parse_since("soon", now).is_err());
}

#[test]
fn test_stats_from_store() {
    let s = SqliteStore::new(":memory:").expect("in-memory store");
    s.save_run(&Run {
        id: "old".into(),
        status: STATUS_COMPLETED.into(),
        model: "gpt-5.6-terra".into(),
        provider: "openai".into(),
        created_at: rfc3339(Utc.with_ymd_and_hms(2026, 10, 10, 0, 0, 0).unwrap()),
        run_ms: Some(10),
        ..Default::default()
    })
    .expect("save old");
    s.save_run(&Run {
        id: "new".into(),
        status: STATUS_FAILED.into(),
        model: "gpt-5.6-terra".into(),
        provider: "openai".into(),
        created_at: rfc3339(Utc.with_ymd_and_hms(2026, 10, 14, 0, 0, 0).unwrap()),
        run_ms: Some(20),
        ..Default::default()
    })
    .expect("save new");

    let rep = from_store(
        Some(&s),
        StatsOptions {
            by: "model".into(),
            now: Some(Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap()),
            ..Default::default()
        },
    )
    .expect("from_store");
    let row = find_row(&rep.rows, "gpt-5.6-terra");
    assert_eq!(row.runs, 2);
    assert_eq!(row.success_pct, 50.0);
    assert_eq!(row.duration_n, 2);
}

#[test]
fn test_stats_by_area_reads_the_routing_decisions() {
    let s = SqliteStore::new(":memory:").expect("in-memory store");
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
    for id in ["a", "b", "c"] {
        s.save_run(&Run {
            id: id.into(),
            status: STATUS_COMPLETED.into(),
            created_at: rfc3339(now - ChronoDuration::hours(1)),
            ..Default::default()
        })
        .expect("save run");
    }
    s.save_route_decision(&RouteDecision {
        run_id: "a".into(),
        area: Area::new("service", "go"),
        created_at: rfc3339(now),
        ..Default::default()
    })
    .expect("save decision a");
    s.save_route_decision(&RouteDecision {
        run_id: "b".into(),
        area: Area::new("service", "go"),
        created_at: rfc3339(now),
        ..Default::default()
    })
    .expect("save decision b");

    let rep = from_store(
        Some(&s),
        StatsOptions {
            by: "area".into(),
            now: Some(now),
            ..Default::default()
        },
    )
    .expect("from_store area");
    assert_eq!(find_row(&rep.rows, "service, go").runs, 2);
    assert_eq!(find_row(&rep.rows, "unknown").runs, 1);
}

#[test]
fn test_capacity_windows() {
    let now = Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap();
    let week_mins = 7 * 24 * 60;
    let snap = |key: &str, used: f64, reset: DateTime<Utc>, at: DateTime<Utc>| QuotaSnapshot {
        agent: key.into(),
        used_percent: Some(used),
        resets_at: rfc3339(reset),
        window_minutes: week_mins,
        sampled_at: rfc3339(at),
        ..Default::default()
    };
    let reset1 = now - ChronoDuration::hours(48);
    let reset2 = now - ChronoDuration::days(14);
    let reset3 = now + ChronoDuration::hours(72);

    let snaps = vec![
        snap("codex", 40.0, reset1, reset1 - ChronoDuration::hours(1)),
        snap("codex", 100.0, reset2, reset2 - ChronoDuration::minutes(30)),
        snap("codex", 80.0, reset3, now - ChronoDuration::hours(1)),
    ];
    let rep = capacity(&snaps, None, Some(now));
    assert_eq!(rep.keys.len(), 1);
    let k = &rep.keys[0];
    assert_eq!(k.windows, 3);
    assert_eq!(k.hit_full, 1);
    assert_eq!(k.leftover_known, 2);
    assert_eq!(k.leftover_unknown, 1);
    assert_eq!(k.consumed_pct, Some(73.3));
    assert_eq!(k.leftover_pct, Some(30.0));
    assert_eq!(k.pace, "ahead");

    let (mut closed, mut open, mut full) = (0, 0, 0);
    for w in &k.detail {
        if w.open {
            open += 1;
            assert_eq!(w.leftover, "unknown");
            assert_eq!(w.leftover_pct, None);
            assert_eq!(w.pace, "ahead");
        } else {
            closed += 1;
        }
        if w.hit_full {
            full += 1;
            assert_eq!(w.leftover_pct, Some(0.0));
            assert_eq!(w.pace, "on pace");
        }
        if !w.open && !w.hit_full {
            assert_eq!(w.leftover_pct, Some(60.0));
            assert_eq!(w.pace, "behind");
        }
    }
    assert_eq!(closed, 2);
    assert_eq!(open, 1);
    assert_eq!(full, 1);
    assert!(rep.text().contains("unknown"));
    assert!(rep.note.contains("unknown"));

    // Also verify capacity_from_store reads stored quota snapshots identically.
    let s = SqliteStore::new(":memory:").expect("in-memory store");
    for qs in &snaps {
        s.save_quota(qs).expect("save quota");
    }
    let stored_rep = capacity_from_store(Some(&s), None, Some(now)).expect("capacity_from_store");
    assert_eq!(stored_rep, rep);

    let empty = capacity(&[], None, Some(now));
    assert!(empty.keys.is_empty());
    assert!(empty.note.contains("unknown"));

    // 99.5 counts as touching 100; a sample far from the reset leaves leftover unknown.
    let barely = now - ChronoDuration::hours(1);
    let early = barely - ChronoDuration::days(7) + ChronoDuration::hours(1);
    let edge = capacity(
        &[
            snap("codex", 99.5, barely, barely - ChronoDuration::hours(1)),
            snap("grok", 10.0, barely, early),
        ],
        None,
        Some(now),
    );
    assert_eq!(edge.keys.len(), 2);
    assert_eq!(edge.keys[0].key, "codex");
    assert_eq!(edge.keys[0].hit_full, 1);
    let grok = &edge.keys[1];
    assert_eq!(grok.key, "grok");
    assert_eq!(grok.leftover, "unknown");
    assert_eq!(grok.hit_full, 0);
}

#[test]
fn test_evidence_segments_by_tier() {
    let recs = vec![
        Run {
            id: "1".into(),
            status: STATUS_COMPLETED.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(1000),
            ..Default::default()
        },
        Run {
            id: "2".into(),
            status: STATUS_COMPLETED.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(3000),
            ..Default::default()
        },
        Run {
            id: "2b".into(),
            status: STATUS_COMPLETED.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(5000),
            ..Default::default()
        },
        Run {
            id: "3".into(),
            status: STATUS_COMPLETED.into(),
            tier: "hard".into(),
            model: "m".into(),
            run_ms: Some(9000),
            ..Default::default()
        },
        Run {
            id: "4".into(),
            status: STATUS_RUNNING.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(1),
            ..Default::default()
        },
        Run {
            id: "5".into(),
            status: STATUS_COMPLETED.into(),
            tier: "easy".into(),
            model: String::new(),
            run_ms: Some(1),
            ..Default::default()
        },
    ];
    let ev = evidence_by_tier_model(&recs);
    let easy = &ev[&evidence_key("easy", "m")];
    let hard = &ev[&evidence_key("hard", "m")];
    assert_eq!(easy.finished, 3);
    assert_eq!(easy.duration_n, 3);
    assert_eq!(hard.finished, 1);
    assert_eq!(easy.p50_ms, Some(3000));
    assert_eq!(hard.p50_ms, Some(9000));
    assert!(!ev.contains_key(&evidence_key("easy", "")));
}

#[test]
fn test_routing_filter_aggregations() {
    let r = Run {
        id: "filters".into(),
        status: STATUS_COMPLETED.into(),
        requested: Some(Filters {
            provider: "google".into(),
            ..Default::default()
        }),
        resolved: Some(Filters {
            provider: "google".into(),
            model: "gemini-fast".into(),
            agent: "antigravity".into(),
            effort: "high".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    for by in ["requested", "resolved"] {
        let report = compute(
            std::slice::from_ref(&r),
            StatsOptions {
                by: by.into(),
                ..Default::default()
            },
        )
        .expect("compute filters");
        assert_eq!(report.rows.len(), 1);
        assert!(report.rows[0].key.contains("google"));
        assert!(report.rows[0].key.starts_with("{\"Provider\":\"google\""));
    }
}

#[test]
fn test_evidence_durations_only_from_completed_runs() {
    let recs = vec![
        Run {
            id: "1".into(),
            status: STATUS_COMPLETED.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(1000),
            ..Default::default()
        },
        Run {
            id: "2".into(),
            status: STATUS_TIMED_OUT.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(600_000),
            ..Default::default()
        },
        Run {
            id: "3".into(),
            status: STATUS_CANCELLED.into(),
            tier: "easy".into(),
            model: "m".into(),
            run_ms: Some(500_000),
            ..Default::default()
        },
    ];
    let ev = evidence_by_tier_model(&recs);
    let e = &ev[&evidence_key("easy", "m")];
    assert_eq!(e.finished, 3);
    assert_eq!(e.duration_n, 1);
    assert_eq!(e.p50_ms, Some(1000));
}

/// Spend and Stats read the runs' history, which deleting a run leaves in place, and a
/// repository's folder keeps the runs in it or below it.
#[test]
fn history_outlives_a_deleted_run_and_filters_by_folder() {
    use dispatch_core::types::{Run, Store as _, Usage};
    let store = dispatch_core::store::Store::new(":memory:").unwrap();
    let run = |id: &str, dir: &str| Run {
        id: id.into(),
        project_id: "ws-home".into(),
        status: "COMPLETED".into(),
        agent: "codex".into(),
        dir: dir.into(),
        usage: Some(Usage {
            input_tokens: 10,
            cost_usd: Some(0.5),
            ..Default::default()
        }),
        ..Default::default()
    };
    store.save_run(&run("a", "/code/repo")).unwrap();
    store.save_run(&run("b", "/code/repo/sub")).unwrap();
    store.save_run(&run("c", "/code/other")).unwrap();
    store.delete_runs(&["a".to_string()], "").unwrap();
    assert!(store.get_run("a").is_err(), "the run itself is gone");
    let all = store.list_run_history("", "").unwrap();
    assert_eq!(all.len(), 3);
    let repo = store.list_run_history("", "/code/repo/").unwrap();
    let mut ids: Vec<&str> = repo.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(repo[0].usage.as_ref().and_then(|u| u.cost_usd), Some(0.5));
}
