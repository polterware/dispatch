use super::{Key, OPENAI_KEY, http::post_json, or_default};
use crate::{
    Error,
    config::CLASSIFIER_OPENAI,
    difficulty::{DIFFICULTY_LEVELS, DIFFICULTY_QUESTION, new_difficulty},
    runner::{Classification, Classifier, Task},
};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Instant;

const OPENAI_URL: &str = "https://api.openai.com/v1/decisions";
const OPENAI_MODEL: &str = "gpt-6-luna";

const LEVEL_LABELS: [&str; 3] = ["easy", "normal", "hard"];

/// Asks OpenAI's Decisions API one score question about the task. The request
/// and answer field names live only in this file. It does not pick a role: a
/// run it classifies stays general unless the request names one.
#[derive(Clone, Debug, Default)]
pub struct OpenAi {
    pub model: String,
    pub url: String,
    pub key: Option<Key>,
    pub client: Option<reqwest::Client>,
}

#[derive(Deserialize)]
struct OpenAiAnswer {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    refusal: String,
    #[serde(default)]
    answers: Vec<OpenAiQuestion>,
}

#[derive(Deserialize)]
struct OpenAiQuestion {
    #[serde(default)]
    name: String,
    #[serde(default)]
    probabilities: Value,
    confidence: Option<f64>,
}

impl OpenAi {
    pub fn new(model: String) -> Self {
        Self {
            model,
            ..Self::default()
        }
    }

    async fn ask(&self, t: Task) -> Result<Classification, Error> {
        let k = self.key.unwrap_or(OPENAI_KEY);
        let key = k
            .read()
            .map_err(|e| Error::Message(format!("no OpenAI key: {e}")))?;
        if key.is_empty() {
            return Err(Error::Message(format!(
                "no OpenAI key: save one in Settings › Classifier or with `dispatch config key set openai`, or set {}",
                k.env
            )));
        }
        let levels: Vec<Value> = DIFFICULTY_LEVELS
            .iter()
            .zip(LEVEL_LABELS)
            .map(|(d, l)| json!({"label": l, "description": d}))
            .collect();
        let model = or_default(&self.model, OPENAI_MODEL);
        let body = json!({
            "model": model,
            "input": t.text,
            "questions": [{
                "name": "difficulty",
                "type": "score",
                "instructions": DIFFICULTY_QUESTION,
                "levels": levels,
            }],
        });
        let start = Instant::now();
        let out: OpenAiAnswer = post_json(
            self.client.as_ref(),
            &or_default(&self.url, OPENAI_URL),
            &key,
            &body,
        )
        .await?;
        if out.kind == "refusal" || !out.refusal.is_empty() {
            return Err(Error::Message(format!(
                "openai refused: {}",
                or_default(&out.refusal, "no reason")
            )));
        }
        let a = out
            .answers
            .iter()
            .find(|a| a.name == "difficulty")
            .ok_or_else(|| Error::Message("openai: no difficulty answer".into()))?;
        let p = parse_level_probs(&a.probabilities)?;
        Ok(Classification {
            difficulty: new_difficulty(Some(p), "")?,
            provider: CLASSIFIER_OPENAI.into(),
            model,
            confidence: a.confidence,
            latency: start.elapsed(),
            role: None,
        })
    }
}

impl Classifier for OpenAi {
    fn name(&self) -> String {
        CLASSIFIER_OPENAI.into()
    }

    fn classify(&self, task: Task) -> BoxFuture<'_, Result<Classification, Error>> {
        Box::pin(self.ask(task))
    }
}

#[derive(Deserialize)]
struct LevelEntry {
    index: Option<i64>,
    #[serde(default)]
    label: String,
    #[serde(default)]
    probability: f64,
}

/// Reads probabilities given as a list of {index, label, probability} entries
/// or as an object keyed by index or label.
pub fn parse_level_probs(raw: &Value) -> Result<[f64; 3], Error> {
    let mut p = [0.0; 3];
    if let Ok(list) = serde_json::from_value::<Vec<LevelEntry>>(raw.clone())
        && !list.is_empty()
    {
        for e in list {
            if let Some(i) = level_index(e.index, &e.label) {
                p[i] += e.probability;
            }
        }
        return Ok(p);
    }
    if let Ok(m) = serde_json::from_value::<BTreeMap<String, f64>>(raw.clone())
        && !m.is_empty()
    {
        for (k, v) in m {
            let idx = match k.as_bytes() {
                [c @ b'0'..=b'2'] => Some(i64::from(c - b'0')),
                _ => None,
            };
            if let Some(i) = level_index(idx, &k) {
                p[i] += v;
            }
        }
        return Ok(p);
    }
    Err(Error::Message("openai: unreadable probabilities".into()))
}

fn level_index(index: Option<i64>, label: &str) -> Option<usize> {
    if let Some(i) = index.filter(|i| (0..3).contains(i)) {
        return usize::try_from(i).ok();
    }
    LEVEL_LABELS
        .iter()
        .position(|l| label.trim().eq_ignore_ascii_case(l))
}
