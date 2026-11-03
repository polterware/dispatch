use super::{JEV_KEY, Key, http::post_json, or_default};
use crate::{
    Error,
    config::CLASSIFIER_JEV,
    difficulty::{DIFFICULTY_LEVELS, DIFFICULTY_QUESTION, ROLE_QUESTION, new_difficulty},
    runner::{Classification, Classifier, RoleAnswer, Task},
};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

const JEV_URL: &str = "https://api.typesafe.ai/v1/systemone";
const JEV_MODEL: &str = "jev-latest";

/// Asks TypeSafe's System One model one score question about the task, and a
/// choice of role when the task has roles.
#[derive(Clone, Debug, Default)]
pub struct Jev {
    pub model: String,
    pub url: String,
    pub key: Option<Key>,
    pub client: Option<reqwest::Client>,
}

#[derive(Deserialize)]
struct JevAnswer {
    #[serde(default)]
    answers: HashMap<String, JevQuestion>,
}

#[derive(Deserialize)]
struct JevQuestion {
    #[serde(default)]
    choice: String,
    #[serde(default)]
    probabilities: BTreeMap<String, f64>,
    confidence: Option<f64>,
}

impl Jev {
    pub fn new(model: String) -> Self {
        Self {
            model,
            ..Self::default()
        }
    }

    fn key(&self) -> Key {
        self.key.unwrap_or(JEV_KEY)
    }

    async fn ask(&self, t: Task) -> Result<Classification, Error> {
        let k = self.key();
        let key = k
            .read()
            .map_err(|e| Error::Message(format!("no TypeSafe key: {e}")))?;
        if key.is_empty() {
            return Err(Error::Message(format!(
                "no TypeSafe key: save one in Settings › Classifier or with `dispatch config key set jev`, or set {}",
                k.env
            )));
        }
        let model = or_default(&self.model, JEV_MODEL);
        let mut questions = json!({
            "difficulty": {
                "type": "score",
                "instructions": DIFFICULTY_QUESTION,
                "criteria": DIFFICULTY_LEVELS,
            }
        });
        // The role is a Choice over the same state, asked in the same request.
        let roles = t.roles.as_ref().filter(|r| !r.is_empty());
        if let Some(roles) = roles {
            questions["role"] = json!({
                "type": "choice",
                "instructions": ROLE_QUESTION,
                "criteria": roles,
            });
        }
        let body = json!({
            "model": model,
            "state": {"task": t.text},
            "questions": questions,
        });
        let start = Instant::now();
        let out: JevAnswer = post_json(
            self.client.as_ref(),
            &or_default(&self.url, JEV_URL),
            &key,
            &body,
        )
        .await?;
        let a = out
            .answers
            .get("difficulty")
            .filter(|a| !a.probabilities.is_empty())
            .ok_or_else(|| Error::Message("jev: no difficulty answer".into()))?;
        let mut p = [0.0; 3];
        for (i, v) in p.iter_mut().enumerate() {
            *v = a.probabilities.get(&i.to_string()).copied().unwrap_or(0.0);
        }
        let difficulty = new_difficulty(Some(p), "")?;
        let role = out
            .answers
            .get("role")
            .filter(|r| roles.is_some() && (!r.choice.is_empty() || !r.probabilities.is_empty()))
            .map(|r| RoleAnswer {
                role: r.choice.clone(),
                probs: r.probabilities.clone(),
                confidence: r.confidence,
            });
        Ok(Classification {
            difficulty,
            provider: CLASSIFIER_JEV.into(),
            model,
            confidence: a.confidence,
            latency: start.elapsed(),
            role,
        })
    }
}

impl Classifier for Jev {
    fn name(&self) -> String {
        CLASSIFIER_JEV.into()
    }

    fn classify(&self, task: Task) -> BoxFuture<'_, Result<Classification, Error>> {
        Box::pin(self.ask(task))
    }
}
