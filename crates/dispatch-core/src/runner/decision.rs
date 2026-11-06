use super::{Candidate, Deps, PRIORITY_BALANCED, TIER_AUTO, fmt_time, routing::price_auto};
use crate::{
    adapters::valid_tier,
    route_decision::{POLICY_AUTO_V1, POLICY_TIER_V2, RouteDecision, decision_candidates},
    types::Run,
};

pub const ROUTE_AUTO: &str = "auto";
pub const ROUTE_TIER: &str = "tier";
pub const ROUTE_MODEL: &str = "model";

/// What a run's request asked for: a model or provider filter pins the
/// route, otherwise the tier table ordered it.
pub fn route_kind(rec: &Run) -> &'static str {
    if rec.requested_tier == TIER_AUTO {
        return ROUTE_AUTO;
    }
    if rec
        .requested
        .as_ref()
        .is_some_and(|f| !f.model.is_empty() || !f.provider.is_empty())
    {
        return ROUTE_MODEL;
    }
    if valid_tier(&rec.tier) {
        return ROUTE_TIER;
    }
    ROUTE_MODEL
}

fn priority_of(rec: &Run) -> String {
    match &rec.requested {
        Some(f) if !f.priority.is_empty() => f.priority.clone(),
        _ => PRIORITY_BALANCED.into(),
    }
}

impl Deps {
    /// Logs how a new run's candidates were ordered. Resumed runs reuse their
    /// session's candidate and are not routing decisions. A failed write
    /// never fails the run.
    pub(crate) fn record_route_decision(&self, rec: &Run, cs: &[Candidate]) {
        let Some(st) = self.store.as_deref() else {
            return;
        };
        if !rec.resumed_of.is_empty() || cs.is_empty() {
            return;
        }
        let views: Vec<_> = cs.iter().map(|c| c.route.clone()).collect();
        let mut candidates = decision_candidates(&views);
        // A route the message was not scored for still names each model's skill and
        // price from the catalog, as Intelligence does, so its candidates can be compared.
        let cat = self.catalog();
        for (dc, c) in candidates.iter_mut().zip(cs) {
            if c.adapter.is_none() {
                dc.agent.clear();
            }
            if dc.theta.is_none() {
                let e = cat.estimate(&dc.model, &dc.effort);
                if !e.theta_source.is_empty() {
                    dc.theta = Some(e.theta);
                    dc.theta_source = e.theta_source;
                }
                dc.price = e.cost_per_task;
            }
        }
        let mut dec = RouteDecision {
            run_id: rec.id.clone(),
            project_id: rec.project_id.clone(),
            policy_version: POLICY_TIER_V2.into(),
            route: route_kind(rec).into(),
            priority: priority_of(rec),
            classifier: rec.classifier.clone(),
            probs: rec.difficulty_probs.clone(),
            difficulty: rec.difficulty,
            area: rec.area.clone(),
            candidates,
            chosen: 0,
            created_at: fmt_time(self.now()),
        };
        if let Some(a) = &cs[0].route.auto {
            dec.difficulty = Some(a.difficulty);
            if dec.route == ROUTE_AUTO {
                dec.policy_version = POLICY_AUTO_V1.into();
            }
        }
        let _ = st.save_route_decision(&dec);
    }

    /// Prices a tier or model route's candidates with the model catalog, in
    /// their routed order, when the run has a classified difficulty, so the
    /// decision log and the bar warning use the same estimate as Auto.
    pub fn price_tier_route(&self, rec: &Run, cs: Vec<Candidate>) -> Vec<Candidate> {
        let Some(d) = rec.difficulty else {
            return cs;
        };
        price_auto(&cs, d, &priority_of(rec), &self.catalog())
    }
}
