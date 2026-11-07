//! The runner side of Auto, roles, and the decision log.

#[path = "runner_support.rs"]
mod support;

use dispatch_core::{
    Error,
    config::Policy,
    difficulty::new_difficulty,
    pass_model::difficulty_tier,
    roles::{GENERAL_ROLE, RoleEntry, builtin_profiles},
    route_decision::{POLICY_AUTO_V1, POLICY_TIER_V2},
    router::Filters,
    runner::{
        Classification, Classifier, Options, PRIORITY_BALANCED, RoleAnswer, TIER_AUTO, Task,
        apply_profile,
    },
    types::STATUS_COMPLETED,
};
use futures::future::BoxFuture;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use support::*;
use tokio_util::sync::CancellationToken;

fn easy_classifier() -> Arc<FakeClassifier> {
    FakeClassifier::new(
        "fake",
        Ok(new_difficulty(Some([0.9, 0.08, 0.02]), "").unwrap().into()),
    )
}

type Asked = Arc<Mutex<Option<BTreeMap<String, String>>>>;

/// Answers every task with probs and, when the task has roles, with role;
/// it keeps the roles it was asked.
struct RoleClassifier {
    probs: [f64; 3],
    role: RoleAnswer,
    asked: Asked,
}

impl RoleClassifier {
    fn new(probs: [f64; 3], role: RoleAnswer) -> (Arc<Self>, Asked) {
        let asked = Asked::default();
        let c = Arc::new(Self {
            probs,
            role,
            asked: asked.clone(),
        });
        (c, asked)
    }
}

impl Classifier for RoleClassifier {
    fn name(&self) -> String {
        "fake".into()
    }

    fn classify(&self, task: Task) -> BoxFuture<'_, Result<Classification, Error>> {
        let has_roles = task.roles.as_ref().is_some_and(|r| !r.is_empty());
        *self.asked.lock().unwrap() = task.roles;
        let out = new_difficulty(Some(self.probs), "").map(|d| {
            let mut c = Classification::from(d);
            if has_roles {
                c.role = Some(self.role.clone());
            }
            c
        });
        Box::pin(async move { out })
    }
}

fn probs(pairs: &[(&str, f64)]) -> RoleAnswer {
    RoleAnswer {
        probs: pairs.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
        ..RoleAnswer::default()
    }
}

fn named(role: &str) -> RoleAnswer {
    RoleAnswer {
        role: role.into(),
        ..RoleAnswer::default()
    }
}

fn pick_deps(c: Option<Arc<dyn Classifier>>) -> TestDeps {
    let r = ran();
    let mut t = test_deps(vec![
        Fake::new("a", &r).out("ok").arc(),
        Fake::new("b", &r).out("ok").arc(),
    ]);
    t.deps
        .table
        .tiers
        .insert("normal".into(), vec![entry("a", "normal")]);
    t.deps
        .table
        .tiers
        .insert("hard".into(), vec![entry("a", "hard")]);
    t.deps.policy = Policy {
        discover: false,
        ..Policy::default()
    };
    t.deps.profiles = builtin_profiles();
    t.deps.classifier = c;
    t
}

fn prompt(p: &str, read_only: bool) -> Options {
    Options {
        prompt: p.into(),
        read_only,
        ..Options::default()
    }
}

