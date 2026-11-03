use dispatch_core::{
    catalog::{BenchScore, Catalog, CatalogModel, CatalogSource, EciScore},
    reports::{
        benchmark_models, report_catalog, with_file_lock, write_file_atomic,
        write_file_atomic_async,
    },
    tiers::{Entry, Table},
};
use nix::fcntl::{Flock, FlockArg};
use std::{collections::BTreeMap, fs::OpenOptions, path::Path};

#[test]
fn test_benchmark_models_reads_the_figures_as_they_are() {
    let cat = Catalog {
        generated: "2026-10-31".into(),
        sources: vec![
            CatalogSource {
                name: "models.dev".into(),
                url: "https://models.dev/api.json".into(),
                ..Default::default()
            },
            CatalogSource {
                name: "Epoch AI, CursorBench".into(),
                url: "https://epoch.ai/benchmarks".into(),
                ..Default::default()
            },
            CatalogSource {
                name: "Epoch AI, Epoch Capabilities Index".into(),
                url: "https://epoch.ai/benchmarks".into(),
                ..Default::default()
            },
        ],
        scores: vec![
            BenchScore {
                model: "gpt-5.6".into(),
                effort: "high".into(),
                score: 0.64,
                date: String::new(),
                ..Default::default()
            },
            BenchScore {
                model: "gpt-5.6".into(),
                effort: "low".into(),
                score: 0.5,
                date: String::new(),
                ..Default::default()
            },
        ],
        eci: vec![
            EciScore {
                model: "gpt-5.6".into(),
                eci: 150.0,
            },
            EciScore {
                model: "grok-4.7".into(),
                eci: 138.0,
            },
        ],
        ..Default::default()
    };

    let r = benchmark_models(
        &cat,
        &[
            Entry {
                agent: "grok".into(),
                model: "grok-4.7-build-fast".into(),
                ..Default::default()
            },
            Entry {
                agent: "codex".into(),
                model: "gpt-5.6".into(),
                provider: "openai".into(),
                ..Default::default()
            },
            Entry {
                agent: "codex".into(),
                model: "gpt-5.6".into(),
                ..Default::default()
            },
            Entry {
                agent: "cursor".into(),
                model: "mystery".into(),
                ..Default::default()
            },
        ],
    );

    assert_eq!(r.sources.len(), 2);
    assert_eq!(r.generated, "2026-10-31");
    assert_eq!(
        r.models.len(),
        3,
        "one row per agent and model: {:?}",
        r.models
    );

    let (gpt, grok, unknown) = (&r.models[0], &r.models[1], &r.models[2]);
    assert_eq!(gpt.model, "gpt-5.6");
    assert_eq!(gpt.cursor_bench.len(), 2);
    assert_eq!(gpt.cursor_bench[0].effort, "low");
    assert_eq!(gpt.eci, Some(150.0));
    assert!(gpt.eci_as.is_empty());

    assert_eq!(grok.model, "grok-4.7-build-fast");
    assert!(grok.cursor_bench.is_empty());
    assert_eq!(grok.eci, Some(138.0));
    assert_eq!(grok.eci_as, "grok-4.7");

    assert_eq!(unknown.model, "mystery");
    assert_eq!(unknown.eci, None);
    assert!(unknown.cursor_bench.is_empty());
}

#[test]
fn test_report_catalog_resolves_table_and_discovered() {
    let cat = Catalog {
        generated: "2026-10-31".into(),
        models: vec![CatalogModel {
            id: "gpt-5.6".into(),
            input: 2.0,
            output: 8.0,
            ..Default::default()
        }],
        scores: vec![BenchScore {
            model: "gpt-5.6".into(),
            effort: "high".into(),
            score: 0.72,
            date: String::new(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let table = Table {
        tiers: BTreeMap::from([(
            "hard".into(),
            vec![Entry {
                agent: "codex".into(),
                model: "gpt-5.6".into(),
                efforts: vec!["low".into(), "medium".into(), "high".into()],
                ..Default::default()
            }],
        )]),
    };
    let discovered = vec![
        Entry {
            agent: "codex".into(),
            model: "gpt-5.6".into(),
            ..Default::default()
        },
        Entry {
            agent: "grok".into(),
            model: "grok-4.7-low".into(),
            ..Default::default()
        },
    ];
    let rep = report_catalog(&cat, &table, &discovered);
    assert_eq!(rep.entries.len(), 2);
    assert_eq!(rep.entries[0].tier, "hard");
    assert_eq!(rep.entries[0].effort, "high");
    assert_eq!(rep.entries[0].bands.len(), 3);
    assert_eq!(rep.entries[1].agent, "grok");
    assert!(rep.entries[1].tier.is_empty());
    assert_eq!(rep.entries[1].effort, "low");

    let json = serde_json::to_value(&rep).expect("serialize report");
    assert!(json["entries"][0].get("matched").is_none());
    assert!(json["entries"][0].get("theta").is_some());
}

#[tokio::test]
async fn test_state_file_atomic_write_and_lock_contention() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("nested").join("state.json");

    // Empty path is a no-op.
    with_file_lock(Path::new(""), || -> Result<(), dispatch_core::Error> {
        panic!("empty path must not run callback");
    })
    .expect("empty path ok");

    with_file_lock(&target, || write_file_atomic(&target, b"{\"v\":1}")).expect("locked write");
    assert_eq!(std::fs::read(&target).expect("read"), b"{\"v\":1}");

    write_file_atomic_async(&target, b"{\"v\":2}")
        .await
        .expect("async atomic write");
    assert_eq!(std::fs::read(&target).expect("read"), b"{\"v\":2}");

    // Contention: hold the sidecar .lock exclusively and verify with_file_lock times out.
    let lock_path = dir.path().join("nested").join("state.json.lock");
    let held_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    let held = Flock::lock(held_file, FlockArg::LockExclusiveNonblock).expect("acquire flock");

    let err = with_file_lock(&target, || Ok(()))
        .expect_err("contended lock must return error")
        .to_string();
    assert!(err.starts_with("state.json lock:"), "unexpected err: {err}");
    drop(held);
}
