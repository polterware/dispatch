use crate::{
    catalog::{Catalog, base_model},
    pass_model::{difficulty_tier, pass_probability, theta_uncertainty},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const PREFER_BIAS: f64 = -50.0;
pub const AVOID_BIAS: f64 = 1500.0;
/// Bounds this close are within benchmark noise, so prefer the cheaper candidate.
pub const NEAR_BEST_BOUND: f64 = 0.02;
#[derive(Clone, Debug, Default)]
pub struct Pace {
    pub actual: f64,
    pub delta: f64,
    pub deficit: bool,
    pub will_last: bool,
}
#[derive(Clone, Debug, Default)]
pub struct History {
    pub valid: bool,
    pub risk: f64,
}
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub note: String,
    pub pace: Option<Pace>,
    pub history: Option<History>,
    pub cooled_down: bool,
    pub bias: f64,
    pub priority: String,
    pub auto: Option<AutoInfo>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoInfo {
    pub difficulty: f64,
    pub theta: f64,
    pub theta_source: String,
    pub p_pass: f64,
    pub p_pass_low: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cost_source: String,
    #[serde(rename = "quotaMultiplier")]
    pub quota: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_cost: Option<f64>,
    pub threshold: f64,
    pub qualified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Term {
    pub name: String,
    pub value: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}
pub fn valid_priority(p: &str) -> bool {
    matches!(p, "speed" | "balanced" | "quality")
}
pub fn low_quota(c: &Candidate) -> bool {
    c.pace
        .as_ref()
        .is_some_and(|p| p.actual >= 90.0 || !p.will_last)
}
pub fn order_tier(cs: &[Candidate]) -> Vec<Candidate> {
    let group = |c: &Candidate| {
        if c.cooled_down {
            2
        } else if low_quota(c) {
            1
        } else {
            0
        }
    };
    let mut out = cs.to_vec();
    out.sort_by(|a, b| {
        group(a)
            .cmp(&group(b))
            .then_with(|| a.bias.total_cmp(&b.bias))
            .then_with(|| {
                if group(a) == 1 {
                    match (&a.pace, &b.pace) {
                        (Some(x), Some(y)) => x.actual.total_cmp(&y.actual),
                        _ => std::cmp::Ordering::Equal,
                    }
                } else {
                    std::cmp::Ordering::Equal
                }
            })
    });
    out
}
fn pace_parts(c: &Candidate) -> (f64, f64) {
    let base = match &c.pace {
        None => 1000.0,
        Some(p) if !p.will_last => 3000.0 + p.delta,
        Some(p) if p.deficit => 2000.0 + p.delta,
        Some(p) => p.delta,
    };
    (
        base,
        c.history
            .as_ref()
            .filter(|h| h.valid)
            .map_or(0.0, |h| h.risk * 40.0),
    )
}
fn pace_base(c: &Candidate) -> f64 {
    let (a, b) = pace_parts(c);
    a + b
}
/// Quota pace, cooldown, and prefer/avoid bias; the speed priority weighs
/// pace twice.
pub fn score(c: &Candidate) -> f64 {
    let base = pace_base(c);
    base + if c.cooled_down { 10000.0 } else { 0.0 }
        + c.bias
        + if c.priority == "speed" { base } else { 0.0 }
}
pub fn rank(cs: &[Candidate]) -> Vec<Candidate> {
    let mut out = cs.to_vec();
    out.sort_by(|a, b| score(a).total_cmp(&score(b)));
    out
}
pub fn terms(c: &Candidate) -> Vec<Term> {
    let (base, hist) = pace_parts(c);
    let mut out = vec![Term {
        name: "pace".into(),
        value: base,
        note: String::new(),
    }];
    let mut add = |name: &str, value: f64, note: String| {
        out.push(Term {
            name: name.into(),
            value,
            note,
        })
    };
    if hist != 0.0 {
        add("history", hist, String::new());
    }
    if c.cooled_down {
        add("cooldown", 10000.0, String::new());
    }
    if c.bias != 0.0 {
        add("bias", c.bias, String::new());
    }
    if c.priority == "speed" {
        add("paceSlack", pace_base(c), String::new());
    }
    out
}
pub fn pass_threshold(priority: &str) -> f64 {
    match priority {
        "speed" => 0.7,
        "quality" => 0.9,
        _ => 0.8,
    }
}
pub fn quota_multiplier(c: &Candidate) -> f64 {
    let mut q = match &c.pace {
        None => 1.0,
        Some(p) if !p.will_last => 5.0,
        Some(p) if p.deficit => 3.0,
        Some(p) => (1.0 + p.delta / 100.0).clamp(0.5, 1.0),
    };
    if c.bias < 0.0 {
        q *= 0.9;
    }
    if c.bias >= AVOID_BIAS {
        q *= 2.0;
    }
    q
}
pub fn price_auto(cs: &[Candidate], d: f64, priority: &str, cat: &Catalog) -> Vec<Candidate> {
    let mut out = cs.to_vec();
    let threshold = pass_threshold(priority);
    let mut known = Vec::new();
    for c in &mut out {
        let e = cat.estimate(&c.model, &c.effort);
        let (p, low) = pass_probability(
            e.theta,
            d,
            theta_uncertainty(&e.theta_source, e.extrapolated_levels),
        );
        if let Some(cost) = e.cost_per_task {
            known.push(cost);
        }
        c.auto = Some(AutoInfo {
            difficulty: d,
            theta: e.theta,
            theta_source: e.theta_source,
            p_pass: p,
            p_pass_low: low,
            cost: e.cost_per_task,
            cost_source: e.cost_source,
            quota: quota_multiplier(c),
            effective_cost: None,
            threshold,
            qualified: low >= threshold && !c.cooled_down,
        });
    }
    known.sort_by(f64::total_cmp);
    let median = if known.is_empty() {
        1.0
    } else {
        known[known.len() / 2]
    };
    for c in &mut out {
        if let Some(a) = &mut c.auto {
            let cost = if let Some(cost) = a.cost {
                cost
            } else {
                a.cost_source = "price unknown: median".into();
                median
            };
            a.effective_cost = Some(cost * a.quota);
        }
    }
    out
}
pub fn rank_auto(cs: &[Candidate], d: f64, priority: &str, cat: &Catalog) -> Vec<Candidate> {
    let mut out = price_auto(cs, d, priority, cat);
    let best_bound = if out
        .iter()
        .any(|c| c.auto.as_ref().is_some_and(|a| a.qualified))
    {
        None
    } else {
        out.iter()
            .filter_map(|c| c.auto.as_ref().map(|a| a.p_pass_low))
            .max_by(f64::total_cmp)
    };
    out.sort_by(|x, y| match (&x.auto, &y.auto) {
        (Some(a), Some(b)) => b.qualified.cmp(&a.qualified).then_with(|| {
            let near_a = best_bound.is_some_and(|best| a.p_pass_low >= best - NEAR_BEST_BOUND);
            let near_b = best_bound.is_some_and(|best| b.p_pass_low >= best - NEAR_BEST_BOUND);
            if near_a != near_b {
                return near_b.cmp(&near_a);
            }
            if (a.qualified || near_a) && a.effective_cost != b.effective_cost {
                a.effective_cost
                    .partial_cmp(&b.effective_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            } else {
                b.p_pass_low.total_cmp(&a.p_pass_low)
            }
        }),
        _ => std::cmp::Ordering::Equal,
    });
    let mut seen = HashSet::new();
    out.retain(|c| {
        seen.insert(if c.agent.is_empty() {
            base_model(&c.model).to_owned()
        } else {
            format!("{}/{}", c.agent, base_model(&c.model))
        })
    });
    out
}
pub fn auto_note(cs: &[Candidate]) -> String {
    let Some(c) = cs.first() else {
        return String::new();
    };
    let Some(a) = &c.auto else {
        return String::new();
    };
    let pick = if c.agent.is_empty() {
        c.model.clone()
    } else {
        format!("{}/{}", c.agent, c.model)
    };
    let pick = if c.effort.is_empty() {
        pick
    } else {
        format!("{pick} {}", c.effort)
    };
    let why = if a.qualified {
        "cheapest likely to pass"
    } else {
        "none reaches the bound; the cheapest within 0.02 of the best bound"
    };
    format!(
        "auto: d={:.2} ({}), picked {} (P={:.2}, ${:.2}): {}",
        a.difficulty,
        difficulty_tier(a.difficulty),
        pick,
        a.p_pass,
        a.effective_cost.unwrap_or_default(),
        why
    )
}

pub fn reason(c: &Candidate) -> String {
    let mut basis = match &c.pace {
        None => "quota unknown".to_owned(),
        Some(p) => format!(
            "pace delta={:.2} deficit={} willLast={}",
            p.delta, p.deficit, p.will_last
        ),
    };
    if let Some(h) = c.history.as_ref().filter(|h| h.valid) {
        basis.push_str(&format!(" historyRisk={:.2}", h.risk));
    }
    let mut msg = format!("score={:.2} ({}; bias={:.2})", score(c), basis, c.bias);
    if c.priority == "speed" {
        msg.push_str(&format!("; speed paceSlack={:.2}", pace_base(c)));
    }
    msg
}
fn label(c: &Candidate) -> String {
    let name = if c.agent.is_empty() { "?" } else { &c.agent };
    if c.model.is_empty() {
        name.to_owned()
    } else {
        format!("{name}/{}", c.model)
    }
}
pub fn priority_effect(priority: &str, chosen: &[Candidate], balanced: &[Candidate]) -> String {
    let Some(w) = chosen.first() else {
        return String::new();
    };
    if priority.is_empty() || priority == "balanced" {
        return "balanced: score weights unchanged from the current router".into();
    }
    let Some(b) = balanced.first() else {
        return format!("{priority}: no balanced candidate");
    };
    if w.agent == b.agent && w.model == b.model && w.effort == b.effort {
        return format!(
            "{priority}: winner stays {}; extra weights did not change the balanced order",
            label(w)
        );
    }
    let why = if w.auto.is_some() {
        format!(
            "P(pass) bar {:.2} instead of {:.2}",
            pass_threshold(priority),
            pass_threshold("balanced")
        )
    } else {
        format!(
            "paceSlack {:.2} vs {:.2} (lower wins)",
            pace_base(w),
            pace_base(b)
        )
    };
    format!(
        "{priority}: winner moved from {} to {}; {why}",
        label(b),
        label(w)
    )
}
pub fn auto_reason(c: &Candidate) -> Option<String> {
    let a = c.auto.as_ref()?;
    let effective = a.effective_cost?;
    Some(format!(
        "P(pass)={:.2} (bound {:.2} {} {:.2}), θ={:.2} {}, cost ${:.2} × quota {:.2} = ${:.2} ({})",
        a.p_pass,
        a.p_pass_low,
        if a.qualified { "meets" } else { "below" },
        a.threshold,
        a.theta,
        a.theta_source,
        effective / a.quota,
        a.quota,
        effective,
        a.cost_source
    ))
}

#[derive(Clone, Debug, Default)]
pub struct Filters {
    pub provider: String,
    pub model: String,
    pub agent: String,
    pub effort: String,
    pub priority: String,
    pub relax: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Agent {
    pub name: String,
    pub installed: bool,
    pub read_only: bool,
    pub file_editing: bool,
    pub effort_mode: String,
    pub efforts: Vec<String>,
    pub discovered: Vec<String>,
    pub pace: Option<Pace>,
    pub cooled_down: bool,
}

#[derive(Clone, Debug, Default)]
pub struct RouteOptions {
    pub tier: String,
    pub difficulty: Option<f64>,
    pub filters: Filters,
    pub read_only: bool,
    pub prefer: Vec<String>,
    pub avoid: Vec<String>,
    pub role_board: crate::tiers::Table,
    /// The tier columns the role keeps; empty keeps all three.
    pub role_columns: Vec<String>,
    pub disabled: Vec<crate::types::DisabledModel>,
    pub disabled_agents: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub candidates: Vec<Candidate>,
    pub balanced: Vec<Candidate>,
    pub eliminated: Vec<String>,
    pub relaxed: bool,
}

fn matches_model(model: &str, requested: &str) -> bool {
    !requested.is_empty() && (model == requested || base_model(model) == base_model(requested))
}

fn tier_effort(tier: &str) -> &str {
    match tier {
        "easy" => "low",
        "hard" => "high",
        _ => "medium",
    }
}

pub fn select_candidates(
    table: &crate::tiers::Table,
    policy: &crate::config::Policy,
    agents: &[Agent],
    options: &RouteOptions,
) -> (Vec<Candidate>, Vec<String>) {
    use crate::{
        catalog::effort_of,
        tiers::{Entry, provider_of},
    };
    let f = &options.filters;
    let mut all = Vec::<(Entry, String, bool)>::new();
    let mut entries = Vec::new();
    let mut rejected = Vec::new();
    for tier in ["easy", "normal", "hard"] {
        if let Some(rows) = table.tiers.get(tier) {
            for e in rows {
                let item = (e.clone(), tier.to_owned(), false);
                all.push(item.clone());
                if options.tier.is_empty() || options.tier == "auto" || options.tier == tier {
                    entries.push(item);
                } else {
                    rejected.push(format!(
                        "{}/{}: tier filter requires {}",
                        e.agent, e.model, options.tier
                    ));
                }
            }
        }
    }
    if policy.discover
        && (options.tier.is_empty() || options.tier == "auto" || options.tier == "normal")
    {
        for a in agents.iter().filter(|a| a.installed) {
            for model in &a.discovered {
                if model.is_empty()
                    || all
                        .iter()
                        .any(|(e, _, _)| e.agent == a.name && e.model == *model)
                {
                    continue;
                }
                let item = (
                    Entry {
                        agent: a.name.clone(),
                        model: model.clone(),
                        provider: provider_of(&a.name, model),
                        ..Default::default()
                    },
                    "normal".into(),
                    true,
                );
                entries.push(item.clone());
                all.push(item);
            }
        }
    }
    let outside = !f.model.is_empty()
        && !all
            .iter()
            .any(|(e, _, _)| matches_model(&e.model, &f.model));
    if outside {
        entries.clear();
        for a in agents {
            let mut desired = provider_of("", &f.model);
            if desired.is_empty() {
                desired.clone_from(&f.provider);
            }
            let provider = if desired.is_empty() || (!f.agent.is_empty() && f.agent == a.name) {
                String::new()
            } else if provider_of(&a.name, "") == desired
                || all.iter().any(|(e, _, _)| {
                    e.agent == a.name
                        && (if e.provider.is_empty() {
                            provider_of(&e.agent, &e.model)
                        } else {
                            e.provider.clone()
                        }) == desired
                })
            {
                desired
            } else {
                continue;
            };
            entries.push((
                Entry {
                    agent: a.name.clone(),
                    model: f.model.clone(),
                    provider,
                    ..Default::default()
                },
                options.tier.clone(),
                false,
            ));
        }
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for (e, tier, discovered) in entries {
        let label = format!("{}/{}", e.agent, e.model);
        let provider = if e.provider.is_empty() {
            provider_of(&e.agent, &e.model)
        } else {
            e.provider.clone()
        };
        let mut reason =
            crate::config::disabled_reason(&options.disabled, &provider, &e.agent, &e.model);
        if reason.is_empty() {
            reason = crate::config::disabled_agent_reason(&options.disabled_agents, &e.agent);
        }
        if reason.is_empty() {
            reason = if !f.provider.is_empty() && f.provider != provider {
                format!("provider filter requires {}", f.provider)
            } else if !f.model.is_empty() && !matches_model(&e.model, &f.model) {
                format!("model filter requires {}", f.model)
            } else if !f.agent.is_empty() && f.agent != e.agent {
                format!("agent filter requires {}", f.agent)
            } else {
                String::new()
            };
        }
        if !reason.is_empty() {
            rejected.push(format!("{label}: {reason}"));
            continue;
        }
        let Some(a) = agents.iter().find(|a| a.name == e.agent && a.installed) else {
            rejected.push(format!("{label}: agent unavailable"));
            continue;
        };
        if options.read_only && !a.read_only {
            rejected.push(format!("{label}: does not support readOnly"));
            continue;
        }
        if !options.read_only && !a.file_editing {
            rejected.push(format!("{label}: does not support fileEditing"));
            continue;
        }
        let mut model = e.model.clone();
        let mut effort = f.effort.clone();
        if !f.model.is_empty() && !effort_of(&f.model).is_empty() {
            model.clone_from(&f.model);
            if effort.is_empty() {
                effort = effort_of(&f.model).into();
            }
        }
        if effort.is_empty() && options.tier != "auto" {
            effort.clone_from(&e.effort);
        }
        let explicit = !effort.is_empty();
        if effort.is_empty() {
            effort = effort_of(&model).into();
        }
        let efforts = if effort.is_empty() && !a.effort_mode.is_empty() {
            if options.tier == "auto" {
                if !e.efforts.is_empty() {
                    e.efforts.clone()
                } else if !a.efforts.is_empty() {
                    a.efforts.clone()
                } else {
                    vec![tier_effort(&tier).into()]
                }
            } else {
                vec![tier_effort(&options.tier).into()]
            }
        } else {
            vec![effort]
        };
        for effort in efforts {
            let mut model = model.clone();
            if !effort.is_empty() {
                let supported = if !e.efforts.is_empty() {
                    e.efforts.clone()
                } else if !a.efforts.is_empty() {
                    a.efforts.clone()
                } else if !effort_of(&e.model).is_empty() {
                    vec![effort_of(&e.model).into()]
                } else {
                    vec![]
                };
                let reason = if explicit && a.effort_mode.is_empty() && effort_of(&model) != effort
                {
                    "effort unsupported by agent (no effort control)".to_owned()
                } else if !supported.contains(&effort)
                    || (!a.efforts.is_empty() && !a.efforts.contains(&effort))
                {
                    format!("effort {effort} unsupported by model/agent")
                } else if a.effort_mode.is_empty() && effort_of(&model) != effort {
                    "effort unsupported by agent".into()
                } else {
                    String::new()
                };
                if !reason.is_empty() {
                    rejected.push(format!("{label}: {reason}"));
                    continue;
                }
                if a.effort_mode == "model-suffix" {
                    model = format!("{}-{effort}", base_model(&model));
                } else if a.effort_mode == "config" {
                    model = base_model(&model).into();
                }
            }
            if !seen.insert((e.agent.clone(), model.clone(), effort.clone())) {
                continue;
            }
            let bias = if options.prefer.contains(&e.agent) {
                PREFER_BIAS
            } else {
                0.0
            } + if options.avoid.contains(&e.agent) {
                AVOID_BIAS
            } else {
                0.0
            };
            candidates.push(Candidate {
                agent: e.agent.clone(),
                provider: provider.clone(),
                model,
                effort,
                note: if outside {
                    "explicit model outside tier table".into()
                } else if discovered {
                    "discovered".into()
                } else {
                    String::new()
                },
                pace: a.pace.clone(),
                cooled_down: a.cooled_down,
                bias,
                priority: f.priority.clone(),
                ..Default::default()
            });
        }
    }
    if candidates.is_empty() && rejected.is_empty() {
        rejected.push("no candidate matches tier/model intersection".into());
    }
    (candidates, rejected)
}

pub fn plan(
    table: &crate::tiers::Table,
    policy: &crate::config::Policy,
    agents: &[Agent],
    options: &RouteOptions,
    cat: &Catalog,
) -> Plan {
    let mut o = options.clone();
    if o.tier == "auto" && o.difficulty.is_none() {
        o.tier = "normal".into();
    }
    let mut table = table.clone();
    let mut policy = policy.clone();
    let mut role_note = String::new();
    if o.filters.model.is_empty() && o.filters.provider.is_empty() {
        let mut band = if o.tier == "auto" {
            difficulty_tier(o.difficulty.unwrap_or(-1.8)).to_owned()
        } else if matches!(o.tier.as_str(), "easy" | "normal" | "hard") {
            o.tier.clone()
        } else {
            "normal".into()
        };
        if let Some((kept, note)) = crate::roles::nearest_column(&o.role_columns, &band) {
            band = kept;
            role_note = note;
        }
        if let Some(entries) = o.role_board.tiers.get(&band).filter(|e| !e.is_empty()) {
            table.tiers.clear();
            table.tiers.insert(band.clone(), entries.clone());
            o.tier = band;
            policy.discover = false;
        } else {
            role_note.clear();
        }
    }
    let (mut primary, mut eliminated) = select_candidates(&table, &policy, agents, &o);
    if !role_note.is_empty() {
        for c in &mut primary {
            if !c.note.is_empty() {
                c.note.push_str("; ");
            }
            c.note.push_str(&role_note);
        }
    }
    let mut extra = Vec::new();
    if o.filters.relax
        && crate::config::disabled_request_reason(
            &o.disabled,
            &o.filters.provider,
            &o.filters.agent,
            &o.filters.model,
        )
        .is_empty()
        && crate::config::disabled_agent_reason(&o.disabled_agents, &o.filters.agent).is_empty()
    {
        let mut fallback = o.clone();
        if fallback.tier.is_empty() {
            fallback.tier = "normal".into();
        }
        fallback.filters = Filters {
            priority: o.filters.priority.clone(),
            ..Default::default()
        };
        let (cs, why) = select_candidates(&table, &policy, agents, &fallback);
        for mut c in cs {
            if !primary
                .iter()
                .any(|p| p.agent == c.agent && p.model == c.model && p.effort == c.effort)
            {
                if !c.note.is_empty() {
                    c.note.push_str("; ");
                }
                c.note.push_str("--relax: filters relaxed to tier routing");
                extra.push(c);
            }
        }
        if primary.is_empty() {
            eliminated.extend(why);
        }
    }
    if o.tier == "auto" {
        let mut all = primary;
        all.extend(extra);
        let d = o.difficulty.unwrap_or(-1.8);
        return Plan {
            candidates: rank_auto(&all, d, &o.filters.priority, cat),
            balanced: rank_auto(&all, d, "balanced", cat),
            eliminated,
            relaxed: false,
        };
    }
    let tier_route = !o.tier.is_empty()
        && o.filters.model.is_empty()
        && o.filters.agent.is_empty()
        && o.filters.provider.is_empty();
    let order = |cs: &[Candidate]| if tier_route { order_tier(cs) } else { rank(cs) };
    let mut candidates = order(&primary);
    candidates.extend(order(&extra));
    let mut bp = primary.clone();
    let mut be = extra.clone();
    for c in bp.iter_mut().chain(be.iter_mut()) {
        c.priority = "balanced".into();
    }
    let mut balanced = order(&bp);
    balanced.extend(order(&be));
    Plan {
        candidates,
        balanced,
        eliminated,
        relaxed: !extra.is_empty(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct RouteCandidate {
    pub filters: Filters,
    pub score: f64,
    pub reason: String,
    pub terms: Vec<Term>,
    pub auto: Option<AutoInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct RouteReport {
    pub requested: Filters,
    pub priority: String,
    pub candidates: Vec<RouteCandidate>,
    pub eliminated: Vec<String>,
    pub winner: Option<Filters>,
    pub balanced_winner: Option<Filters>,
    pub reason: String,
    pub priority_note: String,
    pub note: String,
    pub difficulty: Option<f64>,
}

fn route_filters(c: &Candidate) -> Filters {
    Filters {
        provider: c.provider.clone(),
        model: c.model.clone(),
        effort: c.effort.clone(),
        agent: c.agent.clone(),
        priority: if c.priority.is_empty() {
            "balanced".into()
        } else {
            c.priority.clone()
        },
        ..Default::default()
    }
}
fn join_note(existing: &mut String, next: &str) {
    if !next.is_empty() {
        if !existing.is_empty() {
            existing.push_str("; ");
        }
        existing.push_str(next);
    }
}

pub fn route(
    table: &crate::tiers::Table,
    policy: &crate::config::Policy,
    agents: &[Agent],
    options: &RouteOptions,
    cat: &Catalog,
) -> Result<RouteReport, crate::Error> {
    let mut o = options.clone();
    if o.filters.priority.is_empty() {
        o.filters.priority = "balanced".into();
    }
    if !valid_priority(&o.filters.priority) {
        return Err(crate::Error::Message(format!(
            "invalid priority {:?} (want speed|balanced|quality)",
            o.filters.priority
        )));
    }
    if !matches!(o.tier.as_str(), "" | "auto" | "easy" | "normal" | "hard") {
        return Err(crate::Error::Message(format!(
            "invalid tier {:?} (want auto|easy|normal|hard)",
            o.tier
        )));
    }
    if o.tier == "auto" && (!o.filters.model.is_empty() || !o.filters.provider.is_empty()) {
        return Err(crate::Error::Message(
            "tier auto picks the model; drop --model and --provider".into(),
        ));
    }
    if !o.filters.effort.is_empty() && !crate::tiers::valid_effort(&o.filters.effort) {
        return Err(crate::Error::Message(format!(
            "invalid effort {:?} (want low|medium|high|xhigh|max)",
            o.filters.effort
        )));
    }
    let suffix = crate::catalog::effort_of(&o.filters.model);
    if !o.filters.effort.is_empty() && !suffix.is_empty() && suffix != o.filters.effort {
        return Err(crate::Error::Message(
            "model suffix conflicts with requested effort".into(),
        ));
    }
    let planned = plan(table, policy, agents, &o, cat);
    let mut report = RouteReport {
        requested: o.filters.clone(),
        priority: o.filters.priority.clone(),
        eliminated: planned.eliminated,
        ..Default::default()
    };
    if planned.relaxed {
        join_note(&mut report.note, "--relax: filters relaxed to tier routing");
    }
    for c in &planned.candidates {
        let r = if c.auto.is_some() {
            auto_reason(c).unwrap_or_default()
        } else {
            reason(c)
        };
        if let Some(a) = &c.auto {
            report.difficulty = Some(a.difficulty);
        }
        report.candidates.push(RouteCandidate {
            filters: route_filters(c),
            score: score(c),
            reason: r,
            terms: if c.auto.is_some() { vec![] } else { terms(c) },
            auto: c.auto.clone(),
        });
        join_note(&mut report.note, &c.note);
    }
    let Some(first) = report.candidates.first() else {
        report.reason = format!("no eligible candidate: {}", report.eliminated.join("; "));
        return Ok(report);
    };
    report.winner = Some(first.filters.clone());
    report.balanced_winner = planned.balanced.first().map(|c| {
        let mut f = route_filters(c);
        f.priority.clone_from(&o.filters.priority);
        f
    });
    report.priority_note =
        priority_effect(&o.filters.priority, &planned.candidates, &planned.balanced);
    report.reason = format!(
        "lowest score wins; stable table order breaks ties; cached quota only: {}",
        first.reason
    );
    if !o.tier.is_empty()
        && o.tier != "auto"
        && o.filters.model.is_empty()
        && o.filters.agent.is_empty()
        && o.filters.provider.is_empty()
    {
        report.reason = format!(
            "the tier's table order wins, passing over an account with low quota (90% used or out before reset) or a rate limit; cached quota only: {}",
            first.reason
        );
    }
    if planned.candidates[0].auto.is_some() {
        join_note(&mut report.note, &auto_note(&planned.candidates));
        report.reason = format!(
            "auto: the cheapest candidate whose pass bound meets the threshold wins, else the cheapest within 0.02 of the best bound; cached quota only: {}",
            first.reason
        );
    }
    if !report.priority_note.is_empty() {
        report.reason.push_str("; ");
        report.reason.push_str(&report.priority_note);
    }
    Ok(report)
}
