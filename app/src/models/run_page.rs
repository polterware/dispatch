//! Pure logic for the run page, ported from the Electron Dispatch rail.
//!
//! Covers routing copy, worktree and review actions, usage and cost labels,
//! attempt cards, linked rows, and the durations the header and Run group show.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::client::{DispatchAttempt, DispatchRun, DispatchUsage, ProtoI64};

const EM: &str = "\u{2014}";
const API_EQUIVALENT: &str =
    "API-equivalent cost: what these tokens cost at API prices, not what a subscription bills.";

/// Pass bars the core sets for each priority.
const PRIORITY_BARS: &[(&str, f64)] = &[("speed", 0.7), ("balanced", 0.8), ("quality", 0.9)];

#[derive(Clone, Debug, PartialEq)]
pub struct RouteDecision {
    pub run_id: String,
    pub policy: String,
    pub route: String,
    pub priority: String,
    pub difficulty: Option<f64>,
    pub tier: String,
    pub classifier: String,
    pub probs: Vec<f64>,
    pub area_kinds: Vec<String>,
    pub area_tech: Vec<String>,
    pub candidates: Vec<DecisionCandidate>,
    pub chosen: i64,
    pub role: String,
    pub role_pick: Option<RolePick>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionCandidate {
    pub agent: String,
    pub model: String,
    pub effort: String,
    pub p_pass: Option<f64>,
    pub p_pass_low: Option<f64>,
    pub price: Option<f64>,
    pub quota_multiplier: Option<f64>,
    pub effective_cost: Option<f64>,
    pub theta: Option<f64>,
    pub theta_source: String,
    pub qualified: bool,
    /// The quota-pace score every route orders by: lower runs first.
    pub score: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RolePick {
    pub role: String,
    pub probs: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub context_tokens: i64,
    pub context_window: i64,
    pub cost_usd: Option<f64>,
    pub cost_source: String,
}

impl Default for TokenUsage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            context_tokens: 0,
            context_window: 0,
            cost_usd: None,
            cost_source: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextMeter {
    pub label: String,
    pub title: String,
    pub percent: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CostLabel {
    pub label: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttemptCard {
    pub key: String,
    pub agent: String,
    pub title: String,
    pub status: String,
    pub error: String,
    pub exit_class: String,
    pub handoff: String,
    pub duration_ms: Option<i64>,
    pub is_final: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptSwitch {
    pub index: usize,
    pub from_agent: String,
    pub from_model: String,
    pub to_agent: String,
    pub to_model: String,
    pub exit_class: String,
    pub error: String,
    pub handoff: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewAction {
    Merge,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoreAction {
    Recover,
    Clean,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCounts {
    pub files_label: Option<String>,
    pub insertions_label: Option<String>,
    pub deletions_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedRow {
    pub run_id: String,
    pub seed: String,
    pub label: String,
    pub agent: String,
    pub status: String,
    /// When the linked run merged, or empty: with `status` it says whether its bot celebrates.
    pub merged_at: String,
    pub show_status: bool,
}

/// `{ ok, json }` the way `dispatchOpBody` reads it. Missing `ok` counts as success.
pub fn dispatch_op_body(raw: &Value) -> (bool, Value) {
    let ok = !matches!(raw.get("ok"), Some(Value::Bool(false)));
    let text = raw.get("json").and_then(Value::as_str).unwrap_or("");
    let parsed = if text.is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str::<Value>(text).unwrap_or(Value::Null)
    };
    let body = if parsed.is_object() {
        parsed
    } else {
        Value::Object(Default::default())
    };
    (ok, body)
}

/// The run's routing record, or `None` when the op failed or named no run.
pub fn map_route_decision(ok: bool, json: &str) -> Option<RouteDecision> {
    let raw = serde_json::json!({ "ok": ok, "json": json });
    let (ok, body) = dispatch_op_body(&raw);
    if !ok || json_str(&body, "runId").is_empty() {
        return None;
    }
    let area = body.get("area").cloned().unwrap_or(Value::Null);
    let probs = match body.get("probs").and_then(Value::as_array) {
        Some(values) if values.len() == 3 => values.iter().map(as_number).collect(),
        _ => Vec::new(),
    };
    let candidates = body
        .get("candidates")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().map(map_candidate).collect())
        .unwrap_or_default();
    Some(RouteDecision {
        run_id: json_str(&body, "runId"),
        policy: json_str(&body, "policy"),
        route: json_str(&body, "route"),
        priority: json_str(&body, "priority"),
        difficulty: opt_f64(&body, "difficulty"),
        tier: json_str(&body, "tier"),
        classifier: json_str(&body, "classifier"),
        probs,
        area_kinds: json_strings(&area, "kinds"),
        area_tech: json_strings(&area, "tech"),
        candidates,
        chosen: as_number(body.get("chosen").unwrap_or(&Value::Null)) as i64,
        role: json_str(&body, "role"),
        role_pick: map_role_pick(body.get("rolePick").unwrap_or(&Value::Null)),
    })
}

fn map_candidate(value: &Value) -> DecisionCandidate {
    DecisionCandidate {
        agent: json_str(value, "agent"),
        model: json_str(value, "model"),
        effort: json_str(value, "effort"),
        p_pass: opt_f64(value, "pPass"),
        p_pass_low: opt_f64(value, "pPassLow"),
        price: opt_f64(value, "price"),
        quota_multiplier: opt_f64(value, "quotaMultiplier"),
        effective_cost: opt_f64(value, "effectiveCost"),
        theta: opt_f64(value, "theta"),
        theta_source: json_str(value, "thetaSource"),
        qualified: value.get("qualified") == Some(&Value::Bool(true)),
        score: opt_f64(value, "score"),
    }
}

fn map_role_pick(value: &Value) -> Option<RolePick> {
    let role = json_str(value, "role");
    if role.is_empty() {
        return None;
    }
    let mut probs = BTreeMap::new();
    if let Some(map) = value.get("probs").and_then(Value::as_object) {
        for (name, prob) in map {
            if let Some(number) = prob.as_f64().filter(|n| n.is_finite()) {
                probs.insert(name.clone(), number);
            }
        }
    }
    Some(RolePick { role, probs })
}

pub fn policy_label(policy: &str) -> String {
    match policy {
        "auto-v1" => "Auto".to_string(),
        "tier-v2" => "Tier, priced by the router model".to_string(),
        "tier-v1" => "Tier".to_string(),
        other => other.to_string(),
    }
}

/// The task's difficulty and the tier it maps to, such as `-2.49 · easy`.
pub fn difficulty_label(decision: &RouteDecision) -> Option<String> {
    let difficulty = decision.difficulty?;
    Some(if decision.tier.is_empty() {
        fixed(difficulty, 2)
    } else {
        format!("{} \u{00b7} {}", fixed(difficulty, 2), decision.tier)
    })
}

/// The classifier that judged the task and, when it gave them, its chances for easy,
/// normal, and hard, drawn as bars.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassifierBands {
    pub name: String,
    pub bands: Vec<(&'static str, f64)>,
}

pub fn classifier_bands(decision: &RouteDecision) -> Option<ClassifierBands> {
    if decision.classifier.is_empty() {
        return None;
    }
    let bands = if decision.probs.len() == 3 {
        ["easy", "normal", "hard"]
            .into_iter()
            .zip(decision.probs.iter().map(|p| p.clamp(0.0, 1.0)))
            .collect()
    } else {
        Vec::new()
    };
    Some(ClassifierBands {
        name: decision.classifier.clone(),
        bands,
    })
}

/// The priority and the bar it sets, such as `balanced · bar 80%`.
pub fn priority_label(decision: &RouteDecision) -> String {
    let priority = if decision.priority.is_empty() {
        "balanced"
    } else {
        decision.priority.as_str()
    };
    match PRIORITY_BARS
        .iter()
        .find(|(name, _)| *name == priority)
        .map(|(_, bar)| *bar)
    {
        Some(bar) => format!("{priority} \u{00b7} bar {}", percent(bar)),
        None => priority.to_string(),
    }
}

/// How the run got its role: picked with P, general, or named by the request.
pub fn role_summary(decision: &RouteDecision) -> Option<String> {
    let Some(pick) = decision.role_pick.as_ref() else {
        return if decision.role.is_empty() {
            None
        } else {
            Some(format!("{}, named by the request", decision.role))
        };
    };
    if pick.role == "general" {
        return Some("General: no role fit clearly".to_string());
    }
    let p = pick.probs.get(&pick.role).copied();
    let suffix = match p {
        Some(p) => format!(" (P {})", fixed(p, 2)),
        None => String::new(),
    };
    Some(format!("{}, picked{suffix}", pick.role))
}

pub fn area_label(decision: &RouteDecision) -> String {
    decision
        .area_kinds
        .iter()
        .chain(decision.area_tech.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// One candidate's price: P(pass), its bound, effective cost, and quota multiplier.
/// The pass bar of a decision's priority (`balanced` when it names none).
fn priority_bar(decision: &RouteDecision) -> f64 {
    let priority = if decision.priority.is_empty() {
        "balanced"
    } else {
        decision.priority.as_str()
    };
    PRIORITY_BARS
        .iter()
        .find(|(name, _)| *name == priority)
        .map_or(0.8, |(_, bar)| *bar)
}

pub fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

/// What the quota-pace score says, as the router reads it: cooldown, a quota that will not
/// last or runs behind, none known, or on pace.
pub fn pace_label(score: f64) -> &'static str {
    if score >= 10000.0 {
        "Cooling down"
    } else if score >= 3000.0 {
        "Quota won't last"
    } else if score >= 2000.0 {
        "Quota behind pace"
    } else if score >= 1000.0 {
        "Quota unknown"
    } else {
        "Quota on pace"
    }
}

/// A candidate's second line: under Auto its chance to pass this task and what a run costs
/// here; otherwise its quota, which is all a tier or a named model orders by.
pub fn candidate_line(candidate: &DecisionCandidate) -> String {
    let mut parts = Vec::new();
    if let Some(p_pass) = candidate.p_pass {
        parts.push(format!("{} to pass", percent(p_pass)));
        if let Some(cost) = candidate.effective_cost {
            parts.push(format!("${}", fixed(cost, 2)));
        }
    } else if let Some(theta) = candidate.theta {
        // Not scored against the message: its skill, as Intelligence rates it.
        parts.push(format!("Skill {}", skill_score(theta)));
        if let Some(price) = candidate.price {
            parts.push(format!("${}", fixed(price, 2)));
        }
    } else if let Some(score) = candidate.score {
        parts.push(pace_label(score).to_string());
    }
    parts.join(" \u{00b7} ")
}

/// A skill θ as Intelligence's score: 100·σ(θ), the chance to pass a task of difficulty 0.
pub fn skill_score(theta: f64) -> String {
    format!("{:.0}", 100.0 / (1.0 + (-theta).exp()))
}

/// What became of a candidate in this run, for the badge on its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateOutcome {
    /// The model that ran the task (or runs it now): the run's status.
    Used { status: String },
    /// Tried before the one used and failed: how (its exit class) and its error.
    Failed { class: String, error: String },
    /// Before the one used in routing order but never started (cooling down, missing).
    Skipped,
}

impl CandidateOutcome {
    pub fn label(&self) -> String {
        match self {
            Self::Used { status } if is_run_active(status) => "Running".to_string(),
            Self::Used { status } => match status.to_ascii_lowercase().as_str() {
                "failed" | "timed_out" => "Used, failed".to_string(),
                "cancelled" => "Used, cancelled".to_string(),
                _ => "Used".to_string(),
            },
            Self::Failed { class, .. } => match class.trim() {
                "" | "failed" => "Failed".to_string(),
                class => format!("Failed: {}", class.replace('_', " ")),
            },
            Self::Skipped => "Not tried".to_string(),
        }
    }
}

fn same_model(agent: &str, model: &str, candidate: &DecisionCandidate) -> bool {
    !candidate.model.is_empty()
        && candidate.model == model.trim()
        && (candidate.agent.is_empty() || candidate.agent == agent.trim())
}

/// Each candidate's outcome in `run`, in routing order: the failed attempts, the one used,
/// and the ones before it that never started. Those after the one used have none.
pub fn candidate_outcomes(
    decision: &RouteDecision,
    run: &DispatchRun,
) -> Vec<Option<CandidateOutcome>> {
    let mut out: Vec<Option<CandidateOutcome>> = decision
        .candidates
        .iter()
        .map(|candidate| {
            run.attempts
                .iter()
                .find(|attempt| same_model(&attempt.agent, &attempt.model, candidate))
                .map(|attempt| CandidateOutcome::Failed {
                    class: attempt.exit_class.clone(),
                    error: attempt.error.trim().to_string(),
                })
        })
        .collect();
    let used = decision
        .candidates
        .iter()
        .enumerate()
        .position(|(index, candidate)| {
            out[index].is_none() && same_model(&run.agent, &run.model, candidate)
        });
    if let Some(used) = used {
        out[used] = Some(CandidateOutcome::Used {
            status: run.status.clone(),
        });
        for slot in out.iter_mut().take(used) {
            if slot.is_none() {
                *slot = Some(CandidateOutcome::Skipped);
            }
        }
    }
    out
}

/// The failed attempts no candidate stands for (a model the decision named otherwise), which
/// the Routing group lists on their own.
pub fn unmatched_attempts<'a>(
    decision: &RouteDecision,
    run: &'a DispatchRun,
) -> Vec<&'a DispatchAttempt> {
    run.attempts
        .iter()
        .filter(|attempt| {
            !decision
                .candidates
                .iter()
                .any(|candidate| same_model(&attempt.agent, &attempt.model, candidate))
        })
        .collect()
}

/// A candidate's tooltip: what it was judged on, its numbers, and how they made its place.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateExplain {
    pub about: String,
    pub rows: Vec<(String, String)>,
    pub how: String,
}

pub fn candidate_explain(
    decision: &RouteDecision,
    candidate: &DecisionCandidate,
) -> CandidateExplain {
    let mut rows = Vec::new();
    let pace = candidate.score.map(|score| pace_label(score).to_string());
    let Some(p_pass) = candidate.p_pass else {
        if let Some(theta) = candidate.theta {
            let source = if candidate.theta_source.is_empty() {
                String::new()
            } else {
                format!(" ({})", candidate.theta_source)
            };
            rows.push((
                "Skill".to_string(),
                format!(
                    "{} \u{00b7} \u{03b8} {}{source}",
                    skill_score(theta),
                    fixed(theta, 2)
                ),
            ));
        }
        if let Some(price) = candidate.price {
            rows.push(("Price".to_string(), format!("${} a task", fixed(price, 2))));
        }
        if let Some(pace) = pace {
            rows.push(("Quota".to_string(), pace));
        }
        let (about, how) = if decision.route == "model" {
            (
                "Named by the request: the message was not scored.",
                "A named model runs as asked; its quota only orders its own efforts.",
            )
        } else {
            (
                "Picked from the tier table: the message was not scored.",
                "A tier runs its models in the table's order, those with quota to spare first. Skill is the chance to pass a task of difficulty 0; Auto scores each model against the message instead.",
            )
        };
        return CandidateExplain {
            about: about.to_string(),
            rows,
            how: how.to_string(),
        };
    };
    if let Some(difficulty) = decision.difficulty {
        let tier = if decision.tier.is_empty() {
            String::new()
        } else {
            format!(" ({})", decision.tier)
        };
        rows.push((
            "Task".to_string(),
            format!("d {}{tier}", fixed(difficulty, 2)),
        ));
    }
    if let Some(theta) = candidate.theta {
        let source = if candidate.theta_source.is_empty() {
            String::new()
        } else {
            format!(", {}", candidate.theta_source)
        };
        rows.push((
            "Skill \u{03b8}".to_string(),
            format!("{}{source}", fixed(theta, 2)),
        ));
    }
    let low = candidate
        .p_pass_low
        .map(|low| format!(", at least {}", percent(low)))
        .unwrap_or_default();
    rows.push(("Pass".to_string(), format!("{}{low}", percent(p_pass))));
    let bar = priority_bar(decision);
    rows.push((
        "Bar".to_string(),
        format!(
            "{}: {}",
            percent(bar),
            if candidate.qualified {
                "clears it"
            } else {
                "below it"
            }
        ),
    ));
    if let Some(price) = candidate.price {
        rows.push(("Price".to_string(), format!("${} a task", fixed(price, 2))));
    }
    if let Some(cost) = candidate.effective_cost {
        let quota = candidate
            .quota_multiplier
            .filter(|quota| *quota != 1.0)
            .map(|quota| format!(" (quota \u{00d7}{})", fixed(quota, 2)))
            .unwrap_or_default();
        rows.push((
            "Cost here".to_string(),
            format!("${}{quota}", fixed(cost, 2)),
        ));
    }
    if let Some(pace) = pace {
        rows.push(("Quota".to_string(), pace));
    }
    let classifier = if decision.classifier.is_empty() {
        "The classifier".to_string()
    } else {
        decision.classifier.clone()
    };
    CandidateExplain {
        about: format!("How it scored against the first message, as {classifier} read it."),
        rows,
        how: "Pass is \u{03c3}(\u{03b8} \u{2212} d): the more its skill exceeds the task's difficulty, the likelier it passes. Auto runs the cheapest whose lower bound clears the bar; quota scales the price.".to_string(),
    }
}

/// The Routing group's Why: how this route came to try its first model.
pub fn route_reason(decision: &RouteDecision) -> Option<String> {
    let chosen = usize::try_from(decision.chosen)
        .ok()
        .and_then(|index| decision.candidates.get(index));
    let name = chosen
        .map(|candidate| candidate.model.clone())
        .unwrap_or_default();
    // An Auto record always scores; older ones may not name their route.
    let route = if decision.policy == "auto-v1" {
        "auto"
    } else {
        decision.route.as_str()
    };
    match route {
        "model" => Some("The request named the model: nothing was scored".to_string()),
        "auto" => {
            let read = match (decision.difficulty, decision.tier.is_empty()) {
                (Some(d), false) => {
                    format!("read the message as {} (d {})", decision.tier, fixed(d, 2))
                }
                (Some(d), true) => format!("read the message at d {}", fixed(d, 2)),
                (None, _) => "judged the message".to_string(),
            };
            let classifier = if decision.classifier.is_empty() {
                "The classifier".to_string()
            } else {
                decision.classifier.clone()
            };
            let qualified = decision.candidates.iter().filter(|c| c.qualified).count();
            let bar = percent(priority_bar(decision));
            let pick = match chosen {
                Some(candidate) if candidate.qualified => {
                    format!("{name} is the cheapest of {qualified} that clear the {bar} bar")
                }
                Some(_) => format!("none clears the {bar} bar, so {name} came closest"),
                None => String::new(),
            };
            Some(if pick.is_empty() {
                format!("{classifier} {read}")
            } else {
                format!("{classifier} {read}; {pick}")
            })
        }
        "tier" if !name.is_empty() => Some(format!(
            "The tier table's first model with quota to spare: {name}. The message was not scored"
        )),
        _ => None,
    }
}

pub fn worktree_display_status(run_status: &str, worktree_status: &str) -> String {
    let worktree = worktree_status.trim();
    if worktree.to_ascii_lowercase() == "removed" {
        return "deleted".to_string();
    }
    if worktree.to_ascii_lowercase() != "active" {
        return worktree.to_string();
    }
    let run = run_status.trim().to_ascii_lowercase();
    if matches!(
        run.as_str(),
        "failed" | "cancelled" | "timed_out" | "orphaned"
    ) {
        return run_status.trim().to_string();
    }
    worktree.to_string()
}

pub fn run_review_action(
    status: &str,
    merged_at: &str,
    worktree_status: &str,
    has_worktree: bool,
) -> Option<ReviewAction> {
    if !has_worktree || worktree_status == "removed" || worktree_status == "abandoned" {
        return None;
    }
    if !merged_at.trim().is_empty() {
        return if worktree_status == "merged" {
            Some(ReviewAction::Delete)
        } else {
            None
        };
    }
    if status.to_ascii_lowercase() == "completed" {
        Some(ReviewAction::Merge)
    } else {
        None
    }
}

pub fn run_more_actions(status: &str, merged_at: &str, has_worktree: bool) -> Vec<MoreAction> {
    let status = status.to_ascii_lowercase();
    if !has_worktree || status == "pending" || status == "running" || !merged_at.trim().is_empty() {
        return Vec::new();
    }
    let ended = matches!(
        status.as_str(),
        "failed" | "cancelled" | "timed_out" | "orphaned"
    );
    let mut actions = Vec::new();
    if ended {
        actions.push(MoreAction::Recover);
    }
    actions.push(MoreAction::Clean);
    actions
}

/// Header Cancel and the live duration clock. Matches `isRunActive` in the Electron page.
/// What keeps a follow-up to `run` from starting now, as the core would refuse it: a
/// message to it still on its way, or another run working in the folder it worked in.
pub fn follow_up_blocker(run: &DispatchRun, runs: &[DispatchRun]) -> Option<FollowUpBlock> {
    if is_run_active(&run.status) {
        return Some(FollowUpBlock::Working);
    }
    let folder = run.worktree.trim();
    runs.iter()
        .filter(|other| other.id != run.id && is_run_active(&other.status))
        .find_map(|other| {
            if other.resumed_of == run.id {
                Some(FollowUpBlock::Answering)
            } else if !folder.is_empty() && other.worktree.trim() == folder {
                Some(FollowUpBlock::FolderBusy)
            } else {
                None
            }
        })
}

/// Why the run page's composer cannot send yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowUpBlock {
    /// The run itself is still working.
    Working,
    /// A message sent to it is still being answered.
    Answering,
    /// Another run is working in the same folder.
    FolderBusy,
}

pub fn is_run_active(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "pending" | "running" | "queued"
    )
}

pub fn format_duration(ms: i64) -> String {
    let sec = (ms.max(0) as f64 / 1000.0).round() as i64;
    if sec < 60 {
        return format!("{sec}s");
    }
    let min = sec / 60;
    if min < 60 {
        return format!("{min}m");
    }
    let hours = min / 60;
    let rem = min % 60;
    if rem == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {rem}m")
    }
}

pub fn measured_duration(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => format_duration(ms),
        None => EM.to_string(),
    }
}

