use dispatch_core::{
    catalog::{BenchScore, Catalog},
    config::Policy,
    pass_model::sigmoid,
    router::{
        self, AVOID_BIAS, Agent, Candidate, Filters, History, PREFER_BIAS, Pace, RouteOptions,
    },
    tiers::Table,
};

fn candidate(model: &str, pace: Option<Pace>) -> Candidate {
    Candidate {
        model: model.into(),
        pace,
        ..Default::default()
    }
}
fn pace(delta: f64, deficit: bool, will_last: bool) -> Pace {
    Pace {
        delta,
        deficit,
        will_last,
        ..Default::default()
    }
}
fn names(cs: &[Candidate]) -> Vec<&str> {
    cs.iter().map(|c| c.model.as_str()).collect()
}

fn auto_catalog(models: &[(&str, f64, f64)]) -> Catalog {
    Catalog {
        scores: models
            .iter()
            .map(|(model, bound, cost)| BenchScore {
                model: (*model).into(),
                score: sigmoid((bound / (1.0 - bound)).ln() + 0.3),
                cost_per_task: Some(*cost),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn auto_fallback_prefers_cheapest_near_best_bound() {
    let catalog = auto_catalog(&[
        ("gpt-6-astra", 0.79, 26.63),
        ("gpt-6.1-terra", 0.78, 2.66),
        ("far-cheaper", 0.74, 0.10),
    ]);
    let candidates: Vec<_> = ["gpt-6-astra", "gpt-6.1-terra", "far-cheaper"]
        .iter()
        .map(|model| Candidate {
            agent: "codex".into(),
            model: (*model).into(),
            ..Default::default()
        })
        .collect();
    let ranked = router::rank_auto(&candidates, 0.0, "balanced", &catalog);
    assert_eq!(
        names(&ranked),
        ["gpt-6.1-terra", "gpt-6-astra", "far-cheaper"]
    );
    assert!(ranked.iter().all(|c| !c.auto.as_ref().unwrap().qualified));
    assert!(router::auto_note(&ranked).contains("cheapest within 0.02 of the best bound"));
}

#[test]
fn auto_qualified_candidates_still_rank_by_cost() {
    let catalog = auto_catalog(&[
        ("strong", 0.90, 10.0),
        ("cheap", 0.81, 1.0),
        ("below", 0.79, 0.1),
    ]);
    let candidates = [
        candidate("strong", None),
        candidate("cheap", None),
        candidate("below", None),
    ];
    let ranked = router::rank_auto(&candidates, 0.0, "balanced", &catalog);
    assert_eq!(names(&ranked), ["cheap", "strong", "below"]);
    assert!(ranked[0].auto.as_ref().unwrap().qualified);
    assert!(ranked[1].auto.as_ref().unwrap().qualified);
}

#[test]
fn test_rank_order() {
    let cs = [
        candidate("out", Some(pace(30., true, false))),
        candidate("unk", None),
        candidate("deficit", Some(pace(10., true, true))),
        candidate("little", Some(pace(-5., false, true))),
        candidate("reserve", Some(pace(-30., false, true))),
    ];
    assert_eq!(
        names(&router::rank(&cs)),
        ["reserve", "little", "unk", "deficit", "out"]
    );
}
#[test]
fn test_rank_cooldown_and_risk() {
    let mut cooled = candidate("cooled", Some(pace(-10., false, true)));
    cooled.cooled_down = true;
    let mut risky = candidate("risky", cooled.pace.clone());
    risky.history = Some(History {
        valid: true,
        risk: 0.9,
    });
    let mut safe = candidate("safe", cooled.pace.clone());
    safe.history = Some(History {
        valid: true,
        risk: 0.1,
    });
    assert_eq!(
        names(&router::rank(&[cooled, risky, safe])),
        ["safe", "risky", "cooled"]
    );
}
#[test]
fn test_bias_prefer_and_avoid() {
    let mut a = candidate("a", Some(pace(-20., false, true)));
    a.bias = AVOID_BIAS;
    let mut b = candidate("b", a.pace.clone());
    b.bias = PREFER_BIAS;
    assert_eq!(
        names(&router::rank(&[a, candidate("c", None), b])),
        ["b", "c", "a"]
    );
}
#[test]
fn test_balanced_score_and_reason_unchanged() {
    let mut c = candidate("m", Some(pace(-30., false, true)));
    c.bias = PREFER_BIAS;
    c.history = Some(History {
        valid: true,
        risk: 0.4,
    });
    c.cooled_down = true;
    let score = router::score(&c);
    let reason = router::reason(&c);
    c.priority = "balanced".into();
    assert_eq!(router::score(&c), score);
    assert_eq!(router::reason(&c), reason);
    let mut cs = [
        candidate("out", Some(pace(30., true, false))),
        candidate("unk", None),
        candidate("deficit", Some(pace(10., true, true))),
        candidate("little", Some(pace(-5., false, true))),
        candidate("reserve", Some(pace(-30., false, true))),
    ];
    for c in &mut cs {
        c.priority = "balanced".into();
    }
    assert_eq!(
        names(&router::rank(&cs)),
        ["reserve", "little", "unk", "deficit", "out"]
    );
}
#[test]
fn test_quality_scores_as_balanced() {
    let mut c = candidate("m", Some(pace(-30., false, true)));
    c.bias = AVOID_BIAS;
    c.priority = "balanced".into();
    let balanced = router::score(&c);
    c.priority = "quality".into();
    assert_eq!(router::score(&c), balanced);
    assert_eq!(router::terms(&c).len(), 2);
}
#[test]
fn test_priority_does_not_outrank_cooldown() {
    let mut a = candidate("cooled", Some(pace(-50., false, true)));
    a.priority = "speed".into();
    a.cooled_down = true;
    let mut b = candidate("other", Some(pace(80., true, true)));
    b.priority = "speed".into();
    assert_eq!(router::rank(&[a, b])[0].model, "other");
}
#[test]
fn test_terms_sum_to_score() {
    let mut a = candidate("", Some(pace(-10., false, true)));
    a.priority = "speed".into();
    a.bias = PREFER_BIAS;
    let mut b = candidate("", None);
    b.priority = "quality".into();
    let mut c = candidate("", Some(pace(4., true, true)));
    c.priority = "balanced".into();
    c.history = Some(History {
        valid: true,
        risk: 0.25,
    });
    for c in [a, b, c] {
        assert!(
            (router::terms(&c).iter().map(|t| t.value).sum::<f64>() - router::score(&c)).abs()
                < 1e-9
        );
    }
}

/// Antigravity as its CLI reports itself: the effort in the model's name, and
/// Claude models beside Gemini's.
fn antigravity() -> Agent {
    Agent {
        name: "antigravity".into(),
        installed: true,
        read_only: true,
        file_editing: true,
        effort_mode: "model-suffix".into(),
        discovered: [
            "gemini-3.8-flash-high",
            "claude-sonnet-5-5-low",
            "claude-sonnet-5-5-high",
            "claude-opus-5-5-high",
        ]
        .map(String::from)
        .to_vec(),
        ..Default::default()
    }
}

#[test]
fn antigravity_claude_model_routes_when_requested() {
    let cat = Catalog::embedded().unwrap();
    let r = router::route(
        &Table::default(),
        &Policy::default(),
        &[antigravity()],
        &RouteOptions {
            filters: Filters {
                agent: "antigravity".into(),
                model: "claude-sonnet-5-5-high".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        &cat,
    )
    .unwrap();
    let winner = r.winner.expect("winner");
    assert_eq!(
        (
            winner.agent.as_str(),
            winner.model.as_str(),
            winner.provider.as_str()
        ),
        ("antigravity", "claude-sonnet-5-5-high", "anthropic"),
        "{:?}",
        r.eliminated
    );
}

#[test]
fn auto_weighs_antigravity_claude_models_on_the_claude_benchmarks() {
    let cat = Catalog::embedded().unwrap();
    let r = router::route(
        &Table::default(),
        &Policy::default(),
        &[antigravity()],
        &RouteOptions {
            tier: "auto".into(),
            difficulty: Some(0.0),
            ..Default::default()
        },
        &cat,
    )
    .unwrap();
    let got: Vec<&str> = r
        .candidates
        .iter()
        .map(|c| c.filters.model.as_str())
        .collect();
    for want in [
        "gemini-3.8-flash-high",
        "claude-sonnet-5-5-high",
        "claude-opus-5-5-high",
    ] {
        assert!(
            got.contains(&want),
            "auto lacks {want}: {got:?} {:?}",
            r.eliminated
        );
    }
    for c in &r.candidates {
        if !c.filters.model.starts_with("claude-") {
            continue;
        }
        let auto = c.auto.as_ref().expect("auto info");
        let claude = cat.estimate(&c.filters.model, &c.filters.effort);
        assert_eq!(c.filters.provider, "anthropic");
        assert!(
            !auto.theta_source.is_empty() && auto.theta_source != "unknown",
            "{}: {}",
            c.filters.model,
            auto.theta_source
        );
        assert!(
            (auto.theta - claude.theta).abs() < 1e-9,
            "{}",
            c.filters.model
        );
        assert!(auto.cost.is_some(), "{}: unpriced", c.filters.model);
    }
}

#[test]
fn disabled_provider_hides_claude_in_other_clis() {
    let cat = Catalog::embedded().unwrap();
    let policy = Policy::default();
    let r = router::route(
        &Table::default(),
        &policy,
        &[antigravity()],
        &RouteOptions {
            disabled: vec![dispatch_core::types::DisabledModel {
                provider: "anthropic".into(),
                model: String::new(),
                ..Default::default()
            }],
            tier: "auto".into(),
            difficulty: Some(0.0),
            ..Default::default()
        },
        &cat,
    )
    .unwrap();
    assert!(
        r.candidates
            .iter()
            .all(|c| !c.filters.model.starts_with("claude-")),
        "{:?}",
        r.candidates
    );
    assert!(
        r.eliminated
            .iter()
            .any(|e| e.contains("claude-opus-5-5-high: provider anthropic is disabled")),
        "{:?}",
        r.eliminated
    );
}

#[test]
fn max_effort_routes_where_the_agent_takes_it() {
    let cat = Catalog::embedded().unwrap();
    let config_agent = |name: &str, efforts: &[&str]| Agent {
        name: name.into(),
        installed: true,
        read_only: true,
        file_editing: true,
        effort_mode: "config".into(),
        efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
        discovered: vec!["claude-opus-5-5".into()],
        ..Default::default()
    };
    let agents = [
        config_agent("claude", &["low", "medium", "high", "xhigh", "max"]),
        config_agent("codex", &["low", "medium", "high", "xhigh"]),
    ];
    let r = router::route(
        &Table::default(),
        &Policy::default(),
        &agents,
        &RouteOptions {
            filters: Filters {
                model: "claude-opus-5-5".into(),
                effort: "max".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        &cat,
    )
    .unwrap();
    let winner = r.winner.expect("winner");
    assert_eq!(
        (winner.agent.as_str(), winner.effort.as_str()),
        ("claude", "max")
    );
    assert!(
        r.eliminated
            .iter()
            .any(|e| e.starts_with("codex/claude-opus-5-5: effort max unsupported")),
        "{:?}",
        r.eliminated
    );
}

/// A model with no agent runs on every agent that serves it, so a limit on one falls back to
/// the next; a model with an agent runs only there, so nothing else takes it.
#[test]
fn a_model_alone_routes_to_every_agent_that_serves_it() {
    let cat = Catalog::embedded().unwrap();
    let agent = |name: &str, effort_mode: &str, models: &[&str]| Agent {
        name: name.into(),
        installed: true,
        read_only: true,
        file_editing: true,
        effort_mode: effort_mode.into(),
        efforts: vec!["low".into(), "medium".into(), "high".into()],
        discovered: models.iter().map(|m| (*m).to_owned()).collect(),
        ..Default::default()
    };
    let agents = [
        agent("antigravity", "model-suffix", &["claude-opus-5-5-high"]),
        agent("claude", "config", &["claude-opus-5-5"]),
    ];
    let route = |agent: &str| {
        router::route(
            &Table::default(),
            &Policy {
                discover: true,
                ..Default::default()
            },
            &agents,
            &RouteOptions {
                filters: Filters {
                    model: "claude-opus-5-5-high".into(),
                    agent: agent.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            &cat,
        )
        .unwrap()
    };
    let mut both: Vec<String> = route("")
        .candidates
        .iter()
        .map(|c| c.filters.agent.clone())
        .collect();
    both.sort();
    both.dedup();
    assert_eq!(both, ["antigravity", "claude"]);
    let pinned = route("antigravity");
    assert!(
        pinned
            .candidates
            .iter()
            .all(|c| c.filters.agent == "antigravity"),
        "{:?}",
        pinned.candidates
    );
}

#[test]
fn disabled_base_model_is_never_routed_on_any_agent_or_effort() {
    let disabled = vec![dispatch_core::types::DisabledModel {
        provider: "anthropic".into(),
        model: "claude-opus-5-5".into(),
        ..Default::default()
    }];
    let agents = ["claude", "antigravity"]
        .into_iter()
        .map(|name| dispatch_core::router::Agent {
            name: name.into(),
            installed: true,
            file_editing: true,
            read_only: true,
            discovered: vec![
                "claude-opus-5-5".into(),
                "claude-opus-5-5-high".into(),
                "gemini-4-argon-high".into(),
            ],
            ..Default::default()
        })
        .collect::<Vec<_>>();
    for tier in ["auto", "normal"] {
        let opts = RouteOptions {
            tier: tier.into(),
            difficulty: Some(-1.8),
            disabled: disabled.clone(),
            ..Default::default()
        };
        let (candidates, notes) = dispatch_core::router::select_candidates(
            &Table::default(),
            &Policy::default(),
            &agents,
            &opts,
        );
        assert!(
            candidates
                .iter()
                .all(|c| !c.model.starts_with("claude-opus-5-5"))
        );
        assert!(notes.iter().any(|n| n.contains("disabled")), "{notes:?}");
    }
}

#[test]
fn one_agents_disabled_model_still_routes_through_another_agent() {
    let agents = ["claude", "antigravity"]
        .into_iter()
        .map(|name| dispatch_core::router::Agent {
            name: name.into(),
            installed: true,
            file_editing: true,
            read_only: true,
            discovered: vec!["claude-opus-5-5-high".into()],
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let route = |disabled: Vec<dispatch_core::types::DisabledModel>| {
        let (candidates, notes) = dispatch_core::router::select_candidates(
            &Table::default(),
            &Policy {
                discover: true,
                ..Default::default()
            },
            &agents,
            &RouteOptions {
                tier: "auto".into(),
                difficulty: Some(-1.8),
                disabled,
                ..Default::default()
            },
        );
        (candidates, notes)
    };
    let (candidates, notes) = route(vec![dispatch_core::types::DisabledModel {
        agent: "claude".into(),
        model: "claude-opus-5-5".into(),
        ..Default::default()
    }]);
    assert!(
        candidates
            .iter()
            .all(|c| c.agent == "antigravity" && c.model == "claude-opus-5-5-high"),
        "{candidates:?} {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("for agent claude")),
        "{notes:?}"
    );

    let (candidates, notes) = route(vec![
        dispatch_core::types::DisabledModel {
            agent: "claude".into(),
            model: "claude-opus-5-5".into(),
            ..Default::default()
        },
        dispatch_core::types::DisabledModel {
            agent: "antigravity".into(),
            model: "claude-opus-5-5".into(),
            ..Default::default()
        },
    ]);
    assert!(candidates.is_empty(), "{candidates:?} {notes:?}");
}
