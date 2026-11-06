#[path = "runner_support.rs"]
mod support;

use dispatch_core::{
    difficulty::{new_difficulty, tier_from_probs},
    pass_model::difficulty_tier,
    route_decision::POLICY_AUTO_V1,
    router,
    runner::{Candidate, Classification, Options, is_auto_off, tier_warning},
    types::{Run, STATUS_COMPLETED, Store},
};
use support::*;
use tokio_util::sync::CancellationToken;

fn classified(probs: [f64; 3]) -> Result<Classification, String> {
    Ok(new_difficulty(Some(probs), "").unwrap().into())
}

fn with_normal(t: &mut TestDeps) {
    t.deps
        .table
        .tiers
        .insert("normal".into(), vec![entry("a", "normal")]);
}

#[test]
fn test_tier_from_probs_keeps_a_hard_tail() {
    for (p, want) in [
        ([0.95, 0.04, 0.01], "easy"),
        ([0.6, 0.3, 0.1], "normal"),
        ([0.7, 0.1, 0.2], "hard"),
        ([0.1, 0.2, 0.7], "hard"),
    ] {
        assert_eq!(tier_from_probs(p), want, "{p:?}");
        let diff = new_difficulty(Some(p), "").unwrap();
        assert!(
            diff.tier == want && difficulty_tier(diff.d) == want,
            "{p:?}: {diff:?}"
        );
    }
    assert!(new_difficulty(Some([0.0, 0.0, 0.0]), "").is_err());
    let diff = new_difficulty(None, "hard").unwrap();
    assert!(
        diff.probs.is_none() && difficulty_tier(diff.d) == "hard",
        "{diff:?}"
    );
}

#[tokio::test]
async fn test_no_classifier_means_no_auto() {
    let r = ran();
    let mut t = test_deps(vec![Fake::new("a", &r).out("ok").arc()]);
    with_normal(&mut t);
    let cancel = CancellationToken::new();
    let rec = t
        .deps
        .run(
            &cancel,
            Options {
                prompt: "fix the footer link".into(),
                read_only: true,
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert!(
        rec.status == STATUS_COMPLETED
            && rec.tier == "normal"
            && rec.requested_tier == "normal"
            && rec.model == "normal"
            && rec.note.contains("auto is off"),
        "default route: {rec:?}"
    );
    let err = t
        .deps
        .run(
            &cancel,
            Options {
                tier: "auto".into(),
                prompt: "x".into(),
                read_only: true,
                ..Options::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        is_auto_off(&err),
        "explicit auto without a classifier: {err}"
    );
    let route = t
        .deps
        .route(Options {
            prompt: "fix".into(),
            ..Options::default()
        })
        .await
        .unwrap();
    assert!(
        route.difficulty.is_none() && route.winner.as_ref().is_some_and(|w| w.model == "normal"),
        "route: {route:?}"
    );
}

#[tokio::test]
async fn test_classifier_failure_falls_back_to_the_normal_tier() {
    let r = ran();
    let mut t = test_deps(vec![Fake::new("a", &r).out("ok").arc()]);
    with_normal(&mut t);
    t.deps.classifier = Some(FakeClassifier::new("fake", Err("HTTP 503".into())));
    let rec = t
        .deps
        .run(
            &CancellationToken::new(),
            Options {
                prompt: "fix".into(),
                read_only: true,
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert!(
        rec.status == STATUS_COMPLETED
            && rec.tier == "normal"
            && rec.difficulty.is_none()
            && rec.note.contains("classifier fake failed (HTTP 503)"),
        "{rec:?}"
    );
}

#[tokio::test]
async fn test_auto_records_the_classification_and_resume_keeps_it() {
    let r = ran();
    let mut t = test_deps(vec![Fake::new("a", &r).out("ok").arc()]);
    let c = FakeClassifier::new("fake", classified([0.1, 0.2, 0.7]));
    t.deps.classifier = Some(c.clone());
    let cancel = CancellationToken::new();
    let rec = t
        .deps
        .run(
            &cancel,
            Options {
                prompt: "redesign the store".into(),
                card_text: "## card: Store\nintent".into(),
                read_only: true,
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert!(
        rec.requested_tier == "auto"
            && rec.tier == "hard"
            && rec.classifier == "fake"
            && rec.difficulty.is_some()
            && rec.difficulty_probs.len() == 3,
        "{rec:?}"
    );
    let asked: Vec<String> = c.tasks().into_iter().map(|t| t.text).collect();
    assert_eq!(
        asked,
        vec!["## card: Store\nintent\n\nRequest:\nredesign the store"]
    );
    let dec = t.store.get_route_decision(&rec.id).unwrap().unwrap();
    assert!(
        dec.classifier == "fake"
            && dec.probs.len() == 3
            && dec.difficulty.is_some()
            && dec.policy_version == POLICY_AUTO_V1,
        "decision {dec:?}"
    );
    let resume = t
        .deps
        .run(
            &cancel,
            Options {
                resume: rec.id.clone(),
                prompt: "again".into(),
                read_only: true,
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert!(
        resume.tier == rec.tier && c.tasks().len() == 1,
        "resume: {resume:?}"
    );
}

#[test]
fn test_tier_route_warns_when_the_catalog_expects_a_failure() {
    let r = ran();
    let t = test_deps(vec![]);
    // m1 is in no catalog: θ −0.7 with a 0.8 margin, so its bound at d −2.5
    // is σ(1.0) ≈ 0.73, under the balanced bar.
    let cand = |adapter: bool| Candidate {
        adapter: adapter.then(|| Fake::new("a", &r).arc()),
        route: router::Candidate {
            agent: "a".into(),
            model: "m1".into(),
            ..router::Candidate::default()
        },
        ..Candidate::default()
    };
    let rec = Run {
        difficulty: Some(-2.5),
        ..Run::default()
    };
    let cs = t.deps.price_tier_route(&rec, vec![cand(true)]);
    let note = tier_warning(&cs);
    assert!(
        note.contains("auto: m1 P(pass)=") && note.contains("below the bar 0.80"),
        "advisory {note:?}"
    );
    let got = t.deps.price_tier_route(&Run::default(), vec![cand(false)]);
    assert!(
        got[0].route.auto.is_none(),
        "a run with no difficulty must stay unpriced"
    );
}

#[tokio::test]
async fn cli_classifier_skips_disabled_models_and_refuses_an_explicit_one() {
    use dispatch_core::{classifier, runner::Classifier};
    let ran = ran();
    let mut t = test_deps(vec![Fake::new("a", &ran).out("easy").arc()]);
    t.deps.table.tiers.insert(
        "easy".into(),
        vec![entry("a", "blocked"), entry("a", "enabled")],
    );
    t.store.set_model_enabled("a", "blocked", false).unwrap();
    assert_eq!(classifier::cli_for(&t.deps).model, "enabled");
    t.deps.policy.classifier_agent = "a/blocked".into();
    let error = classifier::cli_for(&t.deps)
        .classify(dispatch_core::runner::Task {
            text: "hello".into(),
            roles: None,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("disabled"), "{error}");
    assert!(ran_list(&ran).is_empty());
}
