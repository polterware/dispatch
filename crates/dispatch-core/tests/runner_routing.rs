#[path = "runner_support.rs"]
mod support;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dispatch_core::{
    adapters::{Capabilities, quota::Quota},
    catalog::{BenchScore, Catalog},
    router,
    runner::{
        Candidate, Options, PRIORITY_BALANCED, PRIORITY_QUALITY, PRIORITY_SPEED, RouteReport,
        summarize,
    },
    tiers::{Entry, Table},
    tiers_build::build_tiers,
    types::{Filters, QuotaSnapshot, Run, STATUS_COMPLETED, STATUS_FAILED, Store},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use support::*;
use tokio_util::sync::CancellationToken;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap()
}

struct Routing {
    t: TestDeps,
    ran: Ran,
    fakes: Vec<Fake>,
}

impl Routing {
    fn set(&mut self, fakes: Vec<Fake>) {
        self.t.deps.adapters = fakes.iter().map(|f| f.clone().arc()).collect();
        self.fakes = fakes;
    }

    async fn run(&self, o: Options) -> Run {
        self.t.deps.run(&CancellationToken::new(), o).await.unwrap()
    }

    async fn select(&self, o: Options) -> (Vec<Candidate>, Vec<String>) {
        self.t.deps.select_candidates(&o).await
    }
}

fn routing_deps() -> Routing {
    let ran = ran();
    let caps = Capabilities {
        read_only: true,
        file_editing: true,
        effort_mode: "config".into(),
        efforts: vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()],
        ..Capabilities::default()
    };
    let quota = |used: f64| Quota {
        used_percent: used,
        window_minutes: 10080,
        resets_at: Some(now() + TimeDelta::hours(84)),
        ..Quota::default()
    };
    let a = Fake::new("a", &ran).caps(caps.clone()).quota(quota(40.0));
    let b = Fake::new("b", &ran).caps(caps).quota(quota(10.0));
    let fakes = vec![a, b];
    let mut t = test_deps(fakes.iter().map(|f| f.clone().arc()).collect());
    t.deps.now = Some(Arc::new(now));
    t.deps.table = table(&[
        (
            "easy",
            vec![
                entry("a", "gpt-small"),
                entry("b", "gpt-small"),
                entry("a", "gemini-fast"),
            ],
        ),
        (
            "hard",
            vec![entry("a", "gpt-large"), entry("b", "gemini-pro")],
        ),
    ]);
    Routing { t, ran, fakes }
}

fn filters(f: impl FnOnce(&mut Filters)) -> Filters {
    let mut out = Filters::default();
    f(&mut out);
    out
}

fn opts(tier: &str, f: Filters) -> Options {
    Options {
        tier: tier.into(),
        filters: f,
        prompt: "p".into(),
        no_worktree: true,
        ..Options::default()
    }
}

#[tokio::test]
async fn test_explicit_model_chooses_agent_by_pace() {
    let r = routing_deps();
    let reqs = r.fakes[1].requests.clone();
    let rec = r
        .run(opts(
            "",
            filters(|f| {
                f.model = "gpt-small".into();
                f.effort = "high".into();
            }),
        ))
        .await;
    assert!(
        rec.status == STATUS_COMPLETED && rec.agent == "b" && rec.effort == "high",
        "{rec:?}"
    );
    assert_eq!(ran_list(&r.ran).len(), 1);
    assert_eq!(reqs.lock().unwrap()[0].effort, "high");
    assert_eq!(rec.requested.as_ref().unwrap().model, "gpt-small");
    assert_eq!(rec.resolved.as_ref().unwrap().provider, "openai");
    let persisted = r.t.get(&rec.id);
    assert!(
        persisted.resolved.is_some()
            && summarize(&persisted).requested.unwrap().model == "gpt-small",
        "routing not persisted in summary"
    );
}

