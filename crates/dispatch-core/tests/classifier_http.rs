//! The Jev and OpenAI Decisions classifiers against a local fake of each API: what they send
//! and how they read the answer. Nothing here leaves the machine.
//!
//! The `live_` tests ask the real APIs, which costs a call and needs a key, so they are
//! ignored by default. Run them only while working on the classifier:
//!
//! ```sh
//! TYPESAFE_API_KEY=… cargo test -p dispatch-core --test classifier_http live_jev -- --ignored
//! OPENAI_API_KEY=… cargo test -p dispatch-core --test classifier_http live_openai -- --ignored
//! ```

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::{Json, Router, http::HeaderMap, routing::post};
use dispatch_core::Error;
use dispatch_core::classifier::{Jev, JevChoice, OpenAi, SecretStore, set_secret_store};
use dispatch_core::runner::{Classifier, Task};
use serde_json::{Value, json};

/// What the fake saw: each request's bearer key and body.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// A Keychain whose `jev` and `openai` items hold `test-key`.
struct FakeKeychain;

impl SecretStore for FakeKeychain {
    fn has(&self, account: &str) -> Result<bool, Error> {
        Ok(matches!(account, "jev" | "openai"))
    }
    fn get(&self, account: &str) -> Result<String, Error> {
        Ok(if self.has(account)? {
            "test-key".into()
        } else {
            String::new()
        })
    }
    fn set(&self, _: &str, _: &str) -> Result<(), Error> {
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), Error> {
        Ok(())
    }
    fn legacy(&self, _: &str) -> Result<String, Error> {
        Ok(String::new())
    }
    fn drop_legacy(&self, _: &str) -> Result<(), Error> {
        Ok(())
    }
}

/// Serves `answer` to every POST on a free local port, and records what it was sent.
async fn fake_api(answer: Value) -> (String, Seen) {
    set_secret_store(Arc::new(FakeKeychain));
    let seen: Seen = Arc::default();
    let record = seen.clone();
    let app = Router::new().route(
        "/",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let record = record.clone();
            let answer = answer.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                record.lock().unwrap().push((auth, body));
                Json(answer)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

/// The key the classifier sends: an environment variable wins over the Keychain.
fn expected_auth(env: &str) -> String {
    match std::env::var(env) {
        Ok(value) if !value.trim().is_empty() => format!("Bearer {}", value.trim()),
        _ => "Bearer test-key".into(),
    }
}

fn roles() -> BTreeMap<String, String> {
    [
        ("reviewer", "Review a diff for bugs."),
        ("general", "Anything else."),
    ]
    .into_iter()
    .map(|(name, when)| (name.to_string(), when.to_string()))
    .collect()
}

#[tokio::test]
async fn jev_reads_the_difficulty_and_the_role_in_one_request() {
    let (url, seen) = fake_api(json!({"answers": {
        "difficulty": {"probabilities": {"0": 0.1, "1": 0.3, "2": 0.6}, "confidence": 0.8},
        "role": {"choice": "reviewer", "probabilities": {"reviewer": 0.7, "general": 0.3}},
    }}))
    .await;
    let jev = Jev {
        url,
        ..Jev::default()
    };
    let task = Task {
        text: "Review this diff".into(),
        roles: Some(roles()),
    };
    let answer = jev.classify(task).await.unwrap();
    assert_eq!(answer.difficulty.tier, "hard");
    assert_eq!(answer.difficulty.probs, Some([0.1, 0.3, 0.6]));
    assert_eq!(answer.confidence, Some(0.8));
    let role = answer.role.unwrap();
    assert_eq!(role.role, "reviewer");
    assert_eq!(role.probs["reviewer"], 0.7);

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one request for both questions");
    let (auth, body) = &seen[0];
    assert_eq!(auth, &expected_auth("TYPESAFE_API_KEY"));
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"]["task"], "Review this diff");
    assert_eq!(body["questions"]["difficulty"]["type"], "score");
    assert_eq!(body["questions"]["role"]["type"], "choice");
    assert_eq!(
        body["questions"]["role"]["criteria"]["reviewer"],
        "Review a diff for bugs."
    );
}

#[tokio::test]
async fn jev_with_no_roles_asks_only_the_difficulty() {
    let (url, seen) = fake_api(json!({"answers": {
        "difficulty": {"probabilities": {"0": 0.8, "1": 0.15, "2": 0.05}},
    }}))
    .await;
    let jev = Jev {
        url,
        ..Jev::default()
    };
    let task = Task {
        text: "Rename a variable".into(),
        roles: None,
    };
    let answer = jev.classify(task).await.unwrap();
    assert_eq!(answer.difficulty.tier, "easy");
    assert!(answer.role.is_none());
    let seen = seen.lock().unwrap();
    assert!(seen[0].1["questions"].get("role").is_none());
}

#[tokio::test]
async fn jev_with_no_difficulty_answer_is_an_error() {
    let (url, _) = fake_api(json!({"answers": {}})).await;
    let jev = Jev {
        url,
        ..Jev::default()
    };
    let task = Task {
        text: "Anything".into(),
        roles: None,
    };
    let error = jev.classify(task).await.unwrap_err();
    assert!(
        error.to_string().contains("no difficulty answer"),
        "{error}"
    );
}

#[tokio::test]
async fn jev_choices_answers_each_question_it_was_asked() {
    let (url, seen) = fake_api(json!({"answers": {
        "easy": {"probabilities": {"a/one": 0.6, "b/two": 0.4}},
        "hard": {"probabilities": {}},
    }}))
    .await;
    let jev = Jev {
        url,
        ..Jev::default()
    };
    let question = |key: &str| JevChoice {
        instructions: format!("Which model fits the {key} tier?"),
        criteria: [("a/one".to_string(), "First".to_string())]
            .into_iter()
            .collect(),
    };
    let questions = [
        ("easy".to_string(), question("easy")),
        ("hard".to_string(), question("hard")),
    ]
    .into_iter()
    .collect();
    let answers = jev
        .choices(json!({"role": "reviewer"}), questions)
        .await
        .unwrap();
    // A question answered with no probabilities is left out.
    assert_eq!(answers.len(), 1);
    assert_eq!(answers["easy"]["a/one"], 0.6);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].1["questions"]["hard"]["type"], "choice");
    assert_eq!(seen[0].1["state"]["role"], "reviewer");
}

