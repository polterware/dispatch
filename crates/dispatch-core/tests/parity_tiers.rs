use dispatch_core::{
    catalog::{BenchScore, Catalog},
    config::Policy,
    pass_model::sigmoid,
    router::{Agent, RouteOptions, select_candidates},
    store::Store as SqliteStore,
    tiers::{
        Entry, Table, default_tiers, known_adapters, load_tiers, load_tiers_from_store,
        table_from_rows, write_default,
    },
    tiers_build::build_tiers,
    types::Store,
};
use std::collections::BTreeMap;

fn auto_catalog() -> Catalog {
    Catalog {
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
    }
}

fn effort_catalog() -> Catalog {
    Catalog {
        scores: vec![
            BenchScore {
                model: "elastic".into(),
                effort: "low".into(),
                score: sigmoid(-1.0),
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
                score: sigmoid(1.0),
                cost_per_task: Some(0.8),
                ..Default::default()
            },
            BenchScore {
                model: "dearer".into(),
                effort: "medium".into(),
                score: sigmoid(2.0),
                cost_per_task: Some(2.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

#[test]
fn test_default_covers_all_tiers() {
    let d = default_tiers().expect("default_tiers");
    for tier in ["easy", "normal", "hard"] {
        assert!(
            d.tiers.get(tier).is_some_and(|es| !es.is_empty()),
            "tier {tier} empty"
        );
    }
    let first_hard = &d.tiers["hard"][0];
    assert_eq!(first_hard.provider, "google");
    assert_eq!(first_hard.agent, "antigravity");
    assert_eq!(first_hard.model, "gemini-4-argon-high");
    assert_eq!(first_hard.speed, 3);
    assert_eq!(first_hard.strength, 5);
}

#[test]
fn test_load_missing_and_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    load_tiers(&dir.path().join("none.json")).expect("missing loads default");

    let p = dir.path().join("bad.json");
    std::fs::write(&p, r#"{"tiers":{"wild":[{"agent":"a","model":"m"}]}}"#).expect("write bad");
    assert!(
        load_tiers(&p).is_err(),
        "expected error for unknown tier 'wild'"
    );
}

#[test]
fn test_provider_efforts_compatibility() {
    let mut table = Table::parse(
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","efforts":["low","high"]}]}}"#,
    )
    .expect("parse valid");
    assert_eq!(table.tiers["easy"][0].provider, "openai");

    for body in [
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","efforts":["wild"]}]}}"#,
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","provider":"google"}]}}"#,
    ] {
        assert!(
            Table::parse(body).is_err(),
            "accepted invalid metadata: {body}"
        );
    }

    table.tiers.get_mut("easy").expect("easy")[0].agent = "grok".into();
    assert!(
        !table.capability_problems(&known_adapters()).is_empty(),
        "effort accepted for CLI without control"
    );
}

#[test]
fn test_speed_and_strength() {
    let table = Table::parse(
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","speed":5,"strength":1}]}}"#,
    )
    .expect("parse speed and strength");
    assert_eq!(table.tiers["easy"][0].speed, 5);
    assert_eq!(table.tiers["easy"][0].strength, 1);
    assert!(table.field_problems().is_empty());

    for body in [
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","speed":6}]}}"#,
        r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small","strength":-1}]}}"#,
    ] {
        let err = Table::parse(body)
            .expect_err("expected out of range error")
            .to_string();
        assert!(err.contains("out of range"), "accepted {body}: {err}");
    }

    let omitted =
        Table::parse(r#"{"tiers":{"easy":[{"agent":"codex","model":"gpt-small"}]}}"#).expect("ok");
    assert_eq!(omitted.tiers["easy"][0].speed, 0);
    assert_eq!(omitted.tiers["easy"][0].strength, 0);

    let hand = Table {
        tiers: BTreeMap::from([(
            "hard".into(),
            vec![Entry {
                agent: "a".into(),
                model: "m".into(),
                speed: 9,
                strength: 0,
                ..Default::default()
            }],
        )]),
    };
    assert_eq!(hand.field_problems().len(), 1);
}

#[test]
fn test_build_tiers_puts_the_cheap_model_on_easy_and_the_strong_one_on_hard() {
    let entries = vec![
        Entry {
            agent: "a".into(),
            model: "cheap".into(),
            ..Default::default()
        },
        Entry {
            agent: "b".into(),
            model: "strong".into(),
            ..Default::default()
        },
        Entry {
            agent: "a".into(),
            model: "cheap".into(),
            ..Default::default()
        },
        Entry {
            agent: "c".into(),
            model: "cheap".into(),
            ..Default::default()
        },
        Entry {
            agent: "d".into(),
            model: "cheap-fast".into(),
            ..Default::default()
        },
    ];
    let got = build_tiers(&entries, &auto_catalog(), "balanced");
    let easy = &got.tiers["easy"];
    let hard = &got.tiers["hard"];
    assert_eq!(easy.len(), 2, "easy {easy:?}");
    assert_eq!(easy[0].entry.model, "cheap");
    assert!(easy[0].auto.qualified);

    assert_eq!(hard[0].entry.model, "strong");
    assert!(hard[0].auto.qualified);
    assert!(!hard[1].auto.qualified);

    assert_eq!(
        got.table.tiers["normal"].len(),
        2,
        "one entry per base model: {:?}",
        got.table.tiers["normal"]
    );
    got.table.clone().validated().expect("built table valid");
}

#[test]
fn test_build_tiers_selects_and_stores_each_tiers_best_effort() {
    let got = build_tiers(
        &[Entry {
            agent: "codex".into(),
            model: "elastic".into(),
            efforts: vec!["low".into(), "medium".into(), "high".into()],
            ..Default::default()
        }],
        &effort_catalog(),
        "balanced",
    );
    for (tier, want) in [("easy", "low"), ("normal", "medium"), ("hard", "high")] {
        let built = &got.tiers[tier];
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].entry.effort, want);
        assert_eq!(got.table.tiers[tier][0].effort, want);
    }

    let data = serde_json::to_string(&got.table).expect("serialize table");
    let parsed = Table::parse(&data).expect("parse table");

    // Verify both table_from_rows and SQLite store persistence round-trip.
    let s = SqliteStore::new(":memory:").expect("sqlite store");
    s.replace_tiers(&parsed.rows()).expect("replace_tiers");
    let mut table = load_tiers_from_store(Some(&s)).expect("load_tiers_from_store");
    assert_eq!(table, table_from_rows(&parsed.rows()));

    let agents = vec![Agent {
        name: "codex".into(),
        installed: true,
        read_only: true,
        file_editing: true,
        effort_mode: "config".into(),
        efforts: vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()],
        ..Default::default()
    }];
    let policy = Policy {
        discover: false,
        ..Default::default()
    };

    for (tier, want) in [("easy", "low"), ("normal", "medium"), ("hard", "high")] {
        let (cs, rejected) = select_candidates(
            &table,
            &policy,
            &agents,
            &RouteOptions {
                tier: tier.into(),
                ..Default::default()
            },
        );
        assert_eq!(
            cs.len(),
            1,
            "{tier} restored: {cs:?}, rejected {rejected:?}"
        );
        assert_eq!(cs[0].model, "elastic");
        assert_eq!(cs[0].effort, want);
    }

    // A stored choice overrides the tier default, including after a board move.
    let hard_rows = table.tiers["hard"].clone();
    table.tiers.insert("easy".into(), hard_rows);
    let (cs, _) = select_candidates(
        &table,
        &policy,
        &agents,
        &RouteOptions {
            tier: "easy".into(),
            ..Default::default()
        },
    );
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].effort, "high", "stored effort lost: {cs:?}");
}

#[test]
fn test_build_tiers_preserves_suffix_and_uses_agent_efforts() {
    let got = build_tiers(
        &[Entry {
            agent: "codex".into(),
            model: "elastic-high".into(),
            efforts: vec!["low".into(), "high".into()],
            ..Default::default()
        }],
        &effort_catalog(),
        "balanced",
    );
    for (tier, entries) in &got.tiers {
        assert_eq!(entries.len(), 1, "{tier}: {entries:?}");
        assert_eq!(entries[0].entry.model, "elastic-high");
        assert_eq!(entries[0].entry.effort, "high");
    }

    let got = build_tiers(
        &[Entry {
            agent: "codex".into(),
            model: "elastic".into(),
            ..Default::default()
        }],
        &effort_catalog(),
        "balanced",
    );
    assert_eq!(got.tiers["easy"][0].entry.effort, "low");
    assert_eq!(got.tiers["hard"][0].entry.effort, "high");
}

#[test]
fn test_tier_effort_validation() {
    for body in [
        r#"{"tiers":{"easy":[{"agent":"codex","model":"elastic","effort":"wild"}]}}"#,
        r#"{"tiers":{"easy":[{"agent":"codex","model":"elastic","effort":"high","efforts":["low"]}]}}"#,
        r#"{"tiers":{"easy":[{"agent":"codex","model":"elastic-low","effort":"high"}]}}"#,
    ] {
        assert!(Table::parse(body).is_err(), "accepted {body}");
    }
}

#[test]
fn test_write_default_refuses_existing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tiers.json");
    write_default(&path).expect("write_default");
    let loaded = load_tiers(&path).expect("load written default");
    assert_eq!(loaded, default_tiers().expect("default_tiers"));

    let err = write_default(&path)
        .expect_err("must refuse existing file")
        .to_string();
    assert!(err.contains("already exists"), "unexpected error: {err}");
}