#[tokio::test]
async fn test_provider_and_tier_intersection() {
    let r = routing_deps();
    for (tier, model) in [("easy", "gpt-small"), ("hard", "gpt-large")] {
        let (cs, rejected) = r
            .select(opts(tier, filters(|f| f.provider = "openai".into())))
            .await;
        assert!(
            !cs.is_empty() && cs[0].model() == model,
            "{cs:?} {rejected:?}"
        );
    }
    let (cs, _) = r
        .select(opts("hard", filters(|f| f.model = "gpt-small".into())))
        .await;
    assert!(cs.is_empty(), "escaped tier intersection");
}

#[tokio::test]
async fn test_fixed_model_effort_only_evaluates_serving_agents() {
    let mut r = routing_deps();
    let (cs, _) = r
        .select(opts(
            "",
            filters(|f| {
                f.model = "gpt-small".into();
                f.effort = "xhigh".into();
            }),
        ))
        .await;
    for c in r.t.deps.evaluate(cs, true).await {
        assert!(
            c.model() == "gpt-small" && c.route.effort == "xhigh",
            "ranked a different model: {c:?}"
        );
    }
    // Filtering occurs before quota/ranking: an unrelated model is never evaluated.
    let mut unrelated = r.fakes[0].clone();
    unrelated.name = "unrelated".into();
    unrelated.quota_calls = Default::default();
    let calls = unrelated.quota_calls.clone();
    let mut fakes = r.fakes.clone();
    fakes.push(unrelated);
    r.set(fakes);
    r.t.deps
        .table
        .tiers
        .get_mut("hard")
        .unwrap()
        .push(entry("unrelated", "grok-other"));
    let (cs, _) = r
        .select(opts(
            "",
            filters(|f| {
                f.model = "gpt-small".into();
                f.effort = "high".into();
            }),
        ))
        .await;
    r.t.deps.evaluate(cs, true).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "unrelated model entered ranking"
    );
}

#[tokio::test]
async fn test_filters_fail_and_relax() {
    for relax in [false, true] {
        let r = routing_deps();
        let rec = r
            .run(opts(
                "easy",
                filters(|f| {
                    f.provider = "missing".into();
                    f.relax = relax;
                }),
            ))
            .await;
        if relax {
            assert!(
                rec.status == STATUS_COMPLETED && rec.note.contains("--relax"),
                "{rec:?}"
            );
        } else {
            assert!(
                rec.status == STATUS_FAILED
                    && ran_list(&r.ran).is_empty()
                    && rec.error.contains("provider filter"),
                "{rec:?}"
            );
        }
    }
}

#[tokio::test]
async fn test_fallback_never_changes_fixed_model() {
    let mut r = routing_deps();
    let fakes = r.fakes.iter().map(|f| f.clone().fail("failed")).collect();
    r.set(fakes);
    let rec = r
        .run(opts("easy", filters(|f| f.model = "gpt-small".into())))
        .await;
    let got = ran_list(&r.ran);
    assert!(
        rec.status == STATUS_FAILED && got.len() == 2,
        "{rec:?} {got:?}"
    );
    for run in got {
        assert!(run.ends_with("/gpt-small"), "{run}");
    }
}

#[tokio::test]
async fn test_unsupported_effort_refused() {
    let mut r = routing_deps();
    let a = r.fakes[0].clone();
    let mut caps = a.caps.lock().unwrap().clone();
    caps.effort_mode.clear();
    caps.efforts.clear();
    r.set(vec![a.clone().caps(caps.clone())]);
    let f = filters(|f| {
        f.agent = "a".into();
        f.model = "gpt-small".into();
        f.effort = "high".into();
    });
    let rec = r.run(opts("easy", f.clone())).await;
    assert!(
        rec.status == STATUS_FAILED
            && ran_list(&r.ran).is_empty()
            && rec.error.contains("effort unsupported"),
        "{rec:?}"
    );
    caps.effort_mode = "config".into();
    caps.efforts = vec!["low".into()];
    r.set(vec![Fake::new("a", &r.ran).caps(caps)]);
    let (cs, why) = r.select(opts("easy", f)).await;
    assert!(
        cs.is_empty() && why.join(";").contains("unsupported"),
        "{cs:?} {why:?}"
    );
}

