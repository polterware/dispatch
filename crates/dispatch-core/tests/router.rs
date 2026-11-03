use dispatch_core::router::{self, AVOID_BIAS, Candidate, Evidence, History, PREFER_BIAS, Pace};

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
    c.speed = 1;
    c.strength = 5;
    c.evidence = Evidence {
        duration_n: 8,
        p50_ms: Some(1000),
    };
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
        c.speed = 5;
        c.evidence = Evidence {
            duration_n: 8,
            p50_ms: Some(1000),
        };
    }
    assert_eq!(
        names(&router::rank(&cs)),
        ["reserve", "little", "unk", "deficit", "out"]
    );
}
#[test]
fn test_speed_and_quality_diverge() {
    let mut fast = candidate("fast", None);
    fast.speed = 5;
    fast.strength = 1;
    fast.priority = "speed".into();
    let mut strong = candidate("strong", None);
    strong.speed = 1;
    strong.strength = 5;
    strong.priority = "speed".into();
    assert_eq!(
        router::rank(&[strong.clone(), fast.clone()])[0].model,
        "fast"
    );
    fast.priority = "quality".into();
    strong.priority = "quality".into();
    assert_eq!(
        router::rank(&[fast.clone(), strong.clone()])[0].model,
        "strong"
    );
    fast.priority = "balanced".into();
    strong.priority = "balanced".into();
    assert_eq!(names(&router::rank(&[fast, strong])), ["fast", "strong"]);
}
#[test]
fn test_evidence_overrides_prior_only_after_min_samples() {
    for (n, want) in [(4, "strong"), (5, "fast")] {
        let mut a = candidate("fast", None);
        a.priority = "speed".into();
        a.speed = 1;
        a.evidence = Evidence {
            duration_n: n,
            p50_ms: Some(1000),
        };
        let mut b = candidate("strong", None);
        b.priority = "speed".into();
        b.speed = 5;
        b.evidence = Evidence {
            duration_n: n,
            p50_ms: Some(600000),
        };
        assert_eq!(router::rank(&[a, b])[0].model, want);
    }
}
#[test]
fn test_priority_does_not_outrank_cooldown() {
    let mut a = candidate("cooled", Some(pace(-50., false, true)));
    a.priority = "speed".into();
    a.speed = 5;
    a.cooled_down = true;
    a.evidence = Evidence {
        duration_n: 20,
        p50_ms: Some(1000),
    };
    let mut b = candidate("other", Some(pace(80., true, true)));
    b.priority = "speed".into();
    b.speed = 1;
    b.evidence = Evidence {
        duration_n: 20,
        p50_ms: Some(600000),
    };
    assert_eq!(router::rank(&[a, b])[0].model, "other");
}
#[test]
fn test_terms_sum_to_score() {
    let mut a = candidate("", Some(pace(-10., false, true)));
    a.priority = "speed".into();
    a.speed = 4;
    a.bias = PREFER_BIAS;
    a.evidence = Evidence {
        duration_n: 6,
        p50_ms: Some(1000),
    };
    let mut b = candidate("", None);
    b.priority = "quality".into();
    b.strength = 2;
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
