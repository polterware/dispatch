use axum::{Router, http::StatusCode, routing::get};
use chrono::{TimeZone, Utc};
use dispatch_core::{
    catalog::{BenchScore, Catalog, CatalogModel, EciScore, load_catalog, save_refreshed_prices},
    reports::{CatalogReport, refresh_catalog_from},
    tiers::default_tiers,
};
use std::path::Path;

const EFFORT_THETA_STEP: f64 = 0.15;
const UNKNOWN_THETA: f64 = -0.7;
const STRENGTH_THETA_STEP: f64 = 0.35;
const DEFAULT_TOKENS_PER_TASK: f64 = 40_000.0;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn logit(p: f64) -> f64 {
    let p = p.clamp(0.0001, 0.9999);
    (p / (1.0 - p)).ln()
}

#[test]
fn test_embedded_catalog_resolves_the_default_table() {
    let c = load_catalog(Path::new("")).expect("load_catalog");
    assert!(!c.models.is_empty());
    assert!(!c.scores.is_empty());
    assert!(c.eci_fit.is_some());

    let table = default_tiers().expect("default_tiers");
    for entries in table.tiers.values() {
        for e in entries {
            let est = c.estimate(&e.model, "", e.strength);
            assert_ne!(
                est.theta_source, "unknown",
                "{}/{}: {est:?}",
                e.agent, e.model
            );
            assert!(
                est.cost_per_task.is_some(),
                "{}/{}: {est:?}",
                e.agent,
                e.model
            );
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
                cost_per_task: Some(2.0),
                ..Default::default()
            },
            BenchScore {
                model: "m".into(),
                effort: "high".into(),
                score: 0.6,
                cost_per_task: Some(3.0),
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
            input: 4.0,
            output: 8.0,
            ..Default::default()
        }],
        ..Default::default()
    };

    let high = c.estimate("m-high", "", 0);
    assert_eq!(high.extrapolated_levels, 0);
    assert_eq!(high.theta_source, "cursorbench high");
    assert_eq!(high.cost_per_task, Some(3.0));
    assert!(near(high.theta, logit(0.6)));

    let xhigh = c.estimate("m", "xhigh", 0);
    assert_eq!(xhigh.extrapolated_levels, 1);
    assert!(xhigh.theta_source.contains("nearest to xhigh"));
    assert!(near(xhigh.theta, logit(0.6) + EFFORT_THETA_STEP));
    assert!(
        xhigh
            .cost_per_task
            .is_some_and(|cost| near(cost, 3.0 * 2.5 / 1.6))
    );

    let f = c.estimate("openai/flat", "low", 0);
    assert_eq!(f.theta_source, "cursorbench");
    assert!(near(f.theta, logit(0.3)));

    let s = c.estimate("m-build-fast", "medium", 0);
    assert_eq!(s.theta_source, "cursorbench medium (as m)");

    let s = c.estimate("priced", "medium", 5);
    assert_eq!(s.theta_source, "tiers.json strength");
    assert!(near(s.theta, UNKNOWN_THETA + 2.0 * STRENGTH_THETA_STEP));
    assert_eq!(s.cost_source, "models.dev price");
    assert!(s.cost_per_task.is_some_and(|cost| near(
        cost,
        (3.0 * 4.0 + 8.0) / 4.0 * DEFAULT_TOKENS_PER_TASK / 1e6
    )));

    let u = c.estimate("nothing", "", 0);
    assert_eq!(u.theta_source, "unknown");
    assert_eq!(u.cost_per_task, None);
}

#[test]
fn test_refreshed_prices_override_the_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = br#"{"openai":{"models":{"gpt-x":{"name":"GPT X","tool_call":true,"modalities":{"output":["text"]},"cost":{"input":1,"output":2}},
        "img":{"tool_call":true,"modalities":{"output":["image"]},"cost":{"input":1,"output":2}}}},
        "someaggregator":{"models":{"gpt-y":{"tool_call":true,"modalities":{"output":["text"]},"cost":{"input":1,"output":2}}}}}"#;

    let now = Utc.with_ymd_and_hms(2026, 10, 31, 0, 0, 0).unwrap();
    let n = save_refreshed_prices(dir.path(), raw, now).expect("save_refreshed_prices");
    assert_eq!(n, 1);

    let c = load_catalog(dir.path()).expect("load_catalog");
    assert_eq!(c.models.len(), 1);
    assert_eq!(c.models[0].id, "gpt-x");
    assert!(!c.prices_fetched.is_empty());

    std::fs::write(dir.path().join("models.json"), b"{broken").expect("write broken");
    let fallback = load_catalog(dir.path()).expect("load fallback");
    assert!(
        fallback.models.len() >= 10 && fallback.prices_fetched.is_empty(),
        "a broken refresh must keep the snapshot"
    );
}

