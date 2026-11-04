use dispatch_core::{
    catalog::{BenchScore, Catalog},
    difficulty::{new_difficulty, tier_from_probs},
    router::{Candidate, Pace, order_tier, rank_auto},
    tiers::Entry,
};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct Golden {
    table: HashMap<String, Vec<Entry>>,
    scenarios: Vec<Scenario>,
}
#[derive(Deserialize)]
struct Quota {
    used: f64,
    elapsed: f64,
}
#[derive(Deserialize)]
struct Scenario {
    name: String,
    tier: String,
    quota: HashMap<String, Quota>,
    #[serde(default)]
    cooldown: Vec<String>,
    want: String,
}

#[test]
fn golden_scenarios_follow_the_tier_order() {
    let golden: Golden = serde_json::from_str(include_str!("fixtures/router_golden.json")).unwrap();
    for scenario in golden.scenarios {
        let candidates: Vec<_> = golden.table[&scenario.tier]
            .iter()
            .map(|e| {
                let pace = scenario.quota.get(&e.agent).map(|q| Pace {
                    actual: q.used,
                    delta: q.used - q.elapsed,
                    deficit: q.used > q.elapsed,
                    will_last: q.used <= 0.0 || q.elapsed == 0.0 || q.used <= q.elapsed,
                });
                Candidate {
                    agent: e.agent.clone(),
                    model: e.model.clone(),
                    pace,
                    cooled_down: scenario.cooldown.contains(&e.agent),
                    ..Default::default()
                }
            })
            .collect();
        let result = order_tier(&candidates);
        assert_eq!(
            format!("{}/{}", result[0].agent, result[0].model),
            scenario.want,
            "{}",
            scenario.name
        );
    }
}

#[test]
fn auto_scenarios_on_the_catalog() {
    let cat = Catalog::embedded().unwrap();
    let models = [
        ("gpt-5.6-luna", "medium"),
        ("gpt-5.6-terra", "medium"),
        ("claude-haiku-5-5", "high"),
        ("claude-opus-5-5", "high"),
        ("gemini-4-argon", "high"),
    ];
    for (d, priority, quota, want, qualified) in [
        (-2.8, "speed", false, "gpt-5.6-luna", true),
        (-2.8, "balanced", false, "claude-haiku-5-5", true),
        (-2.8, "quality", false, "claude-opus-5-5", true),
        (-1.8, "balanced", false, "claude-opus-5-5", true),
        (-0.8, "balanced", false, "claude-opus-5-5", false),
        (-2.8, "balanced", true, "gpt-5.6-terra", true),
    ] {
        let pool: Vec<_> = models
            .iter()
            .map(|(model, effort)| Candidate {
                model: (*model).into(),
                effort: (*effort).into(),
                pace: if quota && *model == "claude-haiku-5-5" {
                    Some(Pace::default())
                } else {
                    None
                },
                ..Default::default()
            })
            .collect();
        let got = rank_auto(&pool, d, priority, &cat);
        assert_eq!(got[0].model, want);
        assert_eq!(got[0].auto.as_ref().unwrap().qualified, qualified);
    }
}