#[tokio::test]
async fn test_run_records_one_route_decision_without_the_prompt() {
    let r = ran();
    let t = test_deps(vec![
        Fake::new("a", &r).fail("rate limited").arc(),
        Fake::new("b", &r).out("hello\n").arc(),
    ]);
    let secret = "secret-prompt-text fix the footer link";
    let rec = t
        .deps
        .run(
            &CancellationToken::new(),
            Options {
                tier: "easy".into(),
                prompt: secret.into(),
                ..Options::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    let decs = t.store.decisions();
    assert_eq!(decs.len(), 1, "decisions = {decs:?}");
    let dec = &decs[0];
    assert!(
        dec.run_id == rec.id
            && dec.policy_version == POLICY_TIER_V2
            && dec.route == "tier"
            && dec.priority == PRIORITY_BALANCED,
        "decision = {dec:?}"
    );
    assert!(
        dec.candidates.len() == 2
            && dec.candidates[0].agent == "a"
            && dec.candidates[1].agent == "b",
        "candidates = {:?}",
        dec.candidates
    );
    assert!(
        dec.candidates[0].p_pass.is_none() && dec.difficulty.is_none() && dec.classifier.is_empty(),
        "a tier run with no classifier has no estimate: {dec:?}"
    );
    // RouteDecision has no JSON form here; its Debug output shows every field.
    let b = format!("{dec:?}");
    assert!(
        !b.contains("secret-prompt-text"),
        "the decision log holds the prompt"
    );
}

#[tokio::test]
async fn test_run_without_a_tier_is_auto_with_a_classifier() {
    let r = ran();
    let mut t = test_deps(vec![
        Fake::new("a", &r).fail("rate limited").arc(),
        Fake::new("b", &r).out("hello\n").arc(),
    ]);
    t.deps.classifier = Some(easy_classifier());
    let rec = t
        .deps
        .run(
            &CancellationToken::new(),
            prompt("fix the footer link", false),
        )
        .await
        .unwrap();
    assert_eq!(rec.status, STATUS_COMPLETED, "{rec:?}");
    assert!(
        rec.requested_tier == TIER_AUTO && rec.tier == "easy" && rec.note.contains("auto: d="),
        "requested {:?} tier {:?} note {:?}",
        rec.requested_tier,
        rec.tier,
        rec.note
    );
    let decs = t.store.decisions();
    assert!(
        decs.len() == 1
            && decs[0].policy_version == POLICY_AUTO_V1
            && decs[0].route == "auto"
            && decs[0].difficulty.is_some(),
        "decisions {decs:?}"
    );
    let c = &decs[0].candidates[0];
    assert!(
        c.p_pass.is_some() && c.effective_cost.is_some() && c.theta.is_some(),
        "candidate {c:?}"
    );
}

#[tokio::test]
async fn test_route_auto_reports_difficulty_and_prices() {
    let r = ran();
    let mut t = test_deps(vec![Fake::new("a", &r).arc(), Fake::new("b", &r).arc()]);
    t.deps.classifier = Some(easy_classifier());
    let rep = t
        .deps
        .route(prompt("fix the footer link", false))
        .await
        .unwrap();
    assert!(
        rep.difficulty.is_some()
            && rep.classifier == "fake"
            && rep.probs.len() == 3
            && rep.candidates.len() == 2
            && rep.candidates[0].auto.is_some(),
        "{rep:?}"
    );
    assert!(
        rep.candidates[0].reason.contains("P(pass)="),
        "reason {:?}",
        rep.candidates[0].reason
    );
    let refused = t
        .deps
        .route(Options {
            tier: TIER_AUTO.into(),
            filters: Filters {
                model: "m1".into(),
                ..Filters::default()
            },
            ..Options::default()
        })
        .await;
    assert!(refused.is_err(), "auto with a model filter must be refused");
}

#[tokio::test]
async fn test_auto_picks_a_subagent_without_changing_access() {
    // An easy task the classifier calls a review: the reviewer's hard tier is a floor.
    let (c, asked) = RoleClassifier::new(
        [0.9, 0.08, 0.02],
        probs(&[("reviewer", 0.9), (GENERAL_ROLE, 0.1)]),
    );
    let t = pick_deps(Some(c));
    let r = t
        .deps
        .run(&CancellationToken::new(), prompt("review branch x", false))
        .await
        .unwrap();
    assert_eq!(r.status, STATUS_COMPLETED, "{r:?}");
    let asked = asked.lock().unwrap().clone().unwrap_or_default();
    assert!(
        asked.get("reviewer").is_some_and(|s| !s.is_empty())
            && asked.get(GENERAL_ROLE).is_some_and(|s| !s.is_empty()),
        "roles asked {asked:?}"
    );
    assert!(
        r.profile == "reviewer"
            && !r.read_only
            && r.tier == "hard"
            && r.difficulty.is_some_and(|d| difficulty_tier(d) == "hard"),
        "pick applied wrong: profile {:?} readonly {} tier {}",
        r.profile,
        r.read_only,
        r.tier
    );
    let reviewer = &builtin_profiles()["reviewer"];
    assert!(
        r.prompt.starts_with(&reviewer.instructions) && r.timeout_sec == 15 * 60,
        "instructions or timeout missing: {:?} {}",
        r.prompt,
        r.timeout_sec
    );
    let pick = r.role_pick.as_ref().expect("pick record");
    assert!(
        pick.auto && pick.role == "reviewer" && pick.probs.get("reviewer") == Some(&0.9),
        "pick record {pick:?}"
    );
    assert!(
        r.note.contains("role reviewer picked (P=0.90)")
            && r.note.contains("raised to its hard tier"),
        "note {:?}",
        r.note
    );
}

#[tokio::test]
async fn test_auto_stays_general_when_no_role_is_clear() {
    let (c, _) = RoleClassifier::new(
        [0.1, 0.8, 0.1],
        probs(&[("reviewer", 0.4), ("tester", 0.35), (GENERAL_ROLE, 0.25)]),
    );
    let t = pick_deps(Some(c));
    let r = t
        .deps
        .run(
            &CancellationToken::new(),
            prompt("do a mix of things", true),
        )
        .await
        .unwrap();
    assert!(
        r.profile.is_empty()
            && r.prompt == "do a mix of things"
            && r.role_pick.as_ref().is_some_and(|p| p.role == GENERAL_ROLE),
        "{r:?}"
    );
}

#[tokio::test]
async fn test_no_pick_when_the_request_names_one_or_the_policy_is_off() {
    let (c, asked) = RoleClassifier::new([0.1, 0.8, 0.1], named("tester"));
    let mut t = pick_deps(Some(c));
    let cancel = CancellationToken::new();
    let mut o = prompt("x", true);
    apply_profile(
        &mut o,
        "explorer",
        &dispatch_core::roles::Profile {
            instructions: "look".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let r = t.deps.run(&cancel, o).await.unwrap();
    assert!(
        r.profile == "explorer" && r.role_pick.is_none() && asked.lock().unwrap().is_none(),
        "named: {r:?} asked {:?}",
        asked.lock().unwrap()
    );
    let r = t
        .deps
        .run(
            &cancel,
            Options {
                no_profile: true,
                ..prompt("x", true)
            },
        )
        .await
        .unwrap();
    assert!(
        r.profile.is_empty() && r.role_pick.is_none() && asked.lock().unwrap().is_none(),
        "--as none: {r:?}"
    );
    t.deps.policy.roles = false;
    let r = t.deps.run(&cancel, prompt("x", true)).await.unwrap();
    assert!(
        r.profile.is_empty() && asked.lock().unwrap().is_none(),
        "policy off: {r:?}"
    );
}

#[tokio::test]
async fn test_role_board_routes_its_band_in_order_and_falls_back_when_empty() {
    let (c, _) = RoleClassifier::new([0.1, 0.8, 0.1], named("tester"));
    let mut t = pick_deps(Some(c));
    let board = |tier: &str, agent: &str, model: &str| RoleEntry {
        tier: tier.into(),
        agent: agent.into(),
        model: model.into(),
        ..RoleEntry::default()
    };
    let mut p = t.deps.profiles["tester"].clone();
    p.tiers = vec![
        board("normal", "b", "first"),
        board("normal", "a", "second"),
        board("hard", "a", "deep"),
    ];
    t.deps.profiles.insert("tester".into(), p.clone());
    let route = t.deps.route(prompt("add tests", false)).await.unwrap();
    assert!(
        route.role == "tester"
            && route.candidates.len() == 2
            && route
                .winner
                .as_ref()
                .is_some_and(|w| w.agent == "b" && w.model == "first")
            && route.candidates[1].filters.model == "second",
        "route {route:?}"
    );
    let r = t
        .deps
        .run(&CancellationToken::new(), prompt("add tests", false))
        .await
        .unwrap();
    assert!(
        r.profile == "tester" && r.model == "first" && r.tier == "normal",
        "run {r:?}"
    );
    // An easy task: the tester board has no easy models, so the general route takes it.
    let (c, _) = RoleClassifier::new([0.95, 0.04, 0.01], named("tester"));
    t.deps.classifier = Some(c);
    p.tier.clear();
    t.deps.profiles.insert("tester".into(), p);
    let route = t.deps.route(prompt("rename a test", false)).await.unwrap();
    assert!(
        route.role == "tester"
            && route
                .winner
                .as_ref()
                .is_some_and(|w| w.model != "first" && w.model != "deep"),
        "empty band must use the tier table: {route:?}"
    );
}

#[tokio::test]
async fn test_a_role_without_a_column_routes_on_its_nearest_kept_one() {
    let (c, _) = RoleClassifier::new([0.02, 0.18, 0.8], named("tester")); // hard
    let mut t = pick_deps(Some(c));
    let board = |tier: &str, agent: &str, model: &str| RoleEntry {
        tier: tier.into(),
        agent: agent.into(),
        model: model.into(),
        ..RoleEntry::default()
    };
    let mut p = t.deps.profiles["tester"].clone();
    p.tiers = vec![
        board("easy", "a", "small"),
        board("normal", "b", "mid"),
        board("hard", "c", "deep"),
    ];
    p.columns = Some(vec!["easy".into(), "normal".into()]);
    p.tier.clear();
    t.deps.profiles.insert("tester".into(), p.clone());
    let route = t.deps.route(prompt("hard work", false)).await.unwrap();
    assert!(
        route.role == "tester"
            && route
                .winner
                .as_ref()
                .is_some_and(|w| w.agent == "b" && w.model == "mid")
            && route.note.contains("no hard column")
            && route.note.contains("normal"),
        "a deleted hard column routes on normal: {route:?}"
    );

    // An easy task goes the other way: the next harder kept column is normal.
    let (c, _) = RoleClassifier::new([0.9, 0.05, 0.05], named("tester")); // easy
    t.deps.classifier = Some(c);
    let mut p = t.deps.profiles["tester"].clone();
    p.columns = Some(vec!["normal".into(), "hard".into()]);
    t.deps.profiles.insert("tester".into(), p);
    let route = t.deps.route(prompt("rename a test", false)).await.unwrap();
    assert!(
        route.role == "tester"
            && route
                .winner
                .as_ref()
                .is_some_and(|w| w.agent == "b" && w.model == "mid")
            && route.note.contains("no easy column")
            && route.note.contains("normal"),
        "a deleted easy column routes on the next harder column: {route:?}"
    );
}

#[tokio::test]
async fn test_named_role_tier_is_a_floor_not_a_fixed_tier() {
    let (c, _) = RoleClassifier::new([0.1, 0.1, 0.8], RoleAnswer::default());
    let mut t = pick_deps(Some(c));
    let cancel = CancellationToken::new();
    let mut o = prompt("find where X is", true);
    apply_profile(&mut o, "explorer", &t.deps.profiles["explorer"]).unwrap();
    // The explorer's easy floor does not cap a hard task.
    let r = t.deps.run(&cancel, o).await.unwrap();
    assert!(r.tier == "hard" && r.requested_tier == TIER_AUTO, "{r:?}");
    // Without a classifier the floor lifts the default normal tier.
    t.deps.classifier = None;
    let mut o = prompt("review", true);
    apply_profile(&mut o, "reviewer", &t.deps.profiles["reviewer"]).unwrap();
    let r = t.deps.run(&cancel, o).await.unwrap();
    assert_eq!(r.tier, "hard", "no classifier: {r:?}");
}
