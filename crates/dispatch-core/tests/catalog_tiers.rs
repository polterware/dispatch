use dispatch_core::{
    catalog::{BenchScore, Catalog, CatalogModel, EciScore},
    pass_model::{pass_probability, sigmoid},
    router::{self, Candidate},
    tiers::{Entry, Table},
    tiers_build::build_tiers,
};
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}
#[test]
fn test_default_covers_all_tiers() {
    let d = Table::embedded().unwrap();
    for tier in ["easy", "normal", "hard"] {
        assert!(!d.tiers[tier].is_empty());
    }
    let e = &d.tiers["hard"][0];
    assert_eq!(
        (&*e.provider, &*e.agent, &*e.model),
        ("google", "antigravity", "gemini-4-argon-high")
    );
}
#[test]
fn test_legacy_speed_and_strength_are_ignored() {
    let table = Table::parse(
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","speed":9,"strength":-1}]}}"#,
    )
    .unwrap();
    let plain =
        Table::parse(r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small"}]}}"#).unwrap();
    assert_eq!(table, plain);
    assert!(!serde_json::to_string(&table).unwrap().contains("speed"));
}
#[test]
fn test_tier_effort_validation() {
    for body in [
        r#"{"tiers":{"easy":[{"agent":"codex","model":"elastic","effort":"wild"}]}}"#,
        r#"{"tiers":{"easy":[{"agent":"codex","model":"elastic","effort":"high","efforts":["low"]}]}}"#,
        r#"{"tiers":{"easy":[{"agent":"codex","model":"elastic-low","effort":"high"}]}}"#,
    ] {
        assert!(Table::parse(body).is_err());
    }
}
#[test]
fn test_embedded_catalog_resolves_the_default_table() {
    let c = Catalog::embedded().unwrap();
    assert!(!c.models.is_empty() && !c.scores.is_empty() && c.eci_fit.is_some());
    for entries in Table::embedded().unwrap().tiers.values() {
        for e in entries {
            let est = c.estimate(&e.model, "");
            assert_ne!(est.theta_source, "unknown");
            assert!(est.cost_per_task.is_some());
        }
    }
}
#[test]
fn test_estimate_uses_the_level_then_the_nearest_one() {
    let c = Catalog {
        scores: vec![
            BenchScore {
                model: "m".into(),
                effort: "medium".into(),
                score: 0.5,
                cost_per_task: Some(2.),
                ..Default::default()
            },
            BenchScore {
                model: "m".into(),
                effort: "high".into(),
                score: 0.6,
                cost_per_task: Some(3.),
                ..Default::default()
            },
            BenchScore {
                model: "flat".into(),
                score: 0.3,
                ..Default::default()
            },
        ],
        models: vec![CatalogModel {
            id: "priced".into(),
            input: 4.,
            output: 8.,
            ..Default::default()
        }],
        ..Default::default()
    };
    let high = c.estimate("m-high", "");
    assert_eq!(high.extrapolated_levels, 0);
    assert_eq!(high.theta_source, "cursorbench high");
    assert_eq!(high.cost_per_task, Some(3.));
    assert!(near(high.theta, (0.6f64 / 0.4).ln()));
    let xhigh = c.estimate("m", "xhigh");
    assert_eq!(xhigh.extrapolated_levels, 1);
    assert!(xhigh.theta_source.contains("nearest to xhigh"));
    assert!(near(xhigh.theta, (0.6f64 / 0.4).ln() + 0.15));
    assert!(near(xhigh.cost_per_task.unwrap(), 3. * 2.5 / 1.6));
    let f = c.estimate("openai/flat", "low");
    assert_eq!(f.theta_source, "cursorbench");
    assert!(near(f.theta, (0.3f64 / 0.7).ln()));
    assert_eq!(
        c.estimate("m-build-fast", "medium").theta_source,
        "cursorbench medium (as m)"
    );
    let p = c.estimate("priced", "medium");
    assert_eq!(p.theta_source, "unknown");
    assert!(near(p.theta, -0.7));
    assert_eq!(p.cost_source, "models.dev price");
    assert!(near(p.cost_per_task.unwrap(), 0.2));
    let u = c.estimate("nothing", "");
    assert_eq!(u.theta_source, "unknown");
    assert_eq!(u.cost_per_task, None);
}
#[test]
fn test_estimate_shifts_unmeasured_efforts_from_medium() {
    let cat = Catalog {
        eci: vec![EciScore {
            model: "eci-only".into(),
            eci: 100.,
        }],
        eci_fit: Some((0.01, 0.)),
        ..Default::default()
    };
    for (model, source) in [("eci-only", "eci"), ("unknown", "unknown")] {
        let medium = cat.estimate(model, "medium");
        for (effort, levels) in [
            ("low", -1i32),
            ("medium", 0),
            ("high", 1),
            ("xhigh", 2),
            ("max", 3),
        ] {
            let e = cat.estimate(model, effort);
            assert_eq!(e.theta_source, source);
            assert!(near(e.theta, medium.theta + f64::from(levels) * 0.15));
            assert_eq!(e.extrapolated_levels, levels.unsigned_abs() as usize);
        }
    }
}
#[test]
fn test_extrapolated_levels_widen_the_pass_bound() {
    let cat = Catalog {
        scores: vec![
            BenchScore {
                model: "measured".into(),
                effort: "xhigh".into(),
                score: sigmoid(0.3),
                cost_per_task: Some(1.),
                ..Default::default()
            },
            BenchScore {
                model: "extrapolated".into(),
                effort: "medium".into(),
                score: 0.5,
                cost_per_task: Some(1.),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let cs = router::price_auto(
        &[
            Candidate {
                model: "measured".into(),
                effort: "xhigh".into(),
                ..Default::default()
            },
            Candidate {
                model: "extrapolated".into(),
                effort: "xhigh".into(),
                ..Default::default()
            },
        ],
        -1.5,
        "balanced",
        &cat,
    );
    let a = cs[0].auto.as_ref().unwrap();
    let b = cs[1].auto.as_ref().unwrap();
    assert!(near(a.theta, b.theta));
    assert!(b.p_pass_low < a.p_pass_low);
    assert!(near(b.p_pass_low, pass_probability(b.theta, -1.5, 0.5).1));
}
#[test]
fn test_auto_theta_traces_to_the_catalog() {
    let cat = Catalog::embedded().unwrap();
    let cs = [("claude-opus-5-5", "high"), ("gpt-5.6-luna", "medium")].map(|(model, effort)| {
        Candidate {
            model: model.into(),
            effort: effort.into(),
            ..Default::default()
        }
    });
    for c in router::rank_auto(&cs, -1.8, "balanced", &cat) {
        let auto = c.auto.unwrap();
        assert!(auto.theta_source.starts_with("cursorbench"));
        assert!(auto.cost.is_some());
    }
}
#[test]
fn test_antigravity_claude_ids_read_the_claude_benchmarks() {
    let cat = Catalog::embedded().unwrap();
    for (listed, model, effort) in [
        ("claude-sonnet-5-5-high", "claude-sonnet-5-5", "high"),
        ("claude-opus-5-5-low", "claude-opus-5-5", "low"),
    ] {
        let agy = cat.estimate(listed, "");
        let claude = cat.estimate(model, effort);
        assert_eq!(agy.key, claude.key);
        assert!(
            agy.theta_source.starts_with("cursorbench"),
            "{listed}: {}",
            agy.theta_source
        );
        assert!(near(agy.theta, claude.theta), "{listed}");
        assert!(agy.cost_per_task.is_some(), "{listed}: unpriced");
        assert_eq!(agy.cost_per_task, claude.cost_per_task, "{listed}");
    }
}
#[test]
fn test_build_tiers_offers_antigravity_claude_models() {
    let cat = Catalog::embedded().unwrap();
    let entries: Vec<Entry> = ["claude-opus-5-5-high", "claude-sonnet-5-5-medium"]
        .map(|model| Entry {
            agent: "antigravity".into(),
            model: model.into(),
            provider: "anthropic".into(),
            ..Default::default()
        })
        .to_vec();
    let built = build_tiers(&entries, &cat, "balanced");
    for tier in ["easy", "normal", "hard"] {
        let rows = built.tiers.get(tier).cloned().unwrap_or_default();
        assert!(!rows.is_empty(), "{tier}: no antigravity claude model");
        for b in rows {
            assert_eq!(b.entry.agent, "antigravity");
            assert!(
                b.entry.model.starts_with("claude-"),
                "{tier}: {}",
                b.entry.model
            );
            assert!(
                b.auto.theta_source.starts_with("cursorbench"),
                "{tier}: {}",
                b.auto.theta_source
            );
        }
    }
}