pub fn duration_label(
    run_ms: Option<i64>,
    queued_ms: Option<i64>,
    status: &str,
    started_at: &str,
    created_at: &str,
    now_ms: i64,
) -> Option<String> {
    if let Some(ms) = run_ms {
        return Some(format_duration(ms));
    }
    if is_run_active(status) {
        let start_src = if started_at.is_empty() {
            created_at
        } else {
            started_at
        };
        if let Some(start) = parse_time_ms(start_src) {
            return Some(format_duration((now_ms - start).max(0)));
        }
    }
    queued_ms.map(format_duration)
}

pub fn route_label(tier: &str, effort: &str) -> Option<String> {
    let parts: Vec<&str> = [tier, effort]
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

pub fn timing_label(queued_ms: Option<i64>, run_ms: Option<i64>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ms) = queued_ms {
        parts.push(format!("queue {}", format_duration(ms)));
    }
    if let Some(ms) = run_ms {
        parts.push(format!("exec {}", format_duration(ms)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

pub fn dotted(parts: &[&str]) -> String {
    let visible: Vec<&str> = parts
        .iter()
        .copied()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if visible.is_empty() {
        EM.to_string()
    } else {
        visible.join(" / ")
    }
}

pub fn short_run_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() > 12 {
        chars.into_iter().take(12).collect()
    } else {
        id.to_string()
    }
}

pub fn agent_attempt_cards(run: &DispatchRun) -> Vec<AttemptCard> {
    let final_agent = if run.agent.trim().is_empty() {
        "agent".to_string()
    } else {
        run.agent.trim().to_string()
    };
    let last = run.attempts.len().saturating_sub(1);
    let mut cards: Vec<AttemptCard> = run
        .attempts
        .iter()
        .enumerate()
        .map(|(index, attempt)| map_attempt(run, attempt, index, last, &final_agent))
        .collect();
    let already_final = cards.last().is_some_and(|card| card.is_final);
    if !already_final {
        cards.push(AttemptCard {
            key: format!("{}:final", run.id),
            agent: final_agent.clone(),
            title: dotted(&[&final_agent, &run.model, &run.provider]),
            status: or_unknown(&run.status),
            error: run.error.trim().to_string(),
            exit_class: run.exit_class.trim().to_string(),
            handoff: String::new(),
            duration_ms: run.run_ms.map(|value| value.0),
            is_final: true,
        });
    }
    cards
}

fn map_attempt(
    run: &DispatchRun,
    attempt: &DispatchAttempt,
    index: usize,
    last: usize,
    final_agent: &str,
) -> AttemptCard {
    let agent = if attempt.agent.trim().is_empty() {
        "agent".to_string()
    } else {
        attempt.agent.trim().to_string()
    };
    let is_final = index == last && agent == final_agent;
    let error = {
        let own = attempt.error.trim();
        if !own.is_empty() {
            own.to_string()
        } else if is_final {
            run.error.trim().to_string()
        } else {
            String::new()
        }
    };
    AttemptCard {
        key: format!("{}:attempt:{index}", run.id),
        agent: agent.clone(),
        title: dotted(&[&agent, &attempt.model]),
        status: if is_final {
            or_unknown(&run.status)
        } else {
            "failed".to_string()
        },
        error,
        exit_class: attempt.exit_class.trim().to_string(),
        handoff: attempt.handoff.trim().to_string(),
        duration_ms: attempt.duration_ms.map(|value| value.0),
        is_final,
    }
}

pub fn attempt_model_id(agent: &str, model: &str) -> String {
    match (agent.trim(), model.trim()) {
        ("", "") => "agent".to_string(),
        (a, "") => a.to_string(),
        ("", m) => m.to_string(),
        (a, m) => format!("{a}/{m}"),
    }
}

pub fn switch_reason_label(exit_class: &str) -> String {
    match exit_class.trim() {
        "near_limit" => "95% of usage limit".to_string(),
        "rate_limit" | "credits" => "usage limit".to_string(),
        "" => "failed".to_string(),
        other => other.replace('_', " "),
    }
}

/// The divider copy between two attempts in a conversation, such as
/// `"Switched claude/claude-opus-5-5-high → codex/gpt-6-luna-medium · usage limit · hand-off"`.
pub fn switch_divider_label(
    from_agent: &str,
    from_model: &str,
    to_agent: &str,
    to_model: &str,
    exit_class: &str,
    has_handoff: bool,
) -> String {
    let from = attempt_model_id(from_agent, from_model);
    let to = attempt_model_id(to_agent, to_model);
    let reason = switch_reason_label(exit_class);
    if has_handoff {
        let suffix = if exit_class.trim() == "near_limit" {
            "hand-off by the model"
        } else {
            "hand-off"
        };
        format!("Switched {from} \u{2192} {to} \u{00b7} {reason} \u{00b7} {suffix}")
    } else {
        format!("Switched {from} \u{2192} {to} \u{00b7} {reason}")
    }
}

/// Parses a `── dispatch: <from> failed (<class>); trying <to> ──` separator line.
pub fn parse_switch_line(line: &str) -> Option<(String, String, String, String, String)> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix("── dispatch:")?
        .strip_suffix("──")?
        .trim();
    let (failed_part, to_raw) = inner.split_once("; trying ")?;
    let (from_raw, class_part) = failed_part.trim().rsplit_once(" failed (")?;
    let exit_class = class_part.strip_suffix(')')?.trim().to_string();
    let (from_agent, from_model) = split_agent_model(from_raw);
    let (to_agent, to_model) = split_agent_model(to_raw);
    Some((from_agent, from_model, to_agent, to_model, exit_class))
}

fn split_agent_model(raw: &str) -> (String, String) {
    let trimmed = raw.trim().trim_matches('/');
    match trimmed.split_once('/') {
        Some((a, m)) => (a.trim().to_string(), m.trim().to_string()),
        None => (trimmed.to_string(), String::new()),
    }
}

/// The model switches between attempts of `run`, in order.
pub fn attempt_switches(run: &DispatchRun) -> Vec<AttemptSwitch> {
    let total = run.attempts.len();
    let mut switches = Vec::new();
    for (index, attempt) in run.attempts.iter().enumerate() {
        let (to_agent, to_model) = if index + 1 < total {
            let next = &run.attempts[index + 1];
            (next.agent.trim(), next.model.trim())
        } else {
            let final_agent = run.agent.trim();
            let final_model = run.model.trim();
            let same_target =
                final_agent == attempt.agent.trim() && final_model == attempt.model.trim();
            let switched_to_final = (!final_agent.is_empty() || !final_model.is_empty())
                && (!same_target
                    || !attempt.handoff.trim().is_empty()
                    || is_run_active(&run.status)
                    || run.status.eq_ignore_ascii_case("completed"));
            if !switched_to_final {
                continue;
            }
            (final_agent, final_model)
        };
        let handoff = attempt.handoff.trim().to_string();
        let has_handoff = !handoff.is_empty();
        let label = switch_divider_label(
            &attempt.agent,
            &attempt.model,
            to_agent,
            to_model,
            &attempt.exit_class,
            has_handoff,
        );
        switches.push(AttemptSwitch {
            index,
            from_agent: attempt.agent.trim().to_string(),
            from_model: attempt.model.trim().to_string(),
            to_agent: to_agent.to_string(),
            to_model: to_model.to_string(),
            exit_class: attempt.exit_class.trim().to_string(),
            error: attempt.error.trim().to_string(),
            handoff,
            label,
        });
    }
    switches
}

fn or_unknown(status: &str) -> String {
    if status.is_empty() {
        "unknown".to_string()
    } else {
        status.to_string()
    }
}

pub fn file_counts(
    files_changed: Option<i32>,
    insertions: Option<i32>,
    deletions: Option<i32>,
) -> Option<FileCounts> {
    if files_changed.is_none() && insertions.is_none() && deletions.is_none() {
        return None;
    }
    Some(FileCounts {
        files_label: files_changed.map(|count| {
            if count == 1 {
                "1 file".to_string()
            } else {
                format!("{count} files")
            }
        }),
        insertions_label: insertions.map(|count| format!("+{count}")),
        deletions_label: deletions.map(|count| format!("\u{2212}{count}")),
    })
}

pub fn format_files(counts: &FileCounts) -> String {
    [
        counts.files_label.as_deref(),
        counts.insertions_label.as_deref(),
        counts.deletions_label.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("  ")
}

pub fn linked_rows(run: &DispatchRun, runs: &[DispatchRun]) -> Vec<LinkedRow> {
    let mut rows = Vec::new();
    let resumed = run.resumed_of.trim();
    if !resumed.is_empty() {
        rows.push(LinkedRow {
            run_id: resumed.to_string(),
            seed: format!("{}:resumed-of", run.id),
            label: format!("Resumed from {}", short_run_id(resumed)),
            agent: String::new(),
            status: String::new(),
            merged_at: String::new(),
            show_status: false,
        });
    }
    for next in runs.iter().filter(|item| item.resumed_of.trim() == run.id) {
        rows.push(linked_peer(
            next,
            &format!(
                "Resumed by {} / {}",
                agent_or(next.agent.as_str()),
                short_run_id(&next.id)
            ),
        ));
    }
    for child in runs
        .iter()
        .filter(|item| item.parent_run_id.trim() == run.id)
    {
        rows.push(linked_peer(
            child,
            &format!(
                "Child {} / {}",
                agent_or(child.agent.as_str()),
                short_run_id(&child.id)
            ),
        ));
    }
    rows
}

fn linked_peer(run: &DispatchRun, label: &str) -> LinkedRow {
    LinkedRow {
        run_id: run.id.clone(),
        seed: run.id.clone(),
        label: label.to_string(),
        agent: run.agent.clone(),
        status: run.status.clone(),
        merged_at: run.merged_at.clone(),
        show_status: !run.status.is_empty(),
    }
}

fn agent_or(agent: &str) -> &str {
    if agent.is_empty() { "agent" } else { agent }
}

pub fn coalesce_text(primary: &str, fallback: &str) -> String {
    let primary = primary.trim();
    if primary.is_empty() {
        fallback.trim().to_string()
    } else {
        primary.to_string()
    }
}

pub fn has_worktree(branch: &str, path: &str) -> bool {
    !branch.trim().is_empty() || !path.trim().is_empty()
}

/// Stored usage when the core recorded any of it. An all-zero struct with no price
/// is treated as absent, because the shared `DispatchUsage` is not optional.
pub fn stored_usage(usage: &DispatchUsage) -> Option<TokenUsage> {
    let present = usage.input_tokens.0 != 0
        || usage.cache_read_tokens.0 != 0
        || usage.cache_write_tokens.0 != 0
        || usage.output_tokens.0 != 0
        || usage.reasoning_tokens.0 != 0
        || usage.context_tokens.0 != 0
        || usage.context_window.0 != 0
        || usage.cost_usd.is_some()
        || !usage.cost_source.trim().is_empty();
    if !present {
        return None;
    }
    Some(usage_from_stored(usage))
}

fn usage_from_stored(usage: &DispatchUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens.0,
        cache_read_tokens: usage.cache_read_tokens.0,
        cache_write_tokens: usage.cache_write_tokens.0,
        output_tokens: usage.output_tokens.0,
        reasoning_tokens: usage.reasoning_tokens.0,
        context_tokens: usage.context_tokens.0,
        context_window: usage.context_window.0,
        cost_usd: usage.cost_usd,
        cost_source: usage.cost_source.clone(),
    }
}

/// Tokens and context for the rail. Stored usage wins; a live run falls back to the log.
pub fn rail_tokens(stored: Option<&TokenUsage>, logs: &[&str], live: bool) -> Option<TokenUsage> {
    if let Some(stored) = stored {
        return Some(stored.clone());
    }
    if live {
        logs.iter().find_map(|log| live_usage(log))
    } else {
        None
    }
}

pub fn total_tokens(usage: &TokenUsage) -> i64 {
    usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens + usage.output_tokens
}

pub fn format_tokens(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1_000_000.0 {
        format!("{}M", fixed(n / 1_000_000.0, 1))
    } else if n >= 10_000.0 {
        format!("{}k", (n / 1000.0).round() as i64)
    } else if n >= 1000.0 {
        format!("{}k", fixed(n / 1000.0, 1))
    } else {
        format!("{}", n.round() as i64)
    }
}

pub fn format_cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.01 {
        "<$0.01".to_string()
    } else {
        format!("${}", fixed(usd, 2))
    }
}

pub fn token_breakdown(usage: &TokenUsage) -> String {
    let mut parts = vec![format!("Input {}", format_tokens(usage.input_tokens))];
    if usage.cache_read_tokens > 0 {
        parts.push(format!(
            "Cache read {}",
            format_tokens(usage.cache_read_tokens)
        ));
    }
    if usage.cache_write_tokens > 0 {
        parts.push(format!(
            "Cache write {}",
            format_tokens(usage.cache_write_tokens)
        ));
    }
    let reasoning = if usage.reasoning_tokens > 0 {
        format!(" ({} reasoning)", format_tokens(usage.reasoning_tokens))
    } else {
        String::new()
    };
    parts.push(format!(
        "Output {}{reasoning}",
        format_tokens(usage.output_tokens)
    ));
    parts.join(" \u{00b7} ")
}

pub fn context_usage(tokens: i64, window: i64) -> Option<ContextMeter> {
    if tokens <= 0 {
        return None;
    }
    if window <= 0 {
        return Some(ContextMeter {
            label: format_tokens(tokens),
            title: "Tokens in context at the last model call.".to_string(),
            percent: None,
        });
    }
    let percent = (((tokens as f64 / window as f64) * 100.0).round() as u32).min(100);
    Some(ContextMeter {
        label: format!(
            "{} / {} ({percent}%)",
            format_tokens(tokens),
            format_tokens(window)
        ),
        title: format!(
            "{} of {} tokens in context at the last model call.",
            group_int(tokens),
            group_int(window)
        ),
        percent: Some(percent),
    })
}

pub fn cost_source_label(source: &str) -> String {
    match source {
        "agent" => "Reported by the agent CLI".to_string(),
        "models.dev price" => "Estimated from models.dev list prices".to_string(),
        "agent and models.dev price" => {
            "Reported by the agent CLI and estimated from models.dev list prices".to_string()
        }
        _ => String::new(),
    }
}

pub fn run_cost(usage: &TokenUsage) -> CostLabel {
    match usage.cost_usd {
        None => CostLabel {
            label: "Unknown".to_string(),
            title: "No price is known for this model.".to_string(),
        },
        Some(usd) => {
            let source = cost_source_label(&usage.cost_source);
            let title = if source.is_empty() {
                API_EQUIVALENT.to_string()
            } else {
                format!("{API_EQUIVALENT} {source}")
            };
            CostLabel {
                label: format_cost(usd),
                title,
            }
        }
    }
}

/// The last `{"dispatch":"usage"}` line of a run log.
pub fn live_usage(log: &str) -> Option<TokenUsage> {
    for line in log.lines().rev() {
        if !line.contains("\"dispatch\":\"usage\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("dispatch").and_then(Value::as_str) != Some("usage") {
            continue;
        }
        return Some(TokenUsage {
            context_tokens: json_i64(&value, "context"),
            context_window: json_i64(&value, "window"),
            input_tokens: json_i64(&value, "input"),
            cache_read_tokens: json_i64(&value, "cacheRead"),
            cache_write_tokens: json_i64(&value, "cacheWrite"),
            output_tokens: json_i64(&value, "output"),
            reasoning_tokens: json_i64(&value, "reasoning"),
            cost_usd: value
                .get("costUsd")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite()),
            cost_source: String::new(),
        });
    }
    None
}