#[tokio::test]
async fn test_route_no_side_effects() {
    let r = routing_deps();
    r.t.store
        .save_quota(&QuotaSnapshot {
            agent: "b".into(),
            used_percent: Some(5.0),
            window_minutes: 10080,
            resets_at: rfc(now() + TimeDelta::hours(84)),
            sampled_at: rfc(now()),
            ..QuotaSnapshot::default()
        })
        .unwrap();
    let before = r.t.store.run_writes.load(Ordering::SeqCst);
    let rep =
        r.t.deps
            .route(Options {
                filters: filters(|f| {
                    f.model = "gpt-small".into();
                    f.effort = "high".into();
                }),
                ..Options::default()
            })
            .await
            .unwrap();
    let calls: usize = r
        .fakes
        .iter()
        .map(|f| f.quota_calls.load(Ordering::SeqCst))
        .sum();
    assert!(
        rep.winner.as_ref().is_some_and(|w| w.agent == "b")
            && calls == 0
            && ran_list(&r.ran).is_empty()
            && before == r.t.store.run_writes.load(Ordering::SeqCst),
        "{rep:?} calls={calls}"
    );
    assert!(
        r.t.store.list_runs("", 0, 0, "").unwrap().is_empty(),
        "created runs"
    );
    if let Ok(entries) = std::fs::read_dir(&r.t.deps.worktree_dir) {
        assert_eq!(entries.count(), 0, "created worktree");
    }
}

#[tokio::test]
async fn test_outside_table_and_safety_constraints() {
    let r = routing_deps();
    let (cs, _) = r
        .select(opts("hard", filters(|f| f.model = "gpt-new".into())))
        .await;
    assert!(cs.len() == 2 && !cs[0].route.note.is_empty(), "{cs:?}");
    let (cs, why) = r
        .select(opts(
            "",
            filters(|f| {
                f.model = "claude-opus".into();
                f.agent = "a".into();
            }),
        ))
        .await;
    assert!(
        cs.len() == 1 && cs[0].agent() == "a" && cs[0].route.model == "claude-opus",
        "a Claude model routes through any agent that offers it: {cs:?} {why:?}"
    );
    r.t.store
        .set_cooldown("b", &rfc(now() + TimeDelta::hours(1)))
        .unwrap();
    let (cs, _) = r
        .select(opts("", filters(|f| f.model = "gpt-small".into())))
        .await;
    assert!(cs.len() == 1 && cs[0].agent() == "a", "{cs:?}");
}

#[tokio::test]
async fn test_suffix_effort_and_readonly() {
    let mut r = routing_deps();
    let caps = Capabilities {
        read_only: true,
        file_editing: true,
        effort_mode: "model-suffix".into(),
        ..Capabilities::default()
    };
    r.set(vec![r.fakes[0].clone().caps(caps.clone())]);
    r.t.deps.table = table(&[(
        "easy",
        vec![Entry {
            efforts: vec!["low".into(), "high".into()],
            ..entry("a", "gemini-fast-low")
        }],
    )]);
    let effort = |e: &str| {
        let e = e.to_owned();
        filters(move |f| {
            f.model = "gemini-fast".into();
            f.effort = e;
        })
    };
    let (cs, why) = r.select(opts("easy", effort("high"))).await;
    assert!(
        cs.len() == 1 && cs[0].model() == "gemini-fast-high" && cs[0].route.effort == "high",
        "{cs:?} {why:?}"
    );
    let (cs, why) = r.select(opts("easy", effort("xhigh"))).await;
    assert!(
        cs.is_empty() && why.join(";").contains("unsupported"),
        "{cs:?} {why:?}"
    );
    r.set(vec![Fake::new("a", &r.ran).caps(Capabilities {
        read_only: false,
        ..caps
    })]);
    let (cs, why) = r
        .select(Options {
            read_only: true,
            ..opts("easy", Filters::default())
        })
        .await;
    assert!(
        cs.is_empty() && why.join(";").contains("readOnly"),
        "{cs:?} {why:?}"
    );
}

