use super::stats::{observed_ms, pctile};
use crate::types::{Run, STATUS_COMPLETED, STATUS_PENDING, STATUS_RUNNING};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelEvidence {
    pub tier: String,
    pub model: String,
    pub finished: usize,
    #[serde(rename = "durationSamples")]
    pub duration_n: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<i64>,
}

impl From<&ModelEvidence> for crate::router::Evidence {
    fn from(e: &ModelEvidence) -> Self {
        Self {
            duration_n: e.duration_n,
            p50_ms: e.p50_ms,
        }
    }
}

impl From<ModelEvidence> for crate::router::Evidence {
    fn from(e: ModelEvidence) -> Self {
        Self::from(&e)
    }
}

pub fn evidence_key(tier: &str, model: &str) -> String {
    format!("{tier}\0{model}")
}

pub fn evidence_by_tier_model(recs: &[Run]) -> HashMap<String, ModelEvidence> {
    #[derive(Default)]
    struct Acc {
        tier: String,
        model: String,
        finished: usize,
        durs: Vec<i64>,
    }

    let mut groups: HashMap<String, Acc> = HashMap::new();
    for r in recs {
        if r.status == STATUS_PENDING || r.status == STATUS_RUNNING || r.model.is_empty() {
            continue;
        }
        let key = evidence_key(&r.tier, &r.model);
        let a = groups.entry(key).or_insert_with(|| Acc {
            tier: r.tier.clone(),
            model: r.model.clone(),
            ..Default::default()
        });
        a.finished += 1;
        if r.status == STATUS_COMPLETED
            && let Some(ms) = observed_ms(r)
        {
            a.durs.push(ms);
        }
    }

    let mut out = HashMap::with_capacity(groups.len());
    for (key, a) in groups {
        let p50_ms = pctile(&a.durs, 0.50);
        out.insert(
            key,
            ModelEvidence {
                tier: a.tier,
                model: a.model,
                finished: a.finished,
                duration_n: a.durs.len(),
                p50_ms,
            },
        );
    }
    out
}