/// RFC3339 timestamp as unix milliseconds. Missing zone is treated as UTC.
pub fn parse_time_ms(value: &str) -> Option<i64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i32 = value.get(0..4)?.parse().ok()?;
    let month: u32 = value.get(5..7)?.parse().ok()?;
    let day: u32 = value.get(8..10)?.parse().ok()?;
    let hour: u32 = value.get(11..13)?.parse().ok()?;
    let minute: u32 = value.get(14..16)?.parse().ok()?;
    let second: u32 = value.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = value.get(19..)?;
    let mut millis = 0_i64;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits: String = fraction
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            let mut padded = digits.clone();
            while padded.len() < 3 {
                padded.push('0');
            }
            millis = padded.get(0..3)?.parse().unwrap_or(0);
        }
        rest = fraction.get(digits.len()..)?;
    }
    let offset_minutes = if rest.is_empty() || rest == "Z" || rest == "z" {
        0
    } else if rest.starts_with('+') || rest.starts_with('-') {
        let sign = if rest.starts_with('+') { 1 } else { -1 };
        let off = &rest[1..];
        let (oh, om) = if off.len() >= 5 && off.as_bytes().get(2) == Some(&b':') {
            (
                off.get(0..2)?.parse::<i64>().ok()?,
                off.get(3..5)?.parse::<i64>().ok()?,
            )
        } else if off.len() >= 4 {
            (
                off.get(0..2)?.parse::<i64>().ok()?,
                off.get(2..4)?.parse::<i64>().ok()?,
            )
        } else {
            return None;
        };
        if oh > 23 || om > 59 {
            return None;
        }
        sign * (oh * 60 + om)
    } else {
        return None;
    };
    let days = days_from_civil(year, month, day)?;
    let seconds = days * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64
        - offset_minutes * 60;
    Some(seconds * 1000 + millis)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> Option<i64> {
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u32;
    let month_adj = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_adj + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era as i64 * 146_097 + doe as i64 - 719_468)
}

