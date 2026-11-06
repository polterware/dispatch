use dispatch_core::{
    catalog::{BenchScore, Catalog},
    config::Policy,
    router::{self, Agent, Filters, RouteOptions},
    tiers::{Entry, Table},
};
use std::collections::BTreeMap;
fn entry(agent: &str, model: &str) -> Entry {
    Entry {
        agent: agent.into(),
        model: model.into(),
        ..Default::default()
    }
}
fn agents() -> Vec<Agent> {
    ["a", "b"]
        .into_iter()
        .map(|name| Agent {
            name: name.into(),
            installed: true,
            read_only: true,
            file_editing: true,
            effort_mode: "config".into(),
            efforts: ["low", "medium", "high", "xhigh"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ..Default::default()
        })
        .collect()
}
fn catalog() -> Catalog {
    Catalog {
        scores: vec![
            BenchScore {
                model: "elastic".into(),
                effort: "low".into(),
                score: 1.0 / (1.0 + 1.0f64.exp()),
                cost_per_task: Some(0.3),
                ..Default::default()
            },
            BenchScore {
                model: "elastic".into(),
                effort: "medium".into(),
                score: 0.5,
                cost_per_task: Some(0.5),
                ..Default::default()
            },
            BenchScore {
                model: "elastic".into(),
                effort: "high".into(),
                score: 1.0 / (1.0 + (-1.0f64).exp()),
                cost_per_task: Some(0.8),
                ..Default::default()
            },
            BenchScore {
                model: "dearer".into(),
                effort: "medium".into(),
                score: 1.0 / (1.0 + (-2.0f64).exp()),
                cost_per_task: Some(2.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
#[test]
fn test_auto_picks_medium_over_a_dearer_model_and_keeps_one_effort_per_agent_model() {
    let mut a = entry("a", "elastic");
    a.efforts = ["low", "medium", "high"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut b = a.clone();
    b.agent = "b".into();
    let table = Table {
        tiers: BTreeMap::from([("easy".into(), vec![a, entry("a", "dearer"), b])]),
    };
    let (cs, rejected) = router::select_candidates(
        &table,
        &Policy {
            discover: false,
            ..Default::default()
        },
        &agents(),
        &RouteOptions {
            tier: "auto".into(),
            ..Default::default()
        },
    );
    assert_eq!(cs.len(), 10, "{rejected:?}");
    let got = router::rank_auto(&cs, -1.8, "balanced", &catalog());
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].model, "elastic");
    assert_eq!(got[0].effort, "medium");
    assert!(got[0].auto.as_ref().unwrap().qualified);
    let suffix: Vec<_> = ["elastic-low", "elastic-medium", "elastic-high"]
        .into_iter()
        .map(|model| router::Candidate {
            model: model.into(),
            ..Default::default()
        })
        .collect();
    let got = router::rank_auto(&suffix, -1.8, "balanced", &catalog());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].model, "elastic-medium");
}
#[test]
fn board_order_and_empty_band_fallback() {
    let general = Table {
        tiers: BTreeMap::from([
            ("normal".into(), vec![entry("a", "general")]),
            ("easy".into(), vec![entry("a", "easy")]),
        ]),
    };
    let board = Table {
        tiers: BTreeMap::from([(
            "normal".into(),
            vec![entry("b", "first"), entry("a", "second")],
        )]),
    };
    let p = Policy {
        discover: false,
        ..Default::default()
    };
    let opts = RouteOptions {
        tier: "auto".into(),
        difficulty: Some(-1.8),
        role_board: board,
        ..Default::default()
    };
    let result = router::plan(&general, &p, &agents(), &opts, &catalog());
    assert_eq!(
        result
            .candidates
            .iter()
            .map(|c| c.model.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let result = router::plan(
        &general,
        &p,
        &agents(),
        &RouteOptions {
            difficulty: Some(-2.8),
            ..opts
        },
        &catalog(),
    );
    assert_eq!(result.candidates[0].model, "easy");
}
#[test]
fn filters_relax_and_policy_protection() {
    let table = Table {
        tiers: BTreeMap::from([("normal".into(), vec![entry("a", "one"), entry("b", "two")])]),
    };
    let opts = RouteOptions {
        tier: "normal".into(),
        filters: Filters {
            agent: "a".into(),
            relax: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let policy = Policy {
        discover: false,
        ..Default::default()
    };
    let result = router::plan(&table, &policy, &agents(), &opts, &catalog());
    assert_eq!(
        result
            .candidates
            .iter()
            .map(|c| c.agent.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(result.relaxed);
    assert!(
        result
            .eliminated
            .iter()
            .any(|s| s == "b/two: agent filter requires a")
    );
    let mut opts = opts;
    opts.filters.model = "one".into();
    opts.disabled.push(dispatch_core::types::DisabledModel {
        provider: "a".into(),
        model: String::new(),
    });
    let result = router::plan(&table, &policy, &agents(), &opts, &catalog());
    assert!(result.candidates.is_empty());
    assert!(!result.relaxed);
}
#[test]
fn route_reports_auto_and_tier_explanations() {
    let table = Table {
        tiers: BTreeMap::from([(
            "normal".into(),
            vec![entry("a", "elastic"), entry("b", "dearer")],
        )]),
    };
    let p = Policy {
        discover: false,
        ..Default::default()
    };
    let auto = router::route(
        &table,
        &p,
        &agents(),
        &RouteOptions {
            tier: "auto".into(),
            difficulty: Some(-1.8),
            ..Default::default()
        },
        &catalog(),
    )
    .unwrap();
    assert_eq!(auto.difficulty, Some(-1.8));
    assert_eq!(auto.winner.unwrap().model, "elastic");
    assert!(auto.reason.starts_with("auto: the cheapest candidate"));
    assert!(auto.note.contains("auto: d=-1.80"));
    assert!(auto.candidates[0].reason.contains("P(pass)="));
    let tier = router::route(
        &table,
        &p,
        &agents(),
        &RouteOptions {
            tier: "normal".into(),
            ..Default::default()
        },
        &catalog(),
    )
    .unwrap();
    assert!(tier.reason.starts_with("the tier's table order wins"));
    assert!(
        router::route(
            &table,
            &p,
            &agents(),
            &RouteOptions {
                tier: "auto".into(),
                filters: Filters {
                    model: "elastic".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            &catalog()
        )
        .is_err()
    );
}