#[tokio::test]
async fn openai_reads_the_level_probabilities() {
    let (url, seen) = fake_api(json!({"type": "answer", "answers": [{
        "name": "difficulty",
        "probabilities": [
            {"index": 0, "label": "easy", "probability": 0.2},
            {"index": 1, "label": "normal", "probability": 0.7},
            {"index": 2, "label": "hard", "probability": 0.1},
        ],
    }]}))
    .await;
    let openai = OpenAi {
        url,
        ..OpenAi::default()
    };
    let task = Task {
        text: "Add a flag".into(),
        roles: Some(roles()),
    };
    let answer = openai.classify(task).await.unwrap();
    assert_eq!(answer.difficulty.tier, "normal");
    assert!(answer.role.is_none(), "OpenAI Decisions picks no role");
    let seen = seen.lock().unwrap();
    let (auth, body) = &seen[0];
    assert_eq!(auth, &expected_auth("OPENAI_API_KEY"));
    assert_eq!(body["input"], "Add a flag");
    assert_eq!(body["questions"][0]["name"], "difficulty");
    assert_eq!(body["questions"][0]["levels"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn openai_refusal_is_an_error() {
    let (url, _) = fake_api(json!({"type": "refusal", "refusal": "not a task"})).await;
    let openai = OpenAi {
        url,
        ..OpenAi::default()
    };
    let task = Task {
        text: "Hello".into(),
        roles: None,
    };
    let error = openai.classify(task).await.unwrap_err();
    assert!(error.to_string().contains("openai refused"), "{error}");
}

/// The live tests read the key from the environment only, never the person's saved key.
fn live_key(env: &str) {
    assert!(
        std::env::var(env).is_ok_and(|value| !value.trim().is_empty()),
        "set {env} to run this live test"
    );
}

fn sums_to_one(probs: Option<[f64; 3]>) -> bool {
    probs.is_some_and(|p| (p.iter().sum::<f64>() - 1.0).abs() < 0.05)
}

#[tokio::test]
#[ignore = "calls TypeSafe's Jev: costs a request and needs TYPESAFE_API_KEY"]
async fn live_jev_classifies_a_prompt_and_picks_a_role() {
    live_key("TYPESAFE_API_KEY");
    let task = Task {
        text: "Review the diff on this branch for bugs and report them; do not edit files.".into(),
        roles: Some(roles()),
    };
    let answer = Jev::default().classify(task).await.unwrap();
    assert!(sums_to_one(answer.difficulty.probs), "{answer:?}");
    assert!(["easy", "normal", "hard"].contains(&answer.difficulty.tier.as_str()));
    let role = answer.role.expect("a role answer");
    assert!(roles().contains_key(&role.role), "{role:?}");
}

#[tokio::test]
#[ignore = "calls OpenAI Decisions: costs a request and needs OPENAI_API_KEY"]
async fn live_openai_classifies_a_prompt() {
    live_key("OPENAI_API_KEY");
    let task = Task {
        text: "Rename one variable in a single file.".into(),
        roles: None,
    };
    let answer = OpenAi::default().classify(task).await.unwrap();
    assert!(sums_to_one(answer.difficulty.probs), "{answer:?}");
}