pub fn proto_ms(value: Option<ProtoI64>) -> Option<i64> {
    value.map(|value| value.0)
}

fn fixed(value: f64, digits: usize) -> String {
    format!("{value:.digits$}")
}

fn group_int(value: i64) -> String {
    let negative = value < 0;
    let digits: Vec<char> = value.unsigned_abs().to_string().chars().collect();
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    for (index, ch) in digits.iter().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*ch);
    }
    out
}

fn json_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn json_strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn opt_f64(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
}

fn as_number(value: &Value) -> f64 {
    value.as_f64().filter(|n| n.is_finite()).unwrap_or(0.0)
}

fn json_i64(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map(|n| n as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateOutcome, ClassifierBands, DecisionCandidate, MoreAction, ReviewAction, RolePick,
        RouteDecision, TokenUsage, candidate_explain, candidate_line, candidate_outcomes,
        classifier_bands, context_usage, difficulty_label, dispatch_op_body, file_counts,
        format_cost, format_duration, format_files, format_tokens, live_usage, map_route_decision,
        pace_label, parse_time_ms, priority_label, role_summary, route_reason, run_cost,
        run_more_actions, run_review_action, token_breakdown, total_tokens, unmatched_attempts,
        worktree_display_status,
    };
    use crate::client::{DispatchAttempt, DispatchRun as Run};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    #[test]
    fn a_follow_up_waits_for_the_run_its_answer_and_its_folder() {
        use super::{FollowUpBlock, follow_up_blocker};
        use crate::client::DispatchRun;
        let run = |id: &str, status: &str, worktree: &str, resumed_of: &str| DispatchRun {
            id: id.into(),
            status: status.into(),
            worktree: worktree.into(),
            resumed_of: resumed_of.into(),
            ..DispatchRun::default()
        };
        let mine = run("a", "COMPLETED", "/repo", "");
        assert_eq!(follow_up_blocker(&mine, &[mine.clone()]), None);
        let live = run("a", "RUNNING", "/repo", "");
        assert_eq!(follow_up_blocker(&live, &[]), Some(FollowUpBlock::Working));
        let answer = run("b", "RUNNING", "/repo", "a");
        assert_eq!(
            follow_up_blocker(&mine, &[answer]),
            Some(FollowUpBlock::Answering)
        );
        let neighbor = run("c", "RUNNING", "/repo", "");
        assert_eq!(
            follow_up_blocker(&mine, &[neighbor.clone()]),
            Some(FollowUpBlock::FolderBusy)
        );
        // A run in another folder, or one that ended, is no obstacle.
        let elsewhere = run("d", "RUNNING", "/other", "");
        let ended = run("e", "COMPLETED", "/repo", "");
        assert_eq!(follow_up_blocker(&mine, &[elsewhere, ended]), None);
        // A read-only run with no folder of its own waits only for itself and its answer.
        let bare = run("f", "COMPLETED", "", "");
        assert_eq!(follow_up_blocker(&bare, &[neighbor]), None);
    }

    fn op(body: Value, ok: bool) -> (bool, String) {
        (ok, body.to_string())
    }

    #[test]
    fn reads_a_routing_record_and_hides_a_missing_one() {
        let (ok, json_text) = op(json!({}), false);
        assert!(map_route_decision(ok, &json_text).is_none());
        let (ok, json_text) = op(
            json!({
                "runId": "r1",
                "policy": "auto-v1",
                "priority": "quality",
                "difficulty": -0.5,
                "tier": "hard",
                "classifier": "jev",
                "probs": [0.1, 0.2, 0.7],
                "area": { "kinds": ["service"], "tech": ["go"] },
                "candidates": [{
                    "agent": "codex",
                    "model": "m",
                    "pPass": 0.91,
                    "pPassLow": 0.9,
                    "effectiveCost": 0.2,
                    "quotaMultiplier": 2,
                    "qualified": true
                }],
                "chosen": 0
            }),
            true,
        );
        let decision = map_route_decision(ok, &json_text).expect("decision");
        assert_eq!(decision.area_kinds, vec!["service".to_string()]);
        assert_eq!(decision.area_tech, vec!["go".to_string()]);
        assert_eq!(
            difficulty_label(&decision).as_deref(),
            Some("-0.50 \u{00b7} hard")
        );
        assert_eq!(
            classifier_bands(&decision),
            Some(ClassifierBands {
                name: "jev".into(),
                bands: vec![("easy", 0.1), ("normal", 0.2), ("hard", 0.7)],
            })
        );
        assert_eq!(priority_label(&decision), "quality \u{00b7} bar 90%");
        assert_eq!(
            candidate_line(&decision.candidates[0]),
            "91% to pass \u{00b7} $0.20"
        );
        assert_eq!(
            route_reason(&decision).as_deref(),
            Some(
                "jev read the message as hard (d -0.50); m is the cheapest of 1 that clear the 90% bar"
            )
        );
        let explain = candidate_explain(&decision, &decision.candidates[0]);
        let rows: Vec<(&str, &str)> = explain
            .rows
            .iter()
            .map(|(label, value)| (label.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Task", "d -0.50 (hard)"),
                ("Pass", "91%, at least 90%"),
                ("Bar", "90%: clears it"),
                ("Cost here", "$0.20 (quota \u{00d7}2.00)"),
            ]
        );
        assert!(explain.about.contains("first message"));
    }

    #[test]
    fn a_candidate_shows_what_became_of_it() {
        let mut decision = base_decision();
        let candidate = |agent: &str, model: &str| DecisionCandidate {
            agent: agent.into(),
            model: model.into(),
            effort: String::new(),
            p_pass: None,
            p_pass_low: None,
            price: Some(2.5),
            quota_multiplier: None,
            effective_cost: None,
            theta: Some(0.0),
            theta_source: "cursorbench".into(),
            qualified: false,
            score: Some(0.0),
        };
        decision.candidates = vec![
            candidate("cursor", "gemini-4-argon-low"),
            candidate("claude", "claude-sonnet-5-5"),
            candidate("codex", "gpt-6.1-sol"),
            candidate("codex", "gpt-6-astra"),
        ];
        let run = Run {
            agent: "codex".into(),
            model: "gpt-6.1-sol".into(),
            status: "COMPLETED".into(),
            attempts: vec![
                DispatchAttempt {
                    agent: "cursor".into(),
                    model: "gemini-4-argon-low".into(),
                    error: "out of credits".into(),
                    exit_class: "credits".into(),
                    duration_ms: None,
                    ..DispatchAttempt::default()
                },
                DispatchAttempt {
                    agent: "pi".into(),
                    model: "elsewhere".into(),
                    ..DispatchAttempt::default()
                },
            ],
            ..Run::default()
        };
        let outcomes = candidate_outcomes(&decision, &run);
        let labels: Vec<Option<String>> = outcomes
            .iter()
            .map(|outcome| outcome.as_ref().map(CandidateOutcome::label))
            .collect();
        assert_eq!(
            labels,
            [
                Some("Failed: credits".to_string()),
                Some("Not tried".to_string()),
                Some("Used".to_string()),
                None,
            ]
        );
        assert_eq!(
            outcomes[0],
            Some(CandidateOutcome::Failed {
                class: "credits".into(),
                error: "out of credits".into()
            })
        );
        let unmatched = unmatched_attempts(&decision, &run);
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0].agent, "pi");
        assert_eq!(
            candidate_line(&decision.candidates[0]),
            "Skill 50 \u{00b7} $2.50"
        );
        let running = Run {
            status: "RUNNING".into(),
            ..run
        };
        assert_eq!(
            candidate_outcomes(&decision, &running)[2]
                .as_ref()
                .map(CandidateOutcome::label),
            Some("Running".to_string())
        );
    }

    #[test]
    fn a_tier_or_named_route_says_the_message_was_not_scored() {
        let mut decision = base_decision();
        decision.policy = "tier-v2".into();
        decision.route = "tier".into();
        decision.classifier.clear();
        decision.candidates = vec![DecisionCandidate {
            agent: "claude".into(),
            model: "claude-sonnet-5-5".into(),
            effort: "high".into(),
            p_pass: None,
            p_pass_low: None,
            price: None,
            quota_multiplier: None,
            effective_cost: None,
            theta: None,
            theta_source: String::new(),
            qualified: false,
            score: Some(1000.0),
        }];
        assert_eq!(candidate_line(&decision.candidates[0]), "Quota unknown");
        assert_eq!(
            route_reason(&decision).as_deref(),
            Some(
                "The tier table's first model with quota to spare: claude-sonnet-5-5. The message was not scored"
            )
        );
        let explain = candidate_explain(&decision, &decision.candidates[0]);
        assert_eq!(
            explain.rows,
            [("Quota".to_string(), "Quota unknown".to_string())]
        );
        decision.route = "model".into();
        assert_eq!(
            route_reason(&decision).as_deref(),
            Some("The request named the model: nothing was scored")
        );
        assert_eq!(pace_label(-43.0), "Quota on pace");
        assert_eq!(pace_label(3000.8), "Quota won't last");
        assert_eq!(pace_label(10000.0), "Cooling down");
    }

    fn base_decision() -> RouteDecision {
        RouteDecision {
            run_id: "r".into(),
            policy: "auto-v1".into(),
            route: "auto".into(),
            priority: "balanced".into(),
            difficulty: None,
            tier: String::new(),
            classifier: "jev".into(),
            probs: Vec::new(),
            area_kinds: Vec::new(),
            area_tech: Vec::new(),
            candidates: Vec::new(),
            chosen: 0,
            role: String::new(),
            role_pick: None,
        }
    }

    #[test]
    fn names_how_a_run_got_its_role() {
        let mut picked = base_decision();
        picked.role = "reviewer".into();
        let mut probs = BTreeMap::new();
        probs.insert("reviewer".into(), 0.82);
        picked.role_pick = Some(RolePick {
            role: "reviewer".into(),
            probs,
        });
        assert_eq!(
            role_summary(&picked).as_deref(),
            Some("reviewer, picked (P 0.82)")
        );

        let mut general = base_decision();
        general.role_pick = Some(RolePick {
            role: "general".into(),
            probs: BTreeMap::new(),
        });
        assert!(role_summary(&general).unwrap().contains("General"));

        let mut named = base_decision();
        named.role = "tester".into();
        assert_eq!(
            role_summary(&named).as_deref(),
            Some("tester, named by the request")
        );
        assert_eq!(role_summary(&base_decision()), None);
    }

    #[test]
    fn worktree_chip_follows_how_the_run_ended() {
        assert_eq!(worktree_display_status("FAILED", "active"), "FAILED");
        assert_eq!(worktree_display_status("TIMED_OUT", "active"), "TIMED_OUT");
        assert_eq!(worktree_display_status("cancelled", "active"), "cancelled");
        assert_eq!(worktree_display_status("ORPHANED", "active"), "ORPHANED");
        assert_eq!(worktree_display_status("RUNNING", "active"), "active");
        assert_eq!(worktree_display_status("COMPLETED", "ready"), "ready");
        assert_eq!(worktree_display_status("COMPLETED", "removed"), "deleted");
        assert_eq!(worktree_display_status("FAILED", "abandoned"), "abandoned");
        assert_eq!(worktree_display_status("FAILED", ""), "");
    }

    #[test]
    fn review_action_is_merge_then_delete() {
        assert_eq!(
            run_review_action("completed", "", "", true),
            Some(ReviewAction::Merge)
        );
        assert_eq!(
            run_review_action("completed", "2026-10-01", "merged", true),
            Some(ReviewAction::Delete)
        );
        assert_eq!(
            run_review_action("completed", "2026-10-01", "removed", true),
            None
        );
        assert_eq!(run_review_action("completed", "", "abandoned", true), None);
        assert_eq!(run_review_action("running", "", "", true), None);
        assert_eq!(run_review_action("completed", "2026-10-01", "", true), None);
        assert_eq!(run_review_action("completed", "", "", false), None);
        assert_eq!(run_review_action("cancelled", "", "", true), None);
        assert_eq!(run_review_action("failed", "", "", true), None);
    }

    #[test]
    fn header_menu_offers_recover_and_clean_by_how_the_run_ended() {
        assert_eq!(
            run_more_actions("completed", "", true),
            vec![MoreAction::Clean]
        );
        assert_eq!(
            run_more_actions("failed", "", true),
            vec![MoreAction::Recover, MoreAction::Clean]
        );
        assert_eq!(
            run_more_actions("orphaned", "", true),
            vec![MoreAction::Recover, MoreAction::Clean]
        );
        assert!(run_more_actions("running", "", true).is_empty());
        assert!(run_more_actions("completed", "2026-10-01", true).is_empty());
        assert!(run_more_actions("failed", "", false).is_empty());
    }

    #[test]
    fn op_body_keeps_ok_and_drops_a_broken_payload() {
        let (ok, body) = dispatch_op_body(&json!({
            "ok": false,
            "json": "{\"removed\":\"/tmp/w\"}"
        }));
        assert!(!ok);
        assert_eq!(body["removed"], "/tmp/w");
        let (ok, body) = dispatch_op_body(&json!({ "json": "{" }));
        assert!(ok);
        assert!(body.as_object().unwrap().is_empty());
    }

    #[test]
    fn formats_tokens_cost_and_context_the_way_the_rail_shows_them() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(1500), "1.5k");
        assert_eq!(format_tokens(44608), "45k");
        assert_eq!(format_tokens(6_486_752), "6.5M");
        assert_eq!(format_cost(0.004), "<$0.01");
        assert_eq!(format_cost(3.912308), "$3.91");
        let usage = TokenUsage {
            input_tokens: 100,
            cache_read_tokens: 900,
            cache_write_tokens: 0,
            output_tokens: 40,
            reasoning_tokens: 30,
            ..TokenUsage::default()
        };
        assert_eq!(total_tokens(&usage), 1040);
        assert_eq!(
            token_breakdown(&usage),
            "Input 100 \u{00b7} Cache read 900 \u{00b7} Output 40 (30 reasoning)"
        );
        let mut priced = usage.clone();
        priced.cost_usd = Some(0.092336);
        priced.cost_source = "agent".into();
        assert_eq!(run_cost(&priced).label, "$0.09");
        priced.cost_usd = None;
        assert_eq!(run_cost(&priced).label, "Unknown");

        let meter = context_usage(46200, 272000).unwrap();
        assert_eq!(meter.label, "46k / 272k (17%)");
        assert_eq!(meter.percent, Some(17));
        assert_eq!(context_usage(46200, 0).unwrap().label, "46k");
        assert_eq!(context_usage(46200, 0).unwrap().percent, None);
        assert!(context_usage(0, 272000).is_none());
        assert_eq!(context_usage(300000, 272000).unwrap().percent, Some(100));
    }

    #[test]
    fn keeps_the_last_usage_line_for_the_context_meter() {
        let log = [
            r#"{"dispatch":"usage","context":1000,"window":272000,"input":900,"output":100}"#,
            "plain output",
            r#"{"dispatch":"usage","context":46200,"window":272000,"input":9000,"cacheRead":36000,"output":1200,"reasoning":300,"costUsd":null}"#,
        ]
        .join("\n");
        let usage = live_usage(&log).unwrap();
        assert_eq!(
            usage,
            TokenUsage {
                context_tokens: 46200,
                context_window: 272000,
                input_tokens: 9000,
                cache_read_tokens: 36000,
                cache_write_tokens: 0,
                output_tokens: 1200,
                reasoning_tokens: 300,
                cost_usd: None,
                cost_source: String::new(),
            }
        );
        assert!(live_usage("no usage here").is_none());
    }

    #[test]
    fn duration_and_file_labels_match_the_run_rail() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(59_400), "59s");
        assert_eq!(format_duration(90_000), "1m");
        assert_eq!(format_duration(3_660_000), "1h 1m");
        assert_eq!(parse_time_ms("1970-01-01T00:00:00Z"), Some(0));
        let counts = file_counts(Some(1), Some(2), Some(3)).unwrap();
        assert_eq!(format_files(&counts), "1 file  +2  \u{2212}3");
        assert!(file_counts(None, None, None).is_none());
    }

    #[test]
    fn formats_attempt_switches_and_handoff_dividers() {
        use super::{attempt_switches, parse_switch_line, switch_divider_label};
        assert_eq!(
            switch_divider_label(
                "claude",
                "claude-opus-5-5-high",
                "codex",
                "gpt-6-luna-medium",
                "rate_limit",
                true,
            ),
            "Switched claude/claude-opus-5-5-high \u{2192} codex/gpt-6-luna-medium \u{00b7} usage limit \u{00b7} hand-off"
        );
        assert_eq!(
            parse_switch_line(
                "── dispatch: claude/claude-opus-5-5-high failed (rate_limit); trying codex/gpt-6-luna-medium ──"
            ),
            Some((
                "claude".to_string(),
                "claude-opus-5-5-high".to_string(),
                "codex".to_string(),
                "gpt-6-luna-medium".to_string(),
                "rate_limit".to_string(),
            ))
        );
        let run = Run {
            agent: "codex".into(),
            model: "gpt-6-luna-medium".into(),
            status: "COMPLETED".into(),
            attempts: vec![DispatchAttempt {
                agent: "claude".into(),
                model: "claude-opus-5-5-high".into(),
                error: "429 usage limit".into(),
                exit_class: "rate_limit".into(),
                handoff: "## Original task\nFix it".into(),
                duration_ms: None,
            }],
            ..Run::default()
        };
        let switches = attempt_switches(&run);
        assert_eq!(switches.len(), 1);
        assert_eq!(
            switches[0].label,
            "Switched claude/claude-opus-5-5-high \u{2192} codex/gpt-6-luna-medium \u{00b7} usage limit \u{00b7} hand-off"
        );
        assert_eq!(switches[0].handoff, "## Original task\nFix it");
        assert_eq!(
            switch_divider_label(
                "claude",
                "claude-opus-5-5-high",
                "codex",
                "gpt-6.1-sol",
                "near_limit",
                true,
            ),
            "Switched claude/claude-opus-5-5-high \u{2192} codex/gpt-6.1-sol \u{00b7} 95% of usage limit \u{00b7} hand-off by the model"
        );
    }
}
