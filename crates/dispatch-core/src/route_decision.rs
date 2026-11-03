use crate::{
    pass_model::Area,
    router::{Candidate, score},
};
use serde::{Deserialize, Serialize};

pub const POLICY_AUTO_V1: &str = "auto-v1";
pub const POLICY_TIER_V1: &str = "tier-v1";
pub const POLICY_TIER_V2: &str = "tier-v2";
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionCandidate {
    pub agent: String,
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_pass: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_pass_low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub quota_multiplier: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theta: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub theta_source: String,
    #[serde(skip_serializing_if = "is_false")]
    pub qualified: bool,
    pub score: f64,
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_false(v: &bool) -> bool {
    !*v
}
pub fn decision_candidates(cs: &[Candidate]) -> Vec<DecisionCandidate> {
    cs.iter()
        .map(|c| {
            let mut d = DecisionCandidate {
                agent: c.agent.clone(),
                model: c.model.clone(),
                effort: c.effort.clone(),
                score: score(c),
                ..Default::default()
            };
            if let Some(a) = &c.auto {
                d.p_pass = Some(a.p_pass);
                d.p_pass_low = Some(a.p_pass_low);
                d.price = a.cost;
                d.quota_multiplier = a.quota;
                d.effective_cost = a.effective_cost;
                d.theta = Some(a.theta);
                d.theta_source = a.theta_source.clone();
                d.qualified = a.qualified;
            }
            d
        })
        .collect()
}
#[derive(Clone, Debug, Default)]
pub struct RouteDecision {
    pub run_id: String,
    pub project_id: String,
    pub policy_version: String,
    pub route: String,
    pub priority: String,
    pub classifier: String,
    pub probs: Vec<f64>,
    pub difficulty: Option<f64>,
    pub area: Area,
    pub candidates: Vec<DecisionCandidate>,
    pub chosen: i32,
    pub created_at: String,
}
