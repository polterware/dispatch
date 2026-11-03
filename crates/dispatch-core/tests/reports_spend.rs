use dispatch_core::{
    adapters::usage::Usage as AdapterUsage,
    catalog::{Catalog, CatalogModel, PriceTier},
    reports::{
        COST_AGENT, COST_CATALOG, COST_MIXED, CatalogSpendExt, RunSpendExt, StatsOptions, UsageExt,
        compute,
    },
    types::{Run, STATUS_COMPLETED, STATUS_FAILED, Usage},
};

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn test_usage_cost_and_run_totals() {
    let cat = Catalog {
        models: vec![
            CatalogModel {
                id: "gpt-x".into(),
                input: 2.0,
                output: 10.0,
                cache_read: Some(0.2),
                ..Default::default()
            },
            CatalogModel {
                id: "nocache".into(),
                input: 1.0,
                output: 4.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // 1M fresh input, 1M cache reads, 1M output: 2 + 0.2 + 10.
    let (cost, source) = cat
        .usage_cost(
            "openai/gpt-x-high",
            &AdapterUsage {
                input_tokens: 1_000_000,
                cache_read_tokens: 1_000_000,
                output_tokens: 1_000_000,
                ..Default::default()
            },
        )
        .expect("catalog cost must match");
    assert_eq!(source, COST_CATALOG);
    assert!(near(cost, 12.2), "catalog cost = {cost} {source:?}");

    // No cache price: cache tokens pay the input price.
    let (cost, _) = cat
        .usage_cost(
            "nocache",
            &AdapterUsage {
                cache_read_tokens: 1_000_000,
                cache_write_tokens: 1_000_000,
                ..Default::default()
            },
        )
        .expect("nocache cost");
    assert!(near(cost, 2.0), "no cache price = {cost}");

    let (cost, source) = cat
        .usage_cost(
            "unknown-model",
            &AdapterUsage {
                input_tokens: 5,
                cost_usd: Some(0.5),
                ..Default::default()
            },
        )
        .expect("agent cost");
    assert_eq!(source, COST_AGENT);
    assert_eq!(cost, 0.5);

    assert!(
        cat.usage_cost(
            "unknown-model",
            &AdapterUsage {
                input_tokens: 5,
                ..Default::default()
            },
        )
        .is_none(),
        "an unknown model with no reported price must stay unpriced"
    );

    let mut r = Run::default();
    r.add_usage("gpt-x", None, &cat);
    assert!(
        r.usage.is_none(),
        "no reported usage leaves the run without usage"
    );

    r.add_usage(
        "gpt-x",
        Some(&AdapterUsage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            ..Default::default()
        }),
        &cat,
    );
    r.add_usage(
        "other",
        Some(&AdapterUsage {
            input_tokens: 10,
            cost_usd: Some(1.0),
            ..Default::default()
        }),
        &cat,
    );
    let u = r.usage.as_ref().expect("usage set");
    assert_eq!(u.input_tokens, 1_000_010);
    assert!(
        u.cost_usd.is_some_and(|c| near(c, 4.0)),
        "two priced attempts = {u:?}"
    );
    assert_eq!(u.cost_source, COST_MIXED);

    r.add_usage(
        "unknown-model",
        Some(&AdapterUsage {
            output_tokens: 7,
            ..Default::default()
        }),
        &cat,
    );
    let u = r.usage.as_ref().expect("usage retained");
    assert!(
        u.cost_usd.is_none(),
        "an unpriced attempt must leave the run's cost unknown: {u:?}"
    );
    assert!(u.cost_source.is_empty());
    assert_eq!(u.output_tokens, 100_007);
    assert_eq!(u.total_tokens(), 1_000_010 + 100_007);
}

#[test]
fn test_stats_sum_spend() {
    let recs = vec![
        Run {
            model: "m".into(),
            status: STATUS_COMPLETED.into(),
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 2,
                cost_usd: Some(0.5),
                ..Default::default()
            }),
            ..Default::default()
        },
        Run {
            model: "m".into(),
            status: STATUS_FAILED.into(),
            usage: Some(Usage {
                input_tokens: 5,
                cache_read_tokens: 100,
                ..Default::default()
            }),
            ..Default::default()
        },
        Run {
            model: "m".into(),
            status: STATUS_COMPLETED.into(),
            ..Default::default()
        },
    ];
    let rep = compute(
        &recs,
        StatsOptions {
            by: "model".into(),
            ..Default::default()
        },
    )
    .expect("compute stats");
    assert_eq!(rep.rows.len(), 1);
    let s = &rep.rows[0].spend;
    assert_eq!(s.usage_runs, 2);
    assert_eq!(s.input_tokens, 15);
    assert_eq!(s.cache_read_tokens, 100);
    assert_eq!(s.output_tokens, 2);
    assert_eq!(s.cost_runs, 1);
    assert_eq!(s.cost_usd, 0.5);
}

#[test]
fn test_usage_cost_tiers_and_context() {
    let cat = Catalog {
        models: vec![CatalogModel {
            id: "gpt-y".into(),
            input: 2.0,
            output: 10.0,
            cache_read: Some(0.2),
            context: 400_000,
            tiers: vec![PriceTier {
                size: 272_000,
                input: 4.0,
                output: 15.0,
                cache_read: Some(0.4),
                cache_write: None,
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let mut u = AdapterUsage {
        input_tokens: 1_000_000,
        cache_read_tokens: 1_000_000,
        output_tokens: 1_000_000,
        ..Default::default()
    };
    let (cost, _) = cat.usage_cost("gpt-y", &u).expect("under tier");
    assert!(near(cost, 12.2), "under the tier = {cost}");

    u.context_tokens = 300_000;
    let (cost, _) = cat.usage_cost("gpt-y", &u).expect("past tier");
    assert!(
        near(cost, 19.4),
        "past the tier's size the attempt pays the tier = {cost}"
    );

    let mut r = Run::default();
    r.add_usage(
        "gpt-y",
        Some(&AdapterUsage {
            input_tokens: 10,
            context_tokens: 50_000,
            ..Default::default()
        }),
        &cat,
    );
    let ru = r.usage.as_ref().expect("usage");
    assert_eq!(ru.context_tokens, 50_000);
    assert_eq!(ru.context_window, 400_000);

    r.add_usage(
        "other",
        Some(&AdapterUsage {
            input_tokens: 10,
            context_tokens: 9_000,
            context_window: 128_000,
            ..Default::default()
        }),
        &cat,
    );
    r.add_usage(
        "gpt-y",
        Some(&AdapterUsage {
            input_tokens: 10,
            ..Default::default()
        }),
        &cat,
    );
    let ru = r.usage.as_ref().expect("usage");
    assert_eq!(ru.context_tokens, 9_000);
    assert_eq!(ru.context_window, 128_000);
}