fn priority_deps() -> Routing {
    let mut r = routing_deps();
    let caps = Capabilities {
        read_only: true,
        file_editing: true,
        ..Capabilities::default()
    };
    let fakes = r
        .fakes
        .iter()
        .map(|f| {
            let mut f = f.clone().caps(caps.clone());
            f.caps = Arc::new(std::sync::Mutex::new(caps.clone()));
            f.quota = None;
            f
        })
        .collect();
    r.set(fakes);
    r.t.deps.table = table(&[(
        "normal",
        vec![
            Entry {
                provider: "openai".into(),
                ..entry("a", "fast")
            },
            Entry {
                provider: "google".into(),
                ..entry("b", "strong")
            },
        ],
    )]);
    r
}

/// Ranks the normal tier by score, as a filtered route does, apart from the
/// tier's table order.
async fn ranked_for_priority(r: &Routing, priority: &str) -> Vec<Candidate> {
    let (cs, _) = r
        .select(Options {
            tier: "normal".into(),
            filters: filters(|f| f.priority = priority.into()),
            ..Options::default()
        })
        .await;
    let out = r.t.deps.evaluate(cs, false).await;
    assert!(!out.is_empty(), "no candidates");
    out
}

#[tokio::test]
async fn test_priority_reads_no_hand_set_prior() {
    let r = priority_deps();
    let route = |priority: &'static str| {
        let deps = r.t.deps.clone();
        async move {
            let rep: RouteReport = deps
                .route(Options {
                    tier: "normal".into(),
                    filters: filters(|f| f.priority = priority.into()),
                    ..Options::default()
                })
                .await
                .unwrap();
            assert!(
                rep.winner.is_some() && rep.candidates.len() == 2,
                "{priority}: {rep:?}"
            );
            rep
        }
    };
    // A tier route follows the table whatever the priority.
    for priority in [PRIORITY_SPEED, PRIORITY_QUALITY, PRIORITY_BALANCED] {
        let w = route(priority).await.winner.unwrap();
        assert!(
            w.agent == "a" && w.model == "fast",
            "{priority}: tier route left the table order: {w:?}"
        );
    }
    // With equal quota a ranked route keeps the table order under every
    // priority: nothing but quota, cooldowns, and bias reorders it.
    for priority in [PRIORITY_SPEED, PRIORITY_QUALITY, PRIORITY_BALANCED] {
        let ranked = ranked_for_priority(&r, priority).await;
        assert!(
            ranked[0].agent() == "a" && ranked[0].model() == "fast",
            "{priority}: {ranked:?}"
        );
    }
    let speed = ranked_for_priority(&r, PRIORITY_SPEED).await;
    let reason = router::reason(&speed[0].route);
    assert!(
        reason.contains("paceSlack=") && !reason.contains("duration"),
        "{reason}"
    );
    let o = |p: &str| Options {
        tier: "normal".into(),
        filters: filters(|f| f.priority = p.into()),
        ..Options::default()
    };
    let (_, rej_s) = r.select(o(PRIORITY_SPEED)).await;
    let (_, rej_q) = r.select(o(PRIORITY_QUALITY)).await;
    assert_eq!(rej_s, rej_q, "priority filtered");
    let rec = r
        .run(opts(
            "normal",
            filters(|f| f.priority = PRIORITY_QUALITY.into()),
        ))
        .await;
    assert!(rec.agent == "a" && rec.model == "fast", "{rec:?}");
    assert!(
        rec.requested.as_ref().unwrap().priority == PRIORITY_QUALITY
            && rec.resolved.as_ref().unwrap().priority == PRIORITY_QUALITY,
        "record {:?} {:?}",
        rec.requested,
        rec.resolved
    );
    let sum = summarize(&rec);
    assert!(
        sum.requested.unwrap().priority == PRIORITY_QUALITY
            && sum.resolved.unwrap().priority == PRIORITY_QUALITY
    );
}