#[test]
fn test_estimate_shifts_unmeasured_efforts_from_medium() {
    let cat = Catalog {
        eci: vec![EciScore {
            model: "eci-only".into(),
            eci: 100.0,
        }],
        eci_fit: Some((0.01, 0.0)),
        ..Default::default()
    };

    for (model, strength, source) in [
        ("eci-only", 0, "eci"),
        ("prior", 5, "tiers.json strength"),
        ("unknown", 0, "unknown"),
    ] {
        let medium = cat.estimate(model, "medium", strength);
        for (effort, levels) in [
            ("low", -1i32),
            ("medium", 0),
            ("high", 1),
            ("xhigh", 2),
            ("max", 3),
        ] {
            let est = cat.estimate(model, effort, strength);
            assert_eq!(est.theta_source, source, "{source}/{effort}: {est:?}");
            assert!(
                near(
                    est.theta,
                    medium.theta + f64::from(levels) * EFFORT_THETA_STEP
                ),
                "{source}/{effort}: {est:?}, medium {medium:?}"
            );
            assert_eq!(
                est.extrapolated_levels,
                levels.unsigned_abs() as usize,
                "{source}/{effort}: {est:?}"
            );
        }
    }
}

#[test]
fn test_catalog_report_keeps_discovery_opt_out() {
    let body = serde_json::to_string(&CatalogReport {
        discover: false,
        ..Default::default()
    })
    .expect("serialize");
    assert!(
        body.contains(r#""discover":false"#),
        "discovery opt-out: {body}"
    );
}

#[tokio::test]
async fn test_refresh_catalog_from_fake_http_server() {
    let payload = r#"{
        "openai": {
            "models": {
                "gpt-fake-1": {
                    "name": "GPT Fake 1",
                    "tool_call": true,
                    "reasoning": true,
                    "modalities": { "output": ["text"] },
                    "cost": {
                        "input": 1.5,
                        "output": 6.0,
                        "cache_read": 0.15,
                        "tiers": [
                            {
                                "input": 3.0,
                                "output": 12.0,
                                "tier": { "type": "context", "size": 200000 }
                            }
                        ]
                    },
                    "limit": { "context": 400000, "output": 32000 }
                }
            }
        },
        "google": {
            "models": {
                "gemini-fake-pro": {
                    "name": "Gemini Fake Pro",
                    "tool_call": true,
                    "modalities": { "output": ["text"] },
                    "cost": { "input": 1.25, "output": 5.0 },
                    "limit": { "context": 1000000, "output": 64000 }
                }
            }
        }
    }"#;

    let app = Router::new()
        .route("/api.json", get(move || async move { payload }))
        .route(
            "/down",
            get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "unavailable") }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local fake server");
    let addr = listener.local_addr().expect("local_addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();

    // Non-200 status returns exact "<url>: 503 Service Unavailable" error.
    let down_url = format!("http://{addr}/down");
    let err = refresh_catalog_from(&client, &down_url, dir.path(), now)
        .await
        .expect_err("503 must error")
        .to_string();
    assert_eq!(err, format!("{down_url}: 503 Service Unavailable"));

    // Empty state_dir returns exact "no dispatch state directory" error.
    let empty_err = refresh_catalog_from(&client, &down_url, Path::new(""), now)
        .await
        .expect_err("empty dir must error")
        .to_string();
    assert_eq!(empty_err, "no dispatch state directory");

    // 200 OK refreshes models.json and overrides the embedded snapshot prices.
    let ok_url = format!("http://{addr}/api.json");
    let count = refresh_catalog_from(&client, &ok_url, dir.path(), now)
        .await
        .expect("refresh_catalog_from");
    assert_eq!(count, 2);

    let cat = load_catalog(dir.path()).expect("load_catalog");
    assert_eq!(cat.models.len(), 2);
    assert_eq!(cat.prices_fetched, "2026-11-02T12:00:00Z");
    assert_eq!(cat.models[0].id, "gemini-fake-pro");
    assert_eq!(cat.models[1].id, "gpt-fake-1");
    assert_eq!(cat.models[1].tiers.len(), 1);
    assert_eq!(cat.models[1].tiers[0].size, 200_000);

    server.abort();
}
