use dispatch_core::{
    catalog::{BenchScore, Catalog},
    pass_model::sigmoid,
    router::{self, AVOID_BIAS, Candidate, History, PREFER_BIAS, Pace},
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