#[tokio::test]
async fn test_priority_does_not_bypass_constraints() {
    let r = priority_deps();
    r.t.store
        .set_cooldown("a", &rfc(now() + TimeDelta::hours(1)))
        .unwrap();
    let o = |f: Filters, read_only: bool| Options {
        tier: "normal".into(),
        filters: f,
        read_only,
        ..Options::default()
    };
    let (cs, why) = r
        .select(o(filters(|f| f.priority = PRIORITY_SPEED.into()), false))
        .await;
    assert!(
        cs.len() == 1 && cs[0].agent() == "b" && why.join(";").contains("cooldown"),
        "{cs:?} {why:?}"
    );
    let mut r = priority_deps();
    let (cs, why) = r
        .select(o(
            filters(|f| {
                f.priority = PRIORITY_SPEED.into();
                f.provider = "google".into();
            }),
            false,
        ))
        .await;
    assert!(
        cs.len() == 1 && cs[0].model() == "strong" && why.join(";").contains("provider filter"),
        "{cs:?} {why:?}"
    );
    let mut fakes = r.fakes.clone();
    fakes[0] = Fake::new("a", &r.ran).caps(Capabilities {
        file_editing: true,
        ..Capabilities::default()
    });
    r.set(fakes);
    let (cs, why) = r
        .select(o(filters(|f| f.priority = PRIORITY_QUALITY.into()), true))
        .await;
    assert!(
        cs.len() == 1 && cs[0].agent() == "b" && why.join(";").contains("readOnly"),
        "{cs:?} {why:?}"
    );
}

#[tokio::test]
async fn test_relax_after_filtered_candidates_fail() {
    let mut r = routing_deps();
    let fakes = vec![r.fakes[0].clone().fail("crash"), r.fakes[1].clone()];
    r.set(fakes);
    let rec = r
        .run(opts(
            "easy",
            filters(|f| {
                f.agent = "a".into();
                f.relax = true;
            }),
        ))
        .await;
    let got = ran_list(&r.ran);
    assert!(
        rec.status == STATUS_COMPLETED
            && rec.agent == "b"
            && rec.note.contains("--relax")
            && got.len() == 3,
        "{rec:?} {got:?}"
    );
}

#[tokio::test]
async fn test_disabled_models_apply_to_fallback_relax_and_flags() {
    let r = routing_deps();
    r.t.store.set_model_enabled("google", "", false).unwrap();
    r.t.store
        .set_model_enabled("openai", "gpt-small", false)
        .unwrap();
    let rec = r.run(opts("easy", filters(|f| f.relax = true))).await;
    assert!(
        rec.status == STATUS_FAILED
            && ran_list(&r.ran).is_empty()
            && rec.error.contains("model openai/gpt-small is disabled"),
        "{rec:?}"
    );
    r.t.store
        .set_model_enabled("openai", "gpt-small", true)
        .unwrap();
    let google = filters(|f| {
        f.provider = "google".into();
        f.relax = true;
    });
    let (cs, why) = r.select(opts("easy", google.clone())).await;
    assert!(
        cs.is_empty() && why.join(";").contains("provider google is disabled"),
        "{cs:?} {why:?}"
    );
    let rec = r.run(opts("easy", google)).await;
    assert!(
        rec.status == STATUS_FAILED && rec.error.contains("provider google is disabled"),
        "{rec:?}"
    );
}