#[test]
fn rank_auto_and_unknown_median() {
    let cat = Catalog {
        scores: vec![
            BenchScore {
                model: "cheap".into(),
                score: 0.5,
                cost_per_task: Some(0.5),
                ..Default::default()
            },
            BenchScore {
                model: "strong".into(),
                score: 0.8,
                cost_per_task: Some(1.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    for (d, priority, deficit, want, qualified) in [
        (-2.3, "balanced", false, "cheap", true),
        (-2.3, "balanced", true, "strong", true),
        (0.0, "balanced", false, "strong", false),
        (-2.0, "balanced", false, "cheap", true),
        (-2.0, "quality", false, "strong", true),
    ] {
        let cs = [
            Candidate {
                model: "cheap".into(),
                pace: if deficit {
                    Some(Pace {
                        deficit: true,
                        will_last: true,
                        delta: 20.0,
                        ..Default::default()
                    })
                } else {
                    None
                },
                ..Default::default()
            },
            Candidate {
                model: "strong".into(),
                ..Default::default()
            },
        ];
        let got = rank_auto(&cs, d, priority, &cat);
        assert_eq!(got[0].model, want);
        assert_eq!(got[0].auto.as_ref().unwrap().qualified, qualified);
    }
    let got = rank_auto(
        &[
            Candidate {
                model: "mystery".into(),
                ..Default::default()
            },
            Candidate {
                model: "cheap".into(),
                ..Default::default()
            },
        ],
        -2.3,
        "balanced",
        &cat,
    );
    let mystery = got
        .iter()
        .find(|c| c.model == "mystery")
        .unwrap()
        .auto
        .as_ref()
        .unwrap();
    assert_eq!(mystery.effective_cost, Some(0.5));
    assert_eq!(mystery.theta_source, "unknown");
}

#[test]
fn difficulty_boundaries() {
    assert_eq!(tier_from_probs([0.8, 0.0, 0.2]), "hard");
    assert_eq!(tier_from_probs([0.65, 0.35, 0.0]), "normal");
    let d = new_difficulty(Some([0.8, 0.0, 0.2]), "").unwrap();
    assert_eq!(d.tier, "hard");
    assert!((d.d + 1.3).abs() < 1e-9);
    assert!(new_difficulty(Some([0.0; 3]), "").is_err());
}

#[test]
fn tier_build_picks_cheapest_qualified_then_strongest() {
    use dispatch_core::tiers_build::build_tiers;
    let cat = Catalog {
        scores: vec![
            BenchScore {
                model: "cheap".into(),
                score: 0.5,
                cost_per_task: Some(0.5),
                ..Default::default()
            },
            BenchScore {
                model: "strong".into(),
                score: 0.8,
                cost_per_task: Some(1.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let entries = [
        ("a", "cheap"),
        ("b", "strong"),
        ("a", "cheap"),
        ("c", "cheap"),
        ("d", "cheap-fast"),
    ]
    .into_iter()
    .map(|(agent, model)| Entry {
        agent: agent.into(),
        model: model.into(),
        ..Default::default()
    })
    .collect::<Vec<_>>();
    let built = build_tiers(&entries, &cat, "balanced");
    assert_eq!(built.tiers["easy"].len(), 2);
    assert_eq!(built.tiers["easy"][0].entry.model, "cheap");
    assert!(built.tiers["easy"][0].auto.qualified);
    assert_eq!(built.tiers["hard"][0].entry.model, "strong");
    assert!(built.tiers["hard"][0].auto.qualified);
    assert!(!built.tiers["hard"][1].auto.qualified);
}

#[test]
fn a_judged_tier_keeps_its_best_in_the_judge_s_order() {
    use dispatch_core::tiers_build::{build_tiers_n, clear_tier, rank_tier};
    let cat = Catalog {
        scores: ["one", "two", "three", "four"]
            .into_iter()
            .map(|model| BenchScore {
                model: model.into(),
                score: 0.6,
                cost_per_task: Some(1.0),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let entries = ["one", "two", "three", "four"]
        .into_iter()
        .map(|model| Entry {
            agent: "a".into(),
            model: model.into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let mut built = build_tiers_n(&entries, &cat, "balanced", 8);
    assert_eq!(built.tiers["normal"].len(), 4);
    // The judge likes four best and has no word on one, which keeps its place among ties.
    let score = |model: &str| match model {
        "four" => 0.6,
        "two" => 0.3,
        "three" => 0.1,
        _ => 0.0,
    };
    rank_tier(&mut built, "normal", |entry| score(&entry.entry.model), 3);
    let kept: Vec<_> = built.tiers["normal"]
        .iter()
        .map(|entry| entry.entry.model.as_str())
        .collect();
    assert_eq!(kept, ["four", "two", "three"]);
    let stored: Vec<_> = built.table.tiers["normal"]
        .iter()
        .map(|entry| entry.model.as_str())
        .collect();
    assert_eq!(stored, kept);
    clear_tier(&mut built, "easy");
    assert!(!built.tiers.contains_key("easy"));
    assert!(!built.table.tiers.contains_key("easy"));
}