// From route_decision_test.go.
#[tokio::test]
async fn test_route_never_records_a_decision() {
    let r = priority_deps();
    r.t.deps
        .route(Options {
            tier: "normal".into(),
            ..Options::default()
        })
        .await
        .unwrap();
    let decs = r.t.store.decisions();
    assert!(decs.is_empty(), "Route logged {decs:?}");
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn effort_catalog() -> Catalog {
    let score = |model: &str, effort: &str, score: f64, cost: f64| BenchScore {
        model: model.into(),
        effort: effort.into(),
        score,
        cost_per_task: Some(cost),
        ..BenchScore::default()
    };
    Catalog {
        scores: vec![
            score("elastic", "low", sigmoid(-1.0), 0.3),
            score("elastic", "medium", 0.5, 0.5),
            score("elastic", "high", sigmoid(1.0), 0.8),
            score("dearer", "medium", sigmoid(2.0), 2.0),
        ],
        ..Catalog::default()
    }
}

// From tiers_build_test.go. The Go Rows/TableFromRows step has no Rust
// counterpart, so the stored table is restored by its JSON alone.
#[tokio::test]
async fn test_build_tiers_selects_and_stores_each_tiers_best_effort() {
    let got = build_tiers(
        &[Entry {
            efforts: vec!["low".into(), "medium".into(), "high".into()],
            ..entry("codex", "elastic")
        }],
        &effort_catalog(),
        PRIORITY_BALANCED,
    );
    let want = [("easy", "low"), ("normal", "medium"), ("hard", "high")];
    for (tier, effort) in want {
        let built = &got.tiers[tier];
        assert!(
            built.len() == 1
                && built[0].entry.effort == effort
                && got.table.tiers[tier][0].effort == effort,
            "{tier}: {built:?}, want {effort}"
        );
    }
    let data = serde_json::to_string(&got.table).unwrap();
    let stored = Table::parse(&data).unwrap();
    let mut r = routing_deps();
    let mut a = r.fakes[0].clone();
    a.name = "codex".into();
    r.set(vec![a]);
    r.t.deps.table = stored.clone();
    for (tier, effort) in want {
        let (cs, rejected) = r
            .select(Options {
                tier: tier.into(),
                ..Options::default()
            })
            .await;
        assert!(
            cs.len() == 1 && cs[0].model() == "elastic" && cs[0].route.effort == effort,
            "{tier} restored: {cs:?}, rejected {rejected:?}"
        );
    }
    // A stored choice overrides the tier default, including after a board move.
    r.t.deps
        .table
        .tiers
        .insert("easy".into(), stored.tiers["hard"].clone());
    let (cs, _) = r
        .select(Options {
            tier: "easy".into(),
            ..Options::default()
        })
        .await;
    assert!(
        cs.len() == 1 && cs[0].route.effort == "high",
        "stored effort lost: {cs:?}"
    );
}

#[tokio::test]
async fn test_discover_adds_normal_candidate_only_when_enabled() {
    let mut r = routing_deps();
    let mut a = r.fakes[0].clone();
    a.models = vec!["gpt-discovered".into()];
    let fakes = vec![a, r.fakes[1].clone()];
    r.set(fakes);
    let normal = || Options {
        tier: "normal".into(),
        ..Options::default()
    };
    let (cs, _) = r.select(normal()).await;
    assert!(cs.is_empty(), "discover default must be off: {cs:?}");
    r.t.deps.policy.discover = true;
    let (cs, why) = r.select(normal()).await;
    assert!(
        cs.len() == 1 && cs[0].model() == "gpt-discovered" && cs[0].route.note == "discovered",
        "{cs:?} {why:?}"
    );
}

#[tokio::test]
async fn test_an_agent_the_table_never_names_routes_on_its_own_models() {
    let ran = ran();
    let caps = Capabilities {
        read_only: true,
        file_editing: true,
        effort_mode: "config".into(),
        efforts: vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()],
        ..Capabilities::default()
    };
    let a = Fake::new("a", &ran).caps(caps.clone());
    let mut c = Fake::new("c", &ran).caps(caps);
    c.models = vec!["gpt-large".into(), "gpt-small".into()];
    let mut t = test_deps(vec![a.arc(), c.arc()]);
    t.deps.now = Some(Arc::new(now));
    t.deps.table = table(&[
        ("easy", vec![entry("a", "gpt-small")]),
        ("hard", vec![entry("a", "gpt-large")]),
    ]);
    for tier in ["easy", "hard"] {
        let (cs, why) = t
            .deps
            .select_candidates(&opts(tier, filters(|f| f.agent = "c".into())))
            .await;
        assert!(
            !cs.is_empty() && cs.iter().all(|c| c.route.agent == "c"),
            "{tier}: {cs:?} {why:?}"
        );
    }
}
